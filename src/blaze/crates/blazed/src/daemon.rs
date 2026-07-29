// SPDX-License-Identifier: Apache-2.0
//! Daemon runtime: bind UDS, accept connections, wire signal handlers.

use std::path::Path;
use std::sync::Arc;

use blaze_core::backend::BackendKind;
use blaze_core::config::{DaemonConfig, PolicyLoadErrorMode};
use blaze_core::kernel::HookRegistry;
use blaze_core::policy::PolicyEngine;
use blaze_core::pool::PoolManager;
use blaze_core::storage::StorageProvider;
use blaze_core::template::TemplateRegistry;
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, UnixListener};
use tokio::signal::unix::{SignalKind, signal};

use crate::api;
use crate::error::{BlazeDaemonError, Result};
use crate::spawner::{
    BubblewrapSpawner, DynSpawner, FirecrackerSpawner, MockSpawner, SpawnerRegistry,
};
use crate::state::ServerState;

/// Boot the daemon: load config + policies, prepare state directories,
/// bind the API socket, and run the accept loop until SIGTERM/SIGINT.
pub async fn run(config_path: &Path) -> Result<()> {
    let config = DaemonConfig::load(config_path)?;
    tracing::info!(?config_path, "loaded daemon config");

    ensure_dirs(&config)?;
    let policy = load_policy_engine(&config)?;
    let pool = PoolManager::new();
    let template = TemplateRegistry::new();
    let hook = HookRegistry::new();
    let (spawners, active_backend) = build_spawners(&config).await;

    // Build storage provider
    if config.storage.provider != "file" && config.storage.provider != "auto" {
        tracing::warn!(
            provider = %config.storage.provider,
            "unsupported storage provider, falling back to file"
        );
    }
    let storage: Arc<dyn StorageProvider> = {
        use crate::file_provider::FileStorageProvider;
        // Keep immutable images and provider-owned runtime slots separate.
        tokio::fs::create_dir_all(&config.storage.images_dir).await?;
        tokio::fs::create_dir_all(&config.storage.instances_dir).await?;
        let fp = FileStorageProvider::with_images(
            config.storage.images_dir.clone(),
            config.storage.instances_dir.clone(),
        );
        match fp.probe().await {
            Ok(true) => {
                tracing::info!(dir = %config.storage.images_dir.display(), "storage provider ready");
            }
            _ => {
                tracing::warn!("storage probe returned false, continuing anyway");
            }
        }
        Arc::new(fp)
    };

    let socket_path = config.daemon.socket.clone();
    let http_addr = config.listen.http_addr.clone();
    let state = Arc::new(ServerState::build(
        config,
        policy,
        pool,
        template,
        hook,
        spawners,
        active_backend,
        storage,
    ));

    if socket_path.exists() {
        std::fs::remove_file(&socket_path)?;
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(&socket_path)?;
    tracing::info!(socket = %socket_path.display(), "blaze UDS API listening");

    // Optional TCP listener for remote platform API
    let tcp_listener = if !http_addr.is_empty() {
        let tcp = TcpListener::bind(&http_addr)
            .await
            .map_err(|e| BlazeDaemonError::Internal(format!("bind TCP {http_addr}: {e}")))?;
        tracing::info!(addr = %http_addr, "blaze HTTP API listening");
        Some(tcp)
    } else {
        None
    };

    serve(listener, tcp_listener, state).await
}

fn ensure_dirs(cfg: &DaemonConfig) -> Result<()> {
    cfg.validate()?;
    std::fs::create_dir_all(&cfg.daemon.state_dir)?;
    std::fs::create_dir_all(&cfg.template.dir)?;
    std::fs::create_dir_all(&cfg.storage.images_dir)?;
    std::fs::create_dir_all(&cfg.storage.instances_dir)?;
    let images_dir = std::fs::canonicalize(&cfg.storage.images_dir)?;
    let instances_dir = std::fs::canonicalize(&cfg.storage.instances_dir)?;
    blaze_core::config::validate_storage_paths(&images_dir, &instances_dir)?;
    if let Some(parent) = cfg.daemon.socket.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// Build the [`crate::spawner::BackendSpawner`] implementations used by API
/// handlers. Probes
/// `[backends]` for known backends in priority order:
///   1. `firecracker` → [`FirecrackerSpawner`]
///   2. `bubblewrap` → [`BubblewrapSpawner`]
///   3. fallback → [`MockSpawner`]
async fn build_spawners(cfg: &DaemonConfig) -> (SpawnerRegistry, BackendKind) {
    let firecracker: DynSpawner = Arc::new(FirecrackerSpawner::new(cfg.storage.images_dir.clone()));
    let bubblewrap: DynSpawner = Arc::new(BubblewrapSpawner);
    let mock: DynSpawner = Arc::new(MockSpawner);
    let mut spawners = SpawnerRegistry::new();
    spawners.insert(BackendKind::Firecracker, firecracker.clone());
    spawners.insert(BackendKind::Bubblewrap, bubblewrap.clone());
    spawners.insert(BackendKind::Mock, mock);

    // --- Firecracker --------------------------------------------------------
    if let Some(fc_path) = cfg.backends.get(BackendKind::Firecracker.as_str()).cloned() {
        match firecracker.probe(&fc_path).await {
            Ok(true) => {
                tracing::info!(
                    binary = %fc_path.display(),
                    images_dir = %cfg.storage.images_dir.display(),
                    "data plane: using FirecrackerSpawner",
                );
                return (spawners, BackendKind::Firecracker);
            }
            Ok(false) => {
                tracing::warn!(
                    binary = %fc_path.display(),
                    "firecracker binary probe failed, trying next backend",
                );
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    binary = %fc_path.display(),
                    "firecracker probe error, trying next backend",
                );
            }
        }
    }

    // --- Bubblewrap (bwrap) --------------------------------------------------
    if let Some(path) = cfg.backends.get(BackendKind::Bubblewrap.as_str()).cloned() {
        match bubblewrap.probe(&path).await {
            Ok(true) => {
                tracing::info!(
                    binary = %path.display(),
                    "data plane: using BubblewrapSpawner",
                );
                return (spawners, BackendKind::Bubblewrap);
            }
            Ok(false) => {
                tracing::warn!(
                    binary = %path.display(),
                    "bubblewrap binary missing, falling back to MockSpawner",
                );
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    binary = %path.display(),
                    "bubblewrap probe failed, falling back to MockSpawner",
                );
            }
        }
    }

    // --- Fallback: MockSpawner -----------------------------------------------
    tracing::warn!(
        "no usable backend found in [backends], using MockSpawner (data plane is simulated)",
    );
    (spawners, BackendKind::Mock)
}

fn load_policy_engine(cfg: &DaemonConfig) -> Result<PolicyEngine> {
    if !cfg.policy.dir.exists() {
        if cfg.policy.on_load_error == PolicyLoadErrorMode::Fail {
            return Err(BlazeDaemonError::Internal(format!(
                "policy.dir does not exist: {}",
                cfg.policy.dir.display()
            )));
        }
        tracing::warn!(
            dir = %cfg.policy.dir.display(),
            "policy dir missing, starting with empty policy engine"
        );
        return Ok(PolicyEngine::new());
    }
    match PolicyEngine::load_dir(&cfg.policy.dir) {
        Ok(engine) => Ok(engine),
        Err(err) if cfg.policy.on_load_error == PolicyLoadErrorMode::Warn => {
            tracing::warn!(?err, "policy load failed, continuing with empty engine");
            Ok(PolicyEngine::new())
        }
        Err(err) => Err(err.into()),
    }
}

async fn serve(uds: UnixListener, tcp: Option<TcpListener>, state: Arc<ServerState>) -> Result<()> {
    let mut sighup = signal(SignalKind::hangup())
        .map_err(|e| BlazeDaemonError::Internal(format!("install SIGHUP handler: {e}")))?;
    let mut sigterm = signal(SignalKind::terminate())
        .map_err(|e| BlazeDaemonError::Internal(format!("install SIGTERM handler: {e}")))?;
    let mut sigint = signal(SignalKind::interrupt())
        .map_err(|e| BlazeDaemonError::Internal(format!("install SIGINT handler: {e}")))?;

    loop {
        tokio::select! {
            res = uds.accept() => {
                let (stream, _peer) = match res {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!(error = %e, "UDS accept failed");
                        continue;
                    }
                };
                spawn_conn(TokioIo::new(stream), state.clone());
            }
            res = async { match &tcp { Some(l) => l.accept().await, None => std::future::pending().await }}, if tcp.is_some() => {
                let (stream, peer) = match res {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!(error = %e, "TCP accept failed");
                        continue;
                    }
                };
                tracing::debug!(?peer, "TCP connection");
                spawn_conn(TokioIo::new(stream), state.clone());
            }
            _ = sighup.recv() => {
                tracing::info!("SIGHUP received: reloading policies");
                if let Err(err) = reload_policies(&state) {
                    tracing::error!(?err, "policy reload failed");
                }
            }
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM received: shutting down");
                break;
            }
            _ = sigint.recv() => {
                tracing::info!("SIGINT received: shutting down");
                break;
            }
        }
    }

    tracing::info!("blaze daemon stopped");
    Ok(())
}

fn spawn_conn<I>(io: TokioIo<I>, state: Arc<ServerState>)
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let svc = service_fn(move |req| {
            let state = state.clone();
            async move { api::handle(req, state).await }
        });
        if let Err(err) = http1::Builder::new().serve_connection(io, svc).await {
            tracing::debug!(?err, "connection closed with error");
        }
        let _: Option<Full<Bytes>> = None;
    });
}

fn reload_policies(state: &Arc<ServerState>) -> Result<()> {
    let dir = {
        let cfg = state
            .config
            .lock()
            .map_err(|_| BlazeDaemonError::Internal("config lock poisoned".into()))?;
        cfg.policy.dir.clone()
    };
    let engine = PolicyEngine::load_dir(&dir)?;
    let count = engine.policies().len();
    {
        let mut policy = state
            .policy
            .lock()
            .map_err(|_| BlazeDaemonError::Internal("policy lock poisoned".into()))?;
        *policy = engine;
    }
    tracing::info!(policies = count, "policy engine reloaded via SIGHUP");
    Ok(())
}
