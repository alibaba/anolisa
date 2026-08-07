// SPDX-License-Identifier: Apache-2.0
//! Backend process ownership and runtime lifecycle abstraction.

pub mod firecracker;
mod netns;

use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::time::Instant;

use async_trait::async_trait;
use blaze_core::backend::{BackendKind, SpawnRequest};
#[cfg(test)]
use blaze_core::guest_protocol::DEFAULT_MAX_RESPONSE_BYTES;
use blaze_core::{BlazeError, Result};
#[cfg(test)]
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
#[cfg(test)]
use tokio::net::UnixListener;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::state_store::OwnedRunDir;

pub use firecracker::FirecrackerSpawner;

const TERMINATION_GRACE: Duration = Duration::from_secs(5);
#[cfg(target_os = "linux")]
const PID_HANDOFF_GRACE: Duration = Duration::from_secs(1);
const STOPPED_MARKER: &str = "backend.stopped";
/// Fixed host-wide lock used to serialize network slot allocation.
pub(crate) const HOST_NETWORK_COORDINATION_PATH: &str = "/run/lock/blaze-network.lock";
/// Conventional host directories containing named network namespace objects.
///
/// Upstream iproute2 defaults to `/var/run/netns`, while distributions may
/// compile the same facility to use `/run/netns` directly.
pub(crate) const HOST_NAMED_NETWORK_NAMESPACE_PATHS: [&str; 2] = ["/var/run/netns", "/run/netns"];

/// Result reported when a backend process exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnResult {
    /// Sandbox whose process exited.
    pub instance_id: Uuid,
    /// Normal process exit status.
    pub exit_code: Option<i32>,
    /// Terminating signal on Unix.
    pub signal: Option<i32>,
}

/// Owned runtime instance returned by a backend spawner.
#[async_trait]
pub trait BackendInstance: Send + Sync {
    /// Concrete backend implementation.
    fn backend(&self) -> BackendKind;
    /// Guest transport endpoint, or an empty path for guestless backends.
    fn guest_socket_path(&self) -> &Path {
        Path::new("")
    }
    /// Report an observed backend exit without waiting.
    ///
    /// `None` means the owned process or task was running when checked.
    /// Once an exit is observed, later calls continue to report a completed
    /// result even though the underlying handle has already been consumed.
    async fn try_wait(&self) -> Result<Option<SpawnResult>>;
    /// Terminate the process and release all backend-owned resources.
    async fn kill(&self) -> Result<()>;
}

/// Shared backend instance handle stored in the daemon runtime map.
pub type DynBackendInstance = Arc<dyn BackendInstance>;

/// Backend launch inputs paired with the opened runtime-directory owner.
///
/// The portable request remains in `blaze-core`; this daemon-local wrapper
/// prevents backend implementations from reconstructing the runtime directory
/// from a configured pathname.
#[derive(Debug, Clone)]
pub struct BackendSpawnRequest {
    request: SpawnRequest,
    /// Opened directory used for all backend runtime artifacts.
    pub run_dir: OwnedRunDir,
}

impl BackendSpawnRequest {
    pub(crate) fn new(request: SpawnRequest, run_dir: OwnedRunDir) -> Result<Self> {
        if request.instance_id != run_dir.instance_id() {
            return Err(BlazeError::BackendError {
                msg: format!(
                    "backend request for {} does not match runtime-directory owner for {}",
                    request.instance_id,
                    run_dir.instance_id()
                ),
            });
        }
        Ok(Self { request, run_dir })
    }
}

impl Deref for BackendSpawnRequest {
    type Target = SpawnRequest;

    fn deref(&self) -> &Self::Target {
        &self.request
    }
}

/// Runtime owner that keeps the opened sandbox directory alive for as long as
/// any backend handle can still use paths derived from it.
struct RuntimeOwnedBackend {
    inner: DynBackendInstance,
    _run_dir: OwnedRunDir,
}

#[async_trait]
impl BackendInstance for RuntimeOwnedBackend {
    fn backend(&self) -> BackendKind {
        self.inner.backend()
    }

    fn guest_socket_path(&self) -> &Path {
        self.inner.guest_socket_path()
    }

    async fn try_wait(&self) -> Result<Option<SpawnResult>> {
        self.inner.try_wait().await
    }

    async fn kill(&self) -> Result<()> {
        self.inner.kill().await
    }
}

/// Attach runtime-directory ownership to a successful or partially started
/// backend handle before it enters lifecycle management.
pub(crate) fn bind_runtime_directory(
    inner: DynBackendInstance,
    run_dir: OwnedRunDir,
) -> DynBackendInstance {
    Arc::new(RuntimeOwnedBackend {
        inner,
        _run_dir: run_dir,
    })
}

/// Start one backend while attaching the runtime-directory owner to every
/// returned process owner, including a partial owner carried by a failure.
pub(crate) async fn spawn_with_runtime_directory(
    spawner: &dyn BackendSpawner,
    request: BackendSpawnRequest,
) -> std::result::Result<DynBackendInstance, SpawnFailure> {
    let run_dir = request.run_dir.clone();
    match spawner.spawn(request).await {
        Ok(owner) => Ok(bind_runtime_directory(owner, run_dir)),
        Err(error) => {
            let (source, owner) = error.into_parts();
            Err(match owner {
                Some(owner) => {
                    SpawnFailure::with_owner(source, bind_runtime_directory(owner, run_dir))
                }
                None => SpawnFailure::clean(source),
            })
        }
    }
}

/// Backend start failure that may retain ownership of a started process.
pub struct SpawnFailure {
    source: BlazeError,
    owner: Option<DynBackendInstance>,
}

impl SpawnFailure {
    /// Build a failure after confirming that no backend process remains.
    pub fn clean(source: BlazeError) -> Self {
        Self {
            source,
            owner: None,
        }
    }

    /// Build a failure that transfers a partially started backend owner.
    pub fn with_owner(source: BlazeError, owner: DynBackendInstance) -> Self {
        Self {
            source,
            owner: Some(owner),
        }
    }

    /// Retry termination and retain the owner when cleanup cannot be confirmed.
    async fn compensate_started(source: BlazeError, owner: DynBackendInstance) -> Self {
        match owner.kill().await {
            Ok(()) => Self::clean(source),
            Err(cleanup) => Self::with_owner(
                BlazeError::BackendError {
                    msg: format!("{source}; started backend cleanup failed: {cleanup}"),
                },
                owner,
            ),
        }
    }

    /// Split the original failure from any retained backend owner.
    pub fn into_parts(self) -> (BlazeError, Option<DynBackendInstance>) {
        (self.source, self.owner)
    }
}

impl fmt::Debug for SpawnFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SpawnFailure")
            .field("source", &self.source)
            .field("owner_retained", &self.owner.is_some())
            .finish()
    }
}

impl fmt::Display for SpawnFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.source.fmt(formatter)
    }
}

impl std::error::Error for SpawnFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl From<BlazeError> for SpawnFailure {
    fn from(source: BlazeError) -> Self {
        Self::clean(source)
    }
}

impl From<std::io::Error> for SpawnFailure {
    fn from(source: std::io::Error) -> Self {
        Self::clean(source.into())
    }
}

/// Factory for owned backend runtime instances.
#[async_trait]
pub trait BackendSpawner: Send + Sync {
    /// Persist backend-specific pre-spawn ownership metadata.
    async fn prepare_spawn(&self, _run_dir: &OwnedRunDir) -> Result<()> {
        Ok(())
    }

    /// Start a new sandbox.
    async fn spawn(
        &self,
        request: BackendSpawnRequest,
    ) -> std::result::Result<DynBackendInstance, SpawnFailure>;

    /// Probe whether the configured backend executable is usable.
    async fn probe(&self, binary_path: &Path) -> Result<bool>;

    /// Clean up a backend process and resources whose in-memory handle was
    /// lost across daemon restart.
    async fn cleanup_orphan(&self, instance_id: Uuid, run_dir: &OwnedRunDir) -> Result<()>;
}

/// Shared backend spawner selected during daemon startup.
pub type DynSpawner = Arc<dyn BackendSpawner>;

/// Backend implementations retained for kind-aware restart recovery.
#[derive(Default)]
pub struct SpawnerRegistry {
    spawners: HashMap<BackendKind, DynSpawner>,
}

impl SpawnerRegistry {
    /// Create an empty backend registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register the implementation responsible for one backend kind.
    pub fn insert(&mut self, kind: BackendKind, spawner: DynSpawner) {
        self.spawners.insert(kind, spawner);
    }

    /// Return the implementation for a persisted backend kind.
    pub fn get(&self, kind: BackendKind) -> Option<DynSpawner> {
        self.spawners.get(&kind).cloned()
    }
}

/// Bubblewrap process owner used when a VM backend is not selected.
pub struct BubblewrapSpawner;

#[async_trait]
impl BackendSpawner for BubblewrapSpawner {
    async fn prepare_spawn(&self, run_dir: &OwnedRunDir) -> Result<()> {
        prepare_pid_handoff(&run_dir.path().join("backend.pid"))
    }

    async fn spawn(
        &self,
        request: BackendSpawnRequest,
    ) -> std::result::Result<DynBackendInstance, SpawnFailure> {
        remove_file_if_exists(&request.run_dir.path().join(STOPPED_MARKER)).await?;
        let pid_file = request.run_dir.path().join("backend.pid");
        let mut command = Command::new(&request.binary_path);
        command
            .args([
                "--ro-bind",
                "/",
                "/",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--tmpfs",
                "/tmp",
                "--unshare-pid",
                "--unshare-net",
                "--die-with-parent",
                "--",
                "/bin/sleep",
                "3600",
            ])
            .env("BLAZE_INSTANCE_ID", request.instance_id.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let pid_handoff = configure_pid_handoff(&mut command, &pid_file)?;
        let child = command.spawn();
        drop(pid_handoff);
        let child = child?;
        let stopped_marker = request.run_dir.path().join(STOPPED_MARKER);
        let instance = ProcessInstance::new(
            request.instance_id,
            BackendKind::Bubblewrap,
            child,
            pid_file,
            stopped_marker,
        );
        Ok(Arc::new(instance))
    }

    async fn probe(&self, binary_path: &Path) -> Result<bool> {
        Ok(binary_path.is_file())
    }

    async fn cleanup_orphan(&self, instance_id: Uuid, run_dir: &OwnedRunDir) -> Result<()> {
        cleanup_process_run_dir(instance_id, run_dir.path(), "bubblewrap").await
    }
}

struct ProcessInstance {
    instance_id: Uuid,
    backend: BackendKind,
    child: Mutex<Option<Child>>,
    pid_file: PathBuf,
    stopped_marker: PathBuf,
    killed: AtomicBool,
}

impl ProcessInstance {
    fn new(
        instance_id: Uuid,
        backend: BackendKind,
        child: Child,
        pid_file: PathBuf,
        stopped_marker: PathBuf,
    ) -> Self {
        Self {
            instance_id,
            backend,
            child: Mutex::new(Some(child)),
            pid_file,
            stopped_marker,
            killed: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl BackendInstance for ProcessInstance {
    fn backend(&self) -> BackendKind {
        self.backend
    }

    async fn try_wait(&self) -> Result<Option<SpawnResult>> {
        let mut guard = self.child.lock().await;
        let Some(child) = guard.as_mut() else {
            return Ok(Some(SpawnResult {
                instance_id: self.instance_id,
                exit_code: None,
                signal: None,
            }));
        };
        let Some(status) = child.try_wait()? else {
            return Ok(None);
        };
        record_backend_stopped(&self.stopped_marker).await?;
        *guard = None;
        Ok(Some(spawn_result(self.instance_id, status)))
    }

    async fn kill(&self) -> Result<()> {
        if self.killed.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut guard = self.child.lock().await;
        if self.killed.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(child) = guard.as_mut() {
            terminate_child(child, self.backend.as_str()).await?;
        }
        record_backend_stopped(&self.stopped_marker).await?;
        *guard = None;
        remove_file_if_exists(&self.pid_file).await?;
        self.killed.store(true, Ordering::Release);
        Ok(())
    }
}

/// Portable backend used for API and lifecycle integration tests.
pub struct MockSpawner;

#[async_trait]
impl BackendSpawner for MockSpawner {
    async fn spawn(
        &self,
        request: BackendSpawnRequest,
    ) -> std::result::Result<DynBackendInstance, SpawnFailure> {
        spawn_mock_instance(request.instance_id)
            .await
            .map_err(SpawnFailure::from)
    }

    async fn probe(&self, _binary_path: &Path) -> Result<bool> {
        Ok(true)
    }

    async fn cleanup_orphan(&self, _instance_id: Uuid, _run_dir: &OwnedRunDir) -> Result<()> {
        // Mock owners are in-process tasks and cannot survive daemon exit.
        Ok(())
    }
}

struct MockInstance {
    instance_id: Uuid,
    cancellation: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
    killed: AtomicBool,
}

async fn spawn_mock_instance(instance_id: Uuid) -> Result<DynBackendInstance> {
    let cancellation = CancellationToken::new();
    let task_token = cancellation.clone();
    let task = tokio::spawn(async move { task_token.cancelled().await });
    Ok(Arc::new(MockInstance {
        instance_id,
        cancellation,
        task: Mutex::new(Some(task)),
        killed: AtomicBool::new(false),
    }))
}

#[async_trait]
impl BackendInstance for MockInstance {
    fn backend(&self) -> BackendKind {
        BackendKind::Mock
    }

    async fn try_wait(&self) -> Result<Option<SpawnResult>> {
        let task = {
            let mut task = self.task.lock().await;
            match task.as_ref() {
                Some(handle) if !handle.is_finished() => return Ok(None),
                Some(_) => task.take(),
                None => {
                    return Ok(Some(SpawnResult {
                        instance_id: self.instance_id,
                        exit_code: Some(0),
                        signal: None,
                    }));
                }
            }
        };
        if let Some(task) = task {
            let _ = task.await;
        }
        Ok(Some(SpawnResult {
            instance_id: self.instance_id,
            exit_code: Some(0),
            signal: None,
        }))
    }

    async fn kill(&self) -> Result<()> {
        if self.killed.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut task = self.task.lock().await;
        if self.killed.load(Ordering::Acquire) {
            return Ok(());
        }
        self.cancellation.cancel();
        if let Some(task) = task.take() {
            let _ = task.await;
        }
        self.killed.store(true, Ordering::Release);
        Ok(())
    }
}

/// Guest-capable mock used only by unit and integration tests.
#[cfg(test)]
pub(crate) struct GuestMockSpawner;

#[cfg(test)]
#[async_trait]
impl BackendSpawner for GuestMockSpawner {
    async fn spawn(
        &self,
        request: BackendSpawnRequest,
    ) -> std::result::Result<DynBackendInstance, SpawnFailure> {
        spawn_guest_mock_instance(request.instance_id, &request.run_dir)
            .await
            .map_err(SpawnFailure::from)
    }

    async fn probe(&self, _binary_path: &Path) -> Result<bool> {
        Ok(true)
    }

    async fn cleanup_orphan(&self, _instance_id: Uuid, _run_dir: &OwnedRunDir) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
struct GuestMockInstance {
    instance_id: Uuid,
    guest_socket_path: PathBuf,
    cancellation: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
    killed: AtomicBool,
}

#[cfg(test)]
async fn spawn_guest_mock_instance(
    instance_id: Uuid,
    run_dir: &OwnedRunDir,
) -> Result<DynBackendInstance> {
    let socket = run_dir.path().join("vsock.uds");
    if socket.exists() {
        tokio::fs::remove_file(&socket).await?;
    }
    let listener = UnixListener::bind(&socket)?;
    let cancellation = CancellationToken::new();
    let task_token = cancellation.clone();
    let files = Arc::new(Mutex::new(HashMap::new()));
    let task_files = files.clone();
    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = task_token.cancelled() => break,
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else {
                        break;
                    };
                    let files = task_files.clone();
                    tokio::spawn(async move {
                        if let Err(error) = serve_mock_guest(stream, files).await {
                            tracing::debug!(%error, "test guest connection ended");
                        }
                    });
                }
            }
        }
    });
    Ok(Arc::new(GuestMockInstance {
        instance_id,
        guest_socket_path: socket,
        cancellation,
        task: Mutex::new(Some(task)),
        killed: AtomicBool::new(false),
    }))
}

#[cfg(test)]
#[async_trait]
impl BackendInstance for GuestMockInstance {
    fn backend(&self) -> BackendKind {
        BackendKind::Mock
    }

    fn guest_socket_path(&self) -> &Path {
        &self.guest_socket_path
    }

    async fn try_wait(&self) -> Result<Option<SpawnResult>> {
        let mut task = self.task.lock().await;
        match task.as_ref() {
            Some(handle) if !handle.is_finished() => Ok(None),
            Some(_) => {
                if let Some(handle) = task.take() {
                    let _ = handle.await;
                }
                Ok(Some(SpawnResult {
                    instance_id: self.instance_id,
                    exit_code: Some(0),
                    signal: None,
                }))
            }
            None => Ok(Some(SpawnResult {
                instance_id: self.instance_id,
                exit_code: Some(0),
                signal: None,
            })),
        }
    }

    async fn kill(&self) -> Result<()> {
        if self.killed.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut task = self.task.lock().await;
        if self.killed.load(Ordering::Acquire) {
            return Ok(());
        }
        self.cancellation.cancel();
        if let Some(task) = task.take() {
            let _ = task.await;
        }
        if self.guest_socket_path.exists() {
            tokio::fs::remove_file(&self.guest_socket_path).await?;
        }
        self.killed.store(true, Ordering::Release);
        Ok(())
    }
}

#[cfg(test)]
async fn serve_mock_guest(
    mut stream: tokio::net::UnixStream,
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
) -> std::io::Result<()> {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64;

    let connect = read_mock_line(&mut stream, 128).await?;
    if !connect.starts_with(b"CONNECT ") {
        return Ok(());
    }
    stream.write_all(b"OK 5000\n").await?;
    let request = read_mock_line(&mut stream, DEFAULT_MAX_RESPONSE_BYTES).await?;
    let request: serde_json::Value = match serde_json::from_slice(&request) {
        Ok(request) => request,
        Err(_) => return Ok(()),
    };
    let id = request.get("id").cloned().unwrap_or_default();
    let response = match request.get("op").and_then(serde_json::Value::as_str) {
        Some("exec") => {
            let command = request
                .get("cmd")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            serde_json::json!({
                "id": id,
                "ok": true,
                "rc": 0,
                "stdout_b64": BASE64.encode(command.as_bytes()),
                "stderr_b64": ""
            })
        }
        Some("read") => {
            let path = request
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let data = files.lock().await.get(path).cloned().unwrap_or_default();
            serde_json::json!({"id": id, "ok": true, "data_b64": BASE64.encode(data)})
        }
        Some("write") => {
            let path = request
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let data = request
                .get("data_b64")
                .and_then(serde_json::Value::as_str)
                .and_then(|encoded| BASE64.decode(encoded).ok())
                .unwrap_or_default();
            files.lock().await.insert(path, data);
            serde_json::json!({"id": id, "ok": true})
        }
        _ => serde_json::json!({"id": id, "ok": true}),
    };
    let mut encoded = serde_json::to_vec(&response).unwrap_or_else(|_| b"{}".to_vec());
    encoded.push(b'\n');
    stream.write_all(&encoded).await
}

#[cfg(test)]
async fn read_mock_line<R>(stream: &mut R, limit: usize) -> std::io::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut reader = BufReader::new(stream).take(limit.saturating_add(1) as u64);
    let mut output = Vec::with_capacity(limit.min(8192));
    reader.read_until(b'\n', &mut output).await?;
    if output.last() == Some(&b'\n') {
        output.pop();
        if output.len() <= limit {
            return Ok(output);
        }
    }
    if output.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "mock guest line too long",
        ));
    }
    Ok(output)
}

pub(super) async fn terminate_child(child: &mut Child, backend: &str) -> Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    if let Err(error) = signal_process(child.id(), "-TERM").await {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        tracing::warn!(backend, %error, "SIGTERM request failed; sending SIGKILL");
        child.start_kill()?;
        child.wait().await?;
        return Ok(());
    }
    match tokio::time::timeout(TERMINATION_GRACE, child.wait()).await {
        Ok(status) => {
            status?;
        }
        Err(_) => {
            tracing::warn!(backend, "graceful termination timed out; sending SIGKILL");
            child.start_kill()?;
            child.wait().await?;
        }
    }
    Ok(())
}

pub(super) async fn remove_file_if_exists(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(super) async fn record_backend_stopped(marker: &Path) -> Result<()> {
    tokio::fs::write(marker, b"stopped\n").await?;
    Ok(())
}

pub(super) fn stopped_marker(run_dir: &Path) -> PathBuf {
    run_dir.join(STOPPED_MARKER)
}

#[cfg(unix)]
pub(super) struct PidHandoff {
    _file: std::fs::File,
}

#[cfg(not(unix))]
pub(super) struct PidHandoff;

#[cfg(unix)]
pub(super) fn prepare_pid_handoff(pid_file: &Path) -> Result<()> {
    use std::ffi::CString;
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;

    let pid_path =
        CString::new(pid_file.as_os_str().as_bytes()).map_err(|_| BlazeError::BackendError {
            msg: format!("PID file path contains a NUL byte: {}", pid_file.display()),
        })?;
    let fd = unsafe {
        libc::open(
            pid_path.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    file.sync_all()?;
    if let Some(parent) = pid_file.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn prepare_pid_handoff(pid_file: &Path) -> Result<()> {
    let file = std::fs::File::create(pid_file)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(unix)]
pub(super) fn configure_pid_handoff(command: &mut Command, pid_file: &Path) -> Result<PidHandoff> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::process::CommandExt;

    let pid_path =
        CString::new(pid_file.as_os_str().as_bytes()).map_err(|_| BlazeError::BackendError {
            msg: format!("PID file path contains a NUL byte: {}", pid_file.display()),
        })?;
    let fd = unsafe {
        libc::open(
            pid_path.as_ptr(),
            libc::O_RDWR | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let child_fd = file.as_raw_fd();
    // SAFETY: the closure calls only async-signal-safe libc functions and does
    // not allocate after fork. The returned guard keeps `child_fd` open and
    // locked until `Command::spawn` completes.
    unsafe {
        command
            .as_std_mut()
            .pre_exec(move || write_current_pid(child_fd));
    }
    Ok(PidHandoff { _file: file })
}

#[cfg(not(unix))]
pub(super) fn configure_pid_handoff(
    _command: &mut Command,
    _pid_file: &Path,
) -> Result<PidHandoff> {
    Ok(PidHandoff)
}

#[cfg(unix)]
fn write_current_pid(fd: libc::c_int) -> std::io::Result<()> {
    if unsafe { libc::lseek(fd, 0, libc::SEEK_SET) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::ftruncate(fd, 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    write_pid_and_sync(fd)
}

#[cfg(unix)]
fn write_pid_and_sync(fd: libc::c_int) -> std::io::Result<()> {
    let mut buffer = [0_u8; 16];
    let mut cursor = buffer.len();
    cursor -= 1;
    buffer[cursor] = b'\n';
    let mut pid = unsafe { libc::getpid() } as u32;
    loop {
        cursor -= 1;
        buffer[cursor] = b'0' + (pid % 10) as u8;
        pid /= 10;
        if pid == 0 {
            break;
        }
    }

    let mut remaining = &buffer[cursor..];
    while !remaining.is_empty() {
        let written = unsafe {
            libc::write(
                fd,
                remaining.as_ptr().cast::<libc::c_void>(),
                remaining.len(),
            )
        };
        if written < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if written == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        remaining = &remaining[written as usize..];
    }
    if unsafe { libc::fsync(fd) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

async fn cleanup_process_run_dir(instance_id: Uuid, run_dir: &Path, backend: &str) -> Result<()> {
    let stopped_marker = stopped_marker(run_dir);
    if stopped_marker.is_file() {
        return Ok(());
    }
    let pid_file = run_dir.join("backend.pid");
    #[cfg(target_os = "linux")]
    terminate_recorded_process(instance_id, &pid_file, backend).await?;
    #[cfg(not(target_os = "linux"))]
    {
        let _ = instance_id;
        if pid_file.exists() {
            return Err(BlazeError::BackendError {
                msg: format!(
                    "cannot validate {backend} orphan {} outside Linux",
                    pid_file.display()
                ),
            });
        }
    }
    record_backend_stopped(&stopped_marker).await?;
    remove_file_if_exists(&pid_file).await?;
    Ok(())
}

#[cfg(target_os = "linux")]
pub(super) async fn terminate_recorded_process(
    instance_id: Uuid,
    pid_file: &Path,
    backend: &str,
) -> Result<()> {
    let raw = match wait_for_pid_handoff(pid_file).await? {
        Some(raw) => raw,
        None => return Ok(()),
    };
    let pid: u32 = raw
        .trim()
        .parse()
        .map_err(|error| BlazeError::BackendError {
            msg: format!("invalid {backend} pid file {}: {error}", pid_file.display()),
        })?;
    let process_dir = PathBuf::from(format!("/proc/{pid}"));
    let environ = match tokio::fs::read(process_dir.join("environ")).await {
        Ok(environ) => environ,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let expected = format!("BLAZE_INSTANCE_ID={instance_id}");
    if !environ
        .split(|byte| *byte == 0)
        .any(|entry| entry == expected.as_bytes())
    {
        return Err(BlazeError::BackendError {
            msg: format!(
                "refusing to terminate {backend} pid {pid}: BLAZE_INSTANCE_ID does not match {instance_id}"
            ),
        });
    }

    if let Err(error) = signal_process(Some(pid), "-TERM").await {
        if !process_is_running(&process_dir)? {
            return Ok(());
        }
        return Err(error);
    }
    if wait_for_process_exit(&process_dir, TERMINATION_GRACE).await? {
        return Ok(());
    }
    tracing::warn!(backend, pid, "orphan ignored SIGTERM; sending SIGKILL");
    if let Err(error) = signal_process(Some(pid), "-KILL").await {
        if !process_is_running(&process_dir)? {
            return Ok(());
        }
        return Err(error);
    }
    if !wait_for_process_exit(&process_dir, TERMINATION_GRACE).await? {
        return Err(BlazeError::BackendError {
            msg: format!("{backend} orphan pid {pid} did not exit"),
        });
    }
    Ok(())
}

#[cfg(target_os = "linux")]
async fn wait_for_pid_handoff(pid_file: &Path) -> Result<Option<String>> {
    let deadline = Instant::now() + PID_HANDOFF_GRACE;
    loop {
        match read_pid_handoff(pid_file)? {
            PidHandoffState::NotStarted => return Ok(None),
            PidHandoffState::Missing => {
                return Err(BlazeError::BackendError {
                    msg: format!(
                        "cannot confirm backend process ownership: missing PID handoff {}",
                        pid_file.display()
                    ),
                });
            }
            PidHandoffState::Ready(raw) => return Ok(Some(raw)),
            PidHandoffState::InProgress => {}
        }
        if Instant::now() >= deadline {
            return Err(BlazeError::BackendError {
                msg: format!(
                    "cannot confirm backend process ownership: PID handoff is still in progress at {}",
                    pid_file.display()
                ),
            });
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(target_os = "linux")]
enum PidHandoffState {
    Missing,
    NotStarted,
    InProgress,
    Ready(String),
}

#[cfg(target_os = "linux")]
fn read_pid_handoff(pid_file: &Path) -> Result<PidHandoffState> {
    use std::io::{Read, Seek, SeekFrom};
    use std::os::fd::AsRawFd;

    let mut file = match std::fs::OpenOptions::new().read(true).open(pid_file) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PidHandoffState::Missing);
        }
        Err(error) => return Err(error.into()),
    };
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EAGAIN)
            || error.raw_os_error() == Some(libc::EWOULDBLOCK)
        {
            return Ok(PidHandoffState::InProgress);
        }
        return Err(error.into());
    }
    file.seek(SeekFrom::Start(0))?;
    let mut raw = String::new();
    file.read_to_string(&mut raw)?;
    if raw.trim().is_empty() {
        Ok(PidHandoffState::NotStarted)
    } else {
        Ok(PidHandoffState::Ready(raw))
    }
}

#[cfg(target_os = "linux")]
async fn wait_for_process_exit(process_dir: &Path, timeout: Duration) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    while process_is_running(process_dir)? && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Ok(!process_is_running(process_dir)?)
}

#[cfg(target_os = "linux")]
fn process_is_running(process_dir: &Path) -> Result<bool> {
    let stat = match std::fs::read_to_string(process_dir.join("stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let state = stat
        .rsplit_once(") ")
        .and_then(|(_, fields)| fields.chars().next())
        .ok_or_else(|| BlazeError::BackendError {
            msg: format!("invalid process status in {}", process_dir.display()),
        })?;
    Ok(state != 'Z')
}

async fn signal_process(pid: Option<u32>, signal: &str) -> Result<()> {
    let Some(pid) = pid else {
        return Ok(());
    };
    let status = tokio::time::timeout(
        Duration::from_secs(5),
        Command::new("kill")
            .arg(signal)
            .arg(pid.to_string())
            .env("LC_ALL", "C")
            .status(),
    )
    .await
    .map_err(|_| BlazeError::BackendError {
        msg: format!("kill {signal} {pid} timed out"),
    })??;
    if !status.success() {
        return Err(BlazeError::BackendError {
            msg: format!("kill {signal} {pid} exited with {status}"),
        });
    }
    Ok(())
}

fn spawn_result(instance_id: Uuid, status: std::process::ExitStatus) -> SpawnResult {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        SpawnResult {
            instance_id,
            exit_code: status.code(),
            signal: status.signal(),
        }
    }
    #[cfg(not(unix))]
    {
        SpawnResult {
            instance_id,
            exit_code: status.code(),
            signal: None,
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    use std::time::Duration;

    use blaze_core::backend::SpawnRequest;
    use blaze_core::policy::BackendConfigs;
    use blaze_core::storage::StorageSlot;

    use crate::guest::GuestClient;

    use super::*;

    fn request(root: &Path) -> BackendSpawnRequest {
        let id = Uuid::new_v4();
        let slot_dir = root.join("slot");
        BackendSpawnRequest::new(
            SpawnRequest {
                instance_id: id,
                binary_path: PathBuf::new(),
                storage: StorageSlot {
                    id: id.to_string(),
                    rootfs_path: slot_dir.join("rootfs.ext4"),
                    mem_path: slot_dir.join("mem.bin"),
                    mem_diff_path: slot_dir.join("mem.diff"),
                    rootfs_diff_path: slot_dir.join("rootfs.diff"),
                    instance_dir: slot_dir,
                },
                backend: BackendConfigs::default(),
                vm: None,
            },
            OwnedRunDir::for_test(id, root.join("run")),
        )
        .expect("matching backend request")
    }

    #[test]
    fn backend_request_rejects_a_mismatched_runtime_owner() {
        let temp = tempfile::tempdir().expect("temp");
        let mut request = request(temp.path());
        request.request.instance_id = Uuid::new_v4();
        assert!(
            BackendSpawnRequest::new(request.request.clone(), request.run_dir.clone()).is_err()
        );
    }

    #[cfg(target_os = "linux")]
    struct PartialPathOwner {
        marker: PathBuf,
    }

    #[cfg(target_os = "linux")]
    #[async_trait]
    impl BackendInstance for PartialPathOwner {
        fn backend(&self) -> BackendKind {
            BackendKind::Mock
        }

        async fn try_wait(&self) -> Result<Option<SpawnResult>> {
            Ok(None)
        }

        async fn kill(&self) -> Result<()> {
            std::fs::write(&self.marker, b"partial owner")?;
            Ok(())
        }
    }

    #[cfg(target_os = "linux")]
    struct PartialPathSpawner;

    #[cfg(target_os = "linux")]
    #[async_trait]
    impl BackendSpawner for PartialPathSpawner {
        async fn spawn(
            &self,
            request: BackendSpawnRequest,
        ) -> std::result::Result<DynBackendInstance, SpawnFailure> {
            let owner: DynBackendInstance = Arc::new(PartialPathOwner {
                marker: request.run_dir.path().join("partial-owner-marker"),
            });
            Err(SpawnFailure::with_owner(
                BlazeError::BackendError {
                    msg: "injected partial start".into(),
                },
                owner,
            ))
        }

        async fn probe(&self, _binary_path: &Path) -> Result<bool> {
            Ok(true)
        }

        async fn cleanup_orphan(&self, _instance_id: Uuid, _run_dir: &OwnedRunDir) -> Result<()> {
            Ok(())
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn partial_spawn_failure_retains_the_runtime_directory_owner() {
        let temp = tempfile::tempdir().expect("temp");
        let request = request(temp.path());
        let failure = match spawn_with_runtime_directory(&PartialPathSpawner, request).await {
            Ok(_) => panic!("partial spawn must fail"),
            Err(failure) => failure,
        };
        let (source, owner) = failure.into_parts();
        assert!(source.to_string().contains("injected partial start"));
        let owner = owner.expect("partial backend owner");

        let configured_run_dir = temp.path().join("run");
        let owned_run_dir = temp.path().join("owned-partial-run");
        std::fs::rename(&configured_run_dir, &owned_run_dir).expect("move owned runtime directory");
        std::fs::create_dir(&configured_run_dir).expect("replacement runtime directory");

        owner.kill().await.expect("use retained runtime owner");

        assert_eq!(
            std::fs::read(owned_run_dir.join("partial-owner-marker")).expect("owned marker"),
            b"partial owner"
        );
        assert!(!configured_run_dir.join("partial-owner-marker").exists());
    }

    #[cfg(target_os = "linux")]
    async fn wait_for_instance_marker(child: &Child, instance_id: Uuid) {
        let pid = child.id().expect("child pid");
        let expected = format!("BLAZE_INSTANCE_ID={instance_id}");
        let environ_path = PathBuf::from(format!("/proc/{pid}/environ"));
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Ok(environ) = tokio::fs::read(&environ_path).await
                && environ
                    .split(|byte| *byte == 0)
                    .any(|entry| entry == expected.as_bytes())
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "child environment marker did not become visible"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn production_mock_does_not_advertise_guest_transport() {
        let temp = tempfile::tempdir().expect("temp");
        let instance = MockSpawner
            .spawn(request(temp.path()))
            .await
            .expect("spawn");

        assert!(instance.guest_socket_path().as_os_str().is_empty());
        assert!(!temp.path().join("run/vsock.uds").exists());
        instance.kill().await.expect("kill");
    }

    #[tokio::test]
    async fn test_guest_instance_supports_io_and_idempotent_kill() {
        let temp = tempfile::tempdir().expect("temp");
        let request = request(temp.path());
        let run_dir = request.run_dir.clone();
        let instance = spawn_with_runtime_directory(&GuestMockSpawner, request)
            .await
            .expect("spawn");
        drop(run_dir);
        let configured_run_dir = temp.path().join("run");
        let owned_run_dir = temp.path().join("owned-run");
        std::fs::rename(&configured_run_dir, &owned_run_dir).expect("move owned runtime directory");
        std::fs::create_dir(&configured_run_dir).expect("replacement runtime directory");
        assert_eq!(instance.backend(), BackendKind::Mock);
        let client = GuestClient::new(
            instance.guest_socket_path().to_path_buf(),
            Duration::from_secs(1),
            1024,
        );
        client
            .write_file("/tmp/value".into(), b"hello")
            .await
            .expect("write");
        assert_eq!(
            client.read_file("/tmp/value".into()).await.expect("read"),
            b"hello"
        );
        assert!(owned_run_dir.join("vsock.uds").exists());
        assert!(!configured_run_dir.join("vsock.uds").exists());
        assert_eq!(instance.try_wait().await.expect("try wait"), None);
        instance.kill().await.expect("kill");
        assert!(instance.try_wait().await.expect("try wait").is_some());
        instance.kill().await.expect("idempotent kill");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn child_termination_requests_graceful_exit_first() {
        let temp = tempfile::tempdir().expect("temp");
        let marker = temp.path().join("terminated");
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("trap 'printf term > \"$MARKER\"; exit 0' TERM; while :; do sleep 1; done")
            .env("MARKER", &marker)
            .spawn()
            .expect("spawn child");
        tokio::time::sleep(Duration::from_millis(50)).await;

        terminate_child(&mut child, "test")
            .await
            .expect("terminate child");

        assert_eq!(std::fs::read_to_string(marker).expect("marker"), "term");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn orphan_cleanup_requires_matching_instance_marker() {
        let temp = tempfile::tempdir().expect("temp");
        let expected_id = Uuid::new_v4();
        let actual_id = Uuid::new_v4();
        let pid_file = temp.path().join("backend.pid");
        let mut child = Command::new("sleep")
            .arg("60")
            .env("BLAZE_INSTANCE_ID", actual_id.to_string())
            .spawn()
            .expect("spawn child");
        wait_for_instance_marker(&child, actual_id).await;
        std::fs::write(&pid_file, format!("{}\n", child.id().expect("child pid")))
            .expect("write pid");

        let error = terminate_recorded_process(expected_id, &pid_file, "test")
            .await
            .expect_err("mismatched process must be retained");

        assert!(error.to_string().contains("does not match"));
        assert!(child.try_wait().expect("child status").is_none());
        child.start_kill().expect("kill child");
        child.wait().await.expect("wait child");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn orphan_cleanup_accepts_pre_spawn_handoff_without_pid() {
        let temp = tempfile::tempdir().expect("temp");
        prepare_pid_handoff(&temp.path().join("backend.pid")).expect("prepare handoff");
        cleanup_process_run_dir(Uuid::new_v4(), temp.path(), "test")
            .await
            .expect("an unlocked empty handoff proves the backend was not started");
        assert!(stopped_marker(temp.path()).is_file());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn orphan_cleanup_rejects_missing_pid_handoff() {
        let temp = tempfile::tempdir().expect("temp");
        let error = cleanup_process_run_dir(Uuid::new_v4(), temp.path(), "test")
            .await
            .expect_err("missing handoff cannot prove backend ownership");

        assert!(error.to_string().contains("missing PID handoff"));
        assert!(!stopped_marker(temp.path()).exists());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn orphan_cleanup_retains_an_active_pid_handoff() {
        let temp = tempfile::tempdir().expect("temp");
        let pid_file = temp.path().join("backend.pid");
        prepare_pid_handoff(&pid_file).expect("prepare handoff");
        let mut command = Command::new("sleep");
        let handoff = configure_pid_handoff(&mut command, &pid_file).expect("configure handoff");

        let error = cleanup_process_run_dir(Uuid::new_v4(), temp.path(), "test")
            .await
            .expect_err("an active handoff cannot prove the backend absent");

        assert!(
            error
                .to_string()
                .contains("PID handoff is still in progress")
        );
        assert!(!stopped_marker(temp.path()).exists());
        drop(handoff);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn pid_handoff_is_visible_when_spawn_returns() {
        let temp = tempfile::tempdir().expect("temp");
        let instance_id = Uuid::new_v4();
        let pid_file = temp.path().join("backend.pid");
        prepare_pid_handoff(&pid_file).expect("prepare handoff");
        let mut command = Command::new("sleep");
        command
            .arg("60")
            .env("BLAZE_INSTANCE_ID", instance_id.to_string());
        let handoff = configure_pid_handoff(&mut command, &pid_file).expect("configure handoff");
        let mut child = command.spawn().expect("spawn child");
        drop(handoff);
        wait_for_instance_marker(&child, instance_id).await;

        assert_eq!(
            std::fs::read_to_string(&pid_file)
                .expect("pid handoff")
                .trim(),
            child.id().expect("child pid").to_string()
        );
        terminate_recorded_process(instance_id, &pid_file, "test")
            .await
            .expect("terminate handed-off process");
        child.wait().await.expect("reap child");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn failed_pid_handoff_preparation_does_not_start_backend() {
        let temp = tempfile::tempdir().expect("temp");
        let instance_id = Uuid::new_v4();
        let pid_file = temp.path().join("missing").join("backend.pid");
        let mut command = Command::new("sleep");
        command
            .arg("60")
            .env("BLAZE_INSTANCE_ID", instance_id.to_string());

        assert!(configure_pid_handoff(&mut command, &pid_file).is_err());
        assert!(!pid_file.exists());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn orphan_cleanup_terminates_matching_instance() {
        let temp = tempfile::tempdir().expect("temp");
        let instance_id = Uuid::new_v4();
        let pid_file = temp.path().join("backend.pid");
        let mut child = Command::new("sleep")
            .arg("60")
            .env("BLAZE_INSTANCE_ID", instance_id.to_string())
            .spawn()
            .expect("spawn child");
        wait_for_instance_marker(&child, instance_id).await;
        std::fs::write(&pid_file, format!("{}\n", child.id().expect("child pid")))
            .expect("write pid");

        terminate_recorded_process(instance_id, &pid_file, "test")
            .await
            .expect("matching process is terminated");
        child.wait().await.expect("reap child");
    }
}
