// SPDX-License-Identifier: Apache-2.0
//! Daemon runtime: bind UDS, accept connections, wire signal handlers.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use blaze_core::backend::BackendKind;
use blaze_core::config::{DaemonConfig, PolicyLoadErrorMode, StorageSyncSchedule};
use blaze_core::kernel::HookRegistry;
use blaze_core::policy::PolicyEngine;
use blaze_core::storage::StorageProvider;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, UnixListener};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{Semaphore, oneshot, watch};
use tokio::task::{JoinError, JoinSet};

use crate::api;
use crate::error::{BlazeDaemonError, Result};
use crate::sandbox::StorageSyncLoop;
use crate::sandbox::template::{
    PinnedConfigSource, PolicyLoadDisposition, TemplateCatalog,
    validate_template_roots_with_policy_mode,
};
use crate::spawner::{
    BubblewrapSpawner, DynSpawner, FirecrackerSpawner, MockSpawner, SpawnerRegistry,
};
use crate::state::ServerState;
use crate::state_store::StateStore;

// The packaged service reserves additional time for abort, join, and process exit.
const CONNECTION_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECTION_ABORT_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

struct ConnectionSupervisor {
    shutdown: watch::Sender<bool>,
    tasks: JoinSet<()>,
    task_failed: bool,
    request_permits: Arc<Semaphore>,
}

impl ConnectionSupervisor {
    fn new(max_requests: usize) -> Self {
        assert!(max_requests > 0, "request handler limit must be positive");
        let (shutdown, _) = watch::channel(false);
        Self {
            shutdown,
            tasks: JoinSet::new(),
            task_failed: false,
            request_permits: Arc::new(Semaphore::new(max_requests)),
        }
    }

    fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    fn spawn<I>(&mut self, io: TokioIo<I>, state: Arc<ServerState>)
    where
        I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let shutdown = self.shutdown.subscribe();
        self.spawn_task(serve_connection(
            io,
            state,
            shutdown,
            self.request_permits.clone(),
        ));
    }

    fn spawn_task<F>(&mut self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.tasks.spawn(task);
    }

    async fn reap_next(&mut self) {
        if let Some(result) = self.tasks.join_next().await {
            self.task_failed |= report_connection_result(result);
        }
    }

    fn reap_ready(&mut self) {
        while let Some(result) = self.tasks.try_join_next() {
            self.task_failed |= report_connection_result(result);
        }
    }

    async fn shutdown(self, grace: Duration) -> Result<()> {
        self.shutdown_with_abort_join_timeout(grace, CONNECTION_ABORT_JOIN_TIMEOUT)
            .await
    }

    async fn shutdown_with_abort_join_timeout(
        mut self,
        grace: Duration,
        abort_join_timeout: Duration,
    ) -> Result<()> {
        self.shutdown.send_replace(true);
        let mut task_failed = self.task_failed;
        let completed = tokio::time::timeout(grace, async {
            while let Some(result) = self.tasks.join_next().await {
                task_failed |= report_connection_result(result);
            }
        })
        .await;

        if completed.is_err() {
            let remaining = self.tasks.len();
            tracing::warn!(
                remaining,
                timeout_secs = grace.as_secs(),
                "connection drain timed out; aborting remaining tasks"
            );
            self.tasks.abort_all();
            let abort_join = tokio::time::timeout(abort_join_timeout, async {
                while let Some(result) = self.tasks.join_next().await {
                    if let Err(error) = result
                        && !error.is_cancelled()
                    {
                        task_failed |= report_connection_result(Err(error));
                    }
                }
            })
            .await;
            let mut message = format!(
                "connection drain timed out after {} seconds; aborted {remaining} task(s)",
                grace.as_secs()
            );
            if abort_join.is_err() {
                let unjoined = self.tasks.len();
                tracing::error!(
                    unjoined,
                    timeout_ms = abort_join_timeout.as_millis(),
                    "connection tasks did not join after abort"
                );
                message.push_str(&format!(
                    "; {unjoined} task(s) did not join within {abort_join_timeout:?} after abort"
                ));
            }
            if task_failed {
                message.push_str("; one or more connection tasks failed before shutdown completed");
            }
            return Err(BlazeDaemonError::Internal(message));
        }

        if task_failed {
            return Err(BlazeDaemonError::Internal(
                "one or more connection tasks failed before shutdown completed".to_string(),
            ));
        }
        Ok(())
    }
}

fn request_handler_limit(worker_threads: usize) -> usize {
    worker_threads.saturating_sub(1).max(1)
}

fn report_connection_result(result: std::result::Result<(), JoinError>) -> bool {
    match result {
        Ok(()) => false,
        Err(error) if error.is_cancelled() => {
            tracing::debug!("connection task cancelled");
            false
        }
        Err(error) => {
            tracing::error!(%error, "connection task failed");
            true
        }
    }
}

/// Boot the daemon: load config + policies, prepare state directories,
/// bind the API socket, and run the accept loop until SIGTERM/SIGINT.
pub async fn run(config_path: &Path) -> Result<()> {
    let loaded = load_daemon_config(config_path)?;
    run_loaded_config(loaded).await
}

struct LoadedDaemonConfig {
    config: DaemonConfig,
    source: PinnedConfigSource,
}

fn load_daemon_config(config_path: &Path) -> Result<LoadedDaemonConfig> {
    let mut source = PinnedConfigSource::open(config_path)?;
    let raw = source.read_to_string()?;
    let mut config: DaemonConfig = toml::from_str(&raw).map_err(blaze_core::BlazeError::from)?;
    absolutize_backend_paths(&mut config)?;
    config.validate()?;
    if config.pool.is_some() {
        tracing::warn!(
            path = %config_path.display(),
            "ignoring legacy packaged [pool] defaults; remove this section because reusable-instance management is unavailable"
        );
    }
    tracing::info!(?config_path, "loaded daemon config");
    Ok(LoadedDaemonConfig { config, source })
}

fn absolutize_backend_paths(config: &mut DaemonConfig) -> Result<()> {
    let current_dir = std::env::current_dir()?;
    for path in config.backends.values_mut() {
        if !path.is_absolute() {
            *path = current_dir.join(&*path);
        }
    }
    Ok(())
}

async fn run_loaded_config(loaded: LoadedDaemonConfig) -> Result<()> {
    let LoadedDaemonConfig { config, source } = loaded;
    let sync_schedule = config.storage.sync_schedule()?;
    let sync_timeout = config.storage.sync_timeout_duration()?;

    let template_roots = validate_template_roots_with_policy_mode(
        &config.template,
        &config.storage.images_dir,
        &config.storage.instances_dir,
        &config.policy.dir,
        &config.backends,
        &config.daemon.state_dir,
        &config.daemon.socket,
        Some(&source),
        config.policy.on_load_error,
    )?;
    let policy_load = template_roots.policy_load_disposition();
    let template_catalog = TemplateCatalog::open_validated(&config.template, template_roots)?;
    ensure_dirs(&config)?;
    // Retain the accepted state-root object before policy, backend, and
    // storage initialization so later code cannot reopen a replacement path.
    let state_store = StateStore::open(config.daemon.state_dir.clone())?;
    let policy = load_policy_engine(&config, policy_load)?;
    let hook = HookRegistry::new();
    let network_required = policy.policies().iter().any(|policy| {
        policy
            .backend
            .firecracker
            .as_ref()
            .is_some_and(|config| config.enable_network)
            && policy
                .select
                .backend_priority
                .contains(&BackendKind::Firecracker)
    });
    let (spawners, active_backend) = build_spawners(&config, network_required).await;

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
    let state = Arc::new(ServerState::build_with_store(
        config,
        policy,
        hook,
        spawners,
        active_backend,
        storage,
        template_catalog,
        state_store,
    )?);
    let reconciliation = state.manager.reconcile_startup().await;
    tracing::info!(
        attempted = reconciliation.attempted,
        completed = reconciliation.completed,
        failed = reconciliation.failures.len(),
        "startup sandbox reconciliation completed"
    );
    for failure in reconciliation.failures {
        tracing::warn!(
            instance = %failure.instance_id,
            error = %failure.error,
            "sandbox remains recovery-required after startup reconciliation"
        );
    }

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

    serve(listener, tcp_listener, state, sync_schedule, sync_timeout).await
}

fn ensure_dirs(cfg: &DaemonConfig) -> Result<()> {
    cfg.validate()?;
    std::fs::create_dir_all(&cfg.daemon.state_dir)?;
    // TemplateCatalog::open_validated creates and retains the accepted catalog
    // object. Reopening its configured path here would discard that binding.
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
async fn build_spawners(
    cfg: &DaemonConfig,
    network_required: bool,
) -> (SpawnerRegistry, BackendKind) {
    let firecracker: DynSpawner = Arc::new(FirecrackerSpawner::with_network_requirement(
        cfg.storage.images_dir.clone(),
        network_required,
    ));
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

fn load_policy_engine(
    cfg: &DaemonConfig,
    disposition: PolicyLoadDisposition,
) -> Result<PolicyEngine> {
    if disposition == PolicyLoadDisposition::UseEmpty {
        if cfg.policy.on_load_error == PolicyLoadErrorMode::Fail {
            return Err(BlazeDaemonError::Internal(format!(
                "policy boundary discovery did not complete for {}",
                cfg.policy.dir.display()
            )));
        }
        tracing::warn!(
            dir = %cfg.policy.dir.display(),
            "policy boundary discovery did not complete; starting with empty policy engine"
        );
        return Ok(PolicyEngine::new());
    }
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

async fn serve(
    uds: UnixListener,
    tcp: Option<TcpListener>,
    state: Arc<ServerState>,
    sync_schedule: StorageSyncSchedule,
    sync_timeout: Duration,
) -> Result<()> {
    let mut sighup = signal(SignalKind::hangup())
        .map_err(|e| BlazeDaemonError::Internal(format!("install SIGHUP handler: {e}")))?;
    let mut sigterm = signal(SignalKind::terminate())
        .map_err(|e| BlazeDaemonError::Internal(format!("install SIGTERM handler: {e}")))?;
    let mut sigint = signal(SignalKind::interrupt())
        .map_err(|e| BlazeDaemonError::Internal(format!("install SIGINT handler: {e}")))?;
    let mut sync_loop = match sync_schedule {
        StorageSyncSchedule::Disabled => {
            tracing::info!("periodic storage artifact synchronization is disabled");
            None
        }
        StorageSyncSchedule::Every(interval) => Some(
            state
                .manager
                .start_storage_sync_loop(interval, sync_timeout),
        ),
    };
    let mut service_result = Ok(());
    // Request handlers may call synchronous persistence code. Keep one runtime
    // worker outside their admission limit so signals and bounded shutdown
    // timers can still run when every admitted handler is blocked.
    let worker_threads = tokio::runtime::Handle::current().metrics().num_workers();
    let mut connections = ConnectionSupervisor::new(request_handler_limit(worker_threads));

    loop {
        tokio::select! {
            // Preserve a worker failure that is already observable, then give
            // termination signals priority over every event that admits work.
            biased;
            result = observe_storage_sync_exit(&mut sync_loop), if sync_loop.is_some() => {
                service_result = result;
                break;
            }
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM received: shutting down");
                break;
            }
            _ = sigint.recv() => {
                tracing::info!("SIGINT received: shutting down");
                break;
            }
            () = serve_one_event(
                &uds,
                tcp.as_ref(),
                &state,
                &mut sighup,
                &mut connections,
            ) => {}
        }
    }

    drop(uds);
    drop(tcp);
    // Stop admitting import work before waiting for accepted requests. Cancel
    // the periodic scheduler as those requests drain so it cannot start a new
    // sweep during the graceful window; both supervisors retain and join their
    // owned tasks before the server returns.
    state.manager.cancel_template_imports();
    if let Some(sync_loop) = sync_loop.as_mut() {
        let (connection_result, sync_result) = tokio::join!(
            connections.shutdown(CONNECTION_DRAIN_TIMEOUT),
            sync_loop.shutdown(),
        );
        service_result = merge_stage_result(
            service_result,
            "accepted connection shutdown",
            connection_result,
        );
        service_result = merge_stage_result(
            service_result,
            "storage artifact synchronization shutdown",
            sync_result,
        );
    } else {
        service_result = merge_stage_result(
            service_result,
            "accepted connection shutdown",
            connections.shutdown(CONNECTION_DRAIN_TIMEOUT).await,
        );
    }
    service_result = merge_stage_result(
        service_result,
        "runtime template import shutdown",
        state.manager.wait_for_template_imports().await,
    );
    tracing::info!("blaze daemon stopped");
    service_result
}

async fn observe_storage_sync_exit(sync_loop: &mut Option<StorageSyncLoop>) -> Result<()> {
    sync_loop
        .as_mut()
        .expect("storage sync loop exists while its select branch is enabled")
        .observe_exit()
        .await
}

fn merge_stage_result(current: Result<()>, stage: &str, next: Result<()>) -> Result<()> {
    match (current, next) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(previous), Err(next)) => Err(BlazeDaemonError::RecoveryRequired(format!(
            "service failed: {previous}; {stage} failed: {next}"
        ))),
    }
}

async fn serve_one_event(
    uds: &UnixListener,
    tcp: Option<&TcpListener>,
    state: &Arc<ServerState>,
    sighup: &mut tokio::signal::unix::Signal,
    connections: &mut ConnectionSupervisor,
) {
    // Drain every completion already queued before accepting more work. The
    // select below remains fair for events that become ready afterwards.
    connections.reap_ready();

    // Keep peer listeners and service events fair while the outer loop gives
    // termination signals deterministic priority.
    tokio::select! {
        () = connections.reap_next(), if !connections.is_empty() => {}
        _ = sighup.recv() => {
            tracing::info!("SIGHUP received: reloading policies");
            if let Err(err) = reload_policies(state).await {
                tracing::error!(?err, "policy reload failed");
            }
        }
        result = uds.accept() => match result {
            Ok((stream, _peer)) => connections.spawn(TokioIo::new(stream), state.clone()),
            Err(error) => tracing::error!(%error, "UDS accept failed"),
        },
        result = async {
            match tcp {
                Some(listener) => listener.accept().await,
                None => std::future::pending().await,
            }
        }, if tcp.is_some() => match result {
            Ok((stream, peer)) => {
                tracing::debug!(?peer, "TCP connection");
                connections.spawn(TokioIo::new(stream), state.clone());
            }
            Err(error) => tracing::error!(%error, "TCP accept failed"),
        },
    }
}

async fn serve_connection<I>(
    io: TokioIo<I>,
    state: Arc<ServerState>,
    mut shutdown: watch::Receiver<bool>,
    request_permits: Arc<Semaphore>,
) where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let svc = service_fn(move |req| {
        let state = state.clone();
        let request_permits = request_permits.clone();
        async move { api::handle(req, state, request_permits).await }
    });
    let connection = http1::Builder::new().serve_connection(io, svc);
    tokio::pin!(connection);
    let result = tokio::select! {
        result = &mut connection => result,
        _ = wait_for_shutdown(&mut shutdown) => {
            connection.as_mut().graceful_shutdown();
            connection.await
        }
    };
    if let Err(error) = result {
        tracing::debug!(%error, "connection closed with error");
    }
}

async fn wait_for_shutdown(shutdown: &mut watch::Receiver<bool>) {
    loop {
        let stopping = *shutdown.borrow_and_update();
        if stopping {
            return;
        }
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}

async fn reload_policies(state: &Arc<ServerState>) -> Result<()> {
    reload_policies_with_loader(state, |dir| {
        PolicyEngine::load_dir(&dir).map_err(Into::into)
    })
    .await
}

async fn reload_policies_with_loader<F>(state: &Arc<ServerState>, loader: F) -> Result<()>
where
    F: FnOnce(PathBuf) -> Result<PolicyEngine> + Send + 'static,
{
    let dir = {
        let cfg = state.config.try_lock().map_err(|error| {
            BlazeDaemonError::Internal(format!("config unavailable for policy reload: {error}"))
        })?;
        cfg.policy.dir.clone()
    };
    let (result_tx, result_rx) = oneshot::channel();
    // The loader must not capture or mutate ServerState: shutdown may cancel
    // this async wait while the detached filesystem reader is still running.
    std::thread::Builder::new()
        .name("blaze-policy-reload".to_string())
        .spawn(move || {
            let _ = result_tx.send(loader(dir));
        })
        .map_err(|error| {
            BlazeDaemonError::Internal(format!("start blaze-policy-reload: {error}"))
        })?;
    let engine = result_rx.await.map_err(|_| {
        BlazeDaemonError::Internal("blaze-policy-reload exited without a result".into())
    })??;
    let count = engine.policies().len();
    {
        let mut policy = state.policy.try_lock().map_err(|error| {
            BlazeDaemonError::Internal(format!("policy unavailable for reload: {error}"))
        })?;
        *policy = engine;
    }
    tracing::info!(policies = count, "policy engine reloaded via SIGHUP");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::future;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};

    use http_body_util::{BodyExt, Empty};
    use hyper::Request;
    use hyper::body::{Body, Bytes, Frame, SizeHint};
    use tokio::sync::oneshot;

    use crate::file_provider::FileStorageProvider;

    use super::*;

    const TEST_REQUEST_LIMIT: usize = 128;

    struct ActiveTask(Arc<AtomicBool>);

    struct GatedBody {
        polled: Option<oneshot::Sender<()>>,
        release: Option<oneshot::Receiver<()>>,
    }

    impl Body for GatedBody {
        type Data = Bytes;
        type Error = Infallible;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Option<std::result::Result<Frame<Self::Data>, Self::Error>>> {
            if let Some(polled) = self.polled.take() {
                let _ = polled.send(());
            }
            let Some(release) = &mut self.release else {
                return Poll::Ready(None);
            };
            match Pin::new(release).poll(cx) {
                Poll::Ready(_) => {
                    self.release = None;
                    Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(b"{}")))))
                }
                Poll::Pending => Poll::Pending,
            }
        }

        fn size_hint(&self) -> SizeHint {
            SizeHint::with_exact(2)
        }
    }

    impl Drop for ActiveTask {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }

    fn connection_test_state() -> (tempfile::TempDir, Arc<ServerState>) {
        let temp = tempfile::tempdir().expect("temporary daemon state");
        let mut config = DaemonConfig::default();
        config.daemon.state_dir = temp.path().join("state");
        config.daemon.socket = temp.path().join("run/api.sock");
        config.policy.dir = temp.path().join("policies");
        config.storage.images_dir = temp.path().join("images");
        config.storage.instances_dir = temp.path().join("instances");
        config.template.dir = temp.path().join("templates");
        std::fs::create_dir_all(&config.daemon.state_dir).expect("state directory");
        std::fs::create_dir_all(&config.policy.dir).expect("policy directory");
        std::fs::create_dir_all(&config.storage.images_dir).expect("images directory");
        std::fs::create_dir_all(&config.storage.instances_dir).expect("instances directory");

        let storage: Arc<dyn StorageProvider> = Arc::new(FileStorageProvider::with_images(
            config.storage.images_dir.clone(),
            config.storage.instances_dir.clone(),
        ));
        let mut spawners = SpawnerRegistry::new();
        spawners.insert(BackendKind::Mock, Arc::new(MockSpawner));
        let state = Arc::new(
            ServerState::build(
                config,
                PolicyEngine::new(),
                PoolManager::new(),
                HookRegistry::new(),
                spawners,
                BackendKind::Mock,
                storage,
            )
            .expect("server state"),
        );
        (temp, state)
    }

    #[test]
    fn policy_boundary_fallback_prevents_a_later_directory_rescan() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = DaemonConfig::default();
        config.template.dir = temp.path().join("catalog");
        config.template.import_root = Some(temp.path().join("imports"));
        config.storage.images_dir = temp.path().join("images");
        config.storage.instances_dir = temp.path().join("instances");
        config.policy.dir = temp.path().join("policies");
        config.policy.on_load_error = PolicyLoadErrorMode::Warn;
        config.daemon.state_dir = temp.path().join("state");
        config.daemon.socket = temp.path().join("run/api.sock");
        for directory in [
            &config.template.dir,
            config.template.import_root.as_ref().expect("import root"),
            &config.storage.images_dir,
            &config.storage.instances_dir,
            &config.policy.dir,
            &config.daemon.state_dir,
        ] {
            std::fs::create_dir(directory).expect("startup directory");
        }
        let policy_path = config.policy.dir.join("recovered.toml");
        symlink("recovered.toml", &policy_path).expect("policy loop");

        let roots = validate_template_roots_with_policy_mode(
            &config.template,
            &config.storage.images_dir,
            &config.storage.instances_dir,
            &config.policy.dir,
            &config.backends,
            &config.daemon.state_dir,
            &config.daemon.socket,
            None,
            config.policy.on_load_error,
        )
        .expect("warn mode boundary validation");
        let disposition = roots.policy_load_disposition();
        let _catalog =
            TemplateCatalog::open_validated(&config.template, roots).expect("catalog owner");

        std::fs::remove_file(&policy_path).expect("remove policy loop");
        std::fs::write(
            &policy_path,
            r#"
manifest_version = 1
policy_name = "recovered"

[match]
workload_class = "agent-rl"

[select]
backend_priority = ["bubblewrap"]
"#,
        )
        .expect("policy file");

        let empty = load_policy_engine(&config, disposition).expect("use empty policy engine");
        assert!(empty.policies().is_empty());

        let recovered_roots = validate_template_roots_with_policy_mode(
            &config.template,
            &config.storage.images_dir,
            &config.storage.instances_dir,
            &config.policy.dir,
            &config.backends,
            &config.daemon.state_dir,
            &config.daemon.socket,
            None,
            config.policy.on_load_error,
        )
        .expect("recovered policy boundary validation");
        let loaded = load_policy_engine(&config, recovered_roots.policy_load_disposition())
            .expect("load validated policy directory");
        assert_eq!(loaded.policies().len(), 1);

        config.policy.on_load_error = PolicyLoadErrorMode::Fail;
        load_policy_engine(&config, PolicyLoadDisposition::UseEmpty)
            .expect_err("fail mode must not accept an incomplete boundary discovery");
    }

    #[test]
    fn generic_directory_setup_does_not_reopen_the_template_catalog() {
        let temp = tempfile::tempdir().expect("tempdir");
        let configured_parent = temp.path().join("configured");
        let detached_parent = temp.path().join("detached");
        std::fs::create_dir(&configured_parent).expect("configured parent");
        let mut config = DaemonConfig::default();
        config.template.dir = configured_parent.join("catalog");
        config.template.import_root = Some(temp.path().join("imports"));
        config.storage.images_dir = temp.path().join("images");
        config.storage.instances_dir = temp.path().join("instances");
        config.policy.dir = temp.path().join("policies");
        config.daemon.state_dir = temp.path().join("state");
        config.daemon.socket = temp.path().join("run/api.sock");
        std::fs::create_dir(config.template.import_root.as_ref().expect("import root"))
            .expect("import root");
        std::fs::create_dir(&config.policy.dir).expect("policy directory");

        let roots = validate_template_roots_with_policy_mode(
            &config.template,
            &config.storage.images_dir,
            &config.storage.instances_dir,
            &config.policy.dir,
            &config.backends,
            &config.daemon.state_dir,
            &config.daemon.socket,
            None,
            config.policy.on_load_error,
        )
        .expect("validated roots");
        let _catalog =
            TemplateCatalog::open_validated(&config.template, roots).expect("catalog owner");

        std::fs::rename(&configured_parent, &detached_parent).expect("detach catalog ancestor");
        std::fs::create_dir(&configured_parent).expect("replacement catalog ancestor");

        ensure_dirs(&config).expect("prepare non-catalog directories");

        assert!(!config.template.dir.exists());
        assert!(detached_parent.join("catalog").is_dir());
        assert!(config.storage.images_dir.is_dir());
        assert!(config.storage.instances_dir.is_dir());
        assert!(config.daemon.state_dir.is_dir());
    }

    #[test]
    fn service_and_shutdown_failures_are_all_retained() {
        let service = Err(BlazeDaemonError::Internal(
            "storage synchronization worker failed".to_string(),
        ));
        let connection_shutdown = Err(BlazeDaemonError::Internal(
            "connection drain failed".to_string(),
        ));
        let storage_shutdown = Err(BlazeDaemonError::Internal(
            "worker shutdown failed".to_string(),
        ));
        let import_shutdown = Err(BlazeDaemonError::Internal(
            "import shutdown failed".to_string(),
        ));

        let result =
            merge_stage_result(service, "accepted connection shutdown", connection_shutdown);
        let result = merge_stage_result(
            result,
            "storage artifact synchronization shutdown",
            storage_shutdown,
        );
        let error = merge_stage_result(result, "runtime template import shutdown", import_shutdown)
            .expect_err("all failures must be reported");

        assert!(error.to_string().contains("service failed"));
        assert!(error.to_string().contains("synchronization worker failed"));
        assert!(error.to_string().contains("accepted connection shutdown"));
        assert!(error.to_string().contains("connection drain failed"));
        assert!(
            error
                .to_string()
                .contains("storage artifact synchronization shutdown")
        );
        assert!(error.to_string().contains("worker shutdown failed"));
        assert!(
            error
                .to_string()
                .contains("runtime template import shutdown")
        );
        assert!(error.to_string().contains("import shutdown failed"));
    }

    #[tokio::test]
    async fn resolved_catalog_overlap_fails_before_owned_directory_creation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let catalog = temp.path().join("catalog");
        let alias = temp.path().join("alias");
        let import_root = temp.path().join("imports");
        std::fs::create_dir(&catalog).expect("catalog");
        std::fs::create_dir(&import_root).expect("import root");
        symlink(&catalog, &alias).expect("catalog alias");

        let mut config = DaemonConfig::default();
        config.template.dir = catalog.clone();
        config.template.import_root = Some(import_root);
        config.storage.images_dir = alias.join("images");
        config.storage.instances_dir = temp.path().join("instances");
        config.policy.dir = temp.path().join("policies");
        config.daemon.state_dir = temp.path().join("state");
        config.daemon.socket = temp.path().join("run/api.sock");
        let config_path = temp.path().join("config.toml");
        std::fs::write(
            &config_path,
            toml::to_string(&config).expect("serialize config"),
        )
        .expect("write config");

        let error = run(&config_path)
            .await
            .expect_err("resolved catalog overlap must fail startup");

        assert!(
            error.to_string().contains("storage.images_dir"),
            "unexpected startup error: {error}"
        );
        assert!(!catalog.join("images").exists());
        assert!(!config.storage.instances_dir.exists());
        assert!(config.template.dir.exists());
        assert!(!config.daemon.state_dir.exists());
    }

    #[tokio::test]
    async fn configured_backend_binaries_cannot_overlap_template_roots_before_mutation() {
        for (backend, binary_in_import_root) in [("firecracker", false), ("bubblewrap", true)] {
            let temp = tempfile::tempdir().expect("tempdir");
            let catalog = temp.path().join("catalog");
            let import_root = temp.path().join("imports");
            for root in [&catalog, &import_root] {
                std::fs::create_dir(root).expect("runtime template root");
                std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o750))
                    .expect("root mode");
            }
            let protected_root = if binary_in_import_root {
                &import_root
            } else {
                &catalog
            };
            let binary = protected_root.join(backend);
            std::fs::write(&binary, b"backend executable").expect("backend binary");
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o750))
                .expect("binary mode");

            let mut config = DaemonConfig::default();
            config.template.dir = catalog.clone();
            config.template.import_root = Some(import_root.clone());
            config.storage.images_dir = temp.path().join("images");
            config.storage.instances_dir = temp.path().join("instances");
            config.policy.dir = temp.path().join("policies");
            config.daemon.state_dir = temp.path().join("state");
            config.daemon.socket = temp.path().join("run/api.sock");
            config.backends.insert(backend.to_string(), binary.clone());
            let config_path = temp.path().join("config.toml");
            std::fs::write(
                &config_path,
                toml::to_string(&config).expect("serialize config"),
            )
            .expect("write config");

            let error = run(&config_path)
                .await
                .expect_err("backend binary must remain outside catalog ownership");

            let message = error.to_string();
            assert!(message.contains(&format!("backends.{backend}")));
            assert_eq!(
                std::fs::read(&binary).expect("binary remains readable"),
                b"backend executable"
            );
            for root in [&catalog, &import_root] {
                assert_eq!(
                    std::fs::metadata(root).expect("root metadata").mode() & 0o777,
                    0o750
                );
            }
            assert!(!config.storage.images_dir.exists());
            assert!(!config.storage.instances_dir.exists());
            assert!(config.template.dir.exists());
            assert!(!config.daemon.state_dir.exists());
        }
    }

    #[tokio::test]
    async fn configured_backend_link_locations_cannot_overlap_template_roots_before_mutation() {
        for (backend, link_in_import_root) in [("firecracker", false), ("bubblewrap", true)] {
            let temp = tempfile::tempdir().expect("tempdir");
            let catalog = temp.path().join("catalog");
            let import_root = temp.path().join("imports");
            let backend_root = temp.path().join("backends");
            for root in [&catalog, &import_root, &backend_root] {
                std::fs::create_dir(root).expect("root directory");
                std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o750))
                    .expect("root mode");
            }
            let target = backend_root.join(backend);
            std::fs::write(&target, b"backend executable").expect("backend binary");
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o750))
                .expect("binary mode");
            let protected_root = if link_in_import_root {
                &import_root
            } else {
                &catalog
            };
            let configured_link = protected_root.join(backend);
            symlink(&target, &configured_link).expect("backend symlink");

            let mut config = DaemonConfig::default();
            config.template.dir = catalog.clone();
            config.template.import_root = Some(import_root.clone());
            config.storage.images_dir = temp.path().join("images");
            config.storage.instances_dir = temp.path().join("instances");
            config.policy.dir = temp.path().join("policies");
            config.daemon.state_dir = temp.path().join("state");
            config.daemon.socket = temp.path().join("run/api.sock");
            config
                .backends
                .insert(backend.to_string(), configured_link.clone());
            let config_path = temp.path().join("config.toml");
            std::fs::write(
                &config_path,
                toml::to_string(&config).expect("serialize config"),
            )
            .expect("write config");

            let error = run(&config_path)
                .await
                .expect_err("backend link location must remain outside catalog ownership");

            assert!(
                error
                    .to_string()
                    .contains(&format!("backends.{backend} configured path"))
            );
            assert!(
                std::fs::symlink_metadata(&configured_link)
                    .expect("configured backend link")
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(
                std::fs::read(&target).expect("backend target remains readable"),
                b"backend executable"
            );
            for root in [&catalog, &import_root, &backend_root] {
                assert_eq!(
                    std::fs::metadata(root).expect("root metadata").mode() & 0o777,
                    0o750
                );
            }
            assert!(!config.storage.images_dir.exists());
            assert!(!config.storage.instances_dir.exists());
            assert!(config.template.dir.exists());
            assert!(!config.daemon.state_dir.exists());
        }
    }

    #[tokio::test]
    async fn configured_config_link_locations_cannot_overlap_template_roots_before_mutation() {
        for link_in_import_root in [false, true] {
            let temp = tempfile::tempdir().expect("tempdir");
            let catalog = temp.path().join("catalog");
            let import_root = temp.path().join("imports");
            let config_root = temp.path().join("config");
            for root in [&catalog, &import_root, &config_root] {
                std::fs::create_dir(root).expect("root directory");
                std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o750))
                    .expect("root mode");
            }

            let mut config = DaemonConfig::default();
            config.template.dir = catalog.clone();
            config.template.import_root = Some(import_root.clone());
            config.storage.images_dir = temp.path().join("images");
            config.storage.instances_dir = temp.path().join("instances");
            config.policy.dir = temp.path().join("policies");
            config.daemon.state_dir = temp.path().join("state");
            config.daemon.socket = temp.path().join("run/api.sock");

            let target = config_root.join("config.toml");
            let contents = toml::to_string(&config).expect("serialize config");
            std::fs::write(&target, &contents).expect("write config target");
            let protected_root = if link_in_import_root {
                &import_root
            } else {
                &catalog
            };
            let configured_link = protected_root.join("config.toml");
            symlink(&target, &configured_link).expect("config symlink");

            let error = run(&configured_link)
                .await
                .expect_err("config link location must remain outside catalog ownership");

            assert!(error.to_string().contains("config_path configured path"));
            assert!(
                std::fs::symlink_metadata(&configured_link)
                    .expect("configured config link")
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(
                std::fs::read_to_string(&target).expect("config target remains readable"),
                contents
            );
            for root in [&catalog, &import_root, &config_root] {
                assert_eq!(
                    std::fs::metadata(root).expect("root metadata").mode() & 0o777,
                    0o750
                );
            }
            assert!(!config.storage.images_dir.exists());
            assert!(!config.storage.instances_dir.exists());
            assert!(config.template.dir.exists());
            assert!(!config.daemon.state_dir.exists());
        }
    }

    #[tokio::test]
    async fn every_configured_backend_path_is_checked_before_catalog_creation() {
        let current_dir = std::env::current_dir().expect("current directory");
        let temp = tempfile::tempdir_in(&current_dir).expect("tempdir below current directory");
        let catalog = temp.path().join("catalog");
        let import_root = temp.path().join("imports");
        std::fs::create_dir(&import_root).expect("import root");

        let mut config = DaemonConfig::default();
        config.template.dir = catalog.clone();
        config.template.import_root = Some(import_root);
        config.storage.images_dir = temp.path().join("images");
        config.storage.instances_dir = temp.path().join("instances");
        config.policy.dir = temp.path().join("policies");
        config.daemon.state_dir = temp.path().join("state");
        config.daemon.socket = temp.path().join("run/api.sock");
        let missing_binary = catalog.join("future-backend");
        config.backends.insert(
            "future-backend".to_string(),
            missing_binary
                .strip_prefix(&current_dir)
                .expect("backend path below current directory")
                .to_path_buf(),
        );
        let config_path = temp.path().join("config.toml");
        std::fs::write(
            &config_path,
            toml::to_string(&config).expect("serialize config"),
        )
        .expect("write config");

        let error = run(&config_path)
            .await
            .expect_err("every configured backend path must remain outside catalog ownership");

        assert!(error.to_string().contains("backends.future-backend"));
        assert!(!missing_binary.exists());
        assert!(!catalog.exists());
        assert!(!config.storage.images_dir.exists());
        assert!(!config.storage.instances_dir.exists());
        assert!(!config.template.dir.exists());
        assert!(!config.daemon.state_dir.exists());
    }

    #[test]
    fn config_load_reuses_absolute_backend_paths() {
        let current_dir = std::env::current_dir().expect("current directory");
        let temp = tempfile::tempdir_in(&current_dir).expect("tempdir below current directory");
        let relative_binary = temp
            .path()
            .join("future-backend")
            .strip_prefix(&current_dir)
            .expect("backend path below current directory")
            .to_path_buf();
        let mut config = DaemonConfig::default();
        config
            .backends
            .insert("future-backend".to_string(), relative_binary.clone());
        let config_path = temp.path().join("config.toml");
        std::fs::write(
            &config_path,
            toml::to_string(&config).expect("serialize config"),
        )
        .expect("write config");

        let loaded = load_daemon_config(&config_path).expect("load daemon config");

        assert_eq!(
            loaded.config.backends.get("future-backend"),
            Some(&current_dir.join(relative_binary))
        );
    }

    #[test]
    fn config_load_accepts_a_preserved_packaged_pool_section() {
        let current_dir = std::env::current_dir().expect("current directory");
        let temp = tempfile::tempdir_in(&current_dir).expect("tempdir below current directory");
        let config_path = temp.path().join("config.toml");
        std::fs::write(
            &config_path,
            "[daemon]\nlog_level = \"debug\"\n\n[pool]\ndefault_warm_ttl = \"30m\"\ngc_interval = \"5m\"\n",
        )
        .expect("write legacy packaged configuration");

        let loaded = load_daemon_config(&config_path).expect("load preserved package config");

        assert!(loaded.config.pool.is_some());
        assert_eq!(loaded.config.daemon.log_level, "debug");
    }

    #[tokio::test]
    async fn loaded_config_inside_catalog_fails_before_startup_changes_catalog() {
        let temp = tempfile::tempdir().expect("tempdir");
        let catalog = temp.path().join("catalog");
        let import_root = temp.path().join("imports");
        std::fs::create_dir(&catalog).expect("catalog");
        std::fs::create_dir(&import_root).expect("import root");

        let mut config = DaemonConfig::default();
        config.template.dir = catalog.clone();
        config.template.import_root = Some(import_root);
        config.storage.images_dir = temp.path().join("images");
        config.storage.instances_dir = temp.path().join("instances");
        config.policy.dir = temp.path().join("policies");
        config.daemon.state_dir = temp.path().join("state");
        config.daemon.socket = temp.path().join("run/api.sock");
        let config_path = catalog.join("config.toml");
        let contents = toml::to_string(&config).expect("serialize config");
        std::fs::write(&config_path, &contents).expect("write config");

        let error = run(&config_path)
            .await
            .expect_err("catalog must not contain the loaded config");

        let message = error.to_string();
        assert!(message.contains("template.dir"));
        assert!(message.contains("config_path"));
        assert_eq!(
            std::fs::read_to_string(&config_path).expect("config remains readable"),
            contents
        );
        assert!(!config.storage.images_dir.exists());
        assert!(!config.daemon.state_dir.exists());
    }

    #[tokio::test]
    async fn loaded_config_inside_import_root_fails_before_catalog_creation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let catalog = temp.path().join("catalog");
        let import_root = temp.path().join("imports");
        std::fs::create_dir(&import_root).expect("import root");

        let mut config = DaemonConfig::default();
        config.template.dir = catalog.clone();
        config.template.import_root = Some(import_root.clone());
        config.storage.images_dir = temp.path().join("images");
        config.storage.instances_dir = temp.path().join("instances");
        config.policy.dir = temp.path().join("policies");
        config.daemon.state_dir = temp.path().join("state");
        config.daemon.socket = temp.path().join("run/api.sock");
        let config_path = import_root.join("config.toml");
        std::fs::write(
            &config_path,
            toml::to_string(&config).expect("serialize config"),
        )
        .expect("write config");

        let error = run(&config_path)
            .await
            .expect_err("import root must not contain the loaded config");

        let message = error.to_string();
        assert!(message.contains("template.import_root"));
        assert!(message.contains("config_path"));
        assert!(config_path.exists());
        assert!(!catalog.exists());
        assert!(!config.storage.images_dir.exists());
        assert!(!config.daemon.state_dir.exists());
    }

    #[tokio::test]
    async fn loaded_catalog_config_stays_bound_after_alias_retarget() {
        let temp = tempfile::tempdir().expect("tempdir");
        let catalog = temp.path().join("catalog");
        let import_root = temp.path().join("imports");
        let replacement = temp.path().join("replacement");
        let alias = temp.path().join("config-alias");
        for directory in [&catalog, &import_root, &replacement] {
            std::fs::create_dir(directory).expect("directory");
        }
        std::fs::set_permissions(&catalog, std::fs::Permissions::from_mode(0o750))
            .expect("catalog mode");

        let mut config = DaemonConfig::default();
        config.template.dir = catalog.clone();
        config.template.import_root = Some(import_root);
        config.storage.images_dir = temp.path().join("images");
        config.storage.instances_dir = temp.path().join("instances");
        config.policy.dir = temp.path().join("policies");
        config.daemon.state_dir = temp.path().join("state");
        config.daemon.socket = temp.path().join("run/api.sock");
        let source_path = catalog.join("config.toml");
        let contents = toml::to_string(&config).expect("serialize config");
        std::fs::write(&source_path, &contents).expect("write source config");
        std::fs::set_permissions(&source_path, std::fs::Permissions::from_mode(0o640))
            .expect("source config mode");
        std::fs::write(replacement.join("config.toml"), "replacement = true\n")
            .expect("write replacement config");
        symlink(&catalog, &alias).expect("config alias");

        let loaded = load_daemon_config(&alias.join("config.toml")).expect("load source config");
        std::fs::remove_file(&alias).expect("remove old alias");
        symlink(&replacement, &alias).expect("retarget config alias");

        let error = run_loaded_config(loaded)
            .await
            .expect_err("the loaded catalog config object must remain protected");

        let message = error.to_string();
        assert!(message.contains("template.dir"));
        assert!(message.contains("config_path"));
        assert_eq!(
            std::fs::read_to_string(&source_path).expect("source config remains readable"),
            contents
        );
        assert_eq!(
            std::fs::metadata(&catalog)
                .expect("catalog metadata")
                .mode()
                & 0o777,
            0o750
        );
        assert_eq!(
            std::fs::metadata(&source_path)
                .expect("source config metadata")
                .mode()
                & 0o777,
            0o640
        );
        assert!(!config.storage.images_dir.exists());
        assert!(!config.daemon.state_dir.exists());
    }

    #[tokio::test]
    async fn loaded_import_config_stays_bound_after_alias_retarget() {
        let temp = tempfile::tempdir().expect("tempdir");
        let catalog = temp.path().join("catalog");
        let import_root = temp.path().join("imports");
        let replacement = temp.path().join("replacement");
        let alias = temp.path().join("config-alias");
        for directory in [&import_root, &replacement] {
            std::fs::create_dir(directory).expect("directory");
        }

        let mut config = DaemonConfig::default();
        config.template.dir = catalog.clone();
        config.template.import_root = Some(import_root.clone());
        config.storage.images_dir = temp.path().join("images");
        config.storage.instances_dir = temp.path().join("instances");
        config.policy.dir = temp.path().join("policies");
        config.daemon.state_dir = temp.path().join("state");
        config.daemon.socket = temp.path().join("run/api.sock");
        let source_path = import_root.join("config.toml");
        let contents = toml::to_string(&config).expect("serialize config");
        std::fs::write(&source_path, &contents).expect("write source config");
        std::fs::set_permissions(&source_path, std::fs::Permissions::from_mode(0o640))
            .expect("source config mode");
        std::fs::write(replacement.join("config.toml"), "replacement = true\n")
            .expect("write replacement config");
        symlink(&import_root, &alias).expect("config alias");

        let loaded = load_daemon_config(&alias.join("config.toml")).expect("load source config");
        std::fs::remove_file(&alias).expect("remove old alias");
        symlink(&replacement, &alias).expect("retarget config alias");

        let error = run_loaded_config(loaded)
            .await
            .expect_err("the loaded import config object must remain protected");

        let message = error.to_string();
        assert!(message.contains("template.import_root"));
        assert!(message.contains("config_path"));
        assert_eq!(
            std::fs::read_to_string(&source_path).expect("source config remains readable"),
            contents
        );
        assert_eq!(
            std::fs::metadata(&source_path)
                .expect("source config metadata")
                .mode()
                & 0o777,
            0o640
        );
        assert!(!catalog.exists());
        assert!(!config.storage.images_dir.exists());
        assert!(!config.daemon.state_dir.exists());
    }

    #[tokio::test]
    async fn replaced_config_ancestor_fails_before_catalog_mode_change() {
        let temp = tempfile::tempdir().expect("tempdir");
        let catalog = temp.path().join("catalog");
        let config_root = temp.path().join("config-root");
        let detached_root = temp.path().join("detached-config-root");
        let import_root = temp.path().join("imports");
        for directory in [&catalog, &config_root, &import_root] {
            std::fs::create_dir(directory).expect("directory");
        }
        std::fs::set_permissions(&catalog, std::fs::Permissions::from_mode(0o750))
            .expect("catalog mode");

        let mut config = DaemonConfig::default();
        config.template.dir = catalog.clone();
        config.template.import_root = Some(import_root);
        config.storage.images_dir = temp.path().join("images");
        config.storage.instances_dir = temp.path().join("instances");
        config.policy.dir = temp.path().join("policies");
        config.daemon.state_dir = temp.path().join("state");
        config.daemon.socket = temp.path().join("run/api.sock");
        let config_path = config_root.join("config.toml");
        let contents = toml::to_string(&config).expect("serialize config");
        std::fs::write(&config_path, &contents).expect("write source config");

        let loaded = load_daemon_config(&config_path).expect("load source config");
        std::fs::rename(&config_root, &detached_root).expect("detach loaded config ancestor");
        std::fs::create_dir(&config_root).expect("replacement config root");
        std::fs::write(config_root.join("config.toml"), "replacement = true\n")
            .expect("write replacement config");

        let error = run_loaded_config(loaded)
            .await
            .expect_err("replaced config ancestor must fail closed");

        assert!(
            error
                .to_string()
                .contains("changed while startup boundaries")
        );
        assert_eq!(
            std::fs::read_to_string(detached_root.join("config.toml"))
                .expect("loaded config remains readable"),
            contents
        );
        assert_eq!(
            std::fs::metadata(&catalog)
                .expect("catalog metadata")
                .mode()
                & 0o777,
            0o750
        );
        assert!(!config.storage.images_dir.exists());
        assert!(!config.daemon.state_dir.exists());
    }

    #[tokio::test]
    async fn shutdown_waits_for_inflight_connection_task() {
        let mut connections = ConnectionSupervisor::new(TEST_REQUEST_LIMIT);
        let mut shutdown = connections.shutdown.subscribe();
        let mut shutdown_started = connections.shutdown.subscribe();
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        connections.spawn_task(async move {
            entered_tx.send(()).expect("signal task entry");
            shutdown
                .wait_for(|stopping| *stopping)
                .await
                .expect("shutdown sender");
            release_rx.await.expect("release task");
        });
        entered_rx.await.expect("task entered");

        let drain = tokio::spawn(connections.shutdown(Duration::from_secs(1)));
        shutdown_started
            .wait_for(|stopping| *stopping)
            .await
            .expect("shutdown notification");
        assert!(!drain.is_finished(), "drain returned before task completed");

        release_tx.send(()).expect("release connection task");
        drain
            .await
            .expect("drain task")
            .expect("graceful connection drain");
    }

    #[test]
    fn packaged_stop_timeout_exceeds_application_shutdown_timeouts() {
        let stop_seconds = include_str!("../../../dist/blazed.service")
            .lines()
            .find_map(|line| line.strip_prefix("TimeoutStopSec="))
            .expect("packaged service stop timeout")
            .parse::<u64>()
            .expect("numeric packaged service stop timeout");

        assert!(
            Duration::from_secs(stop_seconds)
                > CONNECTION_DRAIN_TIMEOUT
                    + CONNECTION_ABORT_JOIN_TIMEOUT
                    + crate::RUNTIME_SHUTDOWN_TIMEOUT,
            "packaged stop timeout must leave headroom after connection and runtime shutdown"
        );
    }

    #[test]
    fn request_handler_admission_reserves_one_runtime_worker() {
        assert_eq!(request_handler_limit(2), 1);
        assert_eq!(request_handler_limit(8), 7);

        let connections = ConnectionSupervisor::new(1);
        let permit = connections
            .request_permits
            .try_acquire()
            .expect("first request permit");
        assert!(connections.request_permits.try_acquire().is_err());
        drop(permit);
        assert!(connections.request_permits.try_acquire().is_ok());
    }

    #[tokio::test]
    async fn health_and_metrics_bypass_dispatch_admission() {
        let (_temp, state) = connection_test_state();
        let (client_io, server_io) = tokio::io::duplex(4 * 1024);
        let mut connections = ConnectionSupervisor::new(1);
        let held_permit = connections
            .request_permits
            .clone()
            .acquire_owned()
            .await
            .expect("hold the sole dispatch permit");
        connections.spawn(TokioIo::new(server_io), state);

        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(client_io))
                .await
                .expect("HTTP/1 client handshake");
        let client = tokio::spawn(connection);

        for path in ["/v1/health", "/v1/metrics"] {
            let response = tokio::time::timeout(
                Duration::from_secs(1),
                sender.send_request(
                    Request::builder()
                        .method("GET")
                        .uri(path)
                        .body(Empty::<Bytes>::new())
                        .expect("control request"),
                ),
            )
            .await
            .unwrap_or_else(|_| panic!("{path} waited for the dispatch permit"))
            .expect("control response");
            assert!(response.status().is_success(), "{path} response failed");
            response
                .into_body()
                .collect()
                .await
                .expect("control response body");
        }

        drop(held_permit);
        drop(sender);
        connections
            .shutdown(Duration::from_secs(1))
            .await
            .expect("control connection drained");
        client
            .await
            .expect("HTTP client task")
            .expect("HTTP client connection");
    }

    #[tokio::test]
    async fn shutdown_notifies_idle_connection_task() {
        let mut connections = ConnectionSupervisor::new(TEST_REQUEST_LIMIT);
        let mut shutdown = connections.shutdown.subscribe();
        connections.spawn_task(async move {
            shutdown
                .wait_for(|stopping| *stopping)
                .await
                .expect("shutdown sender");
        });

        connections
            .shutdown(Duration::from_secs(1))
            .await
            .expect("idle task drained");
    }

    #[tokio::test]
    async fn shutdown_closes_an_idle_http_connection() {
        let (_temp, state) = connection_test_state();
        let (client_io, server_io) = tokio::io::duplex(4 * 1024);
        let mut connections = ConnectionSupervisor::new(TEST_REQUEST_LIMIT);
        connections.spawn(TokioIo::new(server_io), state);

        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(client_io))
                .await
                .expect("HTTP/1 client handshake");
        let client = tokio::spawn(connection);
        let response = sender
            .send_request(
                Request::builder()
                    .method("GET")
                    .uri("/v1/health")
                    .body(Empty::<Bytes>::new())
                    .expect("health request"),
            )
            .await
            .expect("health response");
        assert!(response.status().is_success());
        response
            .into_body()
            .collect()
            .await
            .expect("health response body");

        connections
            .shutdown(Duration::from_secs(1))
            .await
            .expect("idle HTTP connection drained");
        tokio::time::timeout(Duration::from_secs(1), client)
            .await
            .expect("HTTP client observed server shutdown")
            .expect("HTTP client task")
            .expect("HTTP client connection");
    }

    #[tokio::test]
    async fn shutdown_waits_for_an_inflight_http_request() {
        let (_temp, state) = connection_test_state();
        let observed_state = state.clone();
        let (client_io, server_io) = tokio::io::duplex(4 * 1024);
        let mut connections = ConnectionSupervisor::new(TEST_REQUEST_LIMIT);
        let mut shutdown_started = connections.shutdown.subscribe();
        connections.spawn(TokioIo::new(server_io), state);

        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(client_io))
                .await
                .expect("HTTP/1 client handshake");
        let client = tokio::spawn(connection);
        let (polled_tx, polled_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let response = tokio::spawn(async move {
            let request = Request::builder()
                .method("POST")
                .uri("/v1/instances")
                .body(GatedBody {
                    polled: Some(polled_tx),
                    release: Some(release_rx),
                })
                .expect("gated instance request");
            sender.send_request(request).await
        });
        polled_rx.await.expect("client started request body");
        tokio::time::timeout(Duration::from_secs(1), async {
            while observed_state
                .metrics
                .requests_total
                .load(Ordering::Relaxed)
                == 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("server entered request handler");

        let drain = tokio::spawn(connections.shutdown(Duration::from_secs(1)));
        shutdown_started
            .wait_for(|stopping| *stopping)
            .await
            .expect("shutdown notification");
        assert!(
            !drain.is_finished(),
            "drain returned while the HTTP request was in flight"
        );

        release_tx.send(()).expect("finish request body");
        let response = response
            .await
            .expect("response task")
            .expect("instance response");
        assert_eq!(response.status(), hyper::StatusCode::BAD_REQUEST);
        response
            .into_body()
            .collect()
            .await
            .expect("instance response body");
        drain
            .await
            .expect("drain task")
            .expect("in-flight HTTP request drained");
        client
            .await
            .expect("HTTP client task")
            .expect("HTTP client connection");
    }

    #[tokio::test]
    async fn incomplete_request_body_does_not_hold_dispatch_permit() {
        let (_temp, state) = connection_test_state();
        let observed_state = state.clone();
        let mut connections = ConnectionSupervisor::new(1);
        let request_permits = connections.request_permits.clone();
        let (slow_client_io, slow_server_io) = tokio::io::duplex(4 * 1024);
        let (health_client_io, health_server_io) = tokio::io::duplex(4 * 1024);
        connections.spawn(TokioIo::new(slow_server_io), state.clone());
        connections.spawn(TokioIo::new(health_server_io), state);

        let (mut slow_sender, slow_connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(slow_client_io))
                .await
                .expect("slow HTTP/1 client handshake");
        let slow_client = tokio::spawn(slow_connection);
        let (mut health_sender, health_connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(health_client_io))
                .await
                .expect("health HTTP/1 client handshake");
        let health_client = tokio::spawn(health_connection);

        let (polled_tx, polled_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let slow_response = tokio::spawn(async move {
            slow_sender
                .send_request(
                    Request::builder()
                        .method("POST")
                        .uri("/v1/instances")
                        .body(GatedBody {
                            polled: Some(polled_tx),
                            release: Some(release_rx),
                        })
                        .expect("gated instance request"),
                )
                .await
        });
        polled_rx.await.expect("client started request body");
        tokio::time::timeout(Duration::from_secs(1), async {
            while observed_state
                .metrics
                .requests_total
                .load(Ordering::Relaxed)
                == 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("server started collecting the incomplete body");
        assert_eq!(
            request_permits.available_permits(),
            1,
            "body collection must happen before dispatch admission"
        );

        let health_response = tokio::time::timeout(
            Duration::from_secs(1),
            health_sender.send_request(
                Request::builder()
                    .method("GET")
                    .uri("/v1/health")
                    .body(Empty::<Bytes>::new())
                    .expect("health request"),
            ),
        )
        .await
        .expect("health request blocked behind incomplete body")
        .expect("health response");
        assert!(health_response.status().is_success());
        health_response
            .into_body()
            .collect()
            .await
            .expect("health response body");

        release_tx.send(()).expect("finish slow request body");
        let slow_response = slow_response
            .await
            .expect("slow response task")
            .expect("slow instance response");
        assert_eq!(slow_response.status(), hyper::StatusCode::BAD_REQUEST);
        slow_response
            .into_body()
            .collect()
            .await
            .expect("slow instance response body");

        drop(health_sender);
        connections
            .shutdown(Duration::from_secs(1))
            .await
            .expect("both HTTP connections drained");
        for client in [slow_client, health_client] {
            client
                .await
                .expect("HTTP client task")
                .expect("HTTP client connection");
        }
    }

    #[tokio::test]
    async fn service_events_accept_both_listener_types() {
        let (temp, state) = connection_test_state();
        let socket_path = temp.path().join("api.sock");
        let uds = UnixListener::bind(&socket_path).expect("UDS listener");
        let tcp = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("TCP listener");
        let tcp_addr = tcp.local_addr().expect("TCP listener address");
        let mut sighup = signal(SignalKind::hangup()).expect("SIGHUP handler");
        let mut connections = ConnectionSupervisor::new(TEST_REQUEST_LIMIT);

        let uds_client = tokio::net::UnixStream::connect(&socket_path)
            .await
            .expect("UDS client");
        let tcp_client = tokio::net::TcpStream::connect(tcp_addr)
            .await
            .expect("TCP client");

        serve_one_event(&uds, Some(&tcp), &state, &mut sighup, &mut connections).await;
        serve_one_event(&uds, Some(&tcp), &state, &mut sighup, &mut connections).await;
        assert_eq!(connections.tasks.len(), 2);

        drop(uds_client);
        drop(tcp_client);
        connections
            .shutdown(Duration::from_secs(1))
            .await
            .expect("accepted connections drained");
    }

    #[tokio::test]
    async fn service_events_reap_all_ready_tasks_before_accepting() {
        let (temp, state) = connection_test_state();
        let socket_path = temp.path().join("api.sock");
        let uds = UnixListener::bind(&socket_path).expect("UDS listener");
        let tcp = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("TCP listener");
        let tcp_addr = tcp.local_addr().expect("TCP listener address");
        let mut sighup = signal(SignalKind::hangup()).expect("SIGHUP handler");
        let mut connections = ConnectionSupervisor::new(TEST_REQUEST_LIMIT);
        let completed: Vec<_> = (0..64).map(|_| connections.tasks.spawn(async {})).collect();
        tokio::time::timeout(Duration::from_secs(1), async {
            while completed.iter().any(|task| !task.is_finished()) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("connection tasks completed");

        let uds_client = tokio::net::UnixStream::connect(&socket_path)
            .await
            .expect("UDS client");
        let tcp_client = tokio::net::TcpStream::connect(tcp_addr)
            .await
            .expect("TCP client");

        serve_one_event(&uds, Some(&tcp), &state, &mut sighup, &mut connections).await;
        assert_eq!(
            connections.tasks.len(),
            1,
            "all ready completions must be reaped before accepting one connection"
        );

        drop(uds_client);
        drop(tcp_client);
        connections
            .shutdown(Duration::from_secs(1))
            .await
            .expect("accepted connection drained");
    }

    #[tokio::test]
    async fn shutdown_timeout_aborts_and_joins_stuck_task() {
        let mut connections = ConnectionSupervisor::new(TEST_REQUEST_LIMIT);
        let active = Arc::new(AtomicBool::new(true));
        let task_active = active.clone();
        let (entered_tx, entered_rx) = oneshot::channel();
        connections.spawn_task(async move {
            let _active = ActiveTask(task_active);
            entered_tx.send(()).expect("signal task entry");
            future::pending::<()>().await;
        });
        entered_rx.await.expect("task entered");

        let error = connections
            .shutdown(Duration::from_millis(10))
            .await
            .expect_err("stuck task must time out");

        assert!(error.to_string().contains("connection drain timed out"));
        assert!(
            !active.load(Ordering::Acquire),
            "aborted task was not joined"
        );
    }

    #[tokio::test]
    async fn completed_tasks_are_reaped_while_serving() {
        let mut connections = ConnectionSupervisor::new(TEST_REQUEST_LIMIT);
        connections.spawn_task(async {});

        connections.reap_next().await;
        assert!(connections.is_empty());
        assert!(!connections.task_failed);
    }

    #[tokio::test]
    async fn shutdown_reports_a_panicked_connection_task() {
        let mut connections = ConnectionSupervisor::new(TEST_REQUEST_LIMIT);
        connections.spawn_task(async { panic!("connection task panic") });

        let error = connections
            .shutdown(Duration::from_secs(1))
            .await
            .expect_err("panicked task must fail the drain");

        assert!(
            error
                .to_string()
                .contains("one or more connection tasks failed before shutdown completed")
        );
    }

    #[tokio::test]
    async fn shutdown_retains_a_task_failure_alongside_a_timeout() {
        let mut connections = ConnectionSupervisor::new(TEST_REQUEST_LIMIT);
        connections.spawn_task(async { panic!("connection task panic") });
        connections.reap_next().await;
        connections.spawn_task(future::pending());

        let error = connections
            .shutdown(Duration::from_millis(10))
            .await
            .expect_err("task failure and timeout must both fail the drain");
        let message = error.to_string();

        assert!(message.contains("connection drain timed out"));
        assert!(message.contains("one or more connection tasks failed"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_bounds_join_after_abort_of_a_non_yielding_task() {
        let mut connections = ConnectionSupervisor::new(1);
        let request_permits = connections.request_permits.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let task_stop = stop.clone();
        let (entered_tx, entered_rx) = oneshot::channel();
        let (exited_tx, exited_rx) = oneshot::channel();
        connections.spawn_task(async move {
            let _permit = request_permits
                .acquire_owned()
                .await
                .expect("request admission semaphore remains open");
            entered_tx.send(()).expect("signal task entry");
            while !task_stop.load(Ordering::Acquire) {
                std::hint::spin_loop();
            }
            exited_tx.send(()).expect("signal task exit");
        });
        entered_rx.await.expect("task entered");

        let error = connections
            .shutdown_with_abort_join_timeout(Duration::from_millis(1), Duration::from_millis(10))
            .await
            .expect_err("non-yielding task must not make abort join unbounded");
        stop.store(true, Ordering::Release);
        tokio::time::timeout(Duration::from_secs(1), exited_rx)
            .await
            .expect("non-yielding task did not exit after test release")
            .expect("task exit signal");

        let message = error.to_string();
        assert!(message.contains("connection drain timed out"));
        assert!(message.contains("did not join within 10ms after abort"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocked_policy_reload_does_not_starve_shutdown_timers() {
        let mut connections = ConnectionSupervisor::new(1);
        let request_permits = connections.request_permits.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let task_stop = stop.clone();
        let (handler_entered_tx, handler_entered_rx) = oneshot::channel();
        let (handler_exited_tx, handler_exited_rx) = oneshot::channel();
        connections.spawn_task(async move {
            let _permit = request_permits
                .acquire_owned()
                .await
                .expect("request admission semaphore remains open");
            handler_entered_tx.send(()).expect("signal handler entry");
            while !task_stop.load(Ordering::Acquire) {
                std::hint::spin_loop();
            }
            handler_exited_tx.send(()).expect("signal handler exit");
        });
        handler_entered_rx.await.expect("handler entered");

        let (_temp, state) = connection_test_state();
        let (load_entered_tx, load_entered_rx) = oneshot::channel();
        let (load_release_tx, load_release_rx) = std::sync::mpsc::channel();
        let reload_state = state.clone();
        let reload = tokio::spawn(async move {
            reload_policies_with_loader(&reload_state, move |_| {
                load_entered_tx.send(()).expect("signal policy load entry");
                load_release_rx.recv().expect("release policy load");
                Ok(PolicyEngine::new())
            })
            .await
        });
        load_entered_rx.await.expect("policy load entered");

        let error = connections
            .shutdown_with_abort_join_timeout(Duration::from_millis(10), Duration::from_millis(10))
            .await
            .expect_err("policy reload must not starve shutdown timers");
        assert!(error.to_string().contains("connection drain timed out"));

        stop.store(true, Ordering::Release);
        tokio::time::timeout(Duration::from_secs(1), handler_exited_rx)
            .await
            .expect("handler did not exit after release")
            .expect("handler exit signal");
        load_release_tx
            .send(())
            .expect("release policy load thread");
        reload
            .await
            .expect("policy reload task")
            .expect("policy reload result");
    }

    #[tokio::test]
    async fn cancelled_policy_reload_cannot_replace_active_policy() {
        let (temp, state) = connection_test_state();
        std::fs::write(
            temp.path().join("policies/reloaded.toml"),
            r#"
manifest_version = 1
policy_name = "reloaded"

[match]
workload_class = "agent-rl"

[select]
backend_priority = ["bubblewrap"]
"#,
        )
        .expect("policy fixture");

        let (load_entered_tx, load_entered_rx) = oneshot::channel();
        let (load_release_tx, load_release_rx) = std::sync::mpsc::channel();
        let (load_exited_tx, load_exited_rx) = oneshot::channel();
        let reload_state = state.clone();
        let reload = tokio::spawn(async move {
            reload_policies_with_loader(&reload_state, move |dir| {
                load_entered_tx.send(()).expect("signal policy load entry");
                load_release_rx.recv().expect("release policy load");
                let result = PolicyEngine::load_dir(&dir).map_err(Into::into);
                load_exited_tx.send(()).expect("signal policy load exit");
                result
            })
            .await
        });
        load_entered_rx.await.expect("policy load entered");

        reload.abort();
        assert!(
            reload
                .await
                .expect_err("reload task must be cancelled")
                .is_cancelled()
        );
        load_release_tx
            .send(())
            .expect("release policy load thread");
        tokio::time::timeout(Duration::from_secs(1), load_exited_rx)
            .await
            .expect("policy load thread did not exit")
            .expect("policy load exit signal");

        assert!(
            state
                .policy
                .lock()
                .expect("policy lock")
                .policies()
                .is_empty(),
            "a cancelled reload must not replace the active policy engine"
        );
    }

    #[tokio::test]
    async fn a_reaped_panic_remains_a_shutdown_failure() {
        let mut connections = ConnectionSupervisor::new(TEST_REQUEST_LIMIT);
        connections.spawn_task(async { panic!("connection task panic") });
        connections.reap_next().await;

        assert!(connections.is_empty());
        let error = connections
            .shutdown(Duration::from_secs(1))
            .await
            .expect_err("reaped panic must remain visible at shutdown");
        assert!(
            error
                .to_string()
                .contains("one or more connection tasks failed before shutdown completed")
        );
    }
}
