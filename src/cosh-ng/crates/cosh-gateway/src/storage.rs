//! SQLite-backed durable storage for the Gateway task plane.
//!
//! The store owns one write connection and commits task events, projections,
//! command receipts, and Outbox intents in one immediate transaction.

mod schema;
mod sqlite;
mod task_store;

pub use sqlite::SqliteTaskStore;
pub use task_store::{CommitOutcome, CommitReceipt, OutboxIntent, TaskCommit};

use std::path::PathBuf;

use thiserror::Error;

use crate::task::AggregateError;

/// Fail-closed storage errors exposed to Task coordination.
#[derive(Debug, Error)]
pub enum StoreError {
    /// The configured database path or a companion file is unsafe.
    #[error("unsafe Gateway database path {path}: {message}")]
    UnsafePath {
        /// Path rejected before or during database open.
        path: PathBuf,
        /// Bounded developer-oriented reason.
        message: String,
    },
    /// The database uses a schema newer than this binary understands.
    #[error("Gateway database schema {found} is newer than supported schema {supported}")]
    NewerSchema {
        /// Schema version read from the database.
        found: u32,
        /// Highest version supported by this binary.
        supported: u32,
    },
    /// An already-applied migration does not match this binary.
    #[error("Gateway migration {version} checksum mismatch")]
    MigrationChecksum {
        /// Migration version whose content changed.
        version: u32,
    },
    /// A Task command reused an idempotency key with another digest.
    #[error("idempotency key was already used with a different command digest")]
    IdempotencyConflict,
    /// A command observed a Task revision different from its precondition.
    #[error("task revision conflict: expected {expected}, found {actual}")]
    RevisionConflict {
        /// Revision supplied by the command.
        expected: u64,
        /// Current durable revision.
        actual: u64,
    },
    /// A requested Task does not exist.
    #[error("task not found")]
    TaskNotFound,
    /// A commit batch violates Task or Outbox invariants.
    #[error("invalid Gateway task commit: {message}")]
    InvalidCommit {
        /// Bounded developer-oriented reason.
        message: String,
    },
    /// A committed Task stream violates reducer invariants.
    #[error("Gateway task transition rejected: {0}")]
    Aggregate(#[from] AggregateError),
    /// Stored data violates the versioned Task contract.
    #[error("corrupt Gateway task storage: {message}")]
    Corrupt {
        /// Bounded decode or invariant detail.
        message: String,
    },
    /// SQLite rejected an operation.
    #[error("Gateway database operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A contract value could not be serialized or decoded.
    #[error("Gateway task contract serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

impl StoreError {
    /// Returns whether a caller can safely retry after refreshing state.
    pub fn recoverable(&self) -> bool {
        match self {
            Self::RevisionConflict { .. } | Self::TaskNotFound => true,
            Self::Sqlite(rusqlite::Error::SqliteFailure(code, _)) => matches!(
                code.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ),
            _ => false,
        }
    }
}
