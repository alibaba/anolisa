pub mod backend_detect;
pub mod backends;
pub mod bootstrap;
pub mod btrfs_ops;
pub mod dispatcher;
pub mod fs_watcher;
pub mod index_store;
pub mod listener;
pub mod scheduler;
pub mod seccomp;
pub mod snapshot_mgr;
pub mod state;
pub mod workspace_mgr;

use std::sync::Arc;

use anyhow::Context;
use tokio::signal::unix::{signal, SignalKind};
use tokio_util::sync::CancellationToken;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::state::DaemonState;
use ws_ckpt_common::DaemonConfig;

pub async fn run_daemon(config: DaemonConfig) -> anyhow::Result<()> {
    // 0. Require root privileges
    if !nix::unistd::geteuid().is_root() {
        anyhow::bail!(
            "ws-ckpt daemon must be run as root (mount, losetup, btrfs commands require root privileges)"
        );
    }

    // 1. Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(&config.log_level))
        .init();

    info!("ws-ckpt daemon starting...");

    // 2. Detect and create storage backend
    let detect_result = backend_detect::detect_and_create_backend(&config).await?;
    info!(
        "Backend selected: {} (method: {})",
        detect_result.backend.backend_type(),
        detect_result.method
    );

    // 3. For non-BtrfsLoop backends, ensure data directories upfront.
    //    BtrfsLoop bootstrap is deferred (lazy) until the first write operation.
    if detect_result.backend.backend_type() != ws_ckpt_common::backend::BackendType::BtrfsLoop {
        let backend = &detect_result.backend;
        let dirs = [backend.data_root(), backend.snapshots_root()];

        for dir in dirs {
            tokio::fs::create_dir_all(dir)
                .await
                .with_context(|| format!("Failed to ensure directory exists: {:?}", dir))?;
        }

        info!(
            "Ensured data directories for {} backend",
            backend.backend_type()
        );
    }

    // 4. Rebuild state from disk
    // For BtrfsLoop, run bootstrap unconditionally on every start so that:
    //   * the image is mounted before `rebuild_from_disk` scans snapshots_root,
    //   * `reconcile_img_size` runs even when the previous instance left the
    //     filesystem mounted (a `systemctl restart` does NOT unmount), so
    //     config changes to img_size / img_max_percent take effect on the
    //     very next restart instead of being silently skipped.
    // bootstrap() is idempotent: each step (image creation, mount, reconcile,
    // snapshots dir) checks current state before acting.
    if detect_result.backend.backend_type() == ws_ckpt_common::backend::BackendType::BtrfsLoop {
        crate::bootstrap::bootstrap(&config).await?;
    }
    let state = Arc::new(DaemonState::rebuild_from_disk(config, detect_result.backend).await?);

    // 5. Re-establish symlinks lost during daemon restart
    bootstrap::ensure_symlinks(&state).await;

    //  6. Apply seccomp-bpf syscall filter (after bootstrap, before listener)
    if let Err(e) = seccomp::apply_seccomp_filter() {
        tracing::warn!(
            "Failed to apply seccomp filter: {:#}. Continuing without syscall filtering.",
            e
        );
    }

    // 7. Start background scheduler
    scheduler::start_scheduler(state.clone());

    // 8.1. Create cancellation token
    let cancel = CancellationToken::new();

    // 8.2. Register signal handlers
    let mut sigterm = signal(SignalKind::terminate())?;

    // 8.3. SIGHUP no-op: default disposition is terminate, so consume it to
    // prevent `kill -HUP <pid>` from accidentally killing the daemon. Reload
    // is driven by `Request::ReloadConfig` (`ws-ckpt reload` / `ExecReload`),
    // not SIGHUP.
    match signal(SignalKind::hangup()) {
        Ok(mut sighup) => {
            tokio::spawn(async move {
                loop {
                    sighup.recv().await;
                    tracing::info!(
                        "Received SIGHUP; no-op (use `systemctl reload ws-ckpt` or \
                         `ws-ckpt reload` to reload config)"
                    );
                }
            });
        }
        Err(e) => {
            tracing::warn!("Failed to install SIGHUP handler: {}", e);
        }
    }

    // 9. Spawn listener
    let listener_cancel = cancel.clone();
    let listener_state = Arc::clone(&state);
    let listener_handle =
        tokio::spawn(async move { listener::run_listener(listener_state, listener_cancel).await });

    // 10. Wait for shutdown signal
    tokio::select! {
        _ = sigterm.recv() => {
            info!("Received SIGTERM, shutting down...");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received SIGINT (Ctrl+C), shutting down...");
        }
    }

    cancel.cancel();

    // 11. Wait for listener to finish
    if let Err(e) = listener_handle.await {
        tracing::error!("Listener task panicked: {}", e);
    }

    // 12. Flush all workspace index.json files
    info!("Flushing workspace indexes...");
    let all_ws = state.all_workspaces();
    for ws in &all_ws {
        let ws_guard = ws.read().await;
        let ws_dir = state.backend.snapshots_root().join(&ws_guard.ws_id);
        if let Err(e) = index_store::save(&ws_dir, &ws_guard.index).await {
            tracing::error!("Failed to save index for {}: {:#}", ws_guard.ws_id, e);
        }
    }

    info!("daemon shutdown complete");
    Ok(())
}
