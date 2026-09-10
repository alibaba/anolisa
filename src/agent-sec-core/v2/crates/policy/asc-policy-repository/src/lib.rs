//! Shared Binding aggregate data and atomic storage operations.
//! Callers own lifecycle, retry and observation rules; storage owns atomicity.
#![forbid(unsafe_code)]
use asc_foundation_types::{ResourceId, Revision};
use asc_policy_types::binding::BindingView;
use asc_policy_types::target::{Failure, PreparedApply, Presence, TargetBindingPlan, TargetRef};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Deployment {
    pub target: TargetRef,
    pub revision: Revision,
    pub presence: Presence,
    pub last_confirmed: Option<Presence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedApply {
    pub revision: Revision,
    pub is_update: bool,
    pub plan: TargetBindingPlan,
    pub prepared: PreparedApply,
}

/// Caller-supplied time is monotonic milliseconds within this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeState {
    pub attempts_started: u32,
    pub next_attempt_at: Option<u64>,
    pub retry_policy: Option<RetryPolicy>,
    pub last_error: Option<Failure>,
    pub prepared: Option<SavedApply>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BindingStateSnapshot {
    pub binding: BindingView,
    pub runtime: RuntimeState,
    pub deployments: Vec<Deployment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("reconciliation storage unavailable")]
    Unavailable,
    #[error("invalid reconciliation transaction")]
    Invalid,
}

/// A conditional replacement or removal of an existing aggregate, with an idempotency key.
/// The authoritative Binding is the same record used by PAP; there is no second authoritative
/// Binding record. Callers preserve the spec unless admitting a new intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingStateWrite {
    pub write_id: Uuid,
    /// `None` atomically removes the Binding and all associated runtime data.
    pub next: Option<BindingStateSnapshot>,
}
impl BindingStateWrite {
    pub fn delete() -> Self {
        Self {
            write_id: Uuid::new_v4(),
            next: None,
        }
    }
    pub fn new(next: BindingStateSnapshot) -> Self {
        Self {
            write_id: Uuid::new_v4(),
            next: Some(next),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteResult {
    Applied,
    AlreadyApplied,
    Conflict,
}

/// Operations are atomic with PAP writes, including on unwind. Reads return a
/// consistent aggregate. Errors leave storage unchanged. CAS compares the entire
/// snapshot (including runtime and deployments), not only public revision/status.
/// An exact replay of the latest committed write returns `AlreadyApplied` even
/// after intervening PAP writes. Reusing its ID with different contents is Invalid.
/// Callers serialize writes per Binding until any ambiguous result is acknowledged;
/// this bounded receipt is not an arbitrary historical deduplication log.
/// `next: None` removes the Binding, runtime, deployments and receipt atomically.
/// Removing an already absent ID returns `AlreadyApplied`; replacing an absent
/// ID returns `Conflict` and never inserts. Binding IDs are server-generated and
/// never reused, so absence acknowledges removal without a permanent tombstone.
pub trait BindingStateRepository: Send + Sync {
    /// # Errors
    /// Returns storage failure distinctly from absence.
    fn get_binding_state(
        &self,
        id: &ResourceId,
    ) -> Result<Option<BindingStateSnapshot>, StoreError>;
    /// # Errors
    /// Returns storage failure or invalid identity/write data without partial changes.
    fn compare_exchange_binding_state(
        &self,
        expected: &BindingStateSnapshot,
        write: &BindingStateWrite,
    ) -> Result<WriteResult, StoreError>;
}
