//! Backend-neutral command execution seam for host package queries.
//!
//! [`CommandRunner`] abstracts spawning a process and capturing its output so
//! that query backends can be tested with a fake runner instead of shelling
//! out to real `rpm`/`dnf`. The runner stays pure: spawn failures surface as
//! [`std::io::Error`] and business-level exit-code interpretation lives in the
//! query layer that consumes the runner.

use std::process::Command;

/// Outcome of a single command invocation after the process has exited.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    /// Exit code; `None` when the process was terminated by a signal.
    pub code: Option<i32>,
    /// Captured standard output, decoded lossily as UTF-8.
    pub stdout: String,
    /// Captured standard error, decoded lossily as UTF-8.
    pub stderr: String,
}

/// Abstraction over "run a program and capture its output", injectable for tests.
///
/// Implementations must keep the spawn/exit distinction from §4 of the design:
/// a successfully spawned command that exits non-zero is **not** an error — it
/// is returned as `Ok` carrying the non-zero [`CommandOutput::code`]. Only
/// spawn-phase failures (binary missing, no execute permission, etc.) become
/// `Err`, letting the query layer classify them by [`std::io::ErrorKind`].
pub trait CommandRunner {
    /// Run `program` with `args`, returning the exit code and captured streams.
    ///
    /// # Errors
    /// Returns the spawn-phase io error (e.g. `NotFound` / `PermissionDenied`);
    /// a successfully spawned command that exits non-zero is **not** an `Err`.
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<CommandOutput>;
}

/// Runs real [`std::process::Command`]s.
///
/// Forces `LC_ALL=C`: rpm/dnf human-readable messages (e.g. the
/// `is not installed` notice and error strings) are localized by default,
/// which would make message-based detection in [`crate::rpm_query`] misfire
/// under non-English locales. The C locale pins these messages to English
/// without affecting `--qf` field values (which are never localized).
/// `LANGUAGE` is ignored by gettext under the C/POSIX locale, so no extra
/// scrubbing is needed.
pub struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<CommandOutput> {
        let output = Command::new(program)
            .args(args)
            .env("LC_ALL", "C")
            .output()?;
        Ok(CommandOutput {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}
