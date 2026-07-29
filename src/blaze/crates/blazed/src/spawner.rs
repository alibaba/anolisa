// SPDX-License-Identifier: Apache-2.0
//! Backend process ownership and runtime lifecycle abstraction.

pub mod firecracker;

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::time::Instant;

use async_trait::async_trait;
use blaze_core::backend::{BackendKind, SpawnRequest};
use blaze_core::{BlazeError, Result};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub use firecracker::FirecrackerSpawner;

const TERMINATION_GRACE: Duration = Duration::from_secs(5);
const STOPPED_MARKER: &str = "backend.stopped";

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
    /// Start a new sandbox.
    async fn spawn(
        &self,
        request: SpawnRequest,
    ) -> std::result::Result<DynBackendInstance, SpawnFailure>;

    /// Probe whether the configured backend executable is usable.
    async fn probe(&self, binary_path: &Path) -> Result<bool>;

    /// Clean up a backend process and resources whose in-memory handle was
    /// lost across daemon restart.
    async fn cleanup_orphan(&self, instance_id: Uuid, run_dir: &Path) -> Result<()>;
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
    async fn spawn(
        &self,
        request: SpawnRequest,
    ) -> std::result::Result<DynBackendInstance, SpawnFailure> {
        tokio::fs::create_dir_all(&request.run_dir).await?;
        remove_file_if_exists(&request.run_dir.join(STOPPED_MARKER)).await?;
        let child = Command::new(&request.binary_path)
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
            .stderr(Stdio::null())
            .spawn()?;
        let pid_file = request.run_dir.join("backend.pid");
        let stopped_marker = request.run_dir.join(STOPPED_MARKER);
        if let Some(pid) = child.id()
            && let Err(error) = tokio::fs::write(&pid_file, format!("{pid}\n")).await
        {
            let owner: DynBackendInstance = Arc::new(ProcessInstance::new(
                request.instance_id,
                BackendKind::Bubblewrap,
                child,
                pid_file,
                stopped_marker,
            ));
            return Err(SpawnFailure::compensate_started(error.into(), owner).await);
        }
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

    async fn cleanup_orphan(&self, instance_id: Uuid, run_dir: &Path) -> Result<()> {
        cleanup_process_run_dir(instance_id, run_dir, "bubblewrap").await
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
        request: SpawnRequest,
    ) -> std::result::Result<DynBackendInstance, SpawnFailure> {
        spawn_mock_instance(request.instance_id, request.run_dir)
            .await
            .map_err(SpawnFailure::from)
    }

    async fn probe(&self, _binary_path: &Path) -> Result<bool> {
        Ok(true)
    }

    async fn cleanup_orphan(&self, _instance_id: Uuid, _run_dir: &Path) -> Result<()> {
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

async fn spawn_mock_instance(instance_id: Uuid, run_dir: PathBuf) -> Result<DynBackendInstance> {
    tokio::fs::create_dir_all(&run_dir).await?;
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
    let raw = match tokio::fs::read_to_string(pid_file).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(BlazeError::BackendError {
                msg: format!(
                    "cannot confirm {backend} instance {instance_id} stopped: missing PID metadata {}",
                    pid_file.display()
                ),
            });
        }
        Err(error) => return Err(error.into()),
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

    use super::*;

    fn request(root: &Path) -> SpawnRequest {
        let id = Uuid::new_v4();
        let slot_dir = root.join("slot");
        SpawnRequest {
            instance_id: id,
            run_dir: root.join("run"),
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
        }
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
    async fn mock_instance_reports_liveness_and_supports_idempotent_kill() {
        let temp = tempfile::tempdir().expect("temp");
        let instance = MockSpawner
            .spawn(request(temp.path()))
            .await
            .expect("spawn");
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
    async fn orphan_cleanup_rejects_missing_pid_without_stop_record() {
        let temp = tempfile::tempdir().expect("temp");
        let error = cleanup_process_run_dir(Uuid::new_v4(), temp.path(), "test")
            .await
            .expect_err("missing metadata cannot prove termination");
        assert!(error.to_string().contains("missing PID metadata"));

        record_backend_stopped(&stopped_marker(temp.path()))
            .await
            .expect("record stopped");
        cleanup_process_run_dir(Uuid::new_v4(), temp.path(), "test")
            .await
            .expect("durable stop record proves termination");
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
