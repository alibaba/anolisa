//! Durable approval, permit, execution, runtime binding, and run lease ledger.

use cosh_gateway_contracts::capability::{
    ApprovalDecision, BrokeredOperation, CapabilityRequest, DenialCode, ExecutionPermit,
    RuntimeExecutionFence,
};
use cosh_gateway_contracts::common::RuntimeBindingRef;
use cosh_gateway_contracts::common::{
    BoundedName, BoundedOpaque, BoundedText, Digest, IdempotencyKey, TargetRef,
};
use cosh_gateway_contracts::error::{ContractError, ErrorCategory};
use cosh_gateway_contracts::ids::{
    ActorId, ApprovalId, ExecutionId, InputRequestId, PermitId, RequestId, RunId, RuntimeBindingId,
    RuntimeInstanceId, TaskId,
};
use cosh_gateway_contracts::runtime::{
    BrokeredExecutionDelivery, BrokeredExecutionOutcome, BrokeredExecutionRef,
    BrokeredOperationResult, RuntimeInputRequest, RuntimeInputResponse, RuntimePermissionRef,
};
use cosh_gateway_contracts::task::{ExecutionOutcome, TaskEvent, TaskState, UncertaintyCode};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::task::TaskAggregate;

use super::{
    task_store::{append_internal_task_event, load_verified_projection},
    SqliteTaskStore, StoreError,
};

const MAX_RUNTIME_INPUT_REQUEST_BYTES: usize = 64 * 1024;
const MAX_RUNTIME_INPUT_RESPONSE_BYTES: usize = 64 * 1024;

/// Idempotent command metadata shared by ledger mutations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerCommand {
    /// Authenticated actor owning the idempotency namespace.
    pub actor_id: ActorId,
    /// Caller-scoped replay key.
    pub idempotency_key: IdempotencyKey,
    /// Canonical digest of the complete command.
    pub command_digest: Digest,
    /// Durable mutation timestamp in Unix milliseconds.
    pub committed_at_ms: u64,
}

/// Result of an idempotent ledger mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerOutcome<T> {
    /// A new durable mutation was applied.
    Applied(T),
    /// An identical command returned its original durable result.
    Replayed(T),
}

/// Durable approval lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    /// Waiting for the bound actor's decision.
    Pending,
    /// Approved for subsequent permit issuance.
    Approved,
    /// Explicitly denied.
    Denied,
    /// Deadline passed before a decision.
    Expired,
    /// Owning run cancelled the request.
    Cancelled,
}

/// Durable approval row with all authorization bindings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRecord {
    /// Approval identity.
    pub approval_id: ApprovalId,
    /// Capability request identity.
    pub request_id: RequestId,
    /// Actor authorized to resolve the approval.
    pub actor_id: ActorId,
    /// Owning Task.
    pub task_id: TaskId,
    /// Owning Run.
    pub run_id: RunId,
    /// Bound target.
    pub target: TargetRef,
    /// Immutable resolved target identity for COSH-brokered authority.
    pub target_identity_digest: Option<Digest>,
    /// Runtime and Run-lease fence for COSH-brokered authority.
    pub runtime_fence: Option<RuntimeExecutionFence>,
    /// Bound normalized operation digest.
    pub operation_digest: Digest,
    /// Bound complete Runtime input digest.
    pub input_digest: Digest,
    /// Exact provider callback binding, absent for legacy or COSH-brokered approvals.
    pub permission: Option<RuntimePermissionRef>,
    /// Current lifecycle state.
    pub state: ApprovalState,
    /// Optimistic revision.
    pub revision: u64,
    /// Fail-closed decision deadline.
    pub expires_at_ms: u64,
    /// Actor that made an explicit decision.
    pub decided_by_actor_id: Option<ActorId>,
    /// Creation timestamp.
    pub created_at_ms: u64,
    /// Last mutation timestamp.
    pub updated_at_ms: u64,
}

/// Requested approval resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalResolution {
    /// Apply an explicit allow-once decision.
    Decide(ApprovalDecision),
    /// Cancel because the owning Run is no longer active.
    Cancel,
}

/// Provider-native decision prepared for exactly one Runtime callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderPermissionDispatchDecision {
    /// Select the provider's one-shot allow option without issuing a COSH permit.
    AllowOnce,
    /// Select the provider's one-shot rejection option.
    Deny,
}

/// Durable provider-native resolution dispatch lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderPermissionDispatchState {
    /// Approval resolution committed, but no provider response has started.
    Prepared,
    /// Dispatch intent committed before writing to the provider transport.
    Started,
    /// The provider transport accepted the one-shot response.
    Delivered,
    /// Restart or transport failure made delivery indeterminate.
    Unknown,
}

/// Durable provider-native response bound to one exact Runtime callback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderPermissionDispatchRecord {
    /// Approval whose resolution created this dispatch.
    pub approval_id: ApprovalId,
    /// Actor authorized to resolve the approval.
    pub actor_id: ActorId,
    /// Task owning the provider callback.
    pub task_id: TaskId,
    /// Run owning the provider callback.
    pub run_id: RunId,
    /// Complete Runtime generation, Turn, tool, and request binding.
    pub permission: RuntimePermissionRef,
    /// Provider-native one-shot response.
    pub decision: ProviderPermissionDispatchDecision,
    /// Current dispatch lifecycle state.
    pub state: ProviderPermissionDispatchState,
    /// Optimistic dispatch revision.
    pub revision: u64,
    /// Creation timestamp.
    pub created_at_ms: u64,
    /// Last mutation timestamp.
    pub updated_at_ms: u64,
}

/// Durable permit lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermitState {
    /// Available for one exact execution.
    Issued,
    /// Atomically consumed when execution started.
    Consumed,
    /// Deadline passed before consumption.
    Expired,
    /// Revoked before consumption.
    Revoked,
}

/// Durable execution-permit row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermitRecord {
    /// Complete immutable permit contract.
    pub permit: ExecutionPermit,
    /// Current lifecycle state.
    pub state: PermitState,
    /// Consumption timestamp.
    pub consumed_at_ms: Option<u64>,
    /// Creation timestamp.
    pub created_at_ms: u64,
}

/// Durable execution lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    /// Permit was issued but the side effect has not started.
    Planned,
    /// Permit consumption and execution start committed atomically.
    Started,
    /// A success receipt was committed.
    Succeeded,
    /// A failure receipt was committed.
    Failed,
    /// Recovery found a started execution without a conclusive receipt.
    Uncertain,
}

/// Durable governed execution row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionRecord {
    /// Execution identity.
    pub execution_id: ExecutionId,
    /// Actor authorized by the permit.
    pub actor_id: ActorId,
    /// Owning Task.
    pub task_id: TaskId,
    /// Owning Run.
    pub run_id: RunId,
    /// Bound target.
    pub target: TargetRef,
    /// Immutable resolved target identity, absent only for pre-v6 legacy rows.
    pub target_identity_digest: Option<Digest>,
    /// Runtime and Run-lease fence, absent only for pre-v6 legacy rows.
    pub runtime_fence: Option<RuntimeExecutionFence>,
    /// Broker-specific pre-effect lifecycle, absent for provider-native rows.
    pub broker_state: Option<BrokerExecutionState>,
    /// Timestamp at which exact authority was claimed before any external effect.
    pub claimed_at_ms: Option<u64>,
    /// Required security-boundary audit proof persisted before target invocation.
    pub start_audit_proof_digest: Option<Digest>,
    /// Durable availability of a typed successful brokered result.
    pub typed_result_state: TypedExecutionResultState,
    /// Bound operation digest.
    pub operation_digest: Digest,
    /// Bound Runtime input digest.
    pub input_digest: Digest,
    /// Current lifecycle state.
    pub state: ExecutionState,
    /// Optimistic revision.
    pub revision: u64,
    /// Start timestamp.
    pub started_at_ms: Option<u64>,
    /// Terminal or uncertainty timestamp.
    pub completed_at_ms: Option<u64>,
    /// Creation timestamp.
    pub created_at_ms: u64,
    /// Last mutation timestamp.
    pub updated_at_ms: u64,
}

/// Availability of the typed result associated with an execution row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypedExecutionResultState {
    /// The execution has no successful typed result.
    NotApplicable,
    /// A validated typed result is durable in the result ledger.
    Available,
    /// A pre-v8 successful row cannot be reconstructed safely.
    LegacyUnavailable,
}

/// Exact bindings presented when consuming a permit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionClaim {
    /// Permit to consume.
    pub permit_id: PermitId,
    /// Execution authorized by the permit.
    pub execution_id: ExecutionId,
    /// Owning Task.
    pub task_id: TaskId,
    /// Owning Run.
    pub run_id: RunId,
    /// Exact target.
    pub target: TargetRef,
    /// Immutable resolved target identity.
    pub target_identity_digest: Digest,
    /// Exact Runtime and renewable Run-lease fence authorized by the permit.
    pub runtime_fence: RuntimeExecutionFence,
    /// Exact normalized operation digest.
    pub operation_digest: Digest,
    /// Exact complete Runtime input digest.
    pub input_digest: Digest,
    /// Policy revision expected by the executor.
    pub policy_revision: u64,
    /// Current coordinator lease fencing the owning Task and Run.
    pub lease: LeaseClaim,
}

/// Durable COSH-brokered lifecycle around the external-effect boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokerExecutionState {
    /// Permit exists but has not been consumed.
    Planned,
    /// Authority is consumed, while the external effect is still known not to have started.
    Claimed,
    /// Security audit proof committed and the external effect may have started.
    Started,
    /// Recovery conclusively established that a claimed effect never started.
    KnownNoEffect,
}

/// Proof returned by a security audit boundary before target invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityAuditProof {
    /// Digest of the complete durable audit record.
    pub proof_digest: Digest,
    /// Time at which the security-boundary record became durable.
    pub persisted_at_ms: u64,
}

/// Complete durable COSH-brokered request admitted before approval or execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrokeredRequestRecord {
    /// Provider-neutral capability request.
    pub request: CapabilityRequest,
    /// Typed operation that the COSH target adapter may execute.
    pub operation: BrokeredOperation,
    /// Storage-owned digest of the canonical typed operation JSON.
    pub typed_operation_digest: Digest,
    /// Immutable resolved target identity.
    pub target_identity_digest: Digest,
    /// Exact Runtime and renewable Run-lease fence.
    pub runtime_fence: RuntimeExecutionFence,
    /// Optional approval created with this request.
    pub approval_id: Option<ApprovalId>,
    /// Durable creation timestamp.
    pub created_at_ms: u64,
}

/// Runtime callback message represented by a durable brokered dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokeredRuntimeDispatchKind {
    /// Gateway accepted ownership of a request awaiting approval.
    Acknowledgement,
    /// Gateway is returning a terminal denial or execution outcome.
    Result,
}

/// Non-replayable brokered callback dispatch lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokeredRuntimeDispatchState {
    /// All prerequisites and the complete outbound payload digest are durable.
    Prepared,
    /// Dispatch intent is durable and the transport write may have happened.
    Started,
    /// The live Runtime accepted the callback message.
    Delivered,
    /// Delivery is indeterminate and must never be retried.
    Unknown,
}

/// Durable source proving a brokered callback is ready to send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum BrokeredRuntimeDispatchSource {
    /// Pending approval and WaitingApproval Task state back an acknowledgement.
    ApprovalPending {
        /// Durable pending approval.
        approval_id: ApprovalId,
    },
    /// Durable explicit denial backs a terminal denied result.
    ApprovalDenied {
        /// Durable denied approval.
        approval_id: ApprovalId,
    },
    /// A terminal, uncertain, or known-no-effect execution backs a result.
    Execution {
        /// Durable governed execution.
        execution_id: ExecutionId,
    },
}

/// Exact durable Runtime callback dispatch binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrokeredRuntimeDispatchRecord {
    /// Complete callback reference, including Runtime generation and event sequence.
    pub brokered: BrokeredExecutionRef,
    /// Authenticated Task owner.
    pub actor_id: ActorId,
    /// Owning Task inferred from the durable request.
    pub task_id: TaskId,
    /// Callback message kind.
    pub kind: BrokeredRuntimeDispatchKind,
    /// Digest of the complete acknowledgement or result payload.
    pub payload_digest: Digest,
    /// Durable fact authorizing preparation.
    pub source: BrokeredRuntimeDispatchSource,
    /// Current non-replayable dispatch state.
    pub state: BrokeredRuntimeDispatchState,
    /// Optimistic dispatch revision.
    pub revision: u64,
    /// Creation timestamp.
    pub created_at_ms: u64,
    /// Last mutation timestamp.
    pub updated_at_ms: u64,
}

/// Conclusive execution result persisted after a started side effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionCompletion {
    /// Execution to complete.
    pub execution_id: ExecutionId,
    /// Expected execution revision.
    pub expected_revision: u64,
    /// Whether the governed operation succeeded.
    pub succeeded: bool,
    /// Digest of the complete evidence receipt.
    pub receipt_digest: Digest,
    /// Optional redacted bounded detail.
    pub safe_detail: Option<BoundedText>,
    /// Typed successful result; required exactly when `succeeded` is true.
    pub typed_result: Option<BrokeredOperationResult>,
}

/// Durable typed result and all authority bindings used to validate it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrokeredExecutionResultRecord {
    /// Governed execution that produced the result.
    pub execution_id: ExecutionId,
    /// Brokered capability request owning the callback.
    pub request_id: RequestId,
    /// Authenticated owner.
    pub actor_id: ActorId,
    /// Owning Task.
    pub task_id: TaskId,
    /// Owning Run.
    pub run_id: RunId,
    /// Validated provider-neutral typed result.
    pub result: BrokeredOperationResult,
    /// Storage-owned digest of the complete typed result JSON.
    pub result_digest: Digest,
    /// Exact typed operation whose result shape was validated.
    pub operation: BrokeredOperation,
    /// Bound normalized operation digest.
    pub operation_digest: Digest,
    /// Bound complete Runtime input digest.
    pub input_digest: Digest,
    /// Immutable resolved target identity.
    pub target_identity_digest: Digest,
    /// Runtime and lease-generation authority fence.
    pub runtime_fence: RuntimeExecutionFence,
    /// Atomic completion timestamp.
    pub committed_at_ms: u64,
}

/// Durable runtime binding lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeBindingState {
    /// Runtime generation may emit events.
    Active,
    /// Runtime was closed cleanly.
    Closed,
    /// Recovery fenced a runtime whose liveness was not proven.
    Lost,
}

/// Durable fenced runtime binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeBindingRecord {
    /// Complete binding contract.
    pub binding: RuntimeBindingRef,
    /// Authenticated Task owner.
    pub actor_id: ActorId,
    /// Current binding state.
    pub state: RuntimeBindingState,
    /// Last accepted monotonic event sequence.
    pub last_sequence: u64,
    /// Creation timestamp.
    pub created_at_ms: u64,
    /// Last mutation timestamp.
    pub updated_at_ms: u64,
}

/// Durable lifecycle of one exact Runtime input request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RuntimeInputRequestState {
    /// Waiting for the authenticated actor's response.
    Pending,
    /// A typed response and digest were committed atomically.
    Resolved,
    /// The response deadline elapsed before resolution.
    Expired,
    /// Run convergence cancelled the pending request.
    Cancelled,
}

/// Durable bounded Runtime input request and its exact authority fence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RuntimeInputRequestRecord {
    /// Complete bounded request presentation.
    pub request: RuntimeInputRequest,
    /// Authenticated Task owner.
    pub actor_id: ActorId,
    /// Owning Task.
    pub task_id: TaskId,
    /// Owning Run.
    pub run_id: RunId,
    /// Runtime binding that emitted the request.
    pub binding_id: RuntimeBindingId,
    /// Exact Runtime process instance.
    pub runtime_instance_id: RuntimeInstanceId,
    /// Exact Runtime process generation.
    pub runtime_generation: u64,
    /// Monotonic Runtime event sequence consumed atomically with the request.
    pub runtime_sequence: u64,
    /// Run lease generation authoritative at request admission.
    pub lease_generation: u64,
    /// Run lease revision authoritative at request admission.
    pub lease_revision: u64,
    /// Current durable request lifecycle.
    pub state: RuntimeInputRequestState,
    /// Digest of the private typed response after resolution.
    pub response_digest: Option<Digest>,
    /// Optimistic request revision.
    pub revision: u64,
    /// Fail-closed response deadline.
    pub expires_at_ms: u64,
    /// Creation timestamp.
    pub created_at_ms: u64,
    /// Last mutation timestamp.
    pub updated_at_ms: u64,
}

/// Durable lifecycle of one private Runtime input response dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RuntimeInputDispatchState {
    /// Raw typed response is durable but no transport write has started.
    Prepared,
    /// The non-replayable transport boundary was committed.
    Started,
    /// Runtime transport acknowledged the one-shot response.
    Delivered,
    /// Delivery became indeterminate and must never be retried.
    Unknown,
}

/// Private typed input response held only in the dispatch ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RuntimeInputDispatchRecord {
    /// Exact Runtime request being resolved.
    pub request_id: InputRequestId,
    /// Authenticated Task owner.
    pub actor_id: ActorId,
    /// Owning Task.
    pub task_id: TaskId,
    /// Owning Run.
    pub run_id: RunId,
    /// Typed bounded response excluded from Task history and receipts.
    pub response: RuntimeInputResponse,
    /// Canonical digest recorded in the Task event.
    pub response_digest: Digest,
    /// Current dispatch lifecycle.
    pub state: RuntimeInputDispatchState,
    /// Optimistic dispatch revision.
    pub revision: u64,
    /// Creation timestamp.
    pub created_at_ms: u64,
    /// Last mutation timestamp.
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RuntimeInputDispatchReceipt {
    request_id: InputRequestId,
    response_digest: Digest,
    state: RuntimeInputDispatchState,
    revision: u64,
}

/// Run-lease mutation metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseCommand {
    /// Common idempotent command metadata.
    pub command: LedgerCommand,
    /// Task protected by the lease.
    pub task_id: TaskId,
    /// Run protected by the lease.
    pub run_id: RunId,
    /// Bounded coordinator instance identity.
    pub lease_owner: BoundedOpaque,
    /// Requested lease deadline.
    pub expires_at_ms: u64,
}

/// Exact fencing claim required to release a Run lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseClaim {
    /// Task protected by the lease.
    pub task_id: TaskId,
    /// Run protected by the lease.
    pub run_id: RunId,
    /// Coordinator instance holding the lease.
    pub lease_owner: BoundedOpaque,
    /// Expected fencing generation.
    pub generation: u64,
    /// Expected optimistic revision.
    pub revision: u64,
}

/// Durable fenced lease for one Run coordinator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLeaseRecord {
    /// Owning Task.
    pub task_id: TaskId,
    /// Protected Run.
    pub run_id: RunId,
    /// Authenticated Task owner.
    pub actor_id: ActorId,
    /// Coordinator instance holding the lease.
    pub lease_owner: BoundedOpaque,
    /// Monotonic fencing generation.
    pub generation: u64,
    /// Optimistic mutation revision.
    pub revision: u64,
    /// Lease deadline.
    pub expires_at_ms: u64,
    /// Last mutation timestamp.
    pub updated_at_ms: u64,
}

/// Counts of fail-closed transitions applied during restart recovery.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Pending approvals expired by their deadline.
    pub approvals_expired: u64,
    /// Unexpired pending approvals cancelled because stdio cannot reattach.
    pub approvals_cancelled: u64,
    /// Prepared or started provider responses made non-replayable by restart.
    pub permission_dispatches_unknown: u64,
    /// Started brokered callbacks made permanently non-replayable by restart.
    pub brokered_dispatches_unknown: u64,
    /// Pending input requests cancelled while their Tasks were suspended.
    pub runtime_input_requests_cancelled: u64,
    /// Prepared or started input responses made permanently non-replayable.
    pub runtime_input_dispatches_unknown: u64,
    /// Issued permits expired by their deadline.
    pub permits_expired: u64,
    /// Started executions marked uncertain.
    pub executions_uncertain: u64,
    /// Claimed executions conclusively recovered before any external effect.
    pub executions_known_no_effect: u64,
    /// Active runtime bindings fenced as lost.
    pub runtime_bindings_lost: u64,
}

/// Counts of brokered execution transitions recovered for one fenced Run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BrokeredExecutionRecoveryReport {
    /// Claimed executions proven not to have crossed the effect boundary.
    pub executions_known_no_effect: u64,
    /// Started executions requiring reconciliation.
    pub executions_uncertain: u64,
}

impl SqliteTaskStore {
    /// Loads one durable bounded Runtime input request.
    pub(crate) fn load_runtime_input_request(
        &self,
        request_id: &InputRequestId,
    ) -> Result<RuntimeInputRequestRecord, StoreError> {
        load_runtime_input_request(self.connection(), request_id)
    }

    /// Loads one private typed response dispatch.
    pub(crate) fn load_runtime_input_dispatch(
        &self,
        request_id: &InputRequestId,
    ) -> Result<RuntimeInputDispatchRecord, StoreError> {
        load_runtime_input_dispatch(self.connection(), request_id)
    }

    /// Atomically consumes one Runtime sequence and records its pending input Task fact.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_runtime_input_request(
        &mut self,
        command: &LedgerCommand,
        request: &RuntimeInputRequest,
        expires_at_ms: u64,
        binding_id: &RuntimeBindingId,
        runtime_instance_id: &RuntimeInstanceId,
        runtime_generation: u64,
        sequence: u64,
        lease: &LeaseClaim,
    ) -> Result<LedgerOutcome<RuntimeInputRequestRecord>, StoreError> {
        validate_command(command)?;
        integer(expires_at_ms, "runtime input deadline")?;
        integer(runtime_generation, "runtime input generation")?;
        integer(sequence, "runtime input sequence")?;
        validate_json_bound(
            request,
            MAX_RUNTIME_INPUT_REQUEST_BYTES,
            "runtime input request",
        )?;
        if expires_at_ms <= command.committed_at_ms || request.run_id() != &lease.run_id {
            return Err(conflict("runtime input request Run or deadline is invalid"));
        }
        let transaction = immediate(self)?;
        if let Some(replayed) = replay::<RuntimeInputRequestRecord>(
            &transaction,
            command,
            "record_runtime_input_request",
        )? {
            if replayed.request != *request
                || replayed.binding_id != *binding_id
                || replayed.runtime_instance_id != *runtime_instance_id
                || replayed.runtime_generation != runtime_generation
                || replayed.runtime_sequence != sequence
                || replayed.expires_at_ms != expires_at_ms
            {
                return Err(StoreError::IdempotencyConflict);
            }
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        let binding = load_runtime_binding(&transaction, binding_id)?;
        if binding.actor_id != command.actor_id
            || binding.binding.task_id != lease.task_id
            || binding.binding.run_id != lease.run_id
            || binding.binding.runtime_instance_id != *runtime_instance_id
            || binding.binding.runtime_generation != runtime_generation
            || binding.state != RuntimeBindingState::Active
        {
            return Err(conflict("runtime input request binding is stale"));
        }
        require_current_lease(
            &transaction,
            lease,
            &command.actor_id,
            command.committed_at_ms,
        )?;
        require_not_before(
            command.committed_at_ms,
            binding.updated_at_ms,
            "runtime input request",
        )?;
        let expected_sequence = next_integer(binding.last_sequence, "runtime input sequence")?;
        if sequence != expected_sequence {
            return Err(conflict("runtime input request sequence is stale"));
        }
        let changed = transaction.execute(
            "UPDATE runtime_bindings SET last_sequence=?2, updated_at_ms=?3
             WHERE binding_id=?1 AND state='active' AND runtime_instance_id=?4
               AND runtime_generation=?5 AND last_sequence=?6",
            params![
                binding_id.as_str(),
                integer(sequence, "runtime input sequence")?,
                integer(command.committed_at_ms, "runtime input timestamp")?,
                runtime_instance_id.as_str(),
                integer(runtime_generation, "runtime input generation")?,
                integer(binding.last_sequence, "runtime input prior sequence")?,
            ],
        )?;
        if changed != 1 {
            return Err(conflict("runtime input request lost its sequence fence"));
        }
        append_internal_task_event(
            &transaction,
            &lease.task_id,
            &command.actor_id,
            command.committed_at_ms,
            TaskEvent::InputRequested {
                request: request.clone(),
            },
            None,
        )?;
        let record = RuntimeInputRequestRecord {
            request: request.clone(),
            actor_id: command.actor_id.clone(),
            task_id: lease.task_id.clone(),
            run_id: lease.run_id.clone(),
            binding_id: binding_id.clone(),
            runtime_instance_id: runtime_instance_id.clone(),
            runtime_generation,
            runtime_sequence: sequence,
            lease_generation: lease.generation,
            lease_revision: lease.revision,
            state: RuntimeInputRequestState::Pending,
            response_digest: None,
            revision: 1,
            expires_at_ms,
            created_at_ms: command.committed_at_ms,
            updated_at_ms: command.committed_at_ms,
        };
        insert_runtime_input_request(&transaction, &record)?;
        insert_receipt(
            &transaction,
            command,
            "record_runtime_input_request",
            &record,
        )?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(record))
    }

    /// Atomically records a private typed response, digest-only Task fact, and Prepared dispatch.
    pub(crate) fn resolve_runtime_input(
        &mut self,
        command: &LedgerCommand,
        request_id: &InputRequestId,
        expected_task_revision: u64,
        response: &RuntimeInputResponse,
    ) -> Result<LedgerOutcome<RuntimeInputDispatchRecord>, StoreError> {
        validate_command(command)?;
        integer(
            expected_task_revision,
            "runtime input expected Task revision",
        )?;
        validate_json_bound(
            response,
            MAX_RUNTIME_INPUT_RESPONSE_BYTES,
            "runtime input response",
        )?;
        let response_digest = runtime_input_response_digest(response)?;
        let transaction = immediate(self)?;
        if let Some(replayed) = replay_runtime_input_dispatch(
            &transaction,
            command,
            "resolve_runtime_input",
            request_id,
            &response_digest,
        )? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        let mut request = load_runtime_input_request(&transaction, request_id)?;
        if request.actor_id != command.actor_id
            || request.state != RuntimeInputRequestState::Pending
            || request.expires_at_ms <= command.committed_at_ms
        {
            return Err(conflict(
                "runtime input request actor, state, or deadline is stale",
            ));
        }
        validate_runtime_input_response(&request.request, response)?;
        let task_revision = transaction.query_row(
            "SELECT revision FROM tasks WHERE task_id=?1 AND owner_actor_id=?2",
            params![request.task_id.as_str(), command.actor_id.as_str()],
            |row| row.get::<_, i64>(0),
        )?;
        if unsigned(task_revision, "runtime input Task revision")? != expected_task_revision {
            return Err(conflict("runtime input Task revision is stale"));
        }
        append_internal_task_event(
            &transaction,
            &request.task_id,
            &command.actor_id,
            command.committed_at_ms,
            TaskEvent::InputSubmitted {
                request_id: request_id.clone(),
                run_id: request.request.run_id().clone(),
                response_digest: response_digest.clone(),
            },
            None,
        )?;
        request.state = RuntimeInputRequestState::Resolved;
        request.response_digest = Some(response_digest.clone());
        request.revision = next_integer(request.revision, "runtime input request revision")?;
        request.updated_at_ms = command.committed_at_ms;
        let changed = transaction.execute(
            "UPDATE runtime_input_requests
             SET state='resolved', response_digest=?2, revision=?3, updated_at_ms=?4
             WHERE request_id=?1 AND state='pending' AND revision=1",
            params![
                request_id.as_str(),
                response_digest.as_str(),
                integer(request.revision, "runtime input request revision")?,
                integer(
                    command.committed_at_ms,
                    "runtime input resolution timestamp"
                )?,
            ],
        )?;
        if changed != 1 {
            return Err(conflict("runtime input request lost its pending revision"));
        }
        let dispatch = RuntimeInputDispatchRecord {
            request_id: request_id.clone(),
            actor_id: command.actor_id.clone(),
            task_id: request.task_id.clone(),
            run_id: request.run_id.clone(),
            response: response.clone(),
            response_digest,
            state: RuntimeInputDispatchState::Prepared,
            revision: 1,
            created_at_ms: command.committed_at_ms,
            updated_at_ms: command.committed_at_ms,
        };
        insert_runtime_input_dispatch(&transaction, &dispatch)?;
        insert_runtime_input_dispatch_receipt(
            &transaction,
            command,
            "resolve_runtime_input",
            &dispatch,
        )?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(dispatch))
    }

    /// Expires one pending input request at or after its durable deadline.
    pub(crate) fn expire_runtime_input_request(
        &mut self,
        command: &LedgerCommand,
        request_id: &InputRequestId,
        expected_revision: u64,
    ) -> Result<LedgerOutcome<RuntimeInputRequestRecord>, StoreError> {
        transition_pending_runtime_input_request(
            self,
            command,
            request_id,
            expected_revision,
            RuntimeInputRequestState::Expired,
            true,
            "expire_runtime_input_request",
        )
    }

    /// Cancels one pending request while converging its Task to Suspended.
    pub(crate) fn cancel_runtime_input_request(
        &mut self,
        command: &LedgerCommand,
        request_id: &InputRequestId,
        expected_revision: u64,
    ) -> Result<LedgerOutcome<RuntimeInputRequestRecord>, StoreError> {
        transition_pending_runtime_input_request(
            self,
            command,
            request_id,
            expected_revision,
            RuntimeInputRequestState::Cancelled,
            false,
            "cancel_runtime_input_request",
        )
    }

    /// Commits the non-replayable boundary before writing one input response.
    pub(crate) fn start_runtime_input_dispatch(
        &mut self,
        command: &LedgerCommand,
        request_id: &InputRequestId,
        response_digest: &Digest,
        expected_revision: u64,
        lease: &LeaseClaim,
    ) -> Result<LedgerOutcome<RuntimeInputDispatchRecord>, StoreError> {
        transition_runtime_input_dispatch(
            self,
            command,
            request_id,
            response_digest,
            expected_revision,
            lease,
            RuntimeInputDispatchState::Prepared,
            RuntimeInputDispatchState::Started,
            "start_runtime_input_dispatch",
        )
    }

    /// Records that Runtime transport accepted the one-shot input response.
    pub(crate) fn complete_runtime_input_dispatch(
        &mut self,
        command: &LedgerCommand,
        request_id: &InputRequestId,
        response_digest: &Digest,
        expected_revision: u64,
        lease: &LeaseClaim,
    ) -> Result<LedgerOutcome<RuntimeInputDispatchRecord>, StoreError> {
        transition_runtime_input_dispatch(
            self,
            command,
            request_id,
            response_digest,
            expected_revision,
            lease,
            RuntimeInputDispatchState::Started,
            RuntimeInputDispatchState::Delivered,
            "complete_runtime_input_dispatch",
        )
    }

    /// Marks a started input response permanently indeterminate.
    pub(crate) fn mark_runtime_input_dispatch_unknown(
        &mut self,
        command: &LedgerCommand,
        request_id: &InputRequestId,
        response_digest: &Digest,
        expected_revision: u64,
        lease: &LeaseClaim,
    ) -> Result<LedgerOutcome<RuntimeInputDispatchRecord>, StoreError> {
        mark_runtime_input_dispatch_unknown_atomic(
            self,
            command,
            request_id,
            response_digest,
            expected_revision,
            lease,
            "mark_runtime_input_dispatch_unknown",
        )
    }

    /// Atomically converges one abandoned input request or dispatch after Run takeover.
    pub fn recover_runtime_input_dispatch_for_run(
        &mut self,
        command: &LedgerCommand,
        run_id: &RunId,
        takeover_lease: &LeaseClaim,
    ) -> Result<LedgerOutcome<u64>, StoreError> {
        validate_command(command)?;
        let transaction = immediate(self)?;
        if let Some(replayed) = replay::<u64>(
            &transaction,
            command,
            "recover_runtime_input_dispatch_for_run",
        )? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        let dispatches = load_recoverable_runtime_input_dispatches(&transaction, run_id)?;
        let requests = load_pending_runtime_input_requests(&transaction, run_id)?;
        if dispatches.len() + requests.len() > 1 {
            return Err(corrupt(
                "one Run has multiple recoverable Runtime input records",
            ));
        }
        let changed = if let Some(dispatch) = dispatches.first() {
            if dispatch.actor_id != command.actor_id {
                return Err(conflict(
                    "runtime input recovery actor does not own the dispatch",
                ));
            }
            let request = load_runtime_input_request(&transaction, &dispatch.request_id)?;
            if takeover_lease.task_id != dispatch.task_id
                || takeover_lease.run_id != dispatch.run_id
                || takeover_lease.generation <= request.lease_generation
            {
                return Err(conflict(
                    "runtime input recovery requires a newer takeover lease",
                ));
            }
            require_current_lease(
                &transaction,
                takeover_lease,
                &command.actor_id,
                command.committed_at_ms,
            )?;
            let changed = transaction.execute(
                "UPDATE runtime_input_dispatches
                 SET state='unknown', revision=revision+1, updated_at_ms=?2
                 WHERE request_id=?1 AND state IN ('prepared', 'started') AND revision=?3",
                params![
                    dispatch.request_id.as_str(),
                    integer(command.committed_at_ms, "runtime input recovery timestamp")?,
                    integer(dispatch.revision, "runtime input dispatch revision")?,
                ],
            )?;
            if changed != 1 {
                return Err(conflict("runtime input recovery lost its started revision"));
            }
            if runtime_input_recovery_requires_suspension(
                &transaction,
                &dispatch.task_id,
                &dispatch.actor_id,
                &dispatch.run_id,
                TaskState::Running,
            )? {
                append_internal_task_event(
                    &transaction,
                    &dispatch.task_id,
                    &dispatch.actor_id,
                    command.committed_at_ms,
                    TaskEvent::RunSuspended {
                        run_id: dispatch.run_id.clone(),
                        reason: cosh_gateway_contracts::task::SuspensionCode::OperatorRequired,
                    },
                    None,
                )?;
            }
            1u64
        } else if let Some(request) = requests.first() {
            if request.actor_id != command.actor_id
                || takeover_lease.task_id != request.task_id
                || takeover_lease.run_id != request.run_id
                || takeover_lease.generation <= request.lease_generation
            {
                return Err(conflict(
                    "runtime input recovery requires an owned newer takeover lease",
                ));
            }
            require_current_lease(
                &transaction,
                takeover_lease,
                &command.actor_id,
                command.committed_at_ms,
            )?;
            let changed = transaction.execute(
                "UPDATE runtime_input_requests
                 SET state='cancelled', revision=revision+1, updated_at_ms=?2
                 WHERE request_id=?1 AND state='pending' AND revision=?3",
                params![
                    request.request.request_id().as_str(),
                    integer(command.committed_at_ms, "runtime input recovery timestamp")?,
                    integer(request.revision, "runtime input request revision")?,
                ],
            )?;
            if changed != 1 {
                return Err(conflict("runtime input recovery lost its pending revision"));
            }
            if runtime_input_recovery_requires_suspension(
                &transaction,
                &request.task_id,
                &request.actor_id,
                &request.run_id,
                TaskState::WaitingInput,
            )? {
                append_internal_task_event(
                    &transaction,
                    &request.task_id,
                    &request.actor_id,
                    command.committed_at_ms,
                    TaskEvent::RunSuspended {
                        run_id: request.run_id.clone(),
                        reason: cosh_gateway_contracts::task::SuspensionCode::OperatorRequired,
                    },
                    None,
                )?;
            }
            1u64
        } else {
            0u64
        };
        insert_receipt(
            &transaction,
            command,
            "recover_runtime_input_dispatch_for_run",
            &changed,
        )?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(changed))
    }

    /// Loads and validates one typed COSH-brokered request.
    pub fn load_brokered_request(
        &self,
        request_id: &RequestId,
    ) -> Result<BrokeredRequestRecord, StoreError> {
        load_brokered_request(self.connection(), request_id)
    }

    /// Loads one durable brokered callback without requiring a live Runtime lease.
    ///
    /// This recovery-oriented read validates the stored row's typed encoding and
    /// primary-key columns only. Callers must still compare actor, Task, Run,
    /// callback reference, payload digest, and source with their request context
    /// before deciding whether a lost API response can be replayed safely.
    pub fn load_brokered_runtime_dispatch_record(
        &self,
        request_id: &RequestId,
        kind: BrokeredRuntimeDispatchKind,
    ) -> Result<BrokeredRuntimeDispatchRecord, StoreError> {
        load_brokered_runtime_dispatch(self.connection(), request_id, kind)
    }

    /// Loads an exact brokered callback only while its Runtime and lease remain authoritative.
    pub fn load_brokered_runtime_dispatch(
        &self,
        actor_id: &ActorId,
        kind: BrokeredRuntimeDispatchKind,
        brokered: &BrokeredExecutionRef,
        payload_digest: &Digest,
        lease: &LeaseClaim,
        now_ms: u64,
    ) -> Result<BrokeredRuntimeDispatchRecord, StoreError> {
        let record = load_brokered_runtime_dispatch(self.connection(), &brokered.request_id, kind)?;
        require_brokered_dispatch_context(
            self.connection(),
            actor_id,
            &record,
            brokered,
            payload_digest,
            lease,
            now_ms,
        )?;
        Ok(record)
    }

    /// Prepares an acknowledgement after request, approval, and WaitingApproval are durable.
    pub fn prepare_brokered_acknowledgement_dispatch(
        &mut self,
        command: &LedgerCommand,
        approval_id: &ApprovalId,
        brokered: &BrokeredExecutionRef,
        payload_digest: &Digest,
        lease: &LeaseClaim,
    ) -> Result<LedgerOutcome<BrokeredRuntimeDispatchRecord>, StoreError> {
        prepare_brokered_runtime_dispatch(
            self,
            command,
            BrokeredRuntimeDispatchKind::Acknowledgement,
            BrokeredRuntimeDispatchSource::ApprovalPending {
                approval_id: approval_id.clone(),
            },
            brokered,
            Some(payload_digest),
            None,
            lease,
        )
    }

    /// Prepares a typed denial only after the bound approval denial is durable.
    ///
    /// Canonicalizes the complete delivery inside the write transaction and
    /// rejects request identity or denial-classification substitution.
    pub fn prepare_brokered_denied_result_dispatch(
        &mut self,
        command: &LedgerCommand,
        approval_id: &ApprovalId,
        brokered: &BrokeredExecutionRef,
        delivery: &BrokeredExecutionDelivery,
        lease: &LeaseClaim,
    ) -> Result<LedgerOutcome<BrokeredRuntimeDispatchRecord>, StoreError> {
        prepare_brokered_runtime_dispatch(
            self,
            command,
            BrokeredRuntimeDispatchKind::Result,
            BrokeredRuntimeDispatchSource::ApprovalDenied {
                approval_id: approval_id.clone(),
            },
            brokered,
            None,
            Some(delivery),
            lease,
        )
    }

    /// Prepares a typed result only after its exact durable execution outcome.
    ///
    /// Canonicalizes the complete delivery inside the write transaction. A
    /// successful delivery must equal the result persisted by
    /// [`Self::complete_execution`]; failure and uncertainty variants must
    /// match the durable execution lifecycle.
    pub fn prepare_brokered_execution_result_dispatch(
        &mut self,
        command: &LedgerCommand,
        execution_id: &ExecutionId,
        brokered: &BrokeredExecutionRef,
        delivery: &BrokeredExecutionDelivery,
        lease: &LeaseClaim,
    ) -> Result<LedgerOutcome<BrokeredRuntimeDispatchRecord>, StoreError> {
        prepare_brokered_runtime_dispatch(
            self,
            command,
            BrokeredRuntimeDispatchKind::Result,
            BrokeredRuntimeDispatchSource::Execution {
                execution_id: execution_id.clone(),
            },
            brokered,
            None,
            Some(delivery),
            lease,
        )
    }

    /// Commits the non-replayable boundary before writing a brokered callback.
    ///
    /// Callers may write to the Runtime only for [`LedgerOutcome::Applied`].
    /// `Replayed`, `Started`, and `Unknown` must never cause another write.
    pub fn start_brokered_runtime_dispatch(
        &mut self,
        command: &LedgerCommand,
        kind: BrokeredRuntimeDispatchKind,
        brokered: &BrokeredExecutionRef,
        payload_digest: &Digest,
        expected_revision: u64,
        lease: &LeaseClaim,
    ) -> Result<LedgerOutcome<BrokeredRuntimeDispatchRecord>, StoreError> {
        transition_brokered_runtime_dispatch(
            self,
            command,
            kind,
            brokered,
            payload_digest,
            expected_revision,
            lease,
            BrokeredRuntimeDispatchState::Prepared,
            BrokeredRuntimeDispatchState::Started,
            "start_brokered_runtime_dispatch",
        )
    }

    /// Records that the live Runtime accepted a previously started callback.
    pub fn complete_brokered_runtime_dispatch(
        &mut self,
        command: &LedgerCommand,
        kind: BrokeredRuntimeDispatchKind,
        brokered: &BrokeredExecutionRef,
        payload_digest: &Digest,
        expected_revision: u64,
        lease: &LeaseClaim,
    ) -> Result<LedgerOutcome<BrokeredRuntimeDispatchRecord>, StoreError> {
        transition_brokered_runtime_dispatch(
            self,
            command,
            kind,
            brokered,
            payload_digest,
            expected_revision,
            lease,
            BrokeredRuntimeDispatchState::Started,
            BrokeredRuntimeDispatchState::Delivered,
            "complete_brokered_runtime_dispatch",
        )
    }

    /// Marks a started callback indeterminate after a live transport failure.
    pub fn mark_brokered_runtime_dispatch_unknown(
        &mut self,
        command: &LedgerCommand,
        kind: BrokeredRuntimeDispatchKind,
        brokered: &BrokeredExecutionRef,
        payload_digest: &Digest,
        expected_revision: u64,
        lease: &LeaseClaim,
    ) -> Result<LedgerOutcome<BrokeredRuntimeDispatchRecord>, StoreError> {
        transition_brokered_runtime_dispatch(
            self,
            command,
            kind,
            brokered,
            payload_digest,
            expected_revision,
            lease,
            BrokeredRuntimeDispatchState::Started,
            BrokeredRuntimeDispatchState::Unknown,
            "mark_brokered_runtime_dispatch_unknown",
        )
    }

    /// Records a typed brokered request that policy may permit without approval.
    pub fn create_brokered_request(
        &mut self,
        command: &LedgerCommand,
        request: &CapabilityRequest,
        operation: &BrokeredOperation,
        target_identity_digest: &Digest,
        runtime_fence: &RuntimeExecutionFence,
    ) -> Result<LedgerOutcome<BrokeredRequestRecord>, StoreError> {
        validate_command(command)?;
        if request.actor.actor_id != command.actor_id
            || request.expires_at_ms <= command.committed_at_ms
        {
            return Err(conflict("brokered request actor or deadline is invalid"));
        }
        let transaction = immediate(self)?;
        if let Some(replayed) = replay(&transaction, command, "create_brokered_request")? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        require_task_run(
            &transaction,
            &request.task_id,
            &request.run_id,
            &command.actor_id,
        )?;
        require_runtime_fence(
            &transaction,
            &command.actor_id,
            &request.task_id,
            &request.run_id,
            runtime_fence,
            command.committed_at_ms,
        )?;
        let record = BrokeredRequestRecord {
            request: request.clone(),
            operation: operation.clone(),
            typed_operation_digest: brokered_operation_digest(operation)?,
            target_identity_digest: target_identity_digest.clone(),
            runtime_fence: runtime_fence.clone(),
            approval_id: None,
            created_at_ms: command.committed_at_ms,
        };
        insert_brokered_request(&transaction, &record)?;
        insert_receipt(&transaction, command, "create_brokered_request", &record)?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(record))
    }

    /// Atomically records a typed COSH-brokered request, approval, Task fact, and Outbox intent.
    pub fn create_brokered_approval(
        &mut self,
        command: &LedgerCommand,
        request: &CapabilityRequest,
        approval: &cosh_gateway_contracts::capability::ApprovalRequest,
        operation: &BrokeredOperation,
        record: &ApprovalRecord,
    ) -> Result<LedgerOutcome<ApprovalRecord>, StoreError> {
        validate_command(command)?;
        integer(record.expires_at_ms, "approval deadline")?;
        let target_identity_digest = record
            .target_identity_digest
            .as_ref()
            .ok_or_else(|| conflict("brokered approval is missing target identity"))?;
        let runtime_fence = record
            .runtime_fence
            .as_ref()
            .ok_or_else(|| conflict("brokered approval is missing Runtime fence"))?;
        if request.request_id != record.request_id
            || request.actor.actor_id != record.actor_id
            || request.task_id != record.task_id
            || request.run_id != record.run_id
            || request.target != record.target
            || request.operation_digest != record.operation_digest
            || request.input_digest != record.input_digest
            || approval.approval_id != record.approval_id
            || approval.request_id != request.request_id
        {
            return Err(conflict("brokered request, approval, and authority differ"));
        }
        let transaction = immediate(self)?;
        if let Some(replayed) = replay(&transaction, command, "create_brokered_approval")? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        require_task_run(
            &transaction,
            &record.task_id,
            &record.run_id,
            &command.actor_id,
        )?;
        validate_initial_approval(command, record)?;
        require_runtime_fence(
            &transaction,
            &command.actor_id,
            &record.task_id,
            &record.run_id,
            runtime_fence,
            command.committed_at_ms,
        )?;
        insert_approval(&transaction, record, command.committed_at_ms)?;
        insert_brokered_request(
            &transaction,
            &BrokeredRequestRecord {
                request: request.clone(),
                operation: operation.clone(),
                typed_operation_digest: brokered_operation_digest(operation)?,
                target_identity_digest: target_identity_digest.clone(),
                runtime_fence: runtime_fence.clone(),
                approval_id: Some(approval.approval_id.clone()),
                created_at_ms: command.committed_at_ms,
            },
        )?;
        let delivery_kind = BoundedName::new("brokered_approval_request")
            .map_err(|_| corrupt("static brokered approval route is invalid"))?;
        append_internal_task_event(
            &transaction,
            &record.task_id,
            &record.actor_id,
            command.committed_at_ms,
            TaskEvent::ApprovalRequested {
                approval: approval.clone(),
            },
            Some((delivery_kind, serde_json::to_value(approval)?)),
        )?;
        insert_receipt(&transaction, command, "create_brokered_approval", record)?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(record.clone()))
    }

    /// Loads one durable approval record.
    pub fn load_approval_record(
        &self,
        approval_id: &ApprovalId,
    ) -> Result<ApprovalRecord, StoreError> {
        load_approval(self.connection(), approval_id)
    }

    /// Loads one durable provider-native permission dispatch.
    pub fn load_provider_permission_dispatch_record(
        &self,
        approval_id: &ApprovalId,
    ) -> Result<ProviderPermissionDispatchRecord, StoreError> {
        load_provider_permission_dispatch(self.connection(), approval_id)
    }

    /// Loads one durable permit record.
    pub fn load_permit_record(&self, permit_id: &PermitId) -> Result<PermitRecord, StoreError> {
        load_permit(self.connection(), permit_id)
    }

    /// Loads one durable execution record.
    pub fn load_execution_record(
        &self,
        execution_id: &ExecutionId,
    ) -> Result<ExecutionRecord, StoreError> {
        load_execution(self.connection(), execution_id)
    }

    /// Loads and fully validates one typed successful brokered result.
    pub fn load_brokered_execution_result(
        &self,
        execution_id: &ExecutionId,
    ) -> Result<BrokeredExecutionResultRecord, StoreError> {
        load_brokered_execution_result(self.connection(), execution_id)
    }

    /// Loads one durable runtime binding record.
    pub fn load_runtime_binding_record(
        &self,
        binding_id: &RuntimeBindingId,
    ) -> Result<RuntimeBindingRecord, StoreError> {
        load_runtime_binding(self.connection(), binding_id)
    }

    /// Loads the current durable lease for a Run.
    pub fn load_run_lease(&self, run_id: &RunId) -> Result<RunLeaseRecord, StoreError> {
        load_run_lease_optional(self.connection(), run_id)?
            .ok_or_else(|| not_found("run lease", run_id.as_str()))
    }

    /// Loads one expired lease whose delivered Runtime cannot be reattached.
    ///
    /// Only active Task states with a delivered `runtime_start` Outbox fact are
    /// eligible. A suspended Run remains recoverable until its lease is
    /// explicitly released, which is encoded by equal deadline and update
    /// timestamps.
    pub fn load_expired_active_lease(
        &self,
        now_ms: u64,
    ) -> Result<Option<RunLeaseRecord>, StoreError> {
        let now = integer(now_ms, "recovery timestamp")?;
        let row = self
            .connection()
            .query_row(
                "SELECT r.run_id, r.task_id, r.actor_id, r.lease_owner, r.generation,
                 r.revision, r.expires_at_ms, r.updated_at_ms
                 FROM run_leases r
                 JOIN tasks t ON t.task_id=r.task_id
                 WHERE r.expires_at_ms <= ?1
                   AND t.state IN ('running', 'waiting_approval', 'waiting_input', 'suspended')
                   AND json_extract(t.snapshot_json, '$.active_run_id') = r.run_id
                   AND (t.state != 'suspended' OR r.expires_at_ms > r.updated_at_ms)
                   AND EXISTS (
                       SELECT 1 FROM outbox o
                       WHERE o.task_id=r.task_id
                         AND o.delivery_kind='runtime_start'
                         AND o.state='delivered'
                         AND json_extract(o.payload_json, '$.run_id') = r.run_id
                   )
                 ORDER BY r.expires_at_ms, r.run_id
                 LIMIT 1",
                params![now],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                    ))
                },
            )
            .optional()?;
        row.map(|row| {
            Ok(RunLeaseRecord {
                run_id: parse_id(&row.0)?,
                task_id: parse_id(&row.1)?,
                actor_id: parse_id(&row.2)?,
                lease_owner: BoundedOpaque::new(row.3)
                    .map_err(|_| corrupt("invalid lease owner"))?,
                generation: unsigned(row.4, "lease generation")?,
                revision: unsigned(row.5, "lease revision")?,
                expires_at_ms: unsigned(row.6, "lease deadline")?,
                updated_at_ms: unsigned(row.7, "lease update")?,
            })
        })
        .transpose()
    }

    /// Fences active Runtime bindings for one unrecoverable Run generation.
    pub fn mark_runtime_bindings_lost_for_run(
        &mut self,
        run_id: &RunId,
        now_ms: u64,
    ) -> Result<u64, StoreError> {
        let changed = self.connection_mut().execute(
            "UPDATE runtime_bindings SET state='lost', updated_at_ms=?2
             WHERE run_id=?1 AND state='active'",
            params![
                run_id.as_str(),
                integer(now_ms, "runtime recovery timestamp")?
            ],
        )?;
        Ok(changed as u64)
    }

    /// Cancels pending approvals only for one unrecoverable Run.
    pub fn cancel_pending_approvals_for_run(
        &mut self,
        run_id: &RunId,
        now_ms: u64,
    ) -> Result<u64, StoreError> {
        let changed = self.connection_mut().execute(
            "UPDATE approvals SET state='cancelled', revision=revision+1, updated_at_ms=?2
             WHERE run_id=?1 AND state='pending'",
            params![
                run_id.as_str(),
                integer(now_ms, "approval recovery timestamp")?
            ],
        )?;
        Ok(changed as u64)
    }

    /// Marks non-terminal provider responses unknown only for one lost Run.
    pub fn mark_provider_dispatches_unknown_for_run(
        &mut self,
        run_id: &RunId,
        now_ms: u64,
    ) -> Result<u64, StoreError> {
        let changed = self.connection_mut().execute(
            "UPDATE provider_permission_dispatches
             SET state='unknown', revision=revision+1, updated_at_ms=?2
             WHERE run_id=?1 AND state IN ('prepared', 'started')",
            params![
                run_id.as_str(),
                integer(now_ms, "dispatch recovery timestamp")?
            ],
        )?;
        Ok(changed as u64)
    }

    /// Marks started brokered callbacks unknown only for one lost Run.
    ///
    /// Prepared callbacks have not crossed the transport boundary and remain
    /// distinguishable for operator-driven convergence. Started callbacks can
    /// never be retried because their write outcome is indeterminate.
    pub fn mark_brokered_dispatches_unknown_for_run(
        &mut self,
        run_id: &RunId,
        now_ms: u64,
    ) -> Result<u64, StoreError> {
        let changed = self.connection_mut().execute(
            "UPDATE brokered_runtime_dispatches
             SET state='unknown', revision=revision+1, updated_at_ms=?2
             WHERE run_id=?1 AND state='started'",
            params![
                run_id.as_str(),
                integer(now_ms, "brokered dispatch recovery timestamp")?
            ],
        )?;
        Ok(changed as u64)
    }

    /// Creates a pending approval bound to an actor, Task, Run, target, and digests.
    pub fn create_approval(
        &mut self,
        command: &LedgerCommand,
        approval: &ApprovalRecord,
    ) -> Result<LedgerOutcome<ApprovalRecord>, StoreError> {
        validate_command(command)?;
        integer(approval.expires_at_ms, "approval deadline")?;
        if approval.permission.is_some() {
            return Err(conflict(
                "provider approvals require a fenced Runtime and Run lease",
            ));
        }
        let transaction = immediate(self)?;
        if let Some(replayed) = replay(&transaction, command, "create_approval")? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        require_task_run(
            &transaction,
            &approval.task_id,
            &approval.run_id,
            &command.actor_id,
        )?;
        validate_initial_approval(command, approval)?;
        insert_approval(&transaction, approval, command.committed_at_ms)?;
        insert_receipt(&transaction, command, "create_approval", approval)?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(approval.clone()))
    }

    /// Creates a provider-native approval only for a live fenced callback.
    pub fn create_provider_approval(
        &mut self,
        command: &LedgerCommand,
        approval: &ApprovalRecord,
        lease: &LeaseClaim,
    ) -> Result<LedgerOutcome<ApprovalRecord>, StoreError> {
        validate_command(command)?;
        integer(approval.expires_at_ms, "approval deadline")?;
        let permission = approval
            .permission
            .as_ref()
            .ok_or_else(|| conflict("provider approval is missing its Runtime permission"))?;
        let transaction = immediate(self)?;
        if let Some(replayed) = replay(&transaction, command, "create_provider_approval")? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        validate_initial_approval(command, approval)?;
        require_provider_permission_context(
            &transaction,
            command,
            approval,
            permission,
            lease,
            TaskState::Running,
        )?;
        insert_approval(&transaction, approval, command.committed_at_ms)?;
        insert_receipt(&transaction, command, "create_provider_approval", approval)?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(approval.clone()))
    }

    /// Resolves a pending approval with revision, actor, and deadline checks.
    pub fn resolve_approval(
        &mut self,
        command: &LedgerCommand,
        approval_id: &ApprovalId,
        expected_revision: u64,
        resolution: ApprovalResolution,
    ) -> Result<LedgerOutcome<ApprovalRecord>, StoreError> {
        validate_command(command)?;
        integer(expected_revision, "approval expected revision")?;
        let transaction = immediate(self)?;
        if let Some(replayed) = replay(&transaction, command, "resolve_approval")? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        let mut record = load_approval(&transaction, approval_id)?;
        require_task_owner(&transaction, &record.task_id, &command.actor_id)?;
        if record.actor_id != command.actor_id || record.revision != expected_revision {
            return Err(conflict("approval actor or revision does not match"));
        }
        require_not_before(
            command.committed_at_ms,
            record.updated_at_ms,
            "approval resolution",
        )?;
        if record.state != ApprovalState::Pending {
            return Err(conflict("approval is no longer pending"));
        }
        let (state, decided_by) = if command.committed_at_ms >= record.expires_at_ms {
            (ApprovalState::Expired, None)
        } else {
            match resolution {
                ApprovalResolution::Decide(ApprovalDecision::Approve) => {
                    (ApprovalState::Approved, Some(command.actor_id.clone()))
                }
                ApprovalResolution::Decide(ApprovalDecision::Deny) => {
                    (ApprovalState::Denied, Some(command.actor_id.clone()))
                }
                ApprovalResolution::Cancel => (ApprovalState::Cancelled, None),
            }
        };
        let next_revision = next_integer(record.revision, "approval revision")?;
        record.state = state;
        record.revision = next_revision;
        record.decided_by_actor_id = decided_by;
        record.updated_at_ms = command.committed_at_ms;
        let changed = transaction.execute(
            "UPDATE approvals SET state = ?2, revision = ?3, decided_by_actor_id = ?4,
             updated_at_ms = ?5 WHERE approval_id = ?1 AND revision = ?6 AND state = 'pending'",
            params![
                approval_id.as_str(),
                state_name(state)?,
                integer(record.revision, "approval revision")?,
                record.decided_by_actor_id.as_ref().map(ActorId::as_str),
                integer(command.committed_at_ms, "approval timestamp")?,
                integer(expected_revision, "approval expected revision")?
            ],
        )?;
        if changed != 1 {
            return Err(conflict("approval resolution lost its pending revision"));
        }
        if record.permission.is_none()
            && record.target_identity_digest.is_some()
            && record.runtime_fence.is_some()
        {
            if let ApprovalResolution::Decide(decision) = resolution {
                append_internal_task_event(
                    &transaction,
                    &record.task_id,
                    &record.actor_id,
                    command.committed_at_ms,
                    TaskEvent::ApprovalResolved {
                        approval_id: record.approval_id.clone(),
                        decision,
                    },
                    None,
                )?;
            }
        }
        insert_receipt(&transaction, command, "resolve_approval", &record)?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(record))
    }

    /// Resolves a provider-native approval and prepares its exact response atomically.
    ///
    /// This path never issues an execution permit. The returned dispatch is
    /// observation-only authority and remains fenced to the active Runtime
    /// generation, Turn, tool, request, Run lease, and authenticated actor.
    pub fn resolve_provider_permission(
        &mut self,
        command: &LedgerCommand,
        approval_id: &ApprovalId,
        expected_revision: u64,
        resolution: ApprovalResolution,
        expected_permission: &RuntimePermissionRef,
        lease: &LeaseClaim,
    ) -> Result<LedgerOutcome<ProviderPermissionDispatchRecord>, StoreError> {
        validate_command(command)?;
        integer(expected_revision, "approval expected revision")?;
        let transaction = immediate(self)?;
        if let Some(replayed) = replay(&transaction, command, "resolve_provider_permission")? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }

        let mut approval = load_approval(&transaction, approval_id)?;
        require_provider_permission_context(
            &transaction,
            command,
            &approval,
            expected_permission,
            lease,
            TaskState::WaitingApproval,
        )?;
        if approval.revision != expected_revision || approval.state != ApprovalState::Pending {
            return Err(conflict(
                "provider approval is no longer at its pending revision",
            ));
        }
        require_not_before(
            command.committed_at_ms,
            approval.updated_at_ms,
            "provider approval resolution",
        )?;
        if command.committed_at_ms >= approval.expires_at_ms {
            return Err(conflict(
                "provider approval deadline elapsed; use fenced expiry",
            ));
        }

        let (approval_state, decided_by, dispatch_decision) = match resolution {
            ApprovalResolution::Decide(ApprovalDecision::Approve) => (
                ApprovalState::Approved,
                Some(command.actor_id.clone()),
                ProviderPermissionDispatchDecision::AllowOnce,
            ),
            ApprovalResolution::Decide(ApprovalDecision::Deny) => (
                ApprovalState::Denied,
                Some(command.actor_id.clone()),
                ProviderPermissionDispatchDecision::Deny,
            ),
            ApprovalResolution::Cancel => (
                ApprovalState::Cancelled,
                None,
                ProviderPermissionDispatchDecision::Deny,
            ),
        };
        approval.state = approval_state;
        approval.revision = next_integer(approval.revision, "approval revision")?;
        approval.decided_by_actor_id = decided_by;
        approval.updated_at_ms = command.committed_at_ms;
        let changed = transaction.execute(
            "UPDATE approvals SET state=?2, revision=?3, decided_by_actor_id=?4,
             updated_at_ms=?5 WHERE approval_id=?1 AND state='pending' AND revision=?6",
            params![
                approval_id.as_str(),
                state_name(approval_state)?,
                integer(approval.revision, "approval revision")?,
                approval.decided_by_actor_id.as_ref().map(ActorId::as_str),
                integer(command.committed_at_ms, "approval timestamp")?,
                integer(expected_revision, "approval expected revision")?,
            ],
        )?;
        if changed != 1 {
            return Err(conflict(
                "provider approval resolution lost its pending revision",
            ));
        }

        let dispatch = ProviderPermissionDispatchRecord {
            approval_id: approval_id.clone(),
            actor_id: approval.actor_id.clone(),
            task_id: approval.task_id.clone(),
            run_id: approval.run_id.clone(),
            permission: expected_permission.clone(),
            decision: dispatch_decision,
            state: ProviderPermissionDispatchState::Prepared,
            revision: 1,
            created_at_ms: command.committed_at_ms,
            updated_at_ms: command.committed_at_ms,
        };
        transaction.execute(
            "INSERT INTO provider_permission_dispatches(
                 approval_id, actor_id, task_id, run_id, permission_ref_json,
                 decision, state, revision, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'prepared', 1, ?7, ?7)",
            params![
                approval_id.as_str(),
                dispatch.actor_id.as_str(),
                dispatch.task_id.as_str(),
                dispatch.run_id.as_str(),
                serde_json::to_string(&dispatch.permission)?,
                state_name(dispatch.decision)?,
                integer(command.committed_at_ms, "dispatch timestamp")?,
            ],
        )?;
        insert_receipt(
            &transaction,
            command,
            "resolve_provider_permission",
            &dispatch,
        )?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(dispatch))
    }

    /// Expires a pending provider-native approval without dispatching a response.
    ///
    /// The callback must still belong to the current live Runtime generation;
    /// losing that fence requires Run recovery instead of rewriting history as
    /// a normal deadline expiry.
    pub fn expire_provider_approval(
        &mut self,
        command: &LedgerCommand,
        approval_id: &ApprovalId,
        expected_permission: &RuntimePermissionRef,
        lease: &LeaseClaim,
    ) -> Result<LedgerOutcome<ApprovalRecord>, StoreError> {
        validate_command(command)?;
        let transaction = immediate(self)?;
        if let Some(replayed) = replay(&transaction, command, "expire_provider_approval")? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        let mut approval = load_approval(&transaction, approval_id)?;
        require_provider_permission_context(
            &transaction,
            command,
            &approval,
            expected_permission,
            lease,
            TaskState::WaitingApproval,
        )?;
        if approval.state != ApprovalState::Pending {
            return Err(conflict("provider approval is no longer pending"));
        }
        require_not_before(
            command.committed_at_ms,
            approval.updated_at_ms,
            "provider approval expiry",
        )?;
        if command.committed_at_ms < approval.expires_at_ms {
            return Err(conflict("provider approval deadline has not elapsed"));
        }
        let prior_revision = approval.revision;
        approval.state = ApprovalState::Expired;
        approval.revision = next_integer(prior_revision, "approval revision")?;
        approval.decided_by_actor_id = None;
        approval.updated_at_ms = command.committed_at_ms;
        let changed = transaction.execute(
            "UPDATE approvals SET state='expired', revision=?2, decided_by_actor_id=NULL,
             updated_at_ms=?3 WHERE approval_id=?1 AND state='pending' AND revision=?4",
            params![
                approval_id.as_str(),
                integer(approval.revision, "approval revision")?,
                integer(command.committed_at_ms, "approval expiry timestamp")?,
                integer(prior_revision, "approval prior revision")?,
            ],
        )?;
        if changed != 1 {
            return Err(conflict(
                "provider approval expiry lost its pending revision",
            ));
        }
        insert_receipt(&transaction, command, "expire_provider_approval", &approval)?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(approval))
    }

    /// Commits the non-replayable boundary before writing a provider response.
    ///
    /// Callers must write to the provider only for [`LedgerOutcome::Applied`].
    /// A replayed result proves that dispatch may already have crossed the
    /// provider boundary and must never trigger another write.
    pub fn start_provider_permission_dispatch(
        &mut self,
        command: &LedgerCommand,
        approval_id: &ApprovalId,
        expected_revision: u64,
        lease: &LeaseClaim,
    ) -> Result<LedgerOutcome<ProviderPermissionDispatchRecord>, StoreError> {
        validate_command(command)?;
        let transaction = immediate(self)?;
        if let Some(replayed) = replay(&transaction, command, "start_provider_permission")? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        let approval = load_approval(&transaction, approval_id)?;
        let mut dispatch = load_provider_permission_dispatch(&transaction, approval_id)?;
        require_provider_permission_context(
            &transaction,
            command,
            &approval,
            &dispatch.permission,
            lease,
            TaskState::Running,
        )?;
        if dispatch.state != ProviderPermissionDispatchState::Prepared
            || dispatch.revision != expected_revision
        {
            return Err(conflict(
                "provider permission dispatch is not prepared at the expected revision",
            ));
        }
        require_not_before(
            command.committed_at_ms,
            dispatch.updated_at_ms,
            "provider permission dispatch start",
        )?;
        dispatch.state = ProviderPermissionDispatchState::Started;
        dispatch.revision = next_integer(dispatch.revision, "dispatch revision")?;
        dispatch.updated_at_ms = command.committed_at_ms;
        update_provider_permission_dispatch(
            &transaction,
            &dispatch,
            expected_revision,
            "prepared",
        )?;
        insert_receipt(
            &transaction,
            command,
            "start_provider_permission",
            &dispatch,
        )?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(dispatch))
    }

    /// Records that the provider transport accepted a previously started response.
    pub fn complete_provider_permission_dispatch(
        &mut self,
        command: &LedgerCommand,
        approval_id: &ApprovalId,
        expected_revision: u64,
    ) -> Result<LedgerOutcome<ProviderPermissionDispatchRecord>, StoreError> {
        validate_command(command)?;
        let transaction = immediate(self)?;
        if let Some(replayed) = replay(&transaction, command, "complete_provider_permission")? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        let mut dispatch = load_provider_permission_dispatch(&transaction, approval_id)?;
        require_task_owner(&transaction, &dispatch.task_id, &command.actor_id)?;
        if dispatch.actor_id != command.actor_id
            || dispatch.state != ProviderPermissionDispatchState::Started
            || dispatch.revision != expected_revision
        {
            return Err(conflict(
                "provider permission dispatch is not started at the expected revision",
            ));
        }
        require_not_before(
            command.committed_at_ms,
            dispatch.updated_at_ms,
            "provider permission dispatch completion",
        )?;
        dispatch.state = ProviderPermissionDispatchState::Delivered;
        dispatch.revision = next_integer(dispatch.revision, "dispatch revision")?;
        dispatch.updated_at_ms = command.committed_at_ms;
        update_provider_permission_dispatch(&transaction, &dispatch, expected_revision, "started")?;
        insert_receipt(
            &transaction,
            command,
            "complete_provider_permission",
            &dispatch,
        )?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(dispatch))
    }

    /// Persists a single-use permit and its planned execution atomically.
    pub fn issue_permit(
        &mut self,
        command: &LedgerCommand,
        permit: &ExecutionPermit,
    ) -> Result<LedgerOutcome<PermitRecord>, StoreError> {
        validate_command(command)?;
        integer(permit.policy_revision, "policy revision")?;
        integer(permit.valid_until_ms, "permit deadline")?;
        let transaction = immediate(self)?;
        if let Some(replayed) = replay(&transaction, command, "issue_permit")? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        require_task_run(
            &transaction,
            &permit.task_id,
            &permit.run_id,
            &command.actor_id,
        )?;
        require_runtime_fence(
            &transaction,
            &command.actor_id,
            &permit.task_id,
            &permit.run_id,
            &permit.runtime_fence,
            command.committed_at_ms,
        )?;
        if permit.actor_id != command.actor_id
            || !permit.single_use
            || permit.valid_until_ms <= command.committed_at_ms
        {
            return Err(conflict(
                "permit actor, single-use flag, or deadline is invalid",
            ));
        }
        let brokered = load_brokered_request(&transaction, &permit.request_id)?;
        if brokered.request.actor.actor_id != permit.actor_id
            || brokered.request.task_id != permit.task_id
            || brokered.request.run_id != permit.run_id
            || brokered.request.target != permit.target
            || brokered.request.operation_digest != permit.operation_digest
            || brokered.request.input_digest != permit.input_digest
            || brokered.target_identity_digest != permit.target_identity_digest
            || brokered.runtime_fence != permit.runtime_fence
            || brokered.approval_id != permit.approval_id
        {
            return Err(conflict(
                "typed brokered request does not exactly bind the permit",
            ));
        }
        if let Some(approval_id) = &permit.approval_id {
            let approval = load_approval(&transaction, approval_id)?;
            if approval.state != ApprovalState::Approved
                || approval.request_id != permit.request_id
                || approval.actor_id != permit.actor_id
                || approval.task_id != permit.task_id
                || approval.run_id != permit.run_id
                || approval.target != permit.target
                || approval.target_identity_digest.as_ref() != Some(&permit.target_identity_digest)
                || approval.runtime_fence.as_ref() != Some(&permit.runtime_fence)
                || approval.operation_digest != permit.operation_digest
                || approval.input_digest != permit.input_digest
                || approval.expires_at_ms <= command.committed_at_ms
                || permit.valid_until_ms > approval.expires_at_ms
                || command.committed_at_ms < approval.updated_at_ms
            {
                return Err(conflict(
                    "approved request does not exactly bind the permit",
                ));
            }
        }
        let active_brokered: bool = transaction.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM executions
                 WHERE task_id=?1 AND run_id=?2
                   AND broker_state IN ('planned', 'claimed', 'started')
                   AND state IN ('planned', 'started')
             )",
            params![permit.task_id.as_str(), permit.run_id.as_str()],
            |row| row.get(0),
        )?;
        if active_brokered {
            return Err(conflict(
                "only one brokered execution may cross the effect boundary per Run",
            ));
        }
        let target_json = serde_json::to_string(&permit.target)?;
        let now = integer(command.committed_at_ms, "permit timestamp")?;
        transaction.execute(
            "INSERT INTO executions(execution_id, actor_id, task_id, run_id, target_json,
             operation_digest, input_digest, state, revision, created_at_ms, updated_at_ms,
             target_identity_digest, runtime_fence_json, broker_state, typed_result_state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'planned', 1, ?8, ?8, ?9, ?10, 'planned',
                     'not_applicable')",
            params![
                permit.execution_id.as_str(),
                permit.actor_id.as_str(),
                permit.task_id.as_str(),
                permit.run_id.as_str(),
                target_json,
                permit.operation_digest.as_str(),
                permit.input_digest.as_str(),
                now,
                permit.target_identity_digest.as_str(),
                serde_json::to_string(&permit.runtime_fence)?,
            ],
        )?;
        transaction.execute(
            "INSERT INTO permits(permit_id, request_id, approval_id, actor_id, task_id, run_id,
             execution_id, target_json, operation_digest, input_digest, policy_revision, state,
             single_use, valid_until_ms, created_at_ms, target_identity_digest, runtime_fence_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'issued', 1, ?12, ?13,
                     ?14, ?15)",
            params![
                permit.permit_id.as_str(),
                permit.request_id.as_str(),
                permit.approval_id.as_ref().map(ApprovalId::as_str),
                permit.actor_id.as_str(),
                permit.task_id.as_str(),
                permit.run_id.as_str(),
                permit.execution_id.as_str(),
                serde_json::to_string(&permit.target)?,
                permit.operation_digest.as_str(),
                permit.input_digest.as_str(),
                integer(permit.policy_revision, "policy revision")?,
                integer(permit.valid_until_ms, "permit deadline")?,
                now,
                permit.target_identity_digest.as_str(),
                serde_json::to_string(&permit.runtime_fence)?
            ],
        )?;
        let record = PermitRecord {
            permit: permit.clone(),
            state: PermitState::Issued,
            consumed_at_ms: None,
            created_at_ms: command.committed_at_ms,
        };
        append_internal_task_event(
            &transaction,
            &permit.task_id,
            &permit.actor_id,
            command.committed_at_ms,
            TaskEvent::ExecutionPlanned {
                execution_id: permit.execution_id.clone(),
                permit_id: permit.permit_id.clone(),
            },
            None,
        )?;
        insert_receipt(&transaction, command, "issue_permit", &record)?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(record))
    }

    /// Revokes an issued permit before execution starts.
    pub fn revoke_permit(
        &mut self,
        command: &LedgerCommand,
        permit_id: &PermitId,
    ) -> Result<LedgerOutcome<PermitRecord>, StoreError> {
        validate_command(command)?;
        let transaction = immediate(self)?;
        if let Some(replayed) = replay(&transaction, command, "revoke_permit")? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        let mut record = load_permit(&transaction, permit_id)?;
        require_task_owner(&transaction, &record.permit.task_id, &command.actor_id)?;
        require_not_before(
            command.committed_at_ms,
            record.created_at_ms,
            "permit revocation",
        )?;
        if record.permit.actor_id != command.actor_id || record.state != PermitState::Issued {
            return Err(conflict("only the bound actor may revoke an issued permit"));
        }
        let changed = transaction.execute(
            "UPDATE permits SET state='revoked' WHERE permit_id=?1 AND state='issued'",
            params![permit_id.as_str()],
        )?;
        if changed != 1 {
            return Err(conflict(
                "permit revocation lost its issued-state precondition",
            ));
        }
        record.state = PermitState::Revoked;
        insert_receipt(&transaction, command, "revoke_permit", &record)?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(record))
    }

    /// Consumes one exact permit and commits a known-no-effect claimed boundary.
    pub fn claim_execution(
        &mut self,
        command: &LedgerCommand,
        claim: &ExecutionClaim,
    ) -> Result<LedgerOutcome<ExecutionRecord>, StoreError> {
        validate_command(command)?;
        let transaction = immediate(self)?;
        if let Some(replayed) = replay(&transaction, command, "claim_execution")? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        require_task_run(
            &transaction,
            &claim.task_id,
            &claim.run_id,
            &command.actor_id,
        )?;
        if claim.lease.task_id != claim.task_id || claim.lease.run_id != claim.run_id {
            return Err(conflict(
                "execution lease does not bind the claimed Task and Run",
            ));
        }
        require_current_lease(
            &transaction,
            &claim.lease,
            &command.actor_id,
            command.committed_at_ms,
        )?;
        integer(claim.policy_revision, "execution policy revision")?;
        let permit = load_permit(&transaction, &claim.permit_id)?;
        if permit.state != PermitState::Issued
            || permit.permit.actor_id != command.actor_id
            || permit.permit.execution_id != claim.execution_id
            || permit.permit.task_id != claim.task_id
            || permit.permit.run_id != claim.run_id
            || permit.permit.target != claim.target
            || permit.permit.target_identity_digest != claim.target_identity_digest
            || permit.permit.runtime_fence != claim.runtime_fence
            || permit.permit.operation_digest != claim.operation_digest
            || permit.permit.input_digest != claim.input_digest
            || permit.permit.policy_revision != claim.policy_revision
        {
            return Err(conflict(
                "execution claim does not exactly match an issued permit",
            ));
        }
        if claim.runtime_fence.lease_generation != claim.lease.generation {
            return Err(conflict(
                "execution Runtime fence does not match the current Run lease generation",
            ));
        }
        require_runtime_fence(
            &transaction,
            &command.actor_id,
            &claim.task_id,
            &claim.run_id,
            &claim.runtime_fence,
            command.committed_at_ms,
        )?;
        if command.committed_at_ms >= permit.permit.valid_until_ms {
            return Err(conflict("permit expired before execution start"));
        }
        require_not_before(
            command.committed_at_ms,
            permit.created_at_ms,
            "execution claim",
        )?;
        let now = integer(command.committed_at_ms, "execution claim timestamp")?;
        let changed = transaction.execute(
            "UPDATE permits SET state = 'consumed', consumed_at_ms = ?2
             WHERE permit_id = ?1 AND state = 'issued' AND consumed_at_ms IS NULL",
            params![claim.permit_id.as_str(), now],
        )?;
        let claimed = transaction.execute(
            "UPDATE executions SET broker_state = 'claimed', revision = 2, claimed_at_ms = ?2,
             updated_at_ms = ?2 WHERE execution_id = ?1 AND state = 'planned'
             AND broker_state = 'planned' AND revision = 1",
            params![claim.execution_id.as_str(), now],
        )?;
        if changed != 1 || claimed != 1 {
            return Err(conflict(
                "permit consumption or execution claim lost its precondition",
            ));
        }
        let record = load_execution(&transaction, &claim.execution_id)?;
        insert_receipt(&transaction, command, "claim_execution", &record)?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(record))
    }

    /// Persists a security-boundary audit proof before exposing a claimed effect target.
    pub fn start_claimed_execution(
        &mut self,
        command: &LedgerCommand,
        execution_id: &ExecutionId,
        expected_revision: u64,
        proof: &SecurityAuditProof,
    ) -> Result<LedgerOutcome<ExecutionRecord>, StoreError> {
        validate_command(command)?;
        integer(expected_revision, "claimed execution revision")?;
        integer(proof.persisted_at_ms, "security audit proof timestamp")?;
        if proof.persisted_at_ms > command.committed_at_ms {
            return Err(conflict("security audit proof timestamp is in the future"));
        }
        let transaction = immediate(self)?;
        if let Some(replayed) = replay(&transaction, command, "start_claimed_execution")? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        let record = load_execution(&transaction, execution_id)?;
        require_task_run(
            &transaction,
            &record.task_id,
            &record.run_id,
            &command.actor_id,
        )?;
        let fence = record
            .runtime_fence
            .as_ref()
            .ok_or_else(|| corrupt("claimed execution is missing its Runtime fence"))?;
        require_runtime_fence(
            &transaction,
            &command.actor_id,
            &record.task_id,
            &record.run_id,
            fence,
            command.committed_at_ms,
        )?;
        if record.actor_id != command.actor_id
            || record.state != ExecutionState::Planned
            || record.broker_state != Some(BrokerExecutionState::Claimed)
            || record.revision != expected_revision
        {
            return Err(conflict(
                "execution is not claimed at the expected revision",
            ));
        }
        require_not_before(
            command.committed_at_ms,
            record.updated_at_ms,
            "execution start",
        )?;
        transaction.execute(
            "INSERT INTO security_audit_proofs(
                 execution_id, proof_digest, durability, persisted_at_ms)
             VALUES (?1, ?2, 'security_boundary', ?3)",
            params![
                execution_id.as_str(),
                proof.proof_digest.as_str(),
                integer(proof.persisted_at_ms, "security audit proof timestamp")?,
            ],
        )?;
        let next_revision = next_integer(record.revision, "execution revision")?;
        let now = integer(command.committed_at_ms, "execution start timestamp")?;
        let changed = transaction.execute(
            "UPDATE executions SET state='started', broker_state='started', revision=?2,
                 started_at_ms=?3, start_audit_proof_digest=?4, updated_at_ms=?3
             WHERE execution_id=?1 AND state='planned' AND broker_state='claimed'
                 AND revision=?5",
            params![
                execution_id.as_str(),
                integer(next_revision, "execution revision")?,
                now,
                proof.proof_digest.as_str(),
                integer(expected_revision, "claimed execution revision")?,
            ],
        )?;
        if changed != 1 {
            return Err(conflict(
                "execution start lost its claimed-state precondition",
            ));
        }
        let started = load_execution(&transaction, execution_id)?;
        insert_receipt(&transaction, command, "start_claimed_execution", &started)?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(started))
    }

    /// Commits a conclusive receipt for a started execution.
    pub fn complete_execution(
        &mut self,
        command: &LedgerCommand,
        completion: &ExecutionCompletion,
    ) -> Result<LedgerOutcome<ExecutionRecord>, StoreError> {
        validate_command(command)?;
        integer(completion.expected_revision, "execution expected revision")?;
        if completion.succeeded != completion.typed_result.is_some() {
            return Err(conflict(
                "a successful execution requires exactly one typed result",
            ));
        }
        let transaction = immediate(self)?;
        if let Some(replayed) =
            replay::<ExecutionRecord>(&transaction, command, "complete_execution")?
        {
            let expected_state = if completion.succeeded {
                ExecutionState::Succeeded
            } else {
                ExecutionState::Failed
            };
            let receipt = transaction
                .query_row(
                    "SELECT receipt_digest, safe_detail FROM execution_receipts
                     WHERE execution_id=?1",
                    params![completion.execution_id.as_str()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .optional()?;
            let result_matches = match completion.typed_result.as_ref() {
                Some(expected) => {
                    load_brokered_execution_result(&transaction, &completion.execution_id)?.result
                        == *expected
                }
                None => replayed.typed_result_state == TypedExecutionResultState::NotApplicable,
            };
            if replayed.execution_id != completion.execution_id
                || replayed.state != expected_state
                || receipt
                    != Some((
                        completion.receipt_digest.as_str().to_owned(),
                        completion
                            .safe_detail
                            .as_ref()
                            .map(|detail| detail.as_str().to_owned()),
                    ))
                || !result_matches
            {
                return Err(StoreError::IdempotencyConflict);
            }
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        let mut record = load_execution(&transaction, &completion.execution_id)?;
        require_task_owner(&transaction, &record.task_id, &command.actor_id)?;
        if record.actor_id != command.actor_id
            || record.state != ExecutionState::Started
            || record.broker_state != Some(BrokerExecutionState::Started)
            || record.start_audit_proof_digest.is_none()
            || record.revision != completion.expected_revision
        {
            return Err(conflict(
                "execution actor, state, or revision does not match",
            ));
        }
        require_not_before(
            command.committed_at_ms,
            record.updated_at_ms,
            "execution completion",
        )?;
        let typed_result = completion
            .typed_result
            .as_ref()
            .map(|result| validate_completion_result(&transaction, &record, result, command))
            .transpose()?;
        let next_revision = next_integer(record.revision, "execution revision")?;
        record.state = if completion.succeeded {
            ExecutionState::Succeeded
        } else {
            ExecutionState::Failed
        };
        record.revision = next_revision;
        record.typed_result_state = if completion.succeeded {
            TypedExecutionResultState::Available
        } else {
            TypedExecutionResultState::NotApplicable
        };
        record.completed_at_ms = Some(command.committed_at_ms);
        record.updated_at_ms = command.committed_at_ms;
        let state = state_name(record.state)?;
        let now = integer(command.committed_at_ms, "execution completion timestamp")?;
        let changed = transaction.execute(
            "UPDATE executions SET state = ?2, revision = ?3, completed_at_ms = ?4,
             updated_at_ms = ?4, typed_result_state = ?6
             WHERE execution_id = ?1 AND state = 'started' AND revision = ?5",
            params![
                completion.execution_id.as_str(),
                state,
                integer(record.revision, "execution revision")?,
                now,
                integer(completion.expected_revision, "execution expected revision")?,
                state_name(record.typed_result_state)?,
            ],
        )?;
        if changed != 1 {
            return Err(conflict("execution completion lost its started revision"));
        }
        transaction.execute(
            "INSERT INTO execution_receipts(execution_id, state, receipt_digest, safe_detail,
             committed_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                completion.execution_id.as_str(),
                state,
                completion.receipt_digest.as_str(),
                completion.safe_detail.as_ref().map(BoundedText::as_str),
                now
            ],
        )?;
        if let Some(result) = typed_result {
            insert_brokered_execution_result(&transaction, &result)?;
        }
        let outcome = if completion.succeeded {
            ExecutionOutcome::Succeeded {
                evidence_ref: Some(
                    BoundedOpaque::new(completion.receipt_digest.as_str())
                        .map_err(|_| corrupt("receipt digest cannot form an evidence reference"))?,
                ),
            }
        } else {
            ExecutionOutcome::Failed {
                error: ContractError::new(
                    "brokered_execution_failed",
                    ErrorCategory::Internal,
                    false,
                    completion
                        .safe_detail
                        .as_ref()
                        .map_or("Brokered execution failed", BoundedText::as_str),
                )
                .map_err(|_| corrupt("static brokered execution error is invalid"))?,
            }
        };
        append_internal_task_event(
            &transaction,
            &record.task_id,
            &record.actor_id,
            command.committed_at_ms,
            TaskEvent::ExecutionResultRecorded {
                execution_id: record.execution_id.clone(),
                outcome,
            },
            None,
        )?;
        insert_receipt(&transaction, command, "complete_execution", &record)?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(record))
    }

    /// Concludes a claimed execution when the pre-effect audit barrier fails.
    ///
    /// This transition is valid only before any start proof exists. It records
    /// a failed Task result in the same transaction, proving the external target
    /// never received control.
    pub fn mark_claimed_execution_known_no_effect(
        &mut self,
        command: &LedgerCommand,
        execution_id: &ExecutionId,
        expected_revision: u64,
        safe_detail: &BoundedText,
        lease: &LeaseClaim,
    ) -> Result<LedgerOutcome<ExecutionRecord>, StoreError> {
        validate_command(command)?;
        integer(expected_revision, "execution expected revision")?;
        let transaction = immediate(self)?;
        if let Some(replayed) = replay::<ExecutionRecord>(
            &transaction,
            command,
            "mark_claimed_execution_known_no_effect",
        )? {
            require_execution_runtime_context(&transaction, command, &replayed, lease)?;
            if replayed.execution_id != *execution_id
                || replayed.state != ExecutionState::Planned
                || replayed.broker_state != Some(BrokerExecutionState::KnownNoEffect)
            {
                return Err(StoreError::IdempotencyConflict);
            }
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        let mut record = load_execution(&transaction, execution_id)?;
        require_execution_runtime_context(&transaction, command, &record, lease)?;
        if record.state != ExecutionState::Planned
            || record.broker_state != Some(BrokerExecutionState::Claimed)
            || record.claimed_at_ms.is_none()
            || record.start_audit_proof_digest.is_some()
            || record.revision != expected_revision
        {
            return Err(conflict(
                "execution is not a proof-free claim at the expected revision",
            ));
        }
        require_not_before(
            command.committed_at_ms,
            record.updated_at_ms,
            "known-no-effect execution completion",
        )?;
        record.broker_state = Some(BrokerExecutionState::KnownNoEffect);
        record.revision = next_integer(record.revision, "execution revision")?;
        record.updated_at_ms = command.committed_at_ms;
        let changed = transaction.execute(
            "UPDATE executions SET broker_state='known_no_effect', revision=?2,
                 updated_at_ms=?3
             WHERE execution_id=?1 AND state='planned' AND broker_state='claimed'
                 AND start_audit_proof_digest IS NULL AND revision=?4",
            params![
                execution_id.as_str(),
                integer(record.revision, "execution revision")?,
                integer(command.committed_at_ms, "known-no-effect timestamp")?,
                integer(expected_revision, "execution expected revision")?,
            ],
        )?;
        if changed != 1 {
            return Err(conflict(
                "known-no-effect transition lost its claimed revision",
            ));
        }
        append_internal_task_event(
            &transaction,
            &record.task_id,
            &record.actor_id,
            command.committed_at_ms,
            TaskEvent::ExecutionResultRecorded {
                execution_id: record.execution_id.clone(),
                outcome: ExecutionOutcome::Failed {
                    error: ContractError::new(
                        "security_audit_failed_before_effect",
                        ErrorCategory::Storage,
                        false,
                        safe_detail.as_str(),
                    )
                    .map_err(|_| conflict("audit failure detail is not a valid Task error"))?,
                },
            },
            None,
        )?;
        insert_receipt(
            &transaction,
            command,
            "mark_claimed_execution_known_no_effect",
            &record,
        )?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(record))
    }

    /// Marks a live target response indeterminate without waiting for restart recovery.
    pub fn mark_execution_uncertain(
        &mut self,
        command: &LedgerCommand,
        execution_id: &ExecutionId,
        expected_revision: u64,
        _safe_detail: &BoundedText,
        lease: &LeaseClaim,
    ) -> Result<LedgerOutcome<ExecutionRecord>, StoreError> {
        validate_command(command)?;
        integer(expected_revision, "execution expected revision")?;
        let transaction = immediate(self)?;
        if let Some(replayed) =
            replay::<ExecutionRecord>(&transaction, command, "mark_execution_uncertain")?
        {
            require_execution_runtime_context(&transaction, command, &replayed, lease)?;
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        let mut record = load_execution(&transaction, execution_id)?;
        require_execution_runtime_context(&transaction, command, &record, lease)?;
        if record.actor_id != command.actor_id
            || record.state != ExecutionState::Started
            || record.broker_state != Some(BrokerExecutionState::Started)
            || record.start_audit_proof_digest.is_none()
            || record.revision != expected_revision
        {
            return Err(conflict(
                "uncertain execution actor, authority, state, or revision does not match",
            ));
        }
        require_not_before(
            command.committed_at_ms,
            record.updated_at_ms,
            "execution uncertainty",
        )?;
        record.state = ExecutionState::Uncertain;
        record.revision = next_integer(record.revision, "execution revision")?;
        record.completed_at_ms = Some(command.committed_at_ms);
        record.updated_at_ms = command.committed_at_ms;
        let changed = transaction.execute(
            "UPDATE executions SET state='uncertain', revision=?2, completed_at_ms=?3,
             updated_at_ms=?3 WHERE execution_id=?1 AND state='started'
             AND broker_state='started' AND revision=?4",
            params![
                execution_id.as_str(),
                integer(record.revision, "execution revision")?,
                integer(command.committed_at_ms, "execution uncertainty timestamp")?,
                integer(expected_revision, "execution expected revision")?,
            ],
        )?;
        if changed != 1 {
            return Err(conflict("execution uncertainty lost its started revision"));
        }
        append_internal_task_event(
            &transaction,
            &record.task_id,
            &record.actor_id,
            command.committed_at_ms,
            TaskEvent::ExecutionUncertain {
                execution_id: record.execution_id.clone(),
                reason: UncertaintyCode::TransportLost,
            },
            None,
        )?;
        insert_receipt(&transaction, command, "mark_execution_uncertain", &record)?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(record))
    }

    /// Persists a new runtime generation and fences older active bindings for the Run.
    pub fn bind_runtime(
        &mut self,
        command: &LedgerCommand,
        binding: &RuntimeBindingRef,
        lease: &LeaseClaim,
    ) -> Result<LedgerOutcome<RuntimeBindingRecord>, StoreError> {
        validate_command(command)?;
        integer(binding.runtime_generation, "runtime generation")?;
        let transaction = immediate(self)?;
        if let Some(replayed) = replay(&transaction, command, "bind_runtime")? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        require_task_run(
            &transaction,
            &binding.task_id,
            &binding.run_id,
            &command.actor_id,
        )?;
        if lease.task_id != binding.task_id || lease.run_id != binding.run_id {
            return Err(conflict(
                "runtime binding lease does not bind the Runtime Task and Run",
            ));
        }
        require_current_lease(
            &transaction,
            lease,
            &command.actor_id,
            command.committed_at_ms,
        )?;
        if binding.runtime_generation != lease.generation {
            return Err(StoreError::GenerationFenced {
                expected: lease.generation,
                actual: binding.runtime_generation,
            });
        }
        let highest = transaction.query_row(
            "SELECT COALESCE(MAX(runtime_generation), 0) FROM runtime_bindings WHERE run_id = ?1",
            params![binding.run_id.as_str()],
            |row| row.get::<_, i64>(0),
        )?;
        let highest = unsigned(highest, "runtime generation")?;
        let minimum = next_integer(highest, "runtime generation")?;
        if binding.runtime_generation < minimum {
            return Err(StoreError::GenerationFenced {
                expected: minimum,
                actual: binding.runtime_generation,
            });
        }
        let now = integer(command.committed_at_ms, "runtime binding timestamp")?;
        let latest_update = transaction.query_row(
            "SELECT COALESCE(MAX(updated_at_ms), 0) FROM runtime_bindings WHERE run_id=?1",
            params![binding.run_id.as_str()],
            |row| row.get::<_, i64>(0),
        )?;
        require_not_before(
            command.committed_at_ms,
            unsigned(latest_update, "runtime binding update")?,
            "runtime binding",
        )?;
        transaction.execute(
            "UPDATE runtime_bindings SET state = 'lost', updated_at_ms = ?2
             WHERE run_id = ?1 AND state = 'active'",
            params![binding.run_id.as_str(), now],
        )?;
        transaction.execute(
            "INSERT INTO runtime_bindings(binding_id, actor_id, task_id, run_id,
             runtime_instance_id, runtime_generation, binding_json, state, last_sequence,
             created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'active', 0, ?8, ?8)",
            params![binding.binding_id.as_str(), command.actor_id.as_str(), binding.task_id.as_str(),
                binding.run_id.as_str(), binding.runtime_instance_id.as_str(),
                integer(binding.runtime_generation, "runtime generation")?,
                serde_json::to_string(binding)?, now],
        )?;
        let record = RuntimeBindingRecord {
            binding: binding.clone(),
            actor_id: command.actor_id.clone(),
            state: RuntimeBindingState::Active,
            last_sequence: 0,
            created_at_ms: command.committed_at_ms,
            updated_at_ms: command.committed_at_ms,
        };
        insert_receipt(&transaction, command, "bind_runtime", &record)?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(record))
    }

    /// Advances a binding's sequence only for its exact active process generation.
    pub fn record_runtime_sequence(
        &mut self,
        binding_id: &RuntimeBindingId,
        runtime_instance_id: &RuntimeInstanceId,
        runtime_generation: u64,
        sequence: u64,
        updated_at_ms: u64,
        lease: &LeaseClaim,
    ) -> Result<RuntimeBindingRecord, StoreError> {
        integer(runtime_generation, "runtime generation")?;
        integer(sequence, "runtime sequence")?;
        integer(updated_at_ms, "runtime event timestamp")?;
        let transaction = immediate(self)?;
        let record = load_runtime_binding(&transaction, binding_id)?;
        if lease.task_id != record.binding.task_id || lease.run_id != record.binding.run_id {
            return Err(conflict(
                "runtime event lease does not bind the runtime Task and Run",
            ));
        }
        require_current_lease(&transaction, lease, &record.actor_id, updated_at_ms)?;
        require_not_before(
            updated_at_ms,
            record.updated_at_ms,
            "runtime event acceptance",
        )?;
        if record.binding.runtime_generation != runtime_generation {
            return Err(StoreError::GenerationFenced {
                expected: record.binding.runtime_generation,
                actual: runtime_generation,
            });
        }
        let expected_sequence = next_integer(record.last_sequence, "runtime sequence")?;
        if record.state != RuntimeBindingState::Active
            || &record.binding.runtime_instance_id != runtime_instance_id
            || sequence != expected_sequence
        {
            return Err(conflict(
                "runtime instance, state, or event sequence is stale",
            ));
        }
        let changed = transaction.execute(
            "UPDATE runtime_bindings SET last_sequence = ?2, updated_at_ms = ?3
             WHERE binding_id = ?1 AND state = 'active' AND runtime_generation = ?4
             AND last_sequence = ?5",
            params![
                binding_id.as_str(),
                integer(sequence, "runtime sequence")?,
                integer(updated_at_ms, "runtime event timestamp")?,
                integer(runtime_generation, "runtime generation")?,
                integer(record.last_sequence, "runtime prior sequence")?
            ],
        )?;
        if changed != 1 {
            return Err(conflict(
                "runtime sequence lost its active-generation precondition",
            ));
        }
        let updated = load_runtime_binding(&transaction, binding_id)?;
        transaction.commit()?;
        Ok(updated)
    }

    /// Closes an active runtime binding only for its exact fenced generation.
    pub fn close_runtime_binding(
        &mut self,
        command: &LedgerCommand,
        binding_id: &RuntimeBindingId,
        runtime_generation: u64,
        lease: &LeaseClaim,
    ) -> Result<LedgerOutcome<RuntimeBindingRecord>, StoreError> {
        validate_command(command)?;
        integer(runtime_generation, "runtime generation")?;
        let transaction = immediate(self)?;
        if let Some(replayed) = replay(&transaction, command, "close_runtime_binding")? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        let mut record = load_runtime_binding(&transaction, binding_id)?;
        require_task_owner(&transaction, &record.binding.task_id, &command.actor_id)?;
        if lease.task_id != record.binding.task_id || lease.run_id != record.binding.run_id {
            return Err(conflict(
                "runtime close lease does not bind the Runtime Task and Run",
            ));
        }
        require_current_lease(
            &transaction,
            lease,
            &command.actor_id,
            command.committed_at_ms,
        )?;
        if record.actor_id != command.actor_id {
            return Err(conflict("runtime binding actor does not match"));
        }
        if record.binding.runtime_generation != runtime_generation {
            return Err(StoreError::GenerationFenced {
                expected: record.binding.runtime_generation,
                actual: runtime_generation,
            });
        }
        if record.state != RuntimeBindingState::Active {
            return Err(conflict("runtime binding is not active"));
        }
        require_not_before(
            command.committed_at_ms,
            record.updated_at_ms,
            "runtime close",
        )?;
        record.state = RuntimeBindingState::Closed;
        record.updated_at_ms = command.committed_at_ms;
        let changed = transaction.execute(
            "UPDATE runtime_bindings SET state='closed', updated_at_ms=?2
             WHERE binding_id=?1 AND state='active' AND runtime_generation=?3",
            params![
                binding_id.as_str(),
                integer(command.committed_at_ms, "runtime close timestamp")?,
                integer(runtime_generation, "runtime generation")?
            ],
        )?;
        if changed != 1 {
            return Err(conflict(
                "runtime close lost its active-generation precondition",
            ));
        }
        insert_receipt(&transaction, command, "close_runtime_binding", &record)?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(record))
    }

    /// Acquires an absent or expired Run lease with a monotonically increasing generation.
    pub fn acquire_run_lease(
        &mut self,
        lease: &LeaseCommand,
    ) -> Result<LedgerOutcome<RunLeaseRecord>, StoreError> {
        validate_command(&lease.command)?;
        integer(lease.expires_at_ms, "lease deadline")?;
        let transaction = immediate(self)?;
        if let Some(replayed) = replay(&transaction, &lease.command, "acquire_run_lease")? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        require_task_run(
            &transaction,
            &lease.task_id,
            &lease.run_id,
            &lease.command.actor_id,
        )?;
        if lease.expires_at_ms <= lease.command.committed_at_ms {
            return Err(conflict("run lease deadline must be in the future"));
        }
        let existing = load_run_lease_optional(&transaction, &lease.run_id)?;
        if let Some(existing) = &existing {
            if existing.task_id != lease.task_id || existing.actor_id != lease.command.actor_id {
                return Err(conflict(
                    "run lease Task or actor binding cannot be replaced",
                ));
            }
            if existing.expires_at_ms > lease.command.committed_at_ms {
                return Err(conflict("run lease is still held"));
            }
            require_not_before(
                lease.command.committed_at_ms,
                existing.updated_at_ms,
                "run lease takeover",
            )?;
        }
        let generation = match &existing {
            Some(row) => next_integer(row.generation, "lease generation")?,
            None => 1,
        };
        let revision = match &existing {
            Some(row) => next_integer(row.revision, "lease revision")?,
            None => 1,
        };
        let record = RunLeaseRecord {
            task_id: lease.task_id.clone(),
            run_id: lease.run_id.clone(),
            actor_id: lease.command.actor_id.clone(),
            lease_owner: lease.lease_owner.clone(),
            generation,
            revision,
            expires_at_ms: lease.expires_at_ms,
            updated_at_ms: lease.command.committed_at_ms,
        };
        transaction.execute(
            "INSERT INTO run_leases(run_id, task_id, actor_id, lease_owner, generation, revision,
             expires_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(run_id) DO UPDATE SET task_id=excluded.task_id, actor_id=excluded.actor_id,
             lease_owner=excluded.lease_owner, generation=excluded.generation,
             revision=excluded.revision, expires_at_ms=excluded.expires_at_ms,
             updated_at_ms=excluded.updated_at_ms",
            params![
                record.run_id.as_str(),
                record.task_id.as_str(),
                record.actor_id.as_str(),
                record.lease_owner.as_str(),
                integer(record.generation, "lease generation")?,
                integer(record.revision, "lease revision")?,
                integer(record.expires_at_ms, "lease deadline")?,
                integer(record.updated_at_ms, "lease timestamp")?
            ],
        )?;
        insert_receipt(&transaction, &lease.command, "acquire_run_lease", &record)?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(record))
    }

    /// Renews an active Run lease without changing its fencing generation.
    pub fn renew_run_lease(
        &mut self,
        lease: &LeaseCommand,
        expected_generation: u64,
        expected_revision: u64,
    ) -> Result<LedgerOutcome<RunLeaseRecord>, StoreError> {
        validate_command(&lease.command)?;
        integer(expected_generation, "lease generation")?;
        integer(expected_revision, "lease expected revision")?;
        integer(lease.expires_at_ms, "lease deadline")?;
        let transaction = immediate(self)?;
        if let Some(replayed) = replay(&transaction, &lease.command, "renew_run_lease")? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        require_task_run(
            &transaction,
            &lease.task_id,
            &lease.run_id,
            &lease.command.actor_id,
        )?;
        let existing = load_run_lease_optional(&transaction, &lease.run_id)?
            .ok_or_else(|| not_found("run lease", lease.run_id.as_str()))?;
        if existing.task_id != lease.task_id
            || existing.actor_id != lease.command.actor_id
            || existing.lease_owner != lease.lease_owner
            || existing.generation != expected_generation
            || existing.revision != expected_revision
            || existing.expires_at_ms <= lease.command.committed_at_ms
            || lease.expires_at_ms <= lease.command.committed_at_ms
        {
            return Err(conflict(
                "run lease renewal binding, revision, or deadline is stale",
            ));
        }
        require_not_before(
            lease.command.committed_at_ms,
            existing.updated_at_ms,
            "run lease renewal",
        )?;
        let next_revision = next_integer(existing.revision, "lease revision")?;
        let record = RunLeaseRecord {
            revision: next_revision,
            expires_at_ms: lease.expires_at_ms,
            updated_at_ms: lease.command.committed_at_ms,
            ..existing
        };
        let changed = transaction.execute(
            "UPDATE run_leases SET revision=?2, expires_at_ms=?3, updated_at_ms=?4
             WHERE run_id=?1 AND generation=?5 AND revision=?6 AND lease_owner=?7",
            params![
                record.run_id.as_str(),
                integer(record.revision, "lease revision")?,
                integer(record.expires_at_ms, "lease deadline")?,
                integer(record.updated_at_ms, "lease update")?,
                integer(expected_generation, "lease generation")?,
                integer(expected_revision, "lease expected revision")?,
                record.lease_owner.as_str()
            ],
        )?;
        if changed != 1 {
            return Err(conflict("run lease renewal lost its fencing precondition"));
        }
        insert_receipt(&transaction, &lease.command, "renew_run_lease", &record)?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(record))
    }

    /// Releases an active Run lease while retaining its fencing generation.
    pub fn release_run_lease(
        &mut self,
        command: &LedgerCommand,
        claim: &LeaseClaim,
    ) -> Result<LedgerOutcome<RunLeaseRecord>, StoreError> {
        validate_command(command)?;
        integer(claim.generation, "lease generation")?;
        integer(claim.revision, "lease expected revision")?;
        let transaction = immediate(self)?;
        if let Some(replayed) = replay(&transaction, command, "release_run_lease")? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        require_task_run(
            &transaction,
            &claim.task_id,
            &claim.run_id,
            &command.actor_id,
        )?;
        let existing = load_run_lease_optional(&transaction, &claim.run_id)?
            .ok_or_else(|| not_found("run lease", claim.run_id.as_str()))?;
        if existing.task_id != claim.task_id
            || existing.actor_id != command.actor_id
            || existing.lease_owner != claim.lease_owner
            || existing.generation != claim.generation
            || existing.revision != claim.revision
            || existing.expires_at_ms <= command.committed_at_ms
        {
            return Err(conflict(
                "run lease release binding, revision, or deadline is stale",
            ));
        }
        require_not_before(
            command.committed_at_ms,
            existing.updated_at_ms,
            "run lease release",
        )?;
        let next_revision = next_integer(existing.revision, "lease revision")?;
        let record = RunLeaseRecord {
            revision: next_revision,
            expires_at_ms: command.committed_at_ms,
            updated_at_ms: command.committed_at_ms,
            ..existing
        };
        let changed = transaction.execute(
            "UPDATE run_leases SET revision=?2, expires_at_ms=?3, updated_at_ms=?3
             WHERE run_id=?1 AND generation=?4 AND revision=?5 AND lease_owner=?6",
            params![
                record.run_id.as_str(),
                integer(record.revision, "lease revision")?,
                integer(record.updated_at_ms, "lease release timestamp")?,
                integer(claim.generation, "lease generation")?,
                integer(claim.revision, "lease expected revision")?,
                record.lease_owner.as_str()
            ],
        )?;
        if changed != 1 {
            return Err(conflict("run lease release lost its fencing precondition"));
        }
        insert_receipt(&transaction, command, "release_run_lease", &record)?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(record))
    }

    /// Recovers stale brokered executions after a Run-lease generation takeover.
    ///
    /// The current unexpired lease must have a newer generation than every
    /// recovered execution. Repeated calls are read-only because only exact
    /// `Claimed` and `Started` states are eligible.
    pub fn recover_brokered_executions_for_run(
        &mut self,
        run_id: &RunId,
        now_ms: u64,
    ) -> Result<BrokeredExecutionRecoveryReport, StoreError> {
        let transaction = immediate(self)?;
        let current = load_run_lease_optional(&transaction, run_id)?
            .ok_or_else(|| not_found("run lease", run_id.as_str()))?;
        if current.expires_at_ms <= now_ms {
            return Err(conflict(
                "brokered execution recovery requires an unexpired takeover lease",
            ));
        }
        require_task_run(
            &transaction,
            &current.task_id,
            &current.run_id,
            &current.actor_id,
        )?;
        let claimed = load_brokered_recovery_candidates_for_run(
            &transaction,
            run_id,
            "claimed",
            ExecutionState::Planned,
            current.generation,
        )?;
        let started = load_brokered_recovery_candidates_for_run(
            &transaction,
            run_id,
            "started",
            ExecutionState::Started,
            current.generation,
        )?;
        let report = apply_brokered_execution_recovery(&transaction, &claimed, &started, now_ms)?;
        transaction.commit()?;
        Ok(report)
    }

    /// Recovers durable state conservatively without retrying side effects.
    pub fn recover_gateway(&mut self, now_ms: u64) -> Result<RecoveryReport, StoreError> {
        let transaction = immediate(self)?;
        let now = integer(now_ms, "recovery timestamp")?;
        validate_all_execution_receipts(&transaction)?;
        let claimed =
            load_brokered_recovery_candidates(&transaction, "claimed", ExecutionState::Planned)?;
        let started =
            load_brokered_recovery_candidates(&transaction, "started", ExecutionState::Started)?;
        let execution_recovery =
            apply_brokered_execution_recovery(&transaction, &claimed, &started, now_ms)?;
        let (runtime_input_requests_cancelled, runtime_input_dispatches_unknown) =
            recover_runtime_inputs_after_restart(&transaction, now_ms)?;
        let approvals_expired = transaction.execute(
            "UPDATE approvals SET state='expired', revision=revision+1, updated_at_ms=?1
             WHERE state='pending' AND expires_at_ms <= ?1",
            params![now],
        )?;
        let approvals_cancelled = transaction.execute(
            "UPDATE approvals SET state='cancelled', revision=revision+1, updated_at_ms=?1
             WHERE state='pending'",
            params![now],
        )?;
        let permission_dispatches_unknown = transaction.execute(
            "UPDATE provider_permission_dispatches
             SET state='unknown', revision=revision+1, updated_at_ms=?1
             WHERE state IN ('prepared', 'started')",
            params![now],
        )?;
        let brokered_dispatches_unknown = transaction.execute(
            "UPDATE brokered_runtime_dispatches
             SET state='unknown', revision=revision+1, updated_at_ms=?1
             WHERE state='started'",
            params![now],
        )?;
        let permits_expired = transaction.execute(
            "UPDATE permits SET state='expired' WHERE state='issued' AND valid_until_ms <= ?1",
            params![now],
        )?;
        let legacy_executions_uncertain = transaction.execute(
            "UPDATE executions SET state='uncertain', revision=revision+1, completed_at_ms=?1,
             updated_at_ms=?1 WHERE state='started' AND broker_state IS NULL",
            params![now],
        )?;
        let runtime_bindings_lost = transaction.execute(
            "UPDATE runtime_bindings SET state='lost', updated_at_ms=?1 WHERE state='active'",
            params![now],
        )?;
        transaction.commit()?;
        Ok(RecoveryReport {
            approvals_expired: approvals_expired as u64,
            approvals_cancelled: approvals_cancelled as u64,
            permission_dispatches_unknown: permission_dispatches_unknown as u64,
            brokered_dispatches_unknown: brokered_dispatches_unknown as u64,
            runtime_input_requests_cancelled,
            runtime_input_dispatches_unknown,
            permits_expired: permits_expired as u64,
            executions_uncertain: execution_recovery.executions_uncertain
                + legacy_executions_uncertain as u64,
            executions_known_no_effect: execution_recovery.executions_known_no_effect,
            runtime_bindings_lost: runtime_bindings_lost as u64,
        })
    }
}

fn immediate(store: &mut SqliteTaskStore) -> Result<Transaction<'_>, StoreError> {
    Ok(store
        .connection_mut()
        .transaction_with_behavior(TransactionBehavior::Immediate)?)
}

#[allow(clippy::too_many_arguments)]
fn transition_pending_runtime_input_request(
    store: &mut SqliteTaskStore,
    command: &LedgerCommand,
    request_id: &InputRequestId,
    expected_revision: u64,
    next_state: RuntimeInputRequestState,
    require_expired: bool,
    operation: &str,
) -> Result<LedgerOutcome<RuntimeInputRequestRecord>, StoreError> {
    validate_command(command)?;
    integer(expected_revision, "runtime input request expected revision")?;
    let transaction = immediate(store)?;
    if let Some(replayed) = replay::<RuntimeInputRequestRecord>(&transaction, command, operation)? {
        if replayed.request.request_id() != request_id || replayed.state != next_state {
            return Err(StoreError::IdempotencyConflict);
        }
        transaction.commit()?;
        return Ok(LedgerOutcome::Replayed(replayed));
    }
    let mut record = load_runtime_input_request(&transaction, request_id)?;
    if record.actor_id != command.actor_id
        || record.state != RuntimeInputRequestState::Pending
        || record.revision != expected_revision
        || (require_expired && command.committed_at_ms < record.expires_at_ms)
    {
        return Err(conflict(
            "runtime input request state, revision, actor, or deadline is stale",
        ));
    }
    if require_expired {
        append_internal_task_event(
            &transaction,
            &record.task_id,
            &record.actor_id,
            command.committed_at_ms,
            TaskEvent::RunSuspended {
                run_id: record.run_id.clone(),
                reason: cosh_gateway_contracts::task::SuspensionCode::OperatorRequired,
            },
            None,
        )?;
    } else {
        let task = load_verified_projection(&transaction, &record.task_id)?
            .ok_or(StoreError::TaskNotFound)?;
        if !task.cancellation_requested() {
            return Err(conflict(
                "runtime input cancellation requires durable Task cancellation",
            ));
        }
    }
    record.state = next_state;
    record.revision = next_integer(record.revision, "runtime input request revision")?;
    record.updated_at_ms = command.committed_at_ms;
    let changed = transaction.execute(
        "UPDATE runtime_input_requests SET state=?2, revision=?3, updated_at_ms=?4
         WHERE request_id=?1 AND state='pending' AND revision=?5",
        params![
            request_id.as_str(),
            state_name(next_state)?,
            integer(record.revision, "runtime input request revision")?,
            integer(command.committed_at_ms, "runtime input request transition")?,
            integer(expected_revision, "runtime input request expected revision")?,
        ],
    )?;
    if changed != 1 {
        return Err(conflict(
            "runtime input request lost its pending revision precondition",
        ));
    }
    insert_receipt(&transaction, command, operation, &record)?;
    transaction.commit()?;
    Ok(LedgerOutcome::Applied(record))
}

#[allow(clippy::too_many_arguments)]
fn transition_runtime_input_dispatch(
    store: &mut SqliteTaskStore,
    command: &LedgerCommand,
    request_id: &InputRequestId,
    response_digest: &Digest,
    expected_revision: u64,
    lease: &LeaseClaim,
    expected_state: RuntimeInputDispatchState,
    next_state: RuntimeInputDispatchState,
    operation: &str,
) -> Result<LedgerOutcome<RuntimeInputDispatchRecord>, StoreError> {
    validate_command(command)?;
    integer(
        expected_revision,
        "runtime input dispatch expected revision",
    )?;
    let transaction = immediate(store)?;
    if let Some(replayed) = replay_runtime_input_dispatch(
        &transaction,
        command,
        operation,
        request_id,
        response_digest,
    )? {
        transaction.commit()?;
        return Ok(LedgerOutcome::Replayed(replayed));
    }
    let request = load_runtime_input_request(&transaction, request_id)?;
    let mut dispatch = load_runtime_input_dispatch(&transaction, request_id)?;
    require_runtime_input_dispatch_context(
        &transaction,
        command,
        &request,
        &dispatch,
        response_digest,
        lease,
    )?;
    if dispatch.state != expected_state || dispatch.revision != expected_revision {
        return Err(conflict(
            "runtime input dispatch is not at the expected state and revision",
        ));
    }
    if expected_state == RuntimeInputDispatchState::Prepared {
        let task = load_verified_projection(&transaction, &dispatch.task_id)?
            .ok_or_else(|| corrupt("Runtime input dispatch references a missing Task"))?;
        if task.owner_actor_id() != &dispatch.actor_id
            || !task.active_run_is_running(&dispatch.run_id)
            || task.cancellation_requested()
        {
            return Err(conflict(
                "runtime input dispatch cannot start after its Task stopped running",
            ));
        }
    }
    require_not_before(
        command.committed_at_ms,
        dispatch.updated_at_ms,
        "runtime input dispatch",
    )?;
    dispatch.state = next_state;
    dispatch.revision = next_integer(dispatch.revision, "runtime input dispatch revision")?;
    dispatch.updated_at_ms = command.committed_at_ms;
    let changed = transaction.execute(
        "UPDATE runtime_input_dispatches SET state=?2, revision=?3, updated_at_ms=?4
         WHERE request_id=?1 AND state=?5 AND revision=?6",
        params![
            request_id.as_str(),
            state_name(next_state)?,
            integer(dispatch.revision, "runtime input dispatch revision")?,
            integer(command.committed_at_ms, "runtime input dispatch timestamp")?,
            state_name(expected_state)?,
            integer(
                expected_revision,
                "runtime input dispatch expected revision"
            )?,
        ],
    )?;
    if changed != 1 {
        return Err(conflict(
            "runtime input dispatch lost its state or revision precondition",
        ));
    }
    insert_runtime_input_dispatch_receipt(&transaction, command, operation, &dispatch)?;
    transaction.commit()?;
    Ok(LedgerOutcome::Applied(dispatch))
}

#[allow(clippy::too_many_arguments)]
fn mark_runtime_input_dispatch_unknown_atomic(
    store: &mut SqliteTaskStore,
    command: &LedgerCommand,
    request_id: &InputRequestId,
    response_digest: &Digest,
    expected_revision: u64,
    lease: &LeaseClaim,
    operation: &str,
) -> Result<LedgerOutcome<RuntimeInputDispatchRecord>, StoreError> {
    validate_command(command)?;
    integer(
        expected_revision,
        "runtime input dispatch expected revision",
    )?;
    let transaction = immediate(store)?;
    if let Some(replayed) = replay_runtime_input_dispatch(
        &transaction,
        command,
        operation,
        request_id,
        response_digest,
    )? {
        transaction.commit()?;
        return Ok(LedgerOutcome::Replayed(replayed));
    }
    let request = load_runtime_input_request(&transaction, request_id)?;
    let mut dispatch = load_runtime_input_dispatch(&transaction, request_id)?;
    require_runtime_input_dispatch_context(
        &transaction,
        command,
        &request,
        &dispatch,
        response_digest,
        lease,
    )?;
    if dispatch.state != RuntimeInputDispatchState::Started
        || dispatch.revision != expected_revision
    {
        return Err(conflict(
            "runtime input dispatch is not at the started state and revision",
        ));
    }
    require_not_before(
        command.committed_at_ms,
        dispatch.updated_at_ms,
        "runtime input uncertainty",
    )?;
    dispatch.state = RuntimeInputDispatchState::Unknown;
    dispatch.revision = next_integer(dispatch.revision, "runtime input dispatch revision")?;
    dispatch.updated_at_ms = command.committed_at_ms;
    let changed = transaction.execute(
        "UPDATE runtime_input_dispatches SET state='unknown', revision=?2, updated_at_ms=?3
         WHERE request_id=?1 AND state='started' AND revision=?4",
        params![
            request_id.as_str(),
            integer(dispatch.revision, "runtime input dispatch revision")?,
            integer(
                command.committed_at_ms,
                "runtime input uncertainty timestamp"
            )?,
            integer(
                expected_revision,
                "runtime input dispatch expected revision"
            )?,
        ],
    )?;
    if changed != 1 {
        return Err(conflict(
            "runtime input uncertainty lost its started revision",
        ));
    }
    append_internal_task_event(
        &transaction,
        &dispatch.task_id,
        &dispatch.actor_id,
        command.committed_at_ms,
        TaskEvent::RunSuspended {
            run_id: dispatch.run_id.clone(),
            reason: cosh_gateway_contracts::task::SuspensionCode::OperatorRequired,
        },
        None,
    )?;
    insert_runtime_input_dispatch_receipt(&transaction, command, operation, &dispatch)?;
    transaction.commit()?;
    Ok(LedgerOutcome::Applied(dispatch))
}

fn require_runtime_input_dispatch_context(
    transaction: &rusqlite::Connection,
    command: &LedgerCommand,
    request: &RuntimeInputRequestRecord,
    dispatch: &RuntimeInputDispatchRecord,
    response_digest: &Digest,
    lease: &LeaseClaim,
) -> Result<(), StoreError> {
    if request.state != RuntimeInputRequestState::Resolved
        || request.response_digest.as_ref() != Some(response_digest)
        || dispatch.request_id != *request.request.request_id()
        || dispatch.actor_id != command.actor_id
        || dispatch.actor_id != request.actor_id
        || dispatch.task_id != request.task_id
        || dispatch.run_id != request.run_id
        || dispatch.response_digest != *response_digest
        || lease.task_id != request.task_id
        || lease.run_id != request.run_id
        || lease.generation != request.lease_generation
    {
        return Err(conflict("runtime input dispatch binding is stale"));
    }
    if runtime_input_response_digest(&dispatch.response)? != *response_digest {
        return Err(corrupt(
            "runtime input dispatch response diverges from its digest",
        ));
    }
    require_current_lease(
        transaction,
        lease,
        &command.actor_id,
        command.committed_at_ms,
    )?;
    let binding = load_runtime_binding(transaction, &request.binding_id)?;
    if binding.state != RuntimeBindingState::Active
        || binding.actor_id != command.actor_id
        || binding.binding.task_id != request.task_id
        || binding.binding.run_id != request.run_id
        || binding.binding.runtime_instance_id != request.runtime_instance_id
        || binding.binding.runtime_generation != request.runtime_generation
    {
        return Err(conflict("runtime input dispatch Runtime binding is stale"));
    }
    Ok(())
}

fn validate_runtime_input_response(
    request: &RuntimeInputRequest,
    response: &RuntimeInputResponse,
) -> Result<(), StoreError> {
    match response {
        RuntimeInputResponse::Text { .. } if !request.allows_free_text() => {
            Err(conflict("runtime input request does not allow free text"))
        }
        RuntimeInputResponse::Options { selections } => {
            if (!request.allows_multiple() && selections.as_slice().len() != 1)
                || selections
                    .as_slice()
                    .iter()
                    .any(|index| usize::from(*index) >= request.options().len())
            {
                return Err(conflict(
                    "runtime input response selections do not match the request",
                ));
            }
            Ok(())
        }
        RuntimeInputResponse::Text { .. } => Ok(()),
    }
}

fn validate_json_bound<T: Serialize>(
    value: &T,
    maximum: usize,
    field: &str,
) -> Result<(), StoreError> {
    if serde_json::to_vec(value)?.len() > maximum {
        return Err(conflict(&format!(
            "{field} exceeds {maximum} serialized bytes"
        )));
    }
    Ok(())
}

fn runtime_input_response_digest(response: &RuntimeInputResponse) -> Result<Digest, StoreError> {
    let encoded = serde_json::to_vec(response)?;
    let digest = Sha256::digest(&encoded);
    Digest::parse(format!("{digest:x}")).map_err(|error| {
        corrupt(&format!(
            "runtime input response digest is invalid: {error}"
        ))
    })
}

fn insert_runtime_input_request(
    transaction: &rusqlite::Connection,
    record: &RuntimeInputRequestRecord,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO runtime_input_requests(
             request_id, actor_id, task_id, run_id, binding_id, runtime_instance_id,
             runtime_generation, runtime_sequence, lease_generation, lease_revision,
             request_json, state, response_digest, revision, expires_at_ms,
             created_at_ms, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'pending', NULL,
                 1, ?12, ?13, ?13)",
        params![
            record.request.request_id().as_str(),
            record.actor_id.as_str(),
            record.task_id.as_str(),
            record.run_id.as_str(),
            record.binding_id.as_str(),
            record.runtime_instance_id.as_str(),
            integer(record.runtime_generation, "runtime input generation")?,
            integer(record.runtime_sequence, "runtime input sequence")?,
            integer(record.lease_generation, "runtime input lease generation")?,
            integer(record.lease_revision, "runtime input lease revision")?,
            serde_json::to_string(&record.request)?,
            integer(record.expires_at_ms, "runtime input deadline")?,
            integer(record.created_at_ms, "runtime input creation timestamp")?,
        ],
    )?;
    Ok(())
}

fn insert_runtime_input_dispatch(
    transaction: &rusqlite::Connection,
    record: &RuntimeInputDispatchRecord,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO runtime_input_dispatches(
             request_id, actor_id, task_id, run_id, response_json, response_digest,
             state, revision, created_at_ms, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'prepared', 1, ?7, ?7)",
        params![
            record.request_id.as_str(),
            record.actor_id.as_str(),
            record.task_id.as_str(),
            record.run_id.as_str(),
            serde_json::to_string(&record.response)?,
            record.response_digest.as_str(),
            integer(record.created_at_ms, "runtime input dispatch creation")?,
        ],
    )?;
    Ok(())
}

fn insert_runtime_input_dispatch_receipt(
    transaction: &Transaction<'_>,
    command: &LedgerCommand,
    operation: &str,
    record: &RuntimeInputDispatchRecord,
) -> Result<(), StoreError> {
    insert_receipt(
        transaction,
        command,
        operation,
        &RuntimeInputDispatchReceipt {
            request_id: record.request_id.clone(),
            response_digest: record.response_digest.clone(),
            state: record.state,
            revision: record.revision,
        },
    )
}

fn replay_runtime_input_dispatch(
    transaction: &Transaction<'_>,
    command: &LedgerCommand,
    operation: &str,
    request_id: &InputRequestId,
    response_digest: &Digest,
) -> Result<Option<RuntimeInputDispatchRecord>, StoreError> {
    let Some(receipt) = replay::<RuntimeInputDispatchReceipt>(transaction, command, operation)?
    else {
        return Ok(None);
    };
    if receipt.request_id != *request_id || receipt.response_digest != *response_digest {
        return Err(StoreError::IdempotencyConflict);
    }
    let record = load_runtime_input_dispatch(transaction, request_id)?;
    let forward_state = match receipt.state {
        RuntimeInputDispatchState::Prepared => true,
        RuntimeInputDispatchState::Started => matches!(
            record.state,
            RuntimeInputDispatchState::Started
                | RuntimeInputDispatchState::Delivered
                | RuntimeInputDispatchState::Unknown
        ),
        RuntimeInputDispatchState::Delivered => {
            record.state == RuntimeInputDispatchState::Delivered
        }
        RuntimeInputDispatchState::Unknown => record.state == RuntimeInputDispatchState::Unknown,
    };
    if record.response_digest != receipt.response_digest
        || record.revision < receipt.revision
        || !forward_state
    {
        return Err(corrupt(
            "runtime input dispatch diverges from its command receipt",
        ));
    }
    Ok(Some(record))
}

fn load_recoverable_runtime_input_dispatches(
    transaction: &rusqlite::Connection,
    run_id: &RunId,
) -> Result<Vec<RuntimeInputDispatchRecord>, StoreError> {
    let mut statement = transaction.prepare(
        "SELECT request_id FROM runtime_input_dispatches
         WHERE run_id=?1 AND state IN ('prepared', 'started') ORDER BY request_id LIMIT 2",
    )?;
    let ids = statement
        .query_map(params![run_id.as_str()], |row| row.get::<_, String>(0))?
        .map(|row| {
            let raw = row?;
            InputRequestId::parse(raw)
                .map_err(|error| corrupt(&format!("invalid input request identity: {error}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    ids.iter()
        .map(|id| load_runtime_input_dispatch(transaction, id))
        .collect()
}

fn load_pending_runtime_input_requests(
    transaction: &rusqlite::Connection,
    run_id: &RunId,
) -> Result<Vec<RuntimeInputRequestRecord>, StoreError> {
    let mut statement = transaction.prepare(
        "SELECT request_id FROM runtime_input_requests
         WHERE run_id=?1 AND state='pending' ORDER BY request_id LIMIT 2",
    )?;
    let ids = statement
        .query_map(params![run_id.as_str()], |row| row.get::<_, String>(0))?
        .map(|row| {
            let raw = row?;
            InputRequestId::parse(raw)
                .map_err(|error| corrupt(&format!("invalid input request identity: {error}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    ids.iter()
        .map(|id| load_runtime_input_request(transaction, id))
        .collect()
}

fn recover_runtime_inputs_after_restart(
    transaction: &Transaction<'_>,
    now_ms: u64,
) -> Result<(u64, u64), StoreError> {
    let now = integer(now_ms, "runtime input restart recovery timestamp")?;
    let mut dispatches_unknown = 0u64;
    loop {
        let request_id = transaction
            .query_row(
                "SELECT request_id FROM runtime_input_dispatches
                 WHERE state IN ('prepared', 'started') ORDER BY updated_at_ms, request_id LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(request_id) = request_id else {
            break;
        };
        let request_id = InputRequestId::parse(request_id)
            .map_err(|error| corrupt(&format!("invalid input request identity: {error}")))?;
        let dispatch = load_runtime_input_dispatch(transaction, &request_id)?;
        let suspend_task = runtime_input_recovery_requires_suspension(
            transaction,
            &dispatch.task_id,
            &dispatch.actor_id,
            &dispatch.run_id,
            TaskState::Running,
        )?;
        let changed = transaction.execute(
            "UPDATE runtime_input_dispatches
             SET state='unknown', revision=revision+1, updated_at_ms=?2
             WHERE request_id=?1 AND state IN ('prepared', 'started') AND revision=?3",
            params![
                request_id.as_str(),
                now,
                integer(dispatch.revision, "runtime input dispatch revision")?,
            ],
        )?;
        if changed != 1 {
            return Err(conflict(
                "runtime input restart recovery lost its dispatch revision",
            ));
        }
        if suspend_task {
            append_internal_task_event(
                transaction,
                &dispatch.task_id,
                &dispatch.actor_id,
                now_ms,
                TaskEvent::RunSuspended {
                    run_id: dispatch.run_id,
                    reason: cosh_gateway_contracts::task::SuspensionCode::OperatorRequired,
                },
                None,
            )?;
        }
        dispatches_unknown = dispatches_unknown
            .checked_add(1)
            .ok_or_else(|| corrupt("runtime input dispatch recovery count overflow"))?;
    }

    let mut requests_cancelled = 0u64;
    loop {
        let request_id = transaction
            .query_row(
                "SELECT request_id FROM runtime_input_requests
                 WHERE state='pending' ORDER BY updated_at_ms, request_id LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(request_id) = request_id else {
            break;
        };
        let request_id = InputRequestId::parse(request_id)
            .map_err(|error| corrupt(&format!("invalid input request identity: {error}")))?;
        let request = load_runtime_input_request(transaction, &request_id)?;
        let suspend_task = runtime_input_recovery_requires_suspension(
            transaction,
            &request.task_id,
            &request.actor_id,
            &request.run_id,
            TaskState::WaitingInput,
        )?;
        let changed = transaction.execute(
            "UPDATE runtime_input_requests
             SET state='cancelled', revision=revision+1, updated_at_ms=?2
             WHERE request_id=?1 AND state='pending' AND revision=?3",
            params![
                request_id.as_str(),
                now,
                integer(request.revision, "runtime input request revision")?,
            ],
        )?;
        if changed != 1 {
            return Err(conflict(
                "runtime input restart recovery lost its request revision",
            ));
        }
        if suspend_task {
            append_internal_task_event(
                transaction,
                &request.task_id,
                &request.actor_id,
                now_ms,
                TaskEvent::RunSuspended {
                    run_id: request.run_id,
                    reason: cosh_gateway_contracts::task::SuspensionCode::OperatorRequired,
                },
                None,
            )?;
        }
        requests_cancelled = requests_cancelled
            .checked_add(1)
            .ok_or_else(|| corrupt("runtime input request recovery count overflow"))?;
    }
    Ok((requests_cancelled, dispatches_unknown))
}

fn runtime_input_recovery_requires_suspension(
    transaction: &Transaction<'_>,
    task_id: &TaskId,
    actor_id: &ActorId,
    run_id: &RunId,
    recoverable_state: TaskState,
) -> Result<bool, StoreError> {
    let task = load_verified_projection(transaction, task_id)?
        .ok_or_else(|| corrupt("Runtime input recovery references a missing Task"))?;
    if task.owner_actor_id() != actor_id || task.active_run_id() != Some(run_id) {
        return Err(corrupt(
            "Runtime input recovery identity diverges from its Task",
        ));
    }
    if task.state() == recoverable_state {
        return Ok(true);
    }
    if matches!(
        task.state(),
        TaskState::Suspended | TaskState::Failed | TaskState::Cancelled
    ) {
        return Ok(false);
    }
    Err(corrupt(
        "Runtime input recovery Task is neither active nor safely terminal",
    ))
}

fn prepare_brokered_runtime_dispatch(
    store: &mut SqliteTaskStore,
    command: &LedgerCommand,
    kind: BrokeredRuntimeDispatchKind,
    source: BrokeredRuntimeDispatchSource,
    brokered: &BrokeredExecutionRef,
    supplied_digest: Option<&Digest>,
    delivery: Option<&BrokeredExecutionDelivery>,
    lease: &LeaseClaim,
) -> Result<LedgerOutcome<BrokeredRuntimeDispatchRecord>, StoreError> {
    validate_command(command)?;
    let transaction = immediate(store)?;
    let canonical_digest = delivery.map(brokered_delivery_digest).transpose()?;
    let payload_digest = match (supplied_digest, canonical_digest.as_ref(), kind) {
        (Some(digest), None, BrokeredRuntimeDispatchKind::Acknowledgement) => digest,
        (None, Some(digest), BrokeredRuntimeDispatchKind::Result) => digest,
        _ => return Err(conflict("brokered dispatch payload kind is invalid")),
    };
    if let Some(replayed) = replay::<BrokeredRuntimeDispatchRecord>(
        &transaction,
        command,
        "prepare_brokered_runtime_dispatch",
    )? {
        if replayed.kind != kind || replayed.source != source {
            return Err(StoreError::IdempotencyConflict);
        }
        require_brokered_dispatch_context(
            &transaction,
            &command.actor_id,
            &replayed,
            brokered,
            payload_digest,
            lease,
            command.committed_at_ms,
        )?;
        transaction.commit()?;
        return Ok(LedgerOutcome::Replayed(replayed));
    }
    let request = load_brokered_request(&transaction, &brokered.request_id)?;
    let record = BrokeredRuntimeDispatchRecord {
        brokered: brokered.clone(),
        actor_id: request.request.actor.actor_id.clone(),
        task_id: request.request.task_id.clone(),
        kind,
        payload_digest: payload_digest.clone(),
        source,
        state: BrokeredRuntimeDispatchState::Prepared,
        revision: 1,
        created_at_ms: command.committed_at_ms,
        updated_at_ms: command.committed_at_ms,
    };
    require_brokered_dispatch_context(
        &transaction,
        &command.actor_id,
        &record,
        brokered,
        payload_digest,
        lease,
        command.committed_at_ms,
    )?;
    require_brokered_dispatch_ready(&transaction, &request, &record, delivery)?;
    let (source_kind, source_id) = brokered_dispatch_source_columns(&record.source);
    transaction.execute(
        "INSERT INTO brokered_runtime_dispatches(
             request_id, dispatch_kind, actor_id, task_id, run_id, brokered_ref_json,
             payload_digest, source_kind, source_id, state, revision, created_at_ms, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'prepared', 1, ?10, ?10)",
        params![
            brokered.request_id.as_str(),
            state_name(kind)?,
            record.actor_id.as_str(),
            record.task_id.as_str(),
            brokered.run_id.as_str(),
            serde_json::to_string(brokered)?,
            payload_digest.as_str(),
            source_kind,
            source_id,
            integer(command.committed_at_ms, "dispatch preparation timestamp")?,
        ],
    )?;
    insert_receipt(
        &transaction,
        command,
        "prepare_brokered_runtime_dispatch",
        &record,
    )?;
    transaction.commit()?;
    Ok(LedgerOutcome::Applied(record))
}

// Keeping every expected binding explicit prevents a partial dispatch transition.
#[allow(clippy::too_many_arguments)]
fn transition_brokered_runtime_dispatch(
    store: &mut SqliteTaskStore,
    command: &LedgerCommand,
    kind: BrokeredRuntimeDispatchKind,
    brokered: &BrokeredExecutionRef,
    payload_digest: &Digest,
    expected_revision: u64,
    lease: &LeaseClaim,
    expected_state: BrokeredRuntimeDispatchState,
    next_state: BrokeredRuntimeDispatchState,
    operation: &str,
) -> Result<LedgerOutcome<BrokeredRuntimeDispatchRecord>, StoreError> {
    validate_command(command)?;
    integer(expected_revision, "brokered dispatch expected revision")?;
    let transaction = immediate(store)?;
    if let Some(replayed) =
        replay::<BrokeredRuntimeDispatchRecord>(&transaction, command, operation)?
    {
        if replayed.kind != kind {
            return Err(StoreError::IdempotencyConflict);
        }
        require_brokered_dispatch_context(
            &transaction,
            &command.actor_id,
            &replayed,
            brokered,
            payload_digest,
            lease,
            command.committed_at_ms,
        )?;
        transaction.commit()?;
        return Ok(LedgerOutcome::Replayed(replayed));
    }
    let mut record = load_brokered_runtime_dispatch(&transaction, &brokered.request_id, kind)?;
    require_brokered_dispatch_context(
        &transaction,
        &command.actor_id,
        &record,
        brokered,
        payload_digest,
        lease,
        command.committed_at_ms,
    )?;
    if record.state != expected_state || record.revision != expected_revision {
        return Err(conflict(
            "brokered Runtime dispatch is not at the expected state and revision",
        ));
    }
    require_not_before(
        command.committed_at_ms,
        record.updated_at_ms,
        "brokered Runtime dispatch transition",
    )?;
    let prior_state = state_name(expected_state)?;
    record.state = next_state;
    record.revision = next_integer(record.revision, "brokered dispatch revision")?;
    record.updated_at_ms = command.committed_at_ms;
    let changed = transaction.execute(
        "UPDATE brokered_runtime_dispatches
         SET state=?3, revision=?4, updated_at_ms=?5
         WHERE request_id=?1 AND dispatch_kind=?2 AND state=?6 AND revision=?7",
        params![
            brokered.request_id.as_str(),
            state_name(kind)?,
            state_name(next_state)?,
            integer(record.revision, "brokered dispatch revision")?,
            integer(command.committed_at_ms, "brokered dispatch timestamp")?,
            prior_state,
            integer(expected_revision, "brokered dispatch expected revision")?,
        ],
    )?;
    if changed != 1 {
        return Err(conflict(
            "brokered Runtime dispatch lost its state or revision precondition",
        ));
    }
    insert_receipt(&transaction, command, operation, &record)?;
    transaction.commit()?;
    Ok(LedgerOutcome::Applied(record))
}

fn require_brokered_dispatch_context(
    connection: &rusqlite::Connection,
    actor_id: &ActorId,
    record: &BrokeredRuntimeDispatchRecord,
    brokered: &BrokeredExecutionRef,
    payload_digest: &Digest,
    lease: &LeaseClaim,
    now_ms: u64,
) -> Result<(), StoreError> {
    if record.actor_id != *actor_id
        || record.brokered != *brokered
        || record.payload_digest != *payload_digest
        || record.task_id != lease.task_id
        || brokered.run_id != lease.run_id
        || brokered.runtime_generation != lease.generation
        || brokered.event_sequence == 0
    {
        return Err(conflict(
            "brokered dispatch actor, payload, callback, or lease binding differs",
        ));
    }
    require_current_lease(connection, lease, actor_id, now_ms)?;
    let request = load_brokered_request(connection, &brokered.request_id)?;
    if request.request.actor.actor_id != *actor_id
        || request.request.task_id != record.task_id
        || request.request.run_id != brokered.run_id
        || request.operation != brokered.operation
        || request.runtime_fence.binding_id != brokered.binding_id
        || request.runtime_fence.runtime_generation != brokered.runtime_generation
        || request.runtime_fence.lease_generation != lease.generation
    {
        return Err(conflict(
            "brokered dispatch diverges from its durable request authority",
        ));
    }
    let binding = load_runtime_binding(connection, &brokered.binding_id)?;
    if binding.actor_id != *actor_id
        || binding.state != RuntimeBindingState::Active
        || binding.binding.task_id != record.task_id
        || binding.binding.run_id != brokered.run_id
        || binding.binding.runtime_generation != brokered.runtime_generation
        || binding.last_sequence < brokered.event_sequence
    {
        return Err(conflict(
            "brokered dispatch Runtime binding is stale or has not observed the event",
        ));
    }
    Ok(())
}

fn require_brokered_dispatch_ready(
    connection: &rusqlite::Connection,
    request: &BrokeredRequestRecord,
    record: &BrokeredRuntimeDispatchRecord,
    delivery: Option<&BrokeredExecutionDelivery>,
) -> Result<(), StoreError> {
    let task = load_authoritative_task(connection, &record.task_id)?;
    match &record.source {
        BrokeredRuntimeDispatchSource::ApprovalPending { approval_id } => {
            let approval = load_approval(connection, approval_id)?;
            if record.kind != BrokeredRuntimeDispatchKind::Acknowledgement
                || request.approval_id.as_ref() != Some(approval_id)
                || !brokered_approval_matches_request(&approval, request)
                || approval.state != ApprovalState::Pending
                || task.state() != TaskState::WaitingApproval
                || delivery.is_some()
            {
                return Err(conflict(
                    "brokered acknowledgement prerequisites are not durable",
                ));
            }
        }
        BrokeredRuntimeDispatchSource::ApprovalDenied { approval_id } => {
            let approval = load_approval(connection, approval_id)?;
            let valid_delivery = matches!(
                delivery,
                Some(BrokeredExecutionDelivery {
                    request_id,
                    outcome: BrokeredExecutionOutcome::Denied {
                        code: DenialCode::ApprovalDenied,
                        ..
                    },
                }) if request_id == &request.request.request_id
            );
            if record.kind != BrokeredRuntimeDispatchKind::Result
                || request.approval_id.as_ref() != Some(approval_id)
                || !brokered_approval_matches_request(&approval, request)
                || approval.state != ApprovalState::Denied
                || task.state() != TaskState::Running
                || !valid_delivery
            {
                return Err(conflict("brokered denial is not durable"));
            }
        }
        BrokeredRuntimeDispatchSource::Execution { execution_id } => {
            let execution = load_execution(connection, execution_id)?;
            let permit_request = connection
                .query_row(
                    "SELECT request_id FROM permits WHERE execution_id=?1",
                    params![execution_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .ok_or_else(|| not_found("execution permit", execution_id.as_str()))?;
            let ready = matches!(
                execution.state,
                ExecutionState::Succeeded | ExecutionState::Failed | ExecutionState::Uncertain
            ) || execution.broker_state == Some(BrokerExecutionState::KnownNoEffect);
            let expected_task_state = if execution.state == ExecutionState::Uncertain {
                TaskState::Suspended
            } else {
                TaskState::Running
            };
            let valid_delivery = match (delivery, execution.state, execution.broker_state) {
                (
                    Some(BrokeredExecutionDelivery {
                        request_id,
                        outcome:
                            BrokeredExecutionOutcome::Succeeded {
                                execution_id: delivered_execution,
                                result,
                            },
                    }),
                    ExecutionState::Succeeded,
                    _,
                ) => {
                    request_id == &request.request.request_id
                        && delivered_execution == execution_id
                        && load_brokered_execution_result(connection, execution_id)
                            .map(|durable| durable.result == *result)?
                }
                (
                    Some(BrokeredExecutionDelivery {
                        request_id,
                        outcome:
                            BrokeredExecutionOutcome::Failed {
                                execution_id: delivered_execution,
                                ..
                            },
                    }),
                    ExecutionState::Failed,
                    _,
                )
                | (
                    Some(BrokeredExecutionDelivery {
                        request_id,
                        outcome:
                            BrokeredExecutionOutcome::Failed {
                                execution_id: delivered_execution,
                                ..
                            },
                    }),
                    ExecutionState::Planned,
                    Some(BrokerExecutionState::KnownNoEffect),
                ) => {
                    request_id == &request.request.request_id && delivered_execution == execution_id
                }
                (
                    Some(BrokeredExecutionDelivery {
                        request_id,
                        outcome:
                            BrokeredExecutionOutcome::Uncertain {
                                execution_id: delivered_execution,
                                ..
                            },
                    }),
                    ExecutionState::Uncertain,
                    _,
                ) => {
                    request_id == &request.request.request_id && delivered_execution == execution_id
                }
                _ => false,
            };
            if record.kind != BrokeredRuntimeDispatchKind::Result
                || permit_request != record.brokered.request_id.as_str()
                || execution.actor_id != record.actor_id
                || execution.task_id != record.task_id
                || execution.run_id != record.brokered.run_id
                || execution.target != request.request.target
                || execution.target_identity_digest.as_ref()
                    != Some(&request.target_identity_digest)
                || execution.runtime_fence.as_ref() != Some(&request.runtime_fence)
                || execution.operation_digest != request.request.operation_digest
                || execution.input_digest != request.request.input_digest
                || !ready
                || task.state() != expected_task_state
                || !valid_delivery
            {
                return Err(conflict(
                    "brokered execution result prerequisites are not durable",
                ));
            }
        }
    }
    Ok(())
}

fn brokered_approval_matches_request(
    approval: &ApprovalRecord,
    request: &BrokeredRequestRecord,
) -> bool {
    approval.request_id == request.request.request_id
        && approval.actor_id == request.request.actor.actor_id
        && approval.task_id == request.request.task_id
        && approval.run_id == request.request.run_id
        && approval.target == request.request.target
        && approval.target_identity_digest.as_ref() == Some(&request.target_identity_digest)
        && approval.runtime_fence.as_ref() == Some(&request.runtime_fence)
        && approval.operation_digest == request.request.operation_digest
        && approval.input_digest == request.request.input_digest
}

fn brokered_dispatch_source_columns(
    source: &BrokeredRuntimeDispatchSource,
) -> (&'static str, &str) {
    match source {
        BrokeredRuntimeDispatchSource::ApprovalPending { approval_id } => {
            ("approval_pending", approval_id.as_str())
        }
        BrokeredRuntimeDispatchSource::ApprovalDenied { approval_id } => {
            ("approval_denied", approval_id.as_str())
        }
        BrokeredRuntimeDispatchSource::Execution { execution_id } => {
            ("execution", execution_id.as_str())
        }
    }
}

fn load_brokered_recovery_candidates(
    transaction: &rusqlite::Connection,
    broker_state: &str,
    execution_state: ExecutionState,
) -> Result<Vec<ExecutionRecord>, StoreError> {
    let state = state_name(execution_state)?;
    let ids = {
        let mut statement = transaction.prepare(
            "SELECT execution_id FROM executions
             WHERE broker_state=?1 AND state=?2 ORDER BY created_at_ms, execution_id",
        )?;
        let rows = statement
            .query_map(params![broker_state, state], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    ids.into_iter()
        .map(|value| {
            let id = ExecutionId::parse(value)
                .map_err(|_| corrupt("invalid brokered recovery execution identity"))?;
            load_execution(transaction, &id)
        })
        .collect()
}

fn load_brokered_recovery_candidates_for_run(
    connection: &rusqlite::Connection,
    run_id: &RunId,
    broker_state: &str,
    execution_state: ExecutionState,
    current_generation: u64,
) -> Result<Vec<ExecutionRecord>, StoreError> {
    let state = state_name(execution_state)?;
    let ids = {
        let mut statement = connection.prepare(
            "SELECT execution_id FROM executions
             WHERE run_id=?1 AND broker_state=?2 AND state=?3
             ORDER BY created_at_ms, execution_id",
        )?;
        let rows = statement
            .query_map(params![run_id.as_str(), broker_state, state], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    ids.into_iter()
        .map(|value| {
            let id = ExecutionId::parse(value)
                .map_err(|_| corrupt("invalid brokered recovery execution identity"))?;
            load_execution(connection, &id)
        })
        .map(|result| match result {
            Ok(execution) => match execution.runtime_fence.as_ref() {
                Some(fence) if fence.lease_generation < current_generation => Ok(execution),
                Some(_) => Err(conflict(
                    "brokered execution recovery requires a newer lease generation",
                )),
                None => Err(corrupt(
                    "brokered recovery execution is missing its Runtime fence",
                )),
            },
            Err(error) => Err(error),
        })
        .collect()
}

fn apply_brokered_execution_recovery(
    transaction: &Transaction<'_>,
    claimed: &[ExecutionRecord],
    started: &[ExecutionRecord],
    now_ms: u64,
) -> Result<BrokeredExecutionRecoveryReport, StoreError> {
    let now = integer(now_ms, "execution recovery timestamp")?;
    for execution in claimed {
        append_internal_task_event(
            transaction,
            &execution.task_id,
            &execution.actor_id,
            now_ms,
            TaskEvent::ExecutionResultRecorded {
                execution_id: execution.execution_id.clone(),
                outcome: ExecutionOutcome::Failed {
                    error: ContractError::new(
                        "executor_restarted_before_effect",
                        ErrorCategory::RuntimeUnavailable,
                        false,
                        "Executor restarted before the external effect began",
                    )
                    .map_err(|_| corrupt("static recovery error is invalid"))?,
                },
            },
            None,
        )?;
        let changed = transaction.execute(
            "UPDATE executions SET broker_state='known_no_effect', revision=revision+1,
                 updated_at_ms=?2
             WHERE execution_id=?1 AND state='planned' AND broker_state='claimed'",
            params![execution.execution_id.as_str(), now],
        )?;
        if changed != 1 {
            return Err(corrupt(
                "claimed recovery lost its known-no-effect precondition",
            ));
        }
    }
    for execution in started {
        append_internal_task_event(
            transaction,
            &execution.task_id,
            &execution.actor_id,
            now_ms,
            TaskEvent::ExecutionUncertain {
                execution_id: execution.execution_id.clone(),
                reason: UncertaintyCode::ExecutorRestarted,
            },
            None,
        )?;
        let changed = transaction.execute(
            "UPDATE executions SET state='uncertain', revision=revision+1,
                 completed_at_ms=?2, updated_at_ms=?2
             WHERE execution_id=?1 AND state='started' AND broker_state='started'",
            params![execution.execution_id.as_str(), now],
        )?;
        if changed != 1 {
            return Err(corrupt(
                "started recovery lost its uncertainty precondition",
            ));
        }
    }
    Ok(BrokeredExecutionRecoveryReport {
        executions_known_no_effect: claimed.len() as u64,
        executions_uncertain: started.len() as u64,
    })
}

fn require_task_owner(
    transaction: &rusqlite::Connection,
    task_id: &TaskId,
    actor_id: &ActorId,
) -> Result<(), StoreError> {
    let task = load_authoritative_task(transaction, task_id)?;
    if task.owner_actor_id() == actor_id {
        Ok(())
    } else {
        Err(conflict("actor does not own the bound Task"))
    }
}

fn require_task_run(
    transaction: &rusqlite::Connection,
    task_id: &TaskId,
    run_id: &RunId,
    actor_id: &ActorId,
) -> Result<(), StoreError> {
    let task = load_authoritative_task(transaction, task_id)?;
    if task.owner_actor_id() != actor_id {
        return Err(conflict("actor does not own the bound Task"));
    }
    if task.active_run_id() != Some(run_id) {
        return Err(conflict(
            "Run is not the authoritative active Run for the bound Task",
        ));
    }
    Ok(())
}

fn load_authoritative_task(
    transaction: &rusqlite::Connection,
    task_id: &TaskId,
) -> Result<TaskAggregate, StoreError> {
    let snapshot_json = transaction
        .query_row(
            "SELECT snapshot_json FROM tasks WHERE task_id=?1",
            params![task_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or(StoreError::TaskNotFound)?;
    let snapshot = serde_json::from_str::<TaskAggregate>(&snapshot_json)
        .map_err(|error| corrupt(&format!("Task snapshot cannot be decoded: {error}")))?;
    let mut statement = transaction
        .prepare("SELECT payload_json FROM task_events WHERE task_id=?1 ORDER BY revision ASC")?;
    let payloads = statement
        .query_map(params![task_id.as_str()], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    if payloads.is_empty() {
        return Err(corrupt("Task projection has no event stream"));
    }
    let events = payloads
        .into_iter()
        .map(|payload| {
            serde_json::from_str(&payload)
                .map_err(|error| corrupt(&format!("Task event cannot be decoded: {error}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let recovered = TaskAggregate::replay(&events)?;
    if recovered != snapshot || recovered.task_id() != task_id {
        return Err(corrupt(
            "Task projection diverges from its authoritative event stream",
        ));
    }
    Ok(recovered)
}

fn require_current_lease(
    transaction: &rusqlite::Connection,
    claim: &LeaseClaim,
    actor_id: &ActorId,
    now_ms: u64,
) -> Result<(), StoreError> {
    integer(claim.generation, "lease generation")?;
    integer(claim.revision, "lease revision")?;
    let current = load_run_lease_optional(transaction, &claim.run_id)?
        .ok_or_else(|| not_found("run lease", claim.run_id.as_str()))?;
    require_task_run(transaction, &claim.task_id, &claim.run_id, actor_id)?;
    if current.task_id != claim.task_id
        || current.actor_id != *actor_id
        || current.lease_owner != claim.lease_owner
        || current.generation != claim.generation
        || current.revision != claim.revision
        || current.expires_at_ms <= now_ms
    {
        return Err(conflict("run lease fencing claim is stale or expired"));
    }
    Ok(())
}

fn require_runtime_fence(
    transaction: &rusqlite::Connection,
    actor_id: &ActorId,
    task_id: &TaskId,
    run_id: &RunId,
    fence: &RuntimeExecutionFence,
    now_ms: u64,
) -> Result<(), StoreError> {
    integer(fence.runtime_generation, "brokered Runtime generation")?;
    integer(fence.lease_generation, "brokered lease generation")?;
    integer(fence.lease_revision, "brokered lease revision")?;
    let binding = load_runtime_binding(transaction, &fence.binding_id)?;
    let lease = load_run_lease_optional(transaction, run_id)?
        .ok_or_else(|| not_found("run lease", run_id.as_str()))?;
    if binding.actor_id != *actor_id
        || binding.state != RuntimeBindingState::Active
        || binding.binding.task_id != *task_id
        || binding.binding.run_id != *run_id
        || binding.binding.runtime_generation != fence.runtime_generation
        || lease.actor_id != *actor_id
        || lease.task_id != *task_id
        || lease.run_id != *run_id
        || lease.generation != fence.lease_generation
        || lease.expires_at_ms <= now_ms
    {
        return Err(conflict(
            "brokered Runtime binding or Run lease fence is stale",
        ));
    }
    Ok(())
}

fn require_execution_runtime_context(
    connection: &rusqlite::Connection,
    command: &LedgerCommand,
    execution: &ExecutionRecord,
    lease: &LeaseClaim,
) -> Result<(), StoreError> {
    let runtime_fence = execution
        .runtime_fence
        .as_ref()
        .ok_or_else(|| conflict("brokered execution is missing a Runtime fence"))?;
    if execution.actor_id != command.actor_id
        || lease.task_id != execution.task_id
        || lease.run_id != execution.run_id
        || lease.generation != runtime_fence.lease_generation
    {
        return Err(conflict(
            "execution actor, lease, or Runtime generation differs",
        ));
    }
    require_current_lease(
        connection,
        lease,
        &command.actor_id,
        command.committed_at_ms,
    )?;
    require_runtime_fence(
        connection,
        &execution.actor_id,
        &execution.task_id,
        &execution.run_id,
        runtime_fence,
        command.committed_at_ms,
    )
}

fn validate_initial_approval(
    command: &LedgerCommand,
    approval: &ApprovalRecord,
) -> Result<(), StoreError> {
    if approval.actor_id != command.actor_id
        || approval.state != ApprovalState::Pending
        || approval.revision != 1
        || approval.decided_by_actor_id.is_some()
        || approval.created_at_ms != command.committed_at_ms
        || approval.updated_at_ms != command.committed_at_ms
        || approval.expires_at_ms <= command.committed_at_ms
    {
        return Err(conflict("invalid initial approval bindings or lifecycle"));
    }
    Ok(())
}

fn insert_approval(
    transaction: &Transaction<'_>,
    approval: &ApprovalRecord,
    committed_at_ms: u64,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO approvals(approval_id, request_id, actor_id, task_id, run_id,
         target_json, operation_digest, input_digest, state, revision, expires_at_ms,
         created_at_ms, updated_at_ms, permission_ref_json, target_identity_digest,
         runtime_fence_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', 1, ?9, ?10, ?10, ?11,
                 ?12, ?13)",
        params![
            approval.approval_id.as_str(),
            approval.request_id.as_str(),
            approval.actor_id.as_str(),
            approval.task_id.as_str(),
            approval.run_id.as_str(),
            serde_json::to_string(&approval.target)?,
            approval.operation_digest.as_str(),
            approval.input_digest.as_str(),
            integer(approval.expires_at_ms, "approval deadline")?,
            integer(committed_at_ms, "approval timestamp")?,
            approval
                .permission
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
            approval.target_identity_digest.as_ref().map(Digest::as_str),
            approval
                .runtime_fence
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
        ],
    )?;
    Ok(())
}

fn validate_permission_binding(
    approval: &ApprovalRecord,
    permission: &RuntimePermissionRef,
) -> Result<(), StoreError> {
    if permission.runtime_generation == 0
        || permission.event_sequence == 0
        || permission.request_id != approval.request_id
        || permission.run_id != approval.run_id
    {
        return Err(conflict(
            "Runtime permission does not match the approval request and Run",
        ));
    }
    integer(
        permission.runtime_generation,
        "Runtime permission generation",
    )?;
    integer(
        permission.event_sequence,
        "Runtime permission event sequence",
    )?;
    Ok(())
}

fn require_provider_permission_context(
    transaction: &Transaction<'_>,
    command: &LedgerCommand,
    approval: &ApprovalRecord,
    expected_permission: &RuntimePermissionRef,
    lease: &LeaseClaim,
    expected_task_state: TaskState,
) -> Result<(), StoreError> {
    validate_permission_binding(approval, expected_permission)?;
    if approval.actor_id != command.actor_id
        || approval.permission.as_ref() != Some(expected_permission)
        || lease.task_id != approval.task_id
        || lease.run_id != approval.run_id
        || lease.generation != expected_permission.runtime_generation
    {
        return Err(conflict(
            "provider permission actor, lease, authority, or callback binding does not match",
        ));
    }
    require_current_lease(
        transaction,
        lease,
        &command.actor_id,
        command.committed_at_ms,
    )?;
    let task = load_authoritative_task(transaction, &approval.task_id)?;
    if task.state() != expected_task_state || task.cancellation_requested() {
        return Err(conflict(
            "provider permission Task is not active in the required approval state",
        ));
    }
    let binding = load_runtime_binding(transaction, &expected_permission.binding_id)?;
    if binding.actor_id != command.actor_id
        || binding.state != RuntimeBindingState::Active
        || binding.binding.task_id != approval.task_id
        || binding.binding.run_id != approval.run_id
        || binding.binding.runtime_generation != expected_permission.runtime_generation
        || binding.last_sequence < expected_permission.event_sequence
    {
        return Err(conflict(
            "provider permission Runtime binding is stale or does not match",
        ));
    }
    Ok(())
}

fn update_provider_permission_dispatch(
    transaction: &Transaction<'_>,
    dispatch: &ProviderPermissionDispatchRecord,
    expected_revision: u64,
    expected_state: &str,
) -> Result<(), StoreError> {
    let changed = transaction.execute(
        "UPDATE provider_permission_dispatches
         SET state=?2, revision=?3, updated_at_ms=?4
         WHERE approval_id=?1 AND state=?5 AND revision=?6",
        params![
            dispatch.approval_id.as_str(),
            state_name(dispatch.state)?,
            integer(dispatch.revision, "dispatch revision")?,
            integer(dispatch.updated_at_ms, "dispatch timestamp")?,
            expected_state,
            integer(expected_revision, "dispatch expected revision")?,
        ],
    )?;
    if changed != 1 {
        return Err(conflict(
            "provider permission dispatch lost its state or revision precondition",
        ));
    }
    Ok(())
}

fn replay<T: DeserializeOwned>(
    transaction: &Transaction<'_>,
    command: &LedgerCommand,
    operation: &str,
) -> Result<Option<T>, StoreError> {
    let used_by_task = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM command_receipts
         WHERE actor_id=?1 AND idempotency_key=?2)",
        params![command.actor_id.as_str(), command.idempotency_key.as_str()],
        |row| row.get::<_, bool>(0),
    )?;
    if used_by_task {
        return Err(StoreError::IdempotencyConflict);
    }
    let row = transaction
        .query_row(
            "SELECT command_digest, operation, result_json FROM ledger_receipts
         WHERE actor_id=?1 AND idempotency_key=?2",
            params![command.actor_id.as_str(), command.idempotency_key.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((digest, stored_operation, result)) = row else {
        return Ok(None);
    };
    if digest != command.command_digest.as_str() || stored_operation != operation {
        return Err(StoreError::IdempotencyConflict);
    }
    Ok(Some(serde_json::from_str(&result)?))
}

fn insert_receipt<T: Serialize>(
    transaction: &Transaction<'_>,
    command: &LedgerCommand,
    operation: &str,
    result: &T,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO ledger_receipts(actor_id, idempotency_key, command_digest, operation,
         result_json, committed_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            command.actor_id.as_str(),
            command.idempotency_key.as_str(),
            command.command_digest.as_str(),
            operation,
            serde_json::to_string(result)?,
            integer(command.committed_at_ms, "ledger timestamp")?
        ],
    )?;
    Ok(())
}

fn load_approval(
    transaction: &rusqlite::Connection,
    id: &ApprovalId,
) -> Result<ApprovalRecord, StoreError> {
    transaction
        .query_row(
            "SELECT request_id, actor_id, task_id, run_id, target_json, operation_digest,
         input_digest, state, revision, expires_at_ms, decided_by_actor_id, created_at_ms,
         updated_at_ms, permission_ref_json, target_identity_digest, runtime_fence_json
         FROM approvals WHERE approval_id=?1",
            params![id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, Option<String>>(14)?,
                    row.get::<_, Option<String>>(15)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| not_found("approval", id.as_str()))
        .and_then(|row| {
            let record = ApprovalRecord {
                approval_id: id.clone(),
                request_id: parse_id(&row.0)?,
                actor_id: parse_id(&row.1)?,
                task_id: parse_id(&row.2)?,
                run_id: parse_id(&row.3)?,
                target: serde_json::from_str(&row.4)?,
                target_identity_digest: row
                    .14
                    .map(Digest::parse)
                    .transpose()
                    .map_err(|_| corrupt("invalid approval target identity digest"))?,
                runtime_fence: row
                    .15
                    .map(|value| serde_json::from_str(&value))
                    .transpose()?,
                operation_digest: Digest::parse(row.5)
                    .map_err(|_| corrupt("invalid approval operation digest"))?,
                input_digest: Digest::parse(row.6)
                    .map_err(|_| corrupt("invalid approval input digest"))?,
                permission: row
                    .13
                    .map(|value| serde_json::from_str(&value))
                    .transpose()?,
                state: parse_approval_state(&row.7)?,
                revision: unsigned(row.8, "approval revision")?,
                expires_at_ms: unsigned(row.9, "approval deadline")?,
                decided_by_actor_id: row.10.map(|value| parse_id(&value)).transpose()?,
                created_at_ms: unsigned(row.11, "approval creation")?,
                updated_at_ms: unsigned(row.12, "approval update")?,
            };
            if let Some(permission) = &record.permission {
                validate_permission_binding(&record, permission)?;
            }
            Ok(record)
        })
}

fn load_brokered_request(
    connection: &rusqlite::Connection,
    request_id: &RequestId,
) -> Result<BrokeredRequestRecord, StoreError> {
    let row = connection
        .query_row(
            "SELECT approval_id, actor_id, task_id, run_id, request_json, operation_json,
                    typed_operation_digest, operation_digest, input_digest,
                    target_identity_digest, runtime_fence_json, created_at_ms
             FROM brokered_requests WHERE request_id=?1",
            params![request_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, i64>(11)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| not_found("brokered request", request_id.as_str()))?;
    let request = serde_json::from_str::<CapabilityRequest>(&row.4)?;
    let operation = serde_json::from_str::<BrokeredOperation>(&row.5)?;
    let typed_operation_digest =
        Digest::parse(row.6).map_err(|_| corrupt("invalid brokered typed operation digest"))?;
    let target_identity_digest = Digest::parse(row.9)
        .map_err(|_| corrupt("invalid brokered request target identity digest"))?;
    let runtime_fence = serde_json::from_str::<RuntimeExecutionFence>(&row.10)?;
    if request.request_id != *request_id
        || request.actor.actor_id.as_str() != row.1
        || request.task_id.as_str() != row.2
        || request.run_id.as_str() != row.3
        || request.operation_digest.as_str() != row.7
        || request.input_digest.as_str() != row.8
        || brokered_operation_digest(&operation)? != typed_operation_digest
    {
        return Err(corrupt(
            "brokered request columns diverge from the typed request",
        ));
    }
    Ok(BrokeredRequestRecord {
        request,
        operation,
        typed_operation_digest,
        target_identity_digest,
        runtime_fence,
        approval_id: row.0.map(|value| parse_id(&value)).transpose()?,
        created_at_ms: unsigned(row.11, "brokered request creation")?,
    })
}

fn insert_brokered_request(
    transaction: &Transaction<'_>,
    record: &BrokeredRequestRecord,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO brokered_requests(
             request_id, approval_id, actor_id, task_id, run_id, request_json,
             operation_json, typed_operation_digest, operation_digest, input_digest,
             target_identity_digest, runtime_fence_json, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            record.request.request_id.as_str(),
            record.approval_id.as_ref().map(ApprovalId::as_str),
            record.request.actor.actor_id.as_str(),
            record.request.task_id.as_str(),
            record.request.run_id.as_str(),
            serde_json::to_string(&record.request)?,
            serde_json::to_string(&record.operation)?,
            record.typed_operation_digest.as_str(),
            record.request.operation_digest.as_str(),
            record.request.input_digest.as_str(),
            record.target_identity_digest.as_str(),
            serde_json::to_string(&record.runtime_fence)?,
            integer(record.created_at_ms, "brokered request timestamp")?,
        ],
    )?;
    Ok(())
}

fn brokered_operation_digest(operation: &BrokeredOperation) -> Result<Digest, StoreError> {
    let encoded = serde_json::to_vec(operation)?;
    let mut hasher = Sha256::new();
    hasher.update(b"cosh.gateway.brokered-operation.v1\0");
    hasher.update(encoded);
    Digest::parse(format!("{:x}", hasher.finalize()))
        .map_err(|_| corrupt("brokered operation digest construction failed"))
}

fn brokered_result_digest(result: &BrokeredOperationResult) -> Result<Digest, StoreError> {
    let encoded = serde_json::to_vec(result)?;
    let mut hasher = Sha256::new();
    hasher.update(b"cosh.gateway.brokered-result.v1\0");
    hasher.update(encoded);
    Digest::parse(format!("{:x}", hasher.finalize()))
        .map_err(|_| corrupt("brokered result digest construction failed"))
}

fn brokered_delivery_digest(delivery: &BrokeredExecutionDelivery) -> Result<Digest, StoreError> {
    let encoded = serde_json::to_vec(delivery)?;
    let mut hasher = Sha256::new();
    hasher.update(encoded);
    Digest::parse(format!("{:x}", hasher.finalize()))
        .map_err(|_| corrupt("brokered delivery digest construction failed"))
}

fn validate_result_shape(
    operation: &BrokeredOperation,
    result: &BrokeredOperationResult,
) -> Result<(), StoreError> {
    match (operation, result) {
        (
            BrokeredOperation::WorkspaceCheckpointCreateV1(operation),
            BrokeredOperationResult::WorkspaceCheckpointCreateV1(result),
        ) if operation.checkpoint_id == result.checkpoint_id => Ok(()),
        _ => Err(conflict(
            "typed result does not match the admitted brokered operation",
        )),
    }
}

fn execution_request_id(
    connection: &rusqlite::Connection,
    execution_id: &ExecutionId,
) -> Result<RequestId, StoreError> {
    let request_id = connection
        .query_row(
            "SELECT request_id FROM permits WHERE execution_id=?1",
            params![execution_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| not_found("execution permit", execution_id.as_str()))?;
    parse_id(&request_id)
}

fn validate_completion_result(
    connection: &rusqlite::Connection,
    execution: &ExecutionRecord,
    result: &BrokeredOperationResult,
    command: &LedgerCommand,
) -> Result<BrokeredExecutionResultRecord, StoreError> {
    let request_id = execution_request_id(connection, &execution.execution_id)?;
    let request = load_brokered_request(connection, &request_id)?;
    let target_identity_digest = execution
        .target_identity_digest
        .as_ref()
        .ok_or_else(|| corrupt("successful brokered execution lacks target identity"))?;
    let runtime_fence = execution
        .runtime_fence
        .as_ref()
        .ok_or_else(|| corrupt("successful brokered execution lacks Runtime fence"))?;
    if execution.broker_state != Some(BrokerExecutionState::Started)
        || request.request.actor.actor_id != execution.actor_id
        || request.request.task_id != execution.task_id
        || request.request.run_id != execution.run_id
        || request.request.target != execution.target
        || request.request.operation_digest != execution.operation_digest
        || request.request.input_digest != execution.input_digest
        || request.target_identity_digest != *target_identity_digest
        || request.runtime_fence != *runtime_fence
    {
        return Err(corrupt(
            "brokered execution authority diverges from its durable request",
        ));
    }
    validate_result_shape(&request.operation, result)?;
    Ok(BrokeredExecutionResultRecord {
        execution_id: execution.execution_id.clone(),
        request_id,
        actor_id: execution.actor_id.clone(),
        task_id: execution.task_id.clone(),
        run_id: execution.run_id.clone(),
        result: result.clone(),
        result_digest: brokered_result_digest(result)?,
        operation: request.operation,
        operation_digest: execution.operation_digest.clone(),
        input_digest: execution.input_digest.clone(),
        target_identity_digest: target_identity_digest.clone(),
        runtime_fence: runtime_fence.clone(),
        committed_at_ms: command.committed_at_ms,
    })
}

fn insert_brokered_execution_result(
    transaction: &Transaction<'_>,
    record: &BrokeredExecutionResultRecord,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO brokered_execution_results(
             execution_id, request_id, actor_id, task_id, run_id, result_json, result_digest,
             operation_json, operation_digest, input_digest, target_identity_digest,
             runtime_fence_json, committed_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            record.execution_id.as_str(),
            record.request_id.as_str(),
            record.actor_id.as_str(),
            record.task_id.as_str(),
            record.run_id.as_str(),
            serde_json::to_string(&record.result)?,
            record.result_digest.as_str(),
            serde_json::to_string(&record.operation)?,
            record.operation_digest.as_str(),
            record.input_digest.as_str(),
            record.target_identity_digest.as_str(),
            serde_json::to_string(&record.runtime_fence)?,
            integer(record.committed_at_ms, "brokered result timestamp")?,
        ],
    )?;
    Ok(())
}

fn load_brokered_runtime_dispatch(
    connection: &rusqlite::Connection,
    request_id: &RequestId,
    kind: BrokeredRuntimeDispatchKind,
) -> Result<BrokeredRuntimeDispatchRecord, StoreError> {
    let row = connection
        .query_row(
            "SELECT actor_id, task_id, run_id, brokered_ref_json, payload_digest,
                    source_kind, source_id, state, revision, created_at_ms, updated_at_ms
             FROM brokered_runtime_dispatches
             WHERE request_id=?1 AND dispatch_kind=?2",
            params![request_id.as_str(), state_name(kind)?],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, i64>(10)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| not_found("brokered Runtime dispatch", request_id.as_str()))?;
    let brokered = serde_json::from_str::<BrokeredExecutionRef>(&row.3)?;
    let source_id = &row.6;
    let source = match row.5.as_str() {
        "approval_pending" => BrokeredRuntimeDispatchSource::ApprovalPending {
            approval_id: parse_id(source_id)?,
        },
        "approval_denied" => BrokeredRuntimeDispatchSource::ApprovalDenied {
            approval_id: parse_id(source_id)?,
        },
        "execution" => BrokeredRuntimeDispatchSource::Execution {
            execution_id: parse_id(source_id)?,
        },
        _ => return Err(corrupt("invalid brokered Runtime dispatch source")),
    };
    let record = BrokeredRuntimeDispatchRecord {
        brokered,
        actor_id: parse_id(&row.0)?,
        task_id: parse_id(&row.1)?,
        kind,
        payload_digest: Digest::parse(row.4)
            .map_err(|_| corrupt("invalid brokered Runtime dispatch payload digest"))?,
        source,
        state: parse_state(&row.7)?,
        revision: unsigned(row.8, "brokered dispatch revision")?,
        created_at_ms: unsigned(row.9, "brokered dispatch creation")?,
        updated_at_ms: unsigned(row.10, "brokered dispatch update")?,
    };
    if record.brokered.request_id != *request_id
        || record.brokered.run_id.as_str() != row.2
        || brokered_dispatch_source_columns(&record.source) != (row.5.as_str(), row.6.as_str())
    {
        return Err(corrupt(
            "brokered Runtime dispatch columns diverge from its typed binding",
        ));
    }
    Ok(record)
}

fn load_provider_permission_dispatch(
    transaction: &rusqlite::Connection,
    approval_id: &ApprovalId,
) -> Result<ProviderPermissionDispatchRecord, StoreError> {
    transaction
        .query_row(
            "SELECT actor_id, task_id, run_id, permission_ref_json, decision, state,
             revision, created_at_ms, updated_at_ms
             FROM provider_permission_dispatches WHERE approval_id=?1",
            params![approval_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| not_found("provider permission dispatch", approval_id.as_str()))
        .and_then(|row| {
            let permission = serde_json::from_str::<RuntimePermissionRef>(&row.3)?;
            let record = ProviderPermissionDispatchRecord {
                approval_id: approval_id.clone(),
                actor_id: parse_id(&row.0)?,
                task_id: parse_id(&row.1)?,
                run_id: parse_id(&row.2)?,
                permission,
                decision: parse_state(&row.4)?,
                state: parse_state(&row.5)?,
                revision: unsigned(row.6, "dispatch revision")?,
                created_at_ms: unsigned(row.7, "dispatch creation")?,
                updated_at_ms: unsigned(row.8, "dispatch update")?,
            };
            if record.permission.request_id != load_approval(transaction, approval_id)?.request_id
                || record.permission.run_id != record.run_id
            {
                return Err(corrupt(
                    "provider permission dispatch diverges from its approval binding",
                ));
            }
            Ok(record)
        })
}

fn load_permit(
    transaction: &rusqlite::Connection,
    id: &PermitId,
) -> Result<PermitRecord, StoreError> {
    let row = transaction
        .query_row(
            "SELECT request_id, approval_id, actor_id, task_id, run_id, execution_id, target_json,
         operation_digest, input_digest, policy_revision, state, single_use, valid_until_ms,
         consumed_at_ms, created_at_ms, target_identity_digest, runtime_fence_json
         FROM permits WHERE permit_id=?1",
            params![id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, Option<i64>>(13)?,
                    row.get::<_, i64>(14)?,
                    row.get::<_, Option<String>>(15)?,
                    row.get::<_, Option<String>>(16)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| not_found("permit", id.as_str()))?;
    if row.11 != 1 {
        return Err(corrupt("durable permit is not single-use"));
    }
    let target_identity_digest = row
        .15
        .ok_or_else(|| corrupt("brokered permit is missing target identity"))
        .and_then(|value| {
            Digest::parse(value).map_err(|_| corrupt("invalid permit target identity digest"))
        })?;
    let runtime_fence = row
        .16
        .ok_or_else(|| corrupt("brokered permit is missing Runtime fence"))
        .and_then(|value| Ok(serde_json::from_str(&value)?))?;
    Ok(PermitRecord {
        permit: ExecutionPermit {
            permit_id: id.clone(),
            request_id: parse_id(&row.0)?,
            approval_id: row.1.map(|value| parse_id(&value)).transpose()?,
            actor_id: parse_id(&row.2)?,
            task_id: parse_id(&row.3)?,
            run_id: parse_id(&row.4)?,
            execution_id: parse_id(&row.5)?,
            target: serde_json::from_str(&row.6)?,
            target_identity_digest,
            runtime_fence,
            operation_digest: Digest::parse(row.7)
                .map_err(|_| corrupt("invalid permit operation digest"))?,
            input_digest: Digest::parse(row.8)
                .map_err(|_| corrupt("invalid permit input digest"))?,
            policy_revision: unsigned(row.9, "policy revision")?,
            valid_until_ms: unsigned(row.12, "permit deadline")?,
            single_use: true,
        },
        state: parse_permit_state(&row.10)?,
        consumed_at_ms: row
            .13
            .map(|value| unsigned(value, "permit consumption"))
            .transpose()?,
        created_at_ms: unsigned(row.14, "permit creation")?,
    })
}

fn load_brokered_execution_result(
    connection: &rusqlite::Connection,
    execution_id: &ExecutionId,
) -> Result<BrokeredExecutionResultRecord, StoreError> {
    let execution = load_execution(connection, execution_id)?;
    match execution.typed_result_state {
        TypedExecutionResultState::LegacyUnavailable => {
            return Err(StoreError::LegacyBrokeredResultUnavailable {
                execution_id: execution_id.as_str().to_owned(),
            });
        }
        TypedExecutionResultState::NotApplicable => {
            return Err(not_found(
                "brokered execution result",
                execution_id.as_str(),
            ));
        }
        TypedExecutionResultState::Available => {}
    }
    let row = connection
        .query_row(
            "SELECT request_id, actor_id, task_id, run_id, result_json, result_digest,
                    operation_json, operation_digest, input_digest, target_identity_digest,
                    runtime_fence_json, committed_at_ms
             FROM brokered_execution_results WHERE execution_id=?1",
            params![execution_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, i64>(11)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| corrupt("available brokered result payload is missing"))?;
    let record = BrokeredExecutionResultRecord {
        execution_id: execution_id.clone(),
        request_id: parse_id(&row.0)?,
        actor_id: parse_id(&row.1)?,
        task_id: parse_id(&row.2)?,
        run_id: parse_id(&row.3)?,
        result: serde_json::from_str(&row.4)?,
        result_digest: Digest::parse(row.5)
            .map_err(|_| corrupt("invalid brokered result digest"))?,
        operation: serde_json::from_str(&row.6)?,
        operation_digest: Digest::parse(row.7)
            .map_err(|_| corrupt("invalid brokered result operation digest"))?,
        input_digest: Digest::parse(row.8)
            .map_err(|_| corrupt("invalid brokered result input digest"))?,
        target_identity_digest: Digest::parse(row.9)
            .map_err(|_| corrupt("invalid brokered result target identity digest"))?,
        runtime_fence: serde_json::from_str(&row.10)?,
        committed_at_ms: unsigned(row.11, "brokered result timestamp")?,
    };
    let request_id = execution_request_id(connection, execution_id)?;
    let request = load_brokered_request(connection, &request_id)?;
    if execution.state != ExecutionState::Succeeded
        || record.request_id != request_id
        || record.actor_id != execution.actor_id
        || record.task_id != execution.task_id
        || record.run_id != execution.run_id
        || record.operation != request.operation
        || record.operation_digest != execution.operation_digest
        || record.input_digest != execution.input_digest
        || Some(&record.target_identity_digest) != execution.target_identity_digest.as_ref()
        || Some(&record.runtime_fence) != execution.runtime_fence.as_ref()
        || record.result_digest != brokered_result_digest(&record.result)?
        || record.committed_at_ms != execution.completed_at_ms.unwrap_or_default()
        || validate_result_shape(&record.operation, &record.result).is_err()
    {
        return Err(corrupt(
            "brokered result payload diverges from its execution authority",
        ));
    }
    Ok(record)
}

fn load_execution(
    transaction: &rusqlite::Connection,
    id: &ExecutionId,
) -> Result<ExecutionRecord, StoreError> {
    let row = transaction
        .query_row(
            "SELECT actor_id, task_id, run_id, target_json, operation_digest, input_digest, state,
         revision, started_at_ms, completed_at_ms, created_at_ms, updated_at_ms,
         target_identity_digest, runtime_fence_json, broker_state, claimed_at_ms,
         start_audit_proof_digest, typed_result_state
         FROM executions WHERE execution_id=?1",
            params![id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, Option<String>>(14)?,
                    row.get::<_, Option<i64>>(15)?,
                    row.get::<_, Option<String>>(16)?,
                    row.get::<_, Option<String>>(17)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| not_found("execution", id.as_str()))?;
    let record = ExecutionRecord {
        execution_id: id.clone(),
        actor_id: parse_id(&row.0)?,
        task_id: parse_id(&row.1)?,
        run_id: parse_id(&row.2)?,
        target: serde_json::from_str(&row.3)?,
        target_identity_digest: row
            .12
            .map(Digest::parse)
            .transpose()
            .map_err(|_| corrupt("invalid execution target identity digest"))?,
        runtime_fence: row
            .13
            .map(|value| serde_json::from_str(&value))
            .transpose()?,
        broker_state: row.14.map(|value| parse_state(&value)).transpose()?,
        claimed_at_ms: row
            .15
            .map(|value| unsigned(value, "execution claim"))
            .transpose()?,
        start_audit_proof_digest: row
            .16
            .map(Digest::parse)
            .transpose()
            .map_err(|_| corrupt("invalid execution start audit proof digest"))?,
        typed_result_state: row
            .17
            .as_deref()
            .ok_or_else(|| corrupt("execution is missing typed result state"))
            .and_then(parse_typed_result_state)?,
        operation_digest: Digest::parse(row.4)
            .map_err(|_| corrupt("invalid execution operation digest"))?,
        input_digest: Digest::parse(row.5)
            .map_err(|_| corrupt("invalid execution input digest"))?,
        state: parse_execution_state(&row.6)?,
        revision: unsigned(row.7, "execution revision")?,
        started_at_ms: row
            .8
            .map(|value| unsigned(value, "execution start"))
            .transpose()?,
        completed_at_ms: row
            .9
            .map(|value| unsigned(value, "execution completion"))
            .transpose()?,
        created_at_ms: unsigned(row.10, "execution creation")?,
        updated_at_ms: unsigned(row.11, "execution update")?,
    };
    validate_execution_receipt(transaction, &record)?;
    Ok(record)
}

fn validate_execution_receipt(
    transaction: &rusqlite::Connection,
    execution: &ExecutionRecord,
) -> Result<(), StoreError> {
    let receipt = transaction
        .query_row(
            "SELECT state FROM execution_receipts WHERE execution_id=?1",
            params![execution.execution_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let expected = match execution.state {
        ExecutionState::Succeeded => Some("succeeded"),
        ExecutionState::Failed => Some("failed"),
        ExecutionState::Planned | ExecutionState::Started | ExecutionState::Uncertain => None,
    };
    if receipt.as_deref() != expected {
        return Err(corrupt(
            "execution terminal state and durable receipt are inconsistent",
        ));
    }
    let has_typed_result: bool = transaction.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM brokered_execution_results WHERE execution_id=?1
         )",
        params![execution.execution_id.as_str()],
        |row| row.get(0),
    )?;
    let valid_result_state = match execution.typed_result_state {
        TypedExecutionResultState::Available => {
            execution.state == ExecutionState::Succeeded && has_typed_result
        }
        TypedExecutionResultState::LegacyUnavailable => {
            execution.state == ExecutionState::Succeeded && !has_typed_result
        }
        TypedExecutionResultState::NotApplicable => {
            execution.state != ExecutionState::Succeeded && !has_typed_result
        }
    };
    if !valid_result_state {
        return Err(corrupt(
            "execution typed result state and durable payload are inconsistent",
        ));
    }
    let audit_proof = transaction
        .query_row(
            "SELECT proof_digest FROM security_audit_proofs WHERE execution_id=?1",
            params![execution.execution_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    match execution.broker_state {
        None => {
            if execution.target_identity_digest.is_some()
                || execution.runtime_fence.is_some()
                || execution.claimed_at_ms.is_some()
                || execution.start_audit_proof_digest.is_some()
                || audit_proof.is_some()
            {
                return Err(corrupt("legacy execution has partial brokered authority"));
            }
        }
        Some(BrokerExecutionState::Planned) => {
            if execution.state != ExecutionState::Planned
                || execution.claimed_at_ms.is_some()
                || execution.start_audit_proof_digest.is_some()
                || audit_proof.is_some()
            {
                return Err(corrupt("planned brokered execution has started evidence"));
            }
        }
        Some(BrokerExecutionState::Claimed) => {
            if execution.state != ExecutionState::Planned
                || execution.claimed_at_ms.is_none()
                || execution.start_audit_proof_digest.is_some()
                || audit_proof.is_some()
            {
                return Err(corrupt(
                    "claimed brokered execution has invalid effect evidence",
                ));
            }
        }
        Some(BrokerExecutionState::Started) => {
            if execution.state == ExecutionState::Planned
                || execution.claimed_at_ms.is_none()
                || execution
                    .start_audit_proof_digest
                    .as_ref()
                    .map(Digest::as_str)
                    != audit_proof.as_deref()
            {
                return Err(corrupt(
                    "started brokered execution lacks exact audit proof",
                ));
            }
        }
        Some(BrokerExecutionState::KnownNoEffect) => {
            if execution.state != ExecutionState::Planned
                || execution.claimed_at_ms.is_none()
                || execution.start_audit_proof_digest.is_some()
                || audit_proof.is_some()
            {
                return Err(corrupt("known-no-effect execution has effect evidence"));
            }
        }
    }
    if execution.broker_state.is_some()
        && (execution.target_identity_digest.is_none() || execution.runtime_fence.is_none())
    {
        return Err(corrupt("brokered execution is missing immutable authority"));
    }
    Ok(())
}

fn validate_all_execution_receipts(transaction: &rusqlite::Connection) -> Result<(), StoreError> {
    let inconsistent: bool = transaction.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM executions e
             LEFT JOIN execution_receipts r ON r.execution_id=e.execution_id
             WHERE (e.state='succeeded' AND (r.state IS NULL OR r.state!='succeeded'))
                OR (e.state='failed' AND (r.state IS NULL OR r.state!='failed'))
                OR (e.state NOT IN ('succeeded', 'failed') AND r.state IS NOT NULL)
                OR (e.state='succeeded' AND e.typed_result_state NOT IN
                    ('available', 'legacy_unavailable'))
                OR (e.state!='succeeded' AND e.typed_result_state!='not_applicable')
                OR (e.typed_result_state='available' AND NOT EXISTS (
                    SELECT 1 FROM brokered_execution_results b
                    WHERE b.execution_id=e.execution_id
                ))
                OR (e.typed_result_state!='available' AND EXISTS (
                    SELECT 1 FROM brokered_execution_results b
                    WHERE b.execution_id=e.execution_id
                ))
         )",
        [],
        |row| row.get(0),
    )?;
    if inconsistent {
        return Err(corrupt(
            "execution ledger contains a terminal receipt inconsistency",
        ));
    }
    Ok(())
}

fn load_runtime_input_request(
    transaction: &rusqlite::Connection,
    request_id: &InputRequestId,
) -> Result<RuntimeInputRequestRecord, StoreError> {
    let row = transaction
        .query_row(
            "SELECT actor_id, task_id, run_id, binding_id, runtime_instance_id,
                    runtime_generation, runtime_sequence, lease_generation, lease_revision,
                    request_json, state, response_digest, revision, expires_at_ms,
                    created_at_ms, updated_at_ms
             FROM runtime_input_requests WHERE request_id=?1",
            params![request_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, i64>(13)?,
                    row.get::<_, i64>(14)?,
                    row.get::<_, i64>(15)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| not_found("runtime input request", request_id.as_str()))?;
    let request = serde_json::from_str::<RuntimeInputRequest>(&row.9)?;
    let task_id = parse_id::<TaskId>(&row.1)?;
    let run_id = parse_id::<RunId>(&row.2)?;
    if request.request_id() != request_id || request.run_id() != &run_id {
        return Err(corrupt(
            "runtime input request columns diverge from its typed payload",
        ));
    }
    let state = parse_state::<RuntimeInputRequestState>(&row.10)?;
    let response_digest = row
        .11
        .as_deref()
        .map(|value| {
            Digest::parse(value.to_owned())
                .map_err(|error| corrupt(&format!("invalid response digest: {error}")))
        })
        .transpose()?;
    if (state == RuntimeInputRequestState::Resolved) != response_digest.is_some() {
        return Err(corrupt(
            "runtime input request response digest diverges from its state",
        ));
    }
    Ok(RuntimeInputRequestRecord {
        request,
        actor_id: parse_id(&row.0)?,
        task_id,
        run_id,
        binding_id: parse_id(&row.3)?,
        runtime_instance_id: parse_id(&row.4)?,
        runtime_generation: unsigned(row.5, "runtime input generation")?,
        runtime_sequence: unsigned(row.6, "runtime input sequence")?,
        lease_generation: unsigned(row.7, "runtime input lease generation")?,
        lease_revision: unsigned(row.8, "runtime input lease revision")?,
        state,
        response_digest,
        revision: unsigned(row.12, "runtime input request revision")?,
        expires_at_ms: unsigned(row.13, "runtime input deadline")?,
        created_at_ms: unsigned(row.14, "runtime input creation")?,
        updated_at_ms: unsigned(row.15, "runtime input update")?,
    })
}

fn load_runtime_input_dispatch(
    transaction: &rusqlite::Connection,
    request_id: &InputRequestId,
) -> Result<RuntimeInputDispatchRecord, StoreError> {
    let row = transaction
        .query_row(
            "SELECT actor_id, task_id, run_id, response_json, response_digest,
                    state, revision, created_at_ms, updated_at_ms
             FROM runtime_input_dispatches WHERE request_id=?1",
            params![request_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| not_found("runtime input dispatch", request_id.as_str()))?;
    let response = serde_json::from_str::<RuntimeInputResponse>(&row.3)?;
    let response_digest = Digest::parse(row.4)
        .map_err(|error| corrupt(&format!("invalid response digest: {error}")))?;
    if runtime_input_response_digest(&response)? != response_digest {
        return Err(corrupt(
            "runtime input dispatch payload diverges from its digest",
        ));
    }
    Ok(RuntimeInputDispatchRecord {
        request_id: request_id.clone(),
        actor_id: parse_id(&row.0)?,
        task_id: parse_id(&row.1)?,
        run_id: parse_id(&row.2)?,
        response,
        response_digest,
        state: parse_state(&row.5)?,
        revision: unsigned(row.6, "runtime input dispatch revision")?,
        created_at_ms: unsigned(row.7, "runtime input dispatch creation")?,
        updated_at_ms: unsigned(row.8, "runtime input dispatch update")?,
    })
}

fn load_runtime_binding(
    transaction: &rusqlite::Connection,
    id: &RuntimeBindingId,
) -> Result<RuntimeBindingRecord, StoreError> {
    let row = transaction
        .query_row(
            "SELECT actor_id, task_id, run_id, runtime_instance_id, runtime_generation,
         binding_json, state, last_sequence, created_at_ms, updated_at_ms
         FROM runtime_bindings WHERE binding_id=?1",
            params![id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| not_found("runtime binding", id.as_str()))?;
    let binding = serde_json::from_str::<RuntimeBindingRef>(&row.5)?;
    if binding.binding_id != *id
        || binding.task_id.as_str() != row.1
        || binding.run_id.as_str() != row.2
        || binding.runtime_instance_id.as_str() != row.3
        || binding.runtime_generation != unsigned(row.4, "runtime generation")?
    {
        return Err(corrupt(
            "runtime binding columns diverge from the versioned binding contract",
        ));
    }
    Ok(RuntimeBindingRecord {
        binding,
        actor_id: parse_id(&row.0)?,
        state: parse_runtime_state(&row.6)?,
        last_sequence: unsigned(row.7, "runtime sequence")?,
        created_at_ms: unsigned(row.8, "runtime binding creation")?,
        updated_at_ms: unsigned(row.9, "runtime binding update")?,
    })
}

fn load_run_lease_optional(
    transaction: &rusqlite::Connection,
    id: &RunId,
) -> Result<Option<RunLeaseRecord>, StoreError> {
    let row = transaction.query_row(
        "SELECT task_id, actor_id, lease_owner, generation, revision, expires_at_ms, updated_at_ms
         FROM run_leases WHERE run_id=?1", params![id.as_str()],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?, row.get::<_, i64>(4)?, row.get::<_, i64>(5)?,
            row.get::<_, i64>(6)?)),
    ).optional()?;
    row.map(|row| {
        Ok(RunLeaseRecord {
            task_id: parse_id(&row.0)?,
            run_id: id.clone(),
            actor_id: parse_id(&row.1)?,
            lease_owner: BoundedOpaque::new(row.2).map_err(|_| corrupt("invalid lease owner"))?,
            generation: unsigned(row.3, "lease generation")?,
            revision: unsigned(row.4, "lease revision")?,
            expires_at_ms: unsigned(row.5, "lease deadline")?,
            updated_at_ms: unsigned(row.6, "lease update")?,
        })
    })
    .transpose()
}

fn state_name<T: Serialize>(state: T) -> Result<String, StoreError> {
    serde_json::to_value(state)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| corrupt("ledger state is not serialized as a string"))
}

fn validate_command(command: &LedgerCommand) -> Result<(), StoreError> {
    integer(command.committed_at_ms, "ledger timestamp")?;
    Ok(())
}

fn next_integer(value: u64, field: &str) -> Result<u64, StoreError> {
    let next = value
        .checked_add(1)
        .ok_or_else(|| conflict(&format!("{field} overflow")))?;
    integer(next, field)?;
    Ok(next)
}

fn require_not_before(now_ms: u64, previous_ms: u64, operation: &str) -> Result<(), StoreError> {
    if now_ms < previous_ms {
        Err(conflict(&format!(
            "{operation} timestamp precedes the durable entity timestamp",
        )))
    } else {
        Ok(())
    }
}

fn parse_approval_state(value: &str) -> Result<ApprovalState, StoreError> {
    parse_state(value)
}
fn parse_permit_state(value: &str) -> Result<PermitState, StoreError> {
    parse_state(value)
}
fn parse_execution_state(value: &str) -> Result<ExecutionState, StoreError> {
    parse_state(value)
}
fn parse_typed_result_state(value: &str) -> Result<TypedExecutionResultState, StoreError> {
    parse_state(value)
}
fn parse_runtime_state(value: &str) -> Result<RuntimeBindingState, StoreError> {
    parse_state(value)
}

fn parse_state<T: DeserializeOwned>(value: &str) -> Result<T, StoreError> {
    serde_json::from_value(serde_json::Value::String(value.to_owned())).map_err(StoreError::from)
}

fn parse_id<T: std::str::FromStr>(value: &str) -> Result<T, StoreError>
where
    T::Err: std::fmt::Display,
{
    value
        .parse()
        .map_err(|error| corrupt(&format!("invalid ledger identity: {error}")))
}

fn integer(value: u64, field: &str) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| conflict(&format!("{field} exceeds SQLite INTEGER range")))
}

fn unsigned(value: i64, field: &str) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| corrupt(&format!("negative {field}")))
}

fn conflict(message: &str) -> StoreError {
    StoreError::LedgerConflict {
        message: message.to_owned(),
    }
}

fn corrupt(message: &str) -> StoreError {
    StoreError::Corrupt {
        message: message.to_owned(),
    }
}

fn not_found(entity: &str, id: &str) -> StoreError {
    StoreError::LedgerNotFound {
        entity: format!("{entity} {id}"),
    }
}

#[cfg(test)]
mod tests;
