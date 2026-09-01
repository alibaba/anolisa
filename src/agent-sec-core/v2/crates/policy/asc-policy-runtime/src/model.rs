use asc_foundation_types::{ResourceId, Revision};
use asc_pap::{PreparedPolicy, PreparedScope};
use serde::{Deserialize, Serialize};

/// Desired Binding state understood before any target-specific Adapter exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BindingDesiredState {
    /// The Adapter should converge this prepared Binding.
    Ready,
    /// The Adapter should remove this Binding.
    Absent,
}

/// Adapter-independent prepared Binding.
// TODO(adapter-status): project observed revision and PEP-effective phase onto the Binding after
// the Adapter receipt contract exists. Reconcile work-item identities must remain internal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedBinding {
    /// Stable Binding identity.
    pub binding_id: ResourceId,
    /// Immutable desired revision.
    pub binding_revision: Revision,
    /// Exactly one authored Policy revision.
    pub policy: PreparedPolicy,
    /// Exactly one authored Scope revision.
    pub scope: PreparedScope,
    /// Desired state.
    pub desired_state: BindingDesiredState,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreparedBindingWire {
    binding_id: ResourceId,
    binding_revision: Revision,
    policy: PreparedPolicy,
    scope: PreparedScope,
    #[serde(default, rename = "executionDomainId")]
    _legacy_execution_domain_id: Option<ResourceId>,
    desired_state: BindingDesiredState,
}

impl<'de> Deserialize<'de> for PreparedBinding {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = PreparedBindingWire::deserialize(deserializer)?;
        Ok(Self {
            binding_id: wire.binding_id,
            binding_revision: wire.binding_revision,
            policy: wire.policy,
            scope: wire.scope,
            desired_state: wire.desired_state,
        })
    }
}

/// Daemon-internal durable operation lifecycle before the Adapter boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OperationState {
    /// Durable and waiting for the worker.
    Queued,
    /// A worker owns the dispatch attempt.
    Dispatching,
    /// The Adapter acknowledged this command. This does not mean enforcement is active.
    Dispatched,
    /// A retryable dispatch failed and awaits an explicit later attempt.
    RetryWait,
    /// No production Adapter is available or a downstream precondition is absent.
    Blocked,
    /// The operation failed permanently before Adapter acceptance.
    Failed,
    /// A newer Binding revision replaced this queued operation.
    Superseded,
}

/// Daemon-internal durable reconcile work item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReconcileOperation {
    /// Daemon-allocated idempotency identity.
    pub operation_id: ResourceId,
    /// Binding being reconciled.
    pub binding_id: ResourceId,
    /// Desired Binding revision.
    pub binding_revision: Revision,
    /// Request digest used to reject idempotency-key reuse.
    pub request_digest: String,
    /// Current pre-Adapter lifecycle state.
    pub state: OperationState,
    /// Stable stage name, currently `dispatch_adapter`.
    pub stage: String,
    /// Stable safe error code.
    pub error_code: Option<String>,
}

/// Command accepted by a future target Adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdapterCommand {
    /// Durable idempotency identity.
    pub operation_id: ResourceId,
    /// Complete target-independent Binding snapshot.
    pub binding: PreparedBinding,
}

/// Minimal Adapter acknowledgement. It makes no downstream readiness claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterAccepted;
