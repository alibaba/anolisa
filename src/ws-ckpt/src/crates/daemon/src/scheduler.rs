//! Background scheduler: auto-cleanup, health check, and orphan recovery.

use std::path::Path;
use std::sync::Arc;

use tokio::time::Duration;
use tracing::{debug, error, info, warn};

use crate::btrfs_ops;
use crate::state::DaemonState;
use ws_ckpt_common::CleanupRetention;

/// Start background scheduler tasks: orphan cleanup on boot, periodic auto-cleanup,
/// and periodic health checks.
///
/// Config hot-reload is **push-based**: the dispatcher calls
/// `state.config_notify.notify_waiters()` after updating `state.config`, and
/// every periodic loop uses `tokio::select!` to react. This replaces the old
/// polling design — loops never wake up "just to check", and a disabled task
/// (`auto_cleanup = false` or `*_interval_secs == 0`) blocks on the notify
/// at zero CPU cost until a reload re-enables it.
pub fn start_scheduler(state: Arc<DaemonState>) {
    // Startup orphan cleanup
    let mount_path = state.mount_path.clone();
    tokio::spawn(async move {
        if let Err(e) = cleanup_orphans(&mount_path).await {
            error!("Failed to cleanup orphans: {}", e);
        }
    });

    // Periodic auto-cleanup: reacts to `ReloadConfig` via `config_notify`.
    let state_clone = state.clone();
    tokio::spawn(async move {
        auto_cleanup_loop(state_clone).await;
    });

    // Periodic health check: same notify-driven pattern.
    let state_clone2 = state.clone();
    tokio::spawn(async move {
        health_check_loop(state_clone2).await;
    });

    info!("Background scheduler started");
}

/// Auto-cleanup loop: each iteration re-reads `auto_cleanup`,
/// `auto_cleanup_interval_secs`, and `auto_cleanup_keep.is_disabled()`.
/// Disabled parks on `config_notify`; active races `sleep` vs `config_notify`
/// for immediate reload.
///
/// `notify_waiters()` does **not** store a permit, so a notify that fires
/// before a waiter has registered is lost. To close the window between the
/// config read and registration, we build the `Notified` future and
/// `enable()` it (registers immediately) **before** reading config. Any
/// `notify_waiters()` issued afterwards is then captured by this waiter.
async fn auto_cleanup_loop(state: Arc<DaemonState>) {
    loop {
        let notified = state.config_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        let (enabled, interval, keep_disabled) = {
            let cfg = state.config.read().unwrap();
            (
                cfg.auto_cleanup,
                cfg.auto_cleanup_interval_secs,
                cfg.auto_cleanup_keep.is_disabled(),
            )
        };
        if !enabled || interval == 0 || keep_disabled {
            // Disabled: block until a reload arrives, then re-check.
            notified.await;
            continue;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(interval)) => {
                auto_cleanup(&state).await;
            }
            _ = notified.as_mut() => {
                // Config changed mid-sleep: skip this cleanup pass and re-read.
            }
        }
    }
}

/// Health-check loop. Same push-based pattern as `auto_cleanup_loop`, keyed
/// off `health_check_interval_secs`. See that function's comment for why
/// `enable()` is called before the config read.
async fn health_check_loop(state: Arc<DaemonState>) {
    loop {
        let notified = state.config_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        let interval = state.config.read().unwrap().health_check_interval_secs;
        if interval == 0 {
            notified.await;
            continue;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(interval)) => {
                health_check(&state).await;
            }
            _ = notified.as_mut() => {}
        }
    }
}

/// Orphan recovery: clean up `.rollback-tmp` residual directories.
///
/// Scans the mount path for directories ending with `.rollback-tmp`
/// and removes them. Returns the list of cleaned-up paths.
pub async fn cleanup_orphans(mount_path: &Path) -> Result<Vec<String>, anyhow::Error> {
    let mut cleaned = Vec::new();

    let read_dir = match std::fs::read_dir(mount_path) {
        Ok(rd) => rd,
        Err(e) => {
            warn!("Cannot read mount path for orphan cleanup: {}", e);
            return Ok(cleaned);
        }
    };

    for entry in read_dir {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let name = entry.file_name();
        let name_str = name.to_string_lossy().to_string();
        let path = entry.path();

        if name_str.ends_with(".rollback-tmp") {
            info!("Cleaning up orphan directory: {:?}", path);

            // Try btrfs subvolume delete first, fall back to remove_dir_all
            match btrfs_ops::delete_subvolume(&path).await {
                Ok(()) => {
                    info!("Deleted orphan subvolume: {:?}", path);
                }
                Err(_) => {
                    // Fallback: try regular directory removal
                    if let Err(e) = tokio::fs::remove_dir_all(&path).await {
                        warn!("Failed to remove orphan directory {:?}: {}", path, e);
                        continue;
                    }
                    info!("Removed orphan directory: {:?}", path);
                }
            }

            cleaned.push(path.to_string_lossy().to_string());
        }
    }

    if !cleaned.is_empty() {
        info!("Orphan cleanup complete: {} items removed", cleaned.len());
    }

    Ok(cleaned)
}

/// Auto-cleanup: purge non-pinned snapshots per `CleanupRetention` (pinned always kept).
/// - `Count(n)`: keep n newest per workspace.
/// - `Age { secs, .. }`: delete if older than `secs` (strict, no count floor).
///
/// Caller ([`auto_cleanup_loop`]) filters disabled retention; this function
/// asserts that invariant in debug builds.
async fn auto_cleanup(state: &DaemonState) {
    let retention = {
        let cfg = state.config.read().unwrap();
        cfg.auto_cleanup_keep.clone()
    };
    debug_assert!(
        !retention.is_disabled(),
        "caller must filter disabled retention"
    );

    info!("Running auto-cleanup (retention={})...", retention);
    let all_ws = state.all_workspaces();
    let now = chrono::Utc::now();

    for ws_arc in &all_ws {
        let mut ws = ws_arc.write().await;
        let ws_id = ws.ws_id.clone();

        // index storage directory (for saving index.json)
        let index_dir = state.index_dir(&ws_id);
        if let Err(e) = tokio::fs::create_dir_all(&index_dir).await {
            warn!(
                "auto-cleanup: failed to create index directory {:?}: {}",
                index_dir, e
            );
            continue;
        }

        // btrfs snapshot subvolume root directory (for actually deleting subvolumes)
        let snapshots_ws_dir = state.backend.snapshots_root().join(&ws_id);

        // Collect non-pinned snapshots sorted by created_at ascending
        let mut unpinned: Vec<(String, chrono::DateTime<chrono::Utc>)> = ws
            .index
            .snapshots
            .iter()
            .filter(|(_, meta)| !meta.pinned)
            .map(|(id, meta)| (id.clone(), meta.created_at))
            .collect();
        unpinned.sort_by_key(|(_, ts)| *ts);

        // Pick victims according to the active mode
        let to_remove: Vec<String> = match &retention {
            CleanupRetention::Count(n) => {
                let keep = *n as usize;
                if unpinned.len() <= keep {
                    continue;
                }
                unpinned[..unpinned.len() - keep]
                    .iter()
                    .map(|(id, _)| id.clone())
                    .collect()
            }
            CleanupRetention::Age { secs, .. } => {
                let cutoff = now - chrono::Duration::seconds(*secs as i64);
                unpinned
                    .iter()
                    .filter(|(_, ts)| *ts < cutoff)
                    .map(|(id, _)| id.clone())
                    .collect()
            }
        };

        if to_remove.is_empty() {
            continue;
        }

        let mut removed_count = 0;
        for snap_id in &to_remove {
            let snap_path = snapshots_ws_dir.join(snap_id);
            match btrfs_ops::delete_subvolume(&snap_path).await {
                Ok(()) => {
                    ws.index.snapshots.remove(snap_id);
                    removed_count += 1;
                }
                Err(e) => {
                    warn!("auto-cleanup: failed to delete {}: {:#}", snap_id, e);
                }
            }
        }

        if removed_count > 0 {
            if let Err(e) = crate::index_store::save(&index_dir, &ws.index).await {
                warn!("auto-cleanup: failed to save index for {}: {:#}", ws_id, e);
            }
            // Release write lock before save_manifest() so that
            // collect_workspace_entries() can acquire a read lock on this workspace.
            drop(ws);
            if let Err(e) = state.save_manifest().await {
                warn!("save_manifest failed after auto-cleanup: {:#}", e);
            }
            info!(
                "auto-cleanup: removed {} snapshots from {}",
                removed_count, ws_id
            );
        }
    }
}

/// Health check: verify filesystem usage.
///
/// Skipped when no workspace is registered. WARN on usage above threshold;
/// ERROR when get_usage fails (umount, fs crash, etc.) so upstream monitors can catch it.
async fn health_check(state: &DaemonState) {
    if state.all_workspaces().is_empty() {
        debug!("Health check skipped: no workspace registered");
        return;
    }

    match state.backend.get_usage().await {
        Ok((total, used)) => {
            if total > 0 {
                let usage_pct = (used as f64 / total as f64) * 100.0;
                const FS_WARN_THRESHOLD_PERCENT: f64 = 90.0;
                if usage_pct > FS_WARN_THRESHOLD_PERCENT {
                    warn!(
                        "Filesystem usage critical: {:.1}% ({} / {} bytes)",
                        usage_pct, used, total
                    );
                } else {
                    info!("Health check OK: filesystem usage {:.1}%", usage_pct);
                }
            }
        }
        Err(e) => {
            // `{:#}` prints the full anyhow cause chain (e.g. outer
            // `with_context` + inner `bail!`), not just the outermost message.
            error!(
                "Health check failed on backend {}: {:#}",
                state.backend.backend_type(),
                e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cleanup_orphans_removes_rollback_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let orphan1 = dir.path().join("ws-abc123.rollback-tmp");
        let normal = dir.path().join("ws-normal");

        std::fs::create_dir(&orphan1).unwrap();
        std::fs::create_dir(&normal).unwrap();

        let cleaned = cleanup_orphans(dir.path()).await.unwrap();

        assert_eq!(cleaned.len(), 1);
        assert!(!orphan1.exists(), "rollback-tmp should be removed");
        assert!(normal.exists(), "normal directory should remain");
    }

    #[tokio::test]
    async fn cleanup_orphans_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let cleaned = cleanup_orphans(dir.path()).await.unwrap();
        assert!(cleaned.is_empty());
    }

    #[tokio::test]
    async fn cleanup_orphans_nonexistent_path() {
        let result = cleanup_orphans(Path::new("/nonexistent/path/12345")).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn cleanup_orphans_only_normal_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("ws-abc")).unwrap();
        std::fs::create_dir(dir.path().join("snapshots")).unwrap();

        let cleaned = cleanup_orphans(dir.path()).await.unwrap();
        assert!(cleaned.is_empty());
    }
}
