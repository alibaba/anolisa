//! `PolicyStore` activation transaction (§9.2): prepare/commit/cancel,
//! atomic revision switch, last-known-good rollback and revision history.

use std::path::PathBuf;
use std::sync::Arc;

use asc_policy_ir::model::PolicyIr;
use asc_policy_types::primitives::{PolicyRevision, Timestamp};
use serde::{Deserialize, Serialize};

use crate::bundle::SignedBundle;

/// Handle to a verified and compiled bundle in the staging area; commit or
/// cancel it. Leftover staging state is discarded on crash recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedRevision {
    /// Revision the staged bundle would become.
    pub revision: PolicyRevision,
    /// Staging directory holding the compiled artifacts.
    pub staging_dir: PathBuf,
}

/// Read-only snapshot of the active revision; consistent for the guard's
/// lifetime, lock-free to read.
#[derive(Debug, Clone)]
pub struct ActiveSnapshot {
    /// Active revision.
    pub revision: PolicyRevision,
    /// Compiled policy the PDP evaluates.
    pub ir: Arc<PolicyIr>,
}

/// Verification state of a locally retained revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    /// Passed the full verification chain; rollback-eligible.
    Verified,
    /// Failed some verification step; kept only for diagnostics.
    Rejected,
}

/// Metadata of a locally retained revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionMeta {
    /// Revision identity.
    pub revision: PolicyRevision,
    /// Verification state.
    pub status: VerificationStatus,
    /// Activation time, when it was ever active.
    pub activated_at: Option<Timestamp>,
}

/// Prepare-phase failures; none of them affects the active revision.
#[derive(Debug, thiserror::Error)]
pub enum PrepareError {
    /// Signature verification failed against the trust anchor.
    #[error("bundle signature invalid")]
    Signature,
    /// policyEpoch is not strictly greater than the active one (T6).
    #[error("policy epoch {offered} does not advance past active {active}")]
    EpochRegression {
        /// Epoch offered by the bundle.
        offered: u64,
        /// Currently active epoch.
        active: u64,
    },
    /// Schema validation failed.
    #[error("bundle schema invalid: {0}")]
    Schema(String),
    /// Compilation to IR failed.
    #[error("bundle compilation failed: {0}")]
    Compile(String),
    /// Conformance corpus failed.
    #[error("conformance tests failed: {0}")]
    Conformance(String),
    /// Staging area IO failure.
    #[error("staging io failure: {0}")]
    Io(String),
}

/// Commit-phase failures; the active revision stays unchanged on error.
#[derive(Debug, thiserror::Error)]
pub enum CommitError {
    /// The staged revision no longer matches the staging area content.
    #[error("staged revision is stale")]
    StaleStaging,
    /// Atomic switch could not complete.
    #[error("commit io failure: {0}")]
    Io(String),
}

/// Rollback failures.
#[derive(Debug, thiserror::Error)]
pub enum RollbackError {
    /// No verified prior revision is retained locally.
    #[error("no last-known-good revision available")]
    NoKnownGood,
    /// Rollback switch could not complete.
    #[error("rollback io failure: {0}")]
    Io(String),
}

/// Policy lifecycle transaction (§9.2), following SELinux
/// prepare/commit/cancel plus OPA bundle semantics. After commit at least
/// N=3 prior revisions are retained for rollback.
pub trait PolicyStore: Send + Sync {
    /// Verifies and compiles a bundle into the staging area; returns a
    /// committable handle. Never affects the active revision.
    ///
    /// # Errors
    /// Returns [`PrepareError`] for any verification-chain step failure.
    fn prepare(&self, bundle: SignedBundle) -> Result<StagedRevision, PrepareError>;

    /// Atomically switches the active revision (ArcSwap pointer swap plus
    /// Tier A double-buffered map switch).
    ///
    /// # Errors
    /// Returns [`CommitError`]; the previous revision stays active.
    fn commit(&self, staged: StagedRevision) -> Result<PolicyRevision, CommitError>;

    /// Discards staged content; the active revision is unaffected.
    fn cancel(&self, staged: StagedRevision);

    /// Read-only snapshot of the active revision.
    fn active(&self) -> ActiveSnapshot;

    /// Rolls back to the most recent fully verified revision, producing an
    /// audit record.
    ///
    /// # Errors
    /// Returns [`RollbackError::NoKnownGood`] when nothing verified remains.
    fn rollback_to_last_known_good(&self) -> Result<PolicyRevision, RollbackError>;

    /// Locally retained revision history with verification state and
    /// activation times.
    fn history(&self) -> Vec<RevisionMeta>;
}
