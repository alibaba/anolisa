//! Deadline-bounded async child execution with whole-process-tree cleanup.
//!
//! Shared by the shell tool and the hook system: `tokio::time::timeout`
//! around `output()` only cancels the future and leaks the process tree.
//! Here the child leads its own process group, the deadline covers stdin
//! writing, waiting, and output collection, and a RAII guard SIGKILLs the
//! group even when the calling future itself is cancelled.

use std::process::Output;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

/// Failure modes of [`output_with_timeout`].
#[derive(Debug)]
pub enum OutputError {
    /// The process could not be spawned.
    Spawn(std::io::Error),
    /// The process spawned but waiting or collecting output failed.
    Io(std::io::Error),
    /// The deadline expired; the process group was killed and reaped.
    Timeout,
}

impl std::fmt::Display for OutputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "failed to spawn process: {e}"),
            Self::Io(e) => write!(f, "process I/O failed: {e}"),
            Self::Timeout => write!(f, "process timed out"),
        }
    }
}

/// SIGKILLs the child's process group on drop unless disarmed.
///
/// Covers cancellation of the caller's future: drop runs synchronously, and
/// tokio's orphan reaper collects the killed child afterwards.
struct ProcessGroupGuard {
    pgid: Option<u32>,
}

impl ProcessGroupGuard {
    fn new(pgid: Option<u32>) -> Self {
        Self { pgid }
    }

    fn disarm(&mut self) {
        self.pgid = None;
    }

    /// Kills the group now and disarms; logs failures other than ESRCH.
    fn kill_now(&mut self) {
        if let Some(pgid) = self.pgid.take() {
            if let Err(e) = cosh_platform::process::kill_process_group(pgid) {
                tracing::warn!(target: "cosh_process", pgid, "failed to kill process group: {e}");
            }
        }
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        self.kill_now();
    }
}

/// Runs `cmd` to completion within `timeout`, returning its collected output.
///
/// The child is spawned as its own process-group leader with stdout/stderr
/// piped; stdin is piped only when `stdin_data` is provided, otherwise null.
/// The deadline covers writing stdin, waiting for exit, and draining output,
/// so a child (or grandchild) that never reads stdin or holds the pipes open
/// cannot stall the caller past the deadline.
///
/// Cancelling the returned future SIGKILLs the process group via a drop
/// guard, falls back to `kill_on_drop` for the direct child, and aborts the
/// output reader tasks.
///
/// # Errors
///
/// - [`OutputError::Spawn`] if the process cannot be started.
/// - [`OutputError::Io`] if waiting or collecting output fails; the process
///   group is killed and the child reaped before returning.
/// - [`OutputError::Timeout`] if the deadline expires; the whole process
///   group receives SIGKILL, the direct child gets a fallback kill and is
///   explicitly reaped, and reader tasks are aborted before returning.
pub async fn output_with_timeout(
    mut cmd: Command,
    stdin_data: Option<Vec<u8>>,
    timeout: Duration,
) -> Result<Output, OutputError> {
    cosh_platform::process::isolate_process_group(cmd.as_std_mut());
    cmd.stdin(if stdin_data.is_some() {
        std::process::Stdio::piped()
    } else {
        std::process::Stdio::null()
    })
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    // Direct-child fallback for the cancellation path, where only the
    // guard's killpg runs and could fail with a non-ESRCH error.
    .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(OutputError::Spawn)?;
    let mut guard = ProcessGroupGuard::new(child.id());

    let stdin = child.stdin.take();
    let mut stdout_task = spawn_reader(child.stdout.take());
    let mut stderr_task = spawn_reader(child.stderr.take());

    let run = async {
        if let (Some(mut stdin), Some(data)) = (stdin, stdin_data) {
            // The child may exit without reading stdin; broken pipes are fine.
            let _ = stdin.write_all(&data).await;
            let _ = stdin.shutdown().await;
        }
        let status = child.wait().await?;
        let stdout = join_reader(&mut stdout_task).await?;
        let stderr = join_reader(&mut stderr_task).await?;
        Ok::<Output, std::io::Error>(Output {
            status,
            stdout,
            stderr,
        })
    };

    let result = tokio::time::timeout(timeout, run).await;
    match result {
        Ok(Ok(output)) => {
            guard.disarm();
            Ok(output)
        }
        Ok(Err(e)) => {
            kill_and_reap(&mut guard, &mut child).await;
            Err(OutputError::Io(e))
        }
        Err(_) => {
            kill_and_reap(&mut guard, &mut child).await;
            Err(OutputError::Timeout)
        }
    }
    // Reader tasks are aborted by their drop guards on every exit path.
}

/// SIGKILLs the process group, then fallback-kills and reaps the child.
async fn kill_and_reap(guard: &mut ProcessGroupGuard, child: &mut Child) {
    guard.kill_now();
    // Fallback for the direct child; harmless if the group kill landed.
    let _ = child.start_kill();
    let _ = child.wait().await;
}

/// Reader task handle that aborts on drop, so neither cancellation nor an
/// early error return leaves a detached task behind.
struct ReaderTask(JoinHandle<std::io::Result<Vec<u8>>>);

impl Drop for ReaderTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn spawn_reader<R>(reader: Option<R>) -> ReaderTask
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    ReaderTask(tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut r) = reader {
            r.read_to_end(&mut buf).await?;
        }
        Ok(buf)
    }))
}

/// Awaits a reader task, surfacing read errors and join failures so output
/// collection failures reach the caller's kill/reap branch as I/O errors.
async fn join_reader(task: &mut ReaderTask) -> std::io::Result<Vec<u8>> {
    match (&mut task.0).await {
        Ok(result) => result,
        Err(e) => Err(std::io::Error::other(format!(
            "output reader task failed: {e}"
        ))),
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Shared fixtures for process-tree cleanup regression tests.

    use std::path::Path;
    use std::time::{Duration, Instant};

    /// Delay before the leaked grandchild writes its marker file. PID
    /// liveness polling must finish well before this elapses; the margin
    /// is generous because CI runs these suites under heavy parallel load.
    pub const MARKER_DELAY: Duration = Duration::from_secs(5);

    /// Shell script that backgrounds a grandchild which writes `marker`
    /// after [`MARKER_DELAY`], records `<shell-pid> <grandchild-pid>` into
    /// `pids`, then blocks far past any test timeout.
    pub fn leak_script(marker: &Path, pids: &Path) -> String {
        format!(
            "(sleep {}; : > '{}') & echo $$ $! > '{}'; sleep 30",
            MARKER_DELAY.as_secs(),
            marker.display(),
            pids.display()
        )
    }

    /// Variant of [`leak_script`] whose direct child exits successfully at
    /// once, leaving the grandchild as the only holder of stdout/stderr.
    pub fn stdout_holder_script(marker: &Path, pids: &Path) -> String {
        format!(
            "(sleep {}; : > '{}') & echo $$ $! > '{}'; exit 0",
            MARKER_DELAY.as_secs(),
            marker.display(),
            pids.display()
        )
    }

    /// Reads the two PIDs recorded by [`leak_script`], polling briefly
    /// because the child writes them right after spawning.
    pub fn read_pids(path: &Path) -> Vec<i32> {
        for _ in 0..100 {
            if let Ok(content) = std::fs::read_to_string(path) {
                let pids: Vec<i32> = content
                    .split_whitespace()
                    .filter_map(|t| t.parse().ok())
                    .collect();
                if pids.len() == 2 {
                    return pids;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("pid file {} was never fully written", path.display());
    }

    /// Best-effort SIGKILL of the recorded PIDs and their groups so a
    /// failed assertion does not leak processes into CI.
    pub struct PidCleanup(pub Vec<i32>);

    impl Drop for PidCleanup {
        fn drop(&mut self) {
            for pid in &self.0 {
                let _ = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(format!("kill -9 -- -{pid} {pid} 2>/dev/null"))
                    .status();
            }
        }
    }

    /// Asserts `pid` terminates within 2.5s — well before [`MARKER_DELAY`],
    /// so a leaked-but-finishing grandchild cannot fake a pass.
    pub fn assert_process_gone(pid: i32) {
        for _ in 0..125 {
            if !process_can_run(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("process {pid} survived the timeout kill");
    }

    /// Whether `pid` can still execute code. Zombie (Z) and dead (X)
    /// states count as terminated: SIGKILL already landed but the parent
    /// (e.g. tokio's best-effort orphan reaper) has not reaped the entry
    /// yet, and `kill -0` would still report such a PID as alive.
    ///
    /// Fails closed: an unrunnable or misbehaving `ps` panics instead of
    /// letting the liveness assertion pass vacuously.
    fn process_can_run(pid: i32) -> bool {
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("failed to run ps to check process state");
        let stat = String::from_utf8_lossy(&output.stdout);
        match stat.trim().chars().next() {
            Some('Z' | 'X') => false,
            Some(_) => true,
            // No stat line: ps signals "no such process" via non-zero
            // exit. A successful exit without output is a ps anomaly and
            // must fail the test rather than report the PID as gone.
            None => {
                assert!(
                    !output.status.success() && output.stderr.is_empty(),
                    "ps failed without reporting that pid {pid} is absent: status={}, stderr={}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                );
                false
            }
        }
    }

    /// Sleeps until safely past the grandchild's scheduled marker write.
    pub fn wait_past_marker_deadline(started: Instant) {
        let budget = MARKER_DELAY + Duration::from_millis(500);
        let elapsed = started.elapsed();
        if elapsed < budget {
            std::thread::sleep(budget - elapsed);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::test_support::*;
    use super::*;

    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(script);
        cmd
    }

    #[tokio::test]
    async fn collects_output_and_status() {
        let out = output_with_timeout(
            sh("printf out; printf err >&2"),
            None,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, b"out");
        assert_eq!(out.stderr, b"err");
    }

    #[tokio::test]
    async fn stdin_is_delivered() {
        let out = output_with_timeout(sh("cat"), Some(b"ping".to_vec()), Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(out.stdout, b"ping");
    }

    #[tokio::test]
    async fn timeout_kills_grandchildren() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        let pid_file = dir.path().join("pids");

        let started = Instant::now();
        let err = output_with_timeout(
            sh(&leak_script(&marker, &pid_file)),
            None,
            Duration::from_millis(300),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, OutputError::Timeout));

        let pids = read_pids(&pid_file);
        let _cleanup = PidCleanup(pids.clone());
        for pid in &pids {
            assert_process_gone(*pid);
        }
        wait_past_marker_deadline(started);
        assert!(!marker.exists(), "grandchild survived the timeout");
    }

    #[tokio::test]
    async fn unread_stdin_respects_deadline() {
        // Larger than any pipe buffer: writing it to a child that never
        // reads stdin must still be bounded by the deadline.
        let payload = vec![b'x'; 1 << 20];
        let started = Instant::now();
        let err = output_with_timeout(sh("sleep 30"), Some(payload), Duration::from_millis(300))
            .await
            .unwrap_err();
        assert!(matches!(err, OutputError::Timeout));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn grandchild_holding_stdout_cannot_stall_past_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        let pid_file = dir.path().join("pids");

        let started = Instant::now();
        let err = output_with_timeout(
            sh(&stdout_holder_script(&marker, &pid_file)),
            None,
            Duration::from_millis(300),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, OutputError::Timeout));
        assert!(
            started.elapsed() < Duration::from_millis(2500),
            "output drain must return at the deadline, not when the grandchild exits"
        );

        let pids = read_pids(&pid_file);
        let _cleanup = PidCleanup(pids.clone());
        // The grandchild pipe holder must be killed with the group.
        assert_process_gone(pids[1]);
        wait_past_marker_deadline(started);
        assert!(!marker.exists(), "grandchild survived the drain timeout");
    }

    // Multi-threaded runtime: the test thread polls with blocking sleeps
    // while the cancelled future runs on another worker.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_caller_kills_process_tree() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        let pid_file = dir.path().join("pids");
        let script = leak_script(&marker, &pid_file);

        let started = Instant::now();
        let handle = tokio::spawn(output_with_timeout(
            sh(&script),
            None,
            Duration::from_secs(30),
        ));

        // The pid file proves the child is running before we cancel.
        let pids = read_pids(&pid_file);
        let _cleanup = PidCleanup(pids.clone());
        handle.abort();
        let _ = handle.await;

        for pid in &pids {
            assert_process_gone(*pid);
        }
        wait_past_marker_deadline(started);
        assert!(!marker.exists(), "grandchild survived caller cancellation");
    }
}
