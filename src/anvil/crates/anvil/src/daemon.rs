// SPDX-License-Identifier: Apache-2.0
//! Daemon runtime: bind UDS, accept connections, wire signal handlers.

use std::path::Path;
use std::sync::Arc;

use anvil_core::backend::BackendKind;
use anvil_core::config::{DaemonConfig, PolicyLoadErrorMode};
use anvil_core::kernel::HookRegistry;
use anvil_core::policy::PolicyEngine;
use anvil_core::pool::PoolManager;
use anvil_core::template::TemplateRegistry;
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::UnixListener;
use tokio::signal::unix::{SignalKind, signal};

use crate::api;
use crate::error::{AnvilDaemonError, Result};
use crate::spawner::{BackendSpawner, DynSpawner, LinuxSandboxSpawner, MockSpawner};
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
    let spawner = build_spawner(&config).await;

    let socket_path = config.daemon.socket.clone();
    let state = Arc::new(ServerState::build(
        config, policy, pool, template, hook, spawner,
    ));

    if socket_path.exists() {
        std::fs::remove_file(&socket_path)?;
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(&socket_path)?;
    tracing::info!(socket = %socket_path.display(), "anvil API listening");

    serve(listener, state).await
}

fn ensure_dirs(cfg: &DaemonConfig) -> Result<()> {
    std::fs::create_dir_all(&cfg.daemon.state_dir)?;
    std::fs::create_dir_all(&cfg.template.dir)?;
    if let Some(parent) = cfg.daemon.socket.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// Build the [`BackendSpawner`] used by API handlers. Probes the
/// `linux-sandbox` backend path declared in `[backends]`; uses
/// the real spawner when the binary exists, otherwise warns and falls
/// back to the mock implementation so the daemon stays functional on
/// macOS dev hosts and in CI without a real backend.
async fn build_spawner(cfg: &DaemonConfig) -> DynSpawner {
    let shim = cfg
        .backends
        .get(BackendKind::LinuxSandbox.as_str())
        .cloned();
    match shim {
        Some(path) => {
            let probe = LinuxSandboxSpawner;
            match probe.probe(&path).await {
                Ok(true) => {
                    tracing::info!(
                        binary = %path.display(),
                        "data plane: using LinuxSandboxSpawner",
                    );
                    Arc::new(LinuxSandboxSpawner)
                }
                Ok(false) => {
                    tracing::warn!(
                        binary = %path.display(),
                        "linux-sandbox binary missing, falling back to MockSpawner",
                    );
                    Arc::new(MockSpawner)
                }
                Err(err) => {
                    tracing::warn!(
                        ?err,
                        binary = %path.display(),
                        "linux-sandbox probe failed, falling back to MockSpawner",
                    );
                    Arc::new(MockSpawner)
                }
            }
        }
        None => {
            tracing::warn!(
                "no [backends].linux-sandbox configured, using MockSpawner (data plane is simulated)",
            );
            Arc::new(MockSpawner)
        }
    }
}

fn load_policy_engine(cfg: &DaemonConfig) -> Result<PolicyEngine> {
    if !cfg.policy.dir.exists() {
        if cfg.policy.on_load_error == PolicyLoadErrorMode::Fail {
            return Err(AnvilDaemonError::Internal(format!(
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

async fn serve(listener: UnixListener, state: Arc<ServerState>) -> Result<()> {
    let mut sighup = signal(SignalKind::hangup())
        .map_err(|e| AnvilDaemonError::Internal(format!("install SIGHUP handler: {e}")))?;
    let mut sigterm = signal(SignalKind::terminate())
        .map_err(|e| AnvilDaemonError::Internal(format!("install SIGTERM handler: {e}")))?;
    let mut sigint = signal(SignalKind::interrupt())
        .map_err(|e| AnvilDaemonError::Internal(format!("install SIGINT handler: {e}")))?;

    loop {
        tokio::select! {
            res = listener.accept() => {
                let (stream, _peer) = match res {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!(error = %e, "accept failed");
                        continue;
                    }
                };
                let state_clone = state.clone();
                tokio::spawn(async move {
                    let io = TokioIo::new(stream);
                    let svc = service_fn(move |req| {
                        let state = state_clone.clone();
                        async move {
                            api::handle(req, state).await
                        }
                    });
                    if let Err(err) = http1::Builder::new()
                        .serve_connection::<_, _>(io, svc)
                        .await
                    {
                        tracing::debug!(?err, "connection closed with error");
                    }
                    // Make the unused-type-parameter inference cooperate
                    // with hyper 1.x's response body bound.
                    let _: Option<Full<Bytes>> = None;
                });
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

    tracing::info!("anvil daemon stopped");
    Ok(())
}

fn reload_policies(state: &Arc<ServerState>) -> Result<()> {
    let dir = {
        let cfg = state
            .config
            .lock()
            .map_err(|_| AnvilDaemonError::Internal("config lock poisoned".into()))?;
        cfg.policy.dir.clone()
    };
    let engine = PolicyEngine::load_dir(&dir)?;
    let count = engine.policies().len();
    {
        let mut policy = state
            .policy
            .lock()
            .map_err(|_| AnvilDaemonError::Internal("policy lock poisoned".into()))?;
        *policy = engine;
    }
    tracing::info!(policies = count, "policy engine reloaded via SIGHUP");
    Ok(())
}
