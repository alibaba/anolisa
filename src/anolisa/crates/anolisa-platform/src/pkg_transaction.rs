//! Backend-neutral package *mutation* contract.
//!
//! [`PackageTransaction`] is the write-side counterpart to
//! [`PackageQuery`](crate::pkg_query::PackageQuery): where the query contract
//! only reads rpmdb / repo metadata, this one runs the package-manager
//! transactions ANOLISA delegates to dnf/rpm. The MVP exposes a single
//! operation — `update` — used by `anolisa update` for `rpm-observed` and
//! `rpm-managed` components.
//!
//! The trait is object-safe so the CLI can hold a `&dyn PackageTransaction`
//! and inject a fake in tests instead of shelling out to a live `dnf`.
//! Privilege checks and post-transaction state refresh are the caller's
//! responsibility; this layer only spawns the transaction and classifies its
//! outcome.

use thiserror::Error;

/// Errors raised by [`PackageTransaction`] backends.
///
/// Mirrors [`PackageQueryError`](crate::pkg_query::PackageQueryError)'s
/// spawn-vs-exit split: a missing or non-executable binary is a spawn-phase
/// fault, while a backend that ran and exited non-zero surfaces as
/// [`TransactionFailed`](PackageTransactionError::TransactionFailed).
#[derive(Debug, Error)]
pub enum PackageTransactionError {
    /// The backend binary could not be found (spawn `NotFound`).
    #[error("command not found: {command}")]
    CommandMissing {
        /// Backend binary that could not be found.
        command: String,
    },
    /// The backend binary existed but could not be executed
    /// (`PermissionDenied`). For a privileged transaction this typically
    /// means the process is not running as root.
    #[error("permission denied running {command}")]
    PermissionDenied {
        /// Backend binary that could not be executed.
        command: String,
    },
    /// The transaction ran but the backend reported a hard failure
    /// (non-zero exit). `stderr` carries the captured diagnostics so the
    /// caller can surface why dnf refused.
    #[error("{command} {operation} failed (code {code:?}): {stderr}")]
    TransactionFailed {
        /// Backend binary that exited with a failure.
        command: String,
        /// Transaction verb that failed (e.g. `update`).
        operation: String,
        /// Exit code; `None` if the process was killed by a signal.
        code: Option<i32>,
        /// Captured diagnostics from the failed transaction.
        stderr: String,
    },
}

/// Backend-neutral package mutation contract.
///
/// All methods take `&self` and return concrete types, so the trait is
/// object-safe and any backend can be held as `Box<dyn PackageTransaction>`.
pub trait PackageTransaction {
    /// Install `package` from the configured repos.
    ///
    /// Delegates the whole file transaction (dependency solving, download,
    /// scriptlets, rpmdb write) to the package manager. ANOLISA records the
    /// result as an ANOLISA-delegated *managed* install — the package manager
    /// owns the files and a later uninstall delegates back to it. A package
    /// that is already installed is a success
    /// (the backend performs a no-op), not an error.
    ///
    /// # Errors
    /// See [`PackageTransactionError`]. The caller owns the privilege
    /// precondition and records ANOLISA state from rpmdb afterwards.
    fn install(&self, package: &str) -> Result<(), PackageTransactionError>;

    /// Update `package` to the latest candidate the configured repos offer.
    ///
    /// Delegates the whole file transaction (download, scriptlets, rpmdb
    /// write) to the package manager — ANOLISA never touches RPM-owned files
    /// directly. The update does **not** switch backends: it upgrades the
    /// package in place. A package that is already at the latest version is a
    /// success (the backend performs a no-op), not an error.
    ///
    /// # Errors
    /// See [`PackageTransactionError`] for the failure conditions. The caller
    /// is responsible for the privilege precondition and for refreshing
    /// ANOLISA state from rpmdb after a successful update.
    fn update(&self, package: &str) -> Result<(), PackageTransactionError>;
}
