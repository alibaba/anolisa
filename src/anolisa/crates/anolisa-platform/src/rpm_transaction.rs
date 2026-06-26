//! RPM/DNF backend for [`PackageTransaction`].
//!
//! Runs `dnf <verb> -y <package>` (`install`/`update`/`remove`) through the
//! injectable [`CommandRunner`] so the transaction can be tested with a fake
//! runner instead of a live `dnf`. Only the spawn/exit classification lives
//! here; privilege checks and state refresh stay in the CLI consumer.

use crate::command::{CommandRunner, SystemCommandRunner};
use crate::pkg_transaction::{PackageTransaction, PackageTransactionError};

const DNF: &str = "dnf";

/// RPM/DNF implementation of [`PackageTransaction`].
///
/// Generic over the [`CommandRunner`] so tests can inject a fake; production
/// code uses [`RpmTransaction::system`]. The default type parameter keeps
/// production call sites parameter-free while staying zero-cost.
pub struct RpmTransaction<R: CommandRunner = SystemCommandRunner> {
    runner: R,
}

impl RpmTransaction<SystemCommandRunner> {
    /// Build a transaction that runs real `dnf` on the host.
    pub fn system() -> Self {
        Self {
            runner: SystemCommandRunner,
        }
    }
}

impl<R: CommandRunner> RpmTransaction<R> {
    /// Build a transaction backed by a custom runner (primarily for tests).
    pub fn with_runner(runner: R) -> Self {
        Self { runner }
    }

    /// Run `dnf <verb> -y <package>` and classify the outcome.
    ///
    /// Shared by [`install`](PackageTransaction::install),
    /// [`update`](PackageTransaction::update), and
    /// [`remove`](PackageTransaction::remove) since they differ only in the
    /// dnf verb; `verb` is echoed into the [`TransactionFailed`] operation so
    /// the caller can tell which transaction failed.
    fn run_dnf(&self, verb: &str, package: &str) -> Result<(), PackageTransactionError> {
        // `-y` is required: ANOLISA orchestrates the lifecycle non-interactively,
        // so there is no TTY to answer dnf's confirmation prompt.
        let out = self
            .runner
            .run(DNF, &[verb, "-y", package])
            .map_err(|e| map_spawn_error(e, DNF, verb))?;

        if out.code == Some(0) {
            return Ok(());
        }

        // Prefer stderr for diagnostics; dnf occasionally writes the actionable
        // line to stdout (e.g. "Error: This command has to be run with
        // superuser privileges"), so fall back to stdout when stderr is empty.
        let detail = if out.stderr.trim().is_empty() {
            out.stdout
        } else {
            out.stderr
        };
        Err(PackageTransactionError::TransactionFailed {
            command: DNF.to_string(),
            operation: verb.to_string(),
            code: out.code,
            stderr: detail,
        })
    }
}

impl<R: CommandRunner> PackageTransaction for RpmTransaction<R> {
    fn install(&self, package: &str) -> Result<(), PackageTransactionError> {
        self.run_dnf("install", package)
    }

    fn update(&self, package: &str) -> Result<(), PackageTransactionError> {
        self.run_dnf("update", package)
    }

    fn remove(&self, package: &str) -> Result<(), PackageTransactionError> {
        self.run_dnf("remove", package)
    }
}

/// Map a spawn-phase [`std::io::Error`] to a transaction error by
/// [`std::io::ErrorKind`], mirroring the query backend's classification.
///
/// `verb` records which dnf transaction was being spawned so a non-spawn
/// error kind still names the operation that failed.
fn map_spawn_error(e: std::io::Error, command: &str, verb: &str) -> PackageTransactionError {
    match e.kind() {
        std::io::ErrorKind::NotFound => PackageTransactionError::CommandMissing {
            command: command.to_string(),
        },
        std::io::ErrorKind::PermissionDenied => PackageTransactionError::PermissionDenied {
            command: command.to_string(),
        },
        _ => PackageTransactionError::TransactionFailed {
            command: command.to_string(),
            operation: verb.to_string(),
            code: None,
            stderr: e.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandOutput;
    use std::io;

    /// Preset result for the fake runner: either a captured output or a
    /// spawn-phase error kind to replay.
    enum FakeOutcome {
        Ok(CommandOutput),
        Err(io::ErrorKind),
    }

    /// Fake runner that asserts the dnf call contract and replays a canned
    /// outcome. A program with no preset yields `NotFound`.
    struct FakeCommandRunner {
        dnf: Option<FakeOutcome>,
        expected_verb: String,
        expected_package: String,
    }

    impl CommandRunner for FakeCommandRunner {
        fn run(&self, program: &str, args: &[&str]) -> io::Result<CommandOutput> {
            // Pin the invocation shape: a regression that drops `-y`, swaps the
            // verb, or misplaces the package argument must fail loudly rather
            // than pass on the canned output alone.
            assert_eq!(program, DNF, "transaction must shell out to dnf: {program}");
            assert_eq!(
                args,
                [
                    self.expected_verb.as_str(),
                    "-y",
                    self.expected_package.as_str()
                ],
                "dnf args drifted: {args:?}"
            );
            match &self.dnf {
                Some(FakeOutcome::Ok(o)) => Ok(o.clone()),
                Some(FakeOutcome::Err(kind)) => {
                    Err(io::Error::new(*kind, format!("fake {program} failure")))
                }
                None => Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no fake preset for {program}"),
                )),
            }
        }
    }

    fn txn(
        expected_verb: &str,
        expected_package: &str,
        outcome: FakeOutcome,
    ) -> RpmTransaction<FakeCommandRunner> {
        RpmTransaction::with_runner(FakeCommandRunner {
            dnf: Some(outcome),
            expected_verb: expected_verb.to_string(),
            expected_package: expected_package.to_string(),
        })
    }

    fn ok_out(code: Option<i32>, stdout: &str, stderr: &str) -> FakeOutcome {
        FakeOutcome::Ok(CommandOutput {
            code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        })
    }

    #[test]
    fn update_success_returns_ok() {
        let t = txn(
            "update",
            "copilot-shell",
            ok_out(Some(0), "Upgraded:\n  copilot-shell\n", ""),
        );
        t.update("copilot-shell").expect("update ok");
    }

    #[test]
    fn install_success_returns_ok() {
        let t = txn(
            "install",
            "copilot-shell",
            ok_out(Some(0), "Installed:\n  copilot-shell\n", ""),
        );
        t.install("copilot-shell").expect("install ok");
    }

    #[test]
    fn remove_success_returns_ok() {
        let t = txn(
            "remove",
            "copilot-shell",
            ok_out(Some(0), "Removed:\n  copilot-shell\n", ""),
        );
        t.remove("copilot-shell").expect("remove ok");
    }

    #[test]
    fn remove_nonzero_exit_records_remove_operation() {
        // The failed-operation label must follow the verb so callers can tell a
        // remove failure apart from an install/update failure.
        let t = txn(
            "remove",
            "copilot-shell",
            ok_out(Some(1), "", "Error: No match for argument: copilot-shell"),
        );
        let err = t.remove("copilot-shell").unwrap_err();
        match err {
            PackageTransactionError::TransactionFailed {
                operation, stderr, ..
            } => {
                assert_eq!(operation, "remove");
                assert!(stderr.contains("No match for argument"));
            }
            other => panic!("expected TransactionFailed, got {other:?}"),
        }
    }

    #[test]
    fn update_nonzero_exit_maps_to_transaction_failed() {
        let t = txn(
            "update",
            "copilot-shell",
            ok_out(Some(1), "", "Error: nothing to do, repo unreachable"),
        );
        let err = t.update("copilot-shell").unwrap_err();
        match err {
            PackageTransactionError::TransactionFailed {
                command,
                operation,
                code,
                stderr,
            } => {
                assert_eq!(command, DNF);
                assert_eq!(operation, "update");
                assert_eq!(code, Some(1));
                assert!(stderr.contains("repo unreachable"));
            }
            other => panic!("expected TransactionFailed, got {other:?}"),
        }
    }

    #[test]
    fn install_nonzero_exit_records_install_operation() {
        // The failed-operation label must follow the verb so callers can tell
        // an install failure apart from an update failure.
        let t = txn(
            "install",
            "copilot-shell",
            ok_out(Some(1), "", "Error: No match for argument"),
        );
        let err = t.install("copilot-shell").unwrap_err();
        match err {
            PackageTransactionError::TransactionFailed {
                operation, stderr, ..
            } => {
                assert_eq!(operation, "install");
                assert!(stderr.contains("No match for argument"));
            }
            other => panic!("expected TransactionFailed, got {other:?}"),
        }
    }

    #[test]
    fn update_failure_falls_back_to_stdout_when_stderr_empty() {
        // dnf's privilege refusal is written to stdout; surface it rather than
        // an empty diagnostic.
        let t = txn(
            "update",
            "copilot-shell",
            ok_out(
                Some(1),
                "Error: This command has to be run with superuser privileges",
                "",
            ),
        );
        let err = t.update("copilot-shell").unwrap_err();
        match err {
            PackageTransactionError::TransactionFailed { stderr, .. } => {
                assert!(stderr.contains("superuser privileges"), "got: {stderr}");
            }
            other => panic!("expected TransactionFailed, got {other:?}"),
        }
    }

    #[test]
    fn command_missing_maps_to_error() {
        let t = txn("update", "x", FakeOutcome::Err(io::ErrorKind::NotFound));
        let err = t.update("x").unwrap_err();
        assert!(matches!(
            err,
            PackageTransactionError::CommandMissing { command } if command == DNF
        ));
    }

    #[test]
    fn permission_denied_maps_to_error() {
        let t = txn(
            "update",
            "x",
            FakeOutcome::Err(io::ErrorKind::PermissionDenied),
        );
        let err = t.update("x").unwrap_err();
        assert!(matches!(
            err,
            PackageTransactionError::PermissionDenied { command } if command == DNF
        ));
    }
}
