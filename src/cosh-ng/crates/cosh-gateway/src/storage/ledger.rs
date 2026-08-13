//! Durable approval, permit, execution, runtime binding, and run lease ledger.

use cosh_gateway_contracts::capability::{ApprovalDecision, ExecutionPermit};
use cosh_gateway_contracts::common::RuntimeBindingRef;
use cosh_gateway_contracts::common::{
    BoundedOpaque, BoundedText, Digest, IdempotencyKey, TargetRef,
};
use cosh_gateway_contracts::ids::{
    ActorId, ApprovalId, ExecutionId, PermitId, RequestId, RunId, RuntimeBindingId,
    RuntimeInstanceId, TaskId,
};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::task::TaskAggregate;

use super::{SqliteTaskStore, StoreError};

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
    /// Bound normalized operation digest.
    pub operation_digest: Digest,
    /// Bound complete Runtime input digest.
    pub input_digest: Digest,
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
    /// Exact normalized operation digest.
    pub operation_digest: Digest,
    /// Exact complete Runtime input digest.
    pub input_digest: Digest,
    /// Policy revision expected by the executor.
    pub policy_revision: u64,
    /// Current coordinator lease fencing the owning Task and Run.
    pub lease: LeaseClaim,
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
    /// Issued permits expired by their deadline.
    pub permits_expired: u64,
    /// Started executions marked uncertain.
    pub executions_uncertain: u64,
    /// Active runtime bindings fenced as lost.
    pub runtime_bindings_lost: u64,
}

impl SqliteTaskStore {
    /// Loads one durable approval record.
    pub fn load_approval_record(
        &self,
        approval_id: &ApprovalId,
    ) -> Result<ApprovalRecord, StoreError> {
        load_approval(self.connection(), approval_id)
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

    /// Creates a pending approval bound to an actor, Task, Run, target, and digests.
    pub fn create_approval(
        &mut self,
        command: &LedgerCommand,
        approval: &ApprovalRecord,
    ) -> Result<LedgerOutcome<ApprovalRecord>, StoreError> {
        validate_command(command)?;
        integer(approval.expires_at_ms, "approval deadline")?;
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
        transaction.execute(
            "INSERT INTO approvals(approval_id, request_id, actor_id, task_id, run_id,
             target_json, operation_digest, input_digest, state, revision, expires_at_ms,
             created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', 1, ?9, ?10, ?10)",
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
                integer(command.committed_at_ms, "approval timestamp")?,
            ],
        )?;
        insert_receipt(&transaction, command, "create_approval", approval)?;
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
        insert_receipt(&transaction, command, "resolve_approval", &record)?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(record))
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
        if permit.actor_id != command.actor_id
            || !permit.single_use
            || permit.valid_until_ms <= command.committed_at_ms
        {
            return Err(conflict(
                "permit actor, single-use flag, or deadline is invalid",
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
        let target_json = serde_json::to_string(&permit.target)?;
        let now = integer(command.committed_at_ms, "permit timestamp")?;
        transaction.execute(
            "INSERT INTO executions(execution_id, actor_id, task_id, run_id, target_json,
             operation_digest, input_digest, state, revision, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'planned', 1, ?8, ?8)",
            params![
                permit.execution_id.as_str(),
                permit.actor_id.as_str(),
                permit.task_id.as_str(),
                permit.run_id.as_str(),
                target_json,
                permit.operation_digest.as_str(),
                permit.input_digest.as_str(),
                now
            ],
        )?;
        transaction.execute(
            "INSERT INTO permits(permit_id, request_id, approval_id, actor_id, task_id, run_id,
             execution_id, target_json, operation_digest, input_digest, policy_revision, state,
             single_use, valid_until_ms, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'issued', 1, ?12, ?13)",
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
                now
            ],
        )?;
        let record = PermitRecord {
            permit: permit.clone(),
            state: PermitState::Issued,
            consumed_at_ms: None,
            created_at_ms: command.committed_at_ms,
        };
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

    /// Consumes one exact permit and marks its execution started in one transaction.
    pub fn consume_permit_and_start_execution(
        &mut self,
        command: &LedgerCommand,
        claim: &ExecutionClaim,
    ) -> Result<LedgerOutcome<ExecutionRecord>, StoreError> {
        validate_command(command)?;
        let transaction = immediate(self)?;
        if let Some(replayed) = replay(&transaction, command, "consume_permit")? {
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
            || permit.permit.operation_digest != claim.operation_digest
            || permit.permit.input_digest != claim.input_digest
            || permit.permit.policy_revision != claim.policy_revision
        {
            return Err(conflict(
                "execution claim does not exactly match an issued permit",
            ));
        }
        if command.committed_at_ms >= permit.permit.valid_until_ms {
            return Err(conflict("permit expired before execution start"));
        }
        require_not_before(
            command.committed_at_ms,
            permit.created_at_ms,
            "execution start",
        )?;
        let now = integer(command.committed_at_ms, "execution start timestamp")?;
        let changed = transaction.execute(
            "UPDATE permits SET state = 'consumed', consumed_at_ms = ?2
             WHERE permit_id = ?1 AND state = 'issued' AND consumed_at_ms IS NULL",
            params![claim.permit_id.as_str(), now],
        )?;
        let started = transaction.execute(
            "UPDATE executions SET state = 'started', revision = 2, started_at_ms = ?2,
             updated_at_ms = ?2 WHERE execution_id = ?1 AND state = 'planned' AND revision = 1",
            params![claim.execution_id.as_str(), now],
        )?;
        if changed != 1 || started != 1 {
            return Err(conflict(
                "permit consumption or execution start lost its precondition",
            ));
        }
        let record = load_execution(&transaction, &claim.execution_id)?;
        insert_receipt(&transaction, command, "consume_permit", &record)?;
        transaction.commit()?;
        Ok(LedgerOutcome::Applied(record))
    }

    /// Commits a conclusive receipt for a started execution.
    pub fn complete_execution(
        &mut self,
        command: &LedgerCommand,
        completion: &ExecutionCompletion,
    ) -> Result<LedgerOutcome<ExecutionRecord>, StoreError> {
        validate_command(command)?;
        integer(completion.expected_revision, "execution expected revision")?;
        let transaction = immediate(self)?;
        if let Some(replayed) = replay(&transaction, command, "complete_execution")? {
            transaction.commit()?;
            return Ok(LedgerOutcome::Replayed(replayed));
        }
        let mut record = load_execution(&transaction, &completion.execution_id)?;
        require_task_owner(&transaction, &record.task_id, &command.actor_id)?;
        if record.actor_id != command.actor_id
            || record.state != ExecutionState::Started
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
        let next_revision = next_integer(record.revision, "execution revision")?;
        record.state = if completion.succeeded {
            ExecutionState::Succeeded
        } else {
            ExecutionState::Failed
        };
        record.revision = next_revision;
        record.completed_at_ms = Some(command.committed_at_ms);
        record.updated_at_ms = command.committed_at_ms;
        let state = state_name(record.state)?;
        let now = integer(command.committed_at_ms, "execution completion timestamp")?;
        let changed = transaction.execute(
            "UPDATE executions SET state = ?2, revision = ?3, completed_at_ms = ?4,
             updated_at_ms = ?4 WHERE execution_id = ?1 AND state = 'started' AND revision = ?5",
            params![
                completion.execution_id.as_str(),
                state,
                integer(record.revision, "execution revision")?,
                now,
                integer(completion.expected_revision, "execution expected revision")?
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
        insert_receipt(&transaction, command, "complete_execution", &record)?;
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
        let highest = transaction.query_row(
            "SELECT COALESCE(MAX(runtime_generation), 0) FROM runtime_bindings WHERE run_id = ?1",
            params![binding.run_id.as_str()],
            |row| row.get::<_, i64>(0),
        )?;
        let highest = unsigned(highest, "runtime generation")?;
        let expected = next_integer(highest, "runtime generation")?;
        if binding.runtime_generation != expected {
            return Err(StoreError::GenerationFenced {
                expected,
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

    /// Recovers durable state conservatively without retrying side effects.
    pub fn recover_gateway(&mut self, now_ms: u64) -> Result<RecoveryReport, StoreError> {
        let transaction = immediate(self)?;
        let now = integer(now_ms, "recovery timestamp")?;
        validate_all_execution_receipts(&transaction)?;
        let approvals_expired = transaction.execute(
            "UPDATE approvals SET state='expired', revision=revision+1, updated_at_ms=?1
             WHERE state='pending' AND expires_at_ms <= ?1",
            params![now],
        )?;
        let permits_expired = transaction.execute(
            "UPDATE permits SET state='expired' WHERE state='issued' AND valid_until_ms <= ?1",
            params![now],
        )?;
        let executions_uncertain = transaction.execute(
            "UPDATE executions SET state='uncertain', revision=revision+1, completed_at_ms=?1,
             updated_at_ms=?1 WHERE state='started'",
            params![now],
        )?;
        let runtime_bindings_lost = transaction.execute(
            "UPDATE runtime_bindings SET state='lost', updated_at_ms=?1 WHERE state='active'",
            params![now],
        )?;
        transaction.commit()?;
        Ok(RecoveryReport {
            approvals_expired: approvals_expired as u64,
            permits_expired: permits_expired as u64,
            executions_uncertain: executions_uncertain as u64,
            runtime_bindings_lost: runtime_bindings_lost as u64,
        })
    }
}

fn immediate(store: &mut SqliteTaskStore) -> Result<Transaction<'_>, StoreError> {
    Ok(store
        .connection_mut()
        .transaction_with_behavior(TransactionBehavior::Immediate)?)
}

fn require_task_owner(
    transaction: &Transaction<'_>,
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
    transaction: &Transaction<'_>,
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
    transaction: &Transaction<'_>,
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
    transaction: &Transaction<'_>,
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
         updated_at_ms FROM approvals WHERE approval_id=?1",
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
                ))
            },
        )
        .optional()?
        .ok_or_else(|| not_found("approval", id.as_str()))
        .and_then(|row| {
            Ok(ApprovalRecord {
                approval_id: id.clone(),
                request_id: parse_id(&row.0)?,
                actor_id: parse_id(&row.1)?,
                task_id: parse_id(&row.2)?,
                run_id: parse_id(&row.3)?,
                target: serde_json::from_str(&row.4)?,
                operation_digest: Digest::parse(row.5)
                    .map_err(|_| corrupt("invalid approval operation digest"))?,
                input_digest: Digest::parse(row.6)
                    .map_err(|_| corrupt("invalid approval input digest"))?,
                state: parse_approval_state(&row.7)?,
                revision: unsigned(row.8, "approval revision")?,
                expires_at_ms: unsigned(row.9, "approval deadline")?,
                decided_by_actor_id: row.10.map(|value| parse_id(&value)).transpose()?,
                created_at_ms: unsigned(row.11, "approval creation")?,
                updated_at_ms: unsigned(row.12, "approval update")?,
            })
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
         consumed_at_ms, created_at_ms FROM permits WHERE permit_id=?1",
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
                ))
            },
        )
        .optional()?
        .ok_or_else(|| not_found("permit", id.as_str()))?;
    if row.11 != 1 {
        return Err(corrupt("durable permit is not single-use"));
    }
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

fn load_execution(
    transaction: &rusqlite::Connection,
    id: &ExecutionId,
) -> Result<ExecutionRecord, StoreError> {
    let row = transaction
        .query_row(
            "SELECT actor_id, task_id, run_id, target_json, operation_digest, input_digest, state,
         revision, started_at_ms, completed_at_ms, created_at_ms, updated_at_ms
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
