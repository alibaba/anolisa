use crate::{BindingStateSnapshot, BindingStateWrite, Failure, Observation};
use asc_foundation_types::{ResourceId, Revision};
use asc_policy_types::binding::{BindingStatus, BindingView};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExpectedBinding {
    pub id: ResourceId,
    pub revision: Revision,
    pub status: BindingStatus,
}

impl ExpectedBinding {
    pub fn from_binding(binding: &BindingView) -> Self {
        Self {
            id: binding.spec.binding_id.clone(),
            revision: binding.spec.binding_revision,
            status: binding.status,
        }
    }

    pub fn matches(&self, binding: &BindingView) -> bool {
        self.id == binding.spec.binding_id
            && self.revision == binding.spec.binding_revision
            && self.status == binding.status
    }
}

/// Atomic completion transaction, also retained in the execution slot on error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AttemptOutcome {
    pub expected: ExpectedBinding,
    pub observations: Vec<Observation>,
    pub next_status: BindingStatus,
    pub next_attempt_at: Option<u64>,
    pub error: Option<Failure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Disposition {
    Skipped,
    Superseded,
    Completed,
    RetryAt { at: u64 },
    Failed { error: Failure },
}

/// Retained before the storage call, including across post-commit unwind.
#[derive(Debug, Clone)]
pub(crate) struct PendingWrite {
    pub expected: BindingStateSnapshot,
    pub write: BindingStateWrite,
    pub matched: bool,
}
#[derive(Debug, Default)]
pub(crate) struct ExecutionSlot {
    pub(crate) removed: bool,
    pub(crate) pending: Option<AttemptOutcome>,
    pub(crate) completion: Option<PendingWrite>,
}
