#![forbid(unsafe_code)]
//! cosh-platform: Distribution Abstraction Layer for the cosh deterministic interaction layer.
//!
//! Detects the current distro and routes pkg/svc operations to the
//! appropriate backend (dnf, apt, zypper, etc.).

pub mod audit;
pub mod checkpoint;
pub mod detect;
pub mod pkg;
pub mod process;
pub mod svc;

pub mod validate;

use std::process::{Command, Output};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cosh_types::error::{CoshError, ErrorCode};

const PKG_TIMEOUT: Duration = Duration::from_secs(120);
const SVC_TIMEOUT: Duration = Duration::from_secs(30);

/// Run an external command with a timeout. Reads stdout/stderr in background
/// threads to avoid pipe-buffer deadlock. Returns `ErrorCode::Timeout` if the
/// process exceeds the deadline. The deadline also covers draining stdout and
/// stderr: a grandchild that keeps the pipes open after the direct child
/// exited gets its whole process group killed instead of stalling the caller.
pub fn run_command(
    cmd: &mut Command,
    timeout: Duration,
    subsystem: &str,
) -> Result<Output, CoshError> {
    // Lead a fresh process group so a timeout can reap grandchildren too.
    process::isolate_process_group(cmd);
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            CoshError::new(
                ErrorCode::Unknown,
                format!("Failed to spawn command: {}", e),
                subsystem,
            )
        })?;
    let pgid = child.id();

    // Drain pipes in background threads to prevent buffer-full deadlock;
    // results come back over channels so draining can honor the deadline.
    let stdout_rx = drain_pipe(child.stdout.take());
    let stderr_rx = drain_pipe(child.stderr.take());

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_group_and_reap(&mut child);
                    return Err(timeout_error(timeout, subsystem));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                kill_group_and_reap(&mut child);
                return Err(CoshError::new(
                    ErrorCode::Unknown,
                    format!("Failed to wait for command: {}", e),
                    subsystem,
                ));
            }
        }
    };

    // The direct child has exited, but a grandchild may still hold the
    // pipes open; draining past the deadline kills the leftover group.
    let Some(stdout) = recv_until(&stdout_rx, deadline) else {
        kill_group(pgid);
        return Err(timeout_error(timeout, subsystem));
    };
    let Some(stderr) = recv_until(&stderr_rx, deadline) else {
        kill_group(pgid);
        return Err(timeout_error(timeout, subsystem));
    };

    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn timeout_error(timeout: Duration, subsystem: &str) -> CoshError {
    CoshError::new(
        ErrorCode::Timeout,
        format!("Command timed out after {}s", timeout.as_secs()),
        subsystem,
    )
    .recoverable(true)
    .with_hint("The operation took too long. Retry or check system load.")
}

/// SIGKILLs the whole group, then fallback-kills and reaps the direct child.
fn kill_group_and_reap(child: &mut std::process::Child) {
    kill_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

fn kill_group(pgid: u32) {
    if let Err(e) = process::kill_process_group(pgid) {
        tracing::warn!(
            target: "cosh_platform",
            pgid,
            "failed to kill timed-out process group: {e}"
        );
    }
}

/// Drains one output pipe on a background thread; the receiver yields the
/// collected bytes once the pipe reaches EOF.
fn drain_pipe<R: std::io::Read + Send + 'static>(pipe: Option<R>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    match pipe {
        Some(r) => {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                std::io::Read::read_to_end(&mut std::io::BufReader::new(r), &mut buf).ok();
                let _ = tx.send(buf);
            });
        }
        None => {
            let _ = tx.send(Vec::new());
        }
    }
    rx
}

/// Receives drained output within the remaining deadline; `None` means a
/// pipe holder (e.g. a background grandchild) outlived the budget.
fn recv_until(rx: &mpsc::Receiver<Vec<u8>>, deadline: Instant) -> Option<Vec<u8>> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    match rx.recv_timeout(remaining) {
        Ok(buf) => Some(buf),
        Err(mpsc::RecvTimeoutError::Timeout) => None,
        // The reader thread died without sending; treat as empty output.
        Err(mpsc::RecvTimeoutError::Disconnected) => Some(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Best-effort SIGKILL of recorded PIDs and their groups so a failed
    /// assertion does not leak processes into CI.
    struct PidCleanup(Vec<i32>);

    impl Drop for PidCleanup {
        fn drop(&mut self) {
            for pid in &self.0 {
                let _ = Command::new("sh")
                    .arg("-c")
                    .arg(format!("kill -9 -- -{pid} {pid} 2>/dev/null"))
                    .status();
            }
        }
    }

    /// Whether `pid` can still execute code. Zombie (Z) and dead (X)
    /// states count as terminated: SIGKILL already landed but the parent
    /// has not reaped the entry yet, and `kill -0` would still report
    /// such a PID as alive.
    ///
    /// Fails closed: an unrunnable or misbehaving `ps` panics instead of
    /// letting the liveness assertion pass vacuously.
    fn process_can_run(pid: i32) -> bool {
        let output = Command::new("ps")
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

    /// Reads the `<shell-pid> <grandchild-pid>` pair, polling briefly
    /// because the child writes it right after spawning.
    fn read_pids(path: &std::path::Path) -> Vec<i32> {
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(path) {
                let pids: Vec<i32> = text
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

    /// Asserts `pid` terminates within 2.5s, well before the grandchild's
    /// scheduled marker write at 5s.
    fn assert_process_gone(pid: i32) {
        for _ in 0..125 {
            if !process_can_run(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("process {pid} survived the timeout kill");
    }

    /// Sleeps until safely past the grandchild's scheduled marker write.
    fn wait_past_marker_deadline(started: Instant) {
        let budget = Duration::from_millis(5500);
        if started.elapsed() < budget {
            std::thread::sleep(budget - started.elapsed());
        }
    }

    #[test]
    fn run_command_timeout_kills_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        let pid_file = dir.path().join("pids");
        // Grandchild writes the marker after 5s; the direct child records
        // `<shell-pid> <grandchild-pid>` and blocks past the timeout.
        let script = format!(
            "(sleep 5; : > '{}') & echo $$ $! > '{}'; sleep 30",
            marker.display(),
            pid_file.display()
        );

        let started = Instant::now();
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(script);
        let err = run_command(&mut cmd, Duration::from_millis(300), "test").unwrap_err();
        assert!(matches!(err.code, ErrorCode::Timeout));

        let pids = read_pids(&pid_file);
        let _cleanup = PidCleanup(pids.clone());

        for pid in &pids {
            assert_process_gone(*pid);
        }

        wait_past_marker_deadline(started);
        assert!(!marker.exists(), "grandchild survived the timeout");
    }

    #[test]
    fn run_command_drain_respects_deadline_when_grandchild_holds_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        let pid_file = dir.path().join("pids");
        // The direct child exits immediately with success, but its
        // backgrounded grandchild inherits stdout and keeps it open.
        let script = format!(
            "(sleep 5; : > '{}') & echo $$ $! > '{}'; exit 0",
            marker.display(),
            pid_file.display()
        );

        let started = Instant::now();
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(script);
        let err = run_command(&mut cmd, Duration::from_millis(300), "test").unwrap_err();
        assert!(matches!(err.code, ErrorCode::Timeout));
        assert!(
            started.elapsed() < Duration::from_millis(2500),
            "drain must return at the deadline, not when the grandchild exits"
        );

        let pids = read_pids(&pid_file);
        let _cleanup = PidCleanup(pids.clone());

        // The grandchild pipe holder must be killed with the group.
        assert_process_gone(pids[1]);

        wait_past_marker_deadline(started);
        assert!(!marker.exists(), "grandchild survived the drain timeout");
    }
}
