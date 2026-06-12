#![forbid(unsafe_code)]
//! cosh-platform: Distribution Abstraction Layer for the cosh deterministic interaction layer.
//!
//! Detects the current distro and routes pkg/svc operations to the
//! appropriate backend (dnf, apt, zypper, etc.).

pub mod audit;
pub mod checkpoint;
pub mod detect;
pub mod pkg;
pub mod svc;

pub mod validate;

use std::process::{Command, Output};
use std::time::{Duration, Instant};

use cosh_types::error::{CoshError, ErrorCode};

const PKG_TIMEOUT: Duration = Duration::from_secs(120);
const SVC_TIMEOUT: Duration = Duration::from_secs(30);

/// Run an external command with a timeout. Reads stdout/stderr in background
/// threads to avoid pipe-buffer deadlock. Returns `ErrorCode::Timeout` if the
/// process exceeds the deadline.
pub fn run_command(cmd: &mut Command, timeout: Duration, subsystem: &str) -> Result<Output, CoshError> {
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

    // Drain pipes in background threads to prevent buffer-full deadlock.
    let stdout_handle = child.stdout.take().map(|r| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut std::io::BufReader::new(r), &mut buf).ok();
            buf
        })
    });
    let stderr_handle = child.stderr.take().map(|r| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut std::io::BufReader::new(r), &mut buf).ok();
            buf
        })
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(CoshError::new(
                        ErrorCode::Timeout,
                        format!("Command timed out after {}s", timeout.as_secs()),
                        subsystem,
                    )
                    .recoverable(true)
                    .with_hint("The operation took too long. Retry or check system load."));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                return Err(CoshError::new(
                    ErrorCode::Unknown,
                    format!("Failed to wait for command: {}", e),
                    subsystem,
                ));
            }
        }
    };

    let stdout = stdout_handle.and_then(|h| h.join().ok()).unwrap_or_default();
    let stderr = stderr_handle.and_then(|h| h.join().ok()).unwrap_or_default();

    Ok(Output { status, stdout, stderr })
}
