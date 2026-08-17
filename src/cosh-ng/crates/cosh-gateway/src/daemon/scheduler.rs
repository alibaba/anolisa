//! Durable single-Run scheduler built on Outbox and Run-lease fencing.

mod brokered;
#[cfg(test)]
mod input_tests;

pub use brokered::{
    BrokeredApprovalContext, BrokeredApprovalPlan, BrokeredExecutionDriver, BrokeredResolution,
    BrokeredResolutionContext, BrokeredResolutionSource,
};
use brokered::{PendingBrokered, RejectingBrokeredExecutionDriver};

use std::path::Path;
use std::time::Duration;

use cosh_gateway_contracts::common::{
    ActorRef, BoundedName, BoundedOpaque, BoundedText, Digest, IdempotencyKey, RuntimeBindingRef,
    RuntimeSelector, TargetRef, WorkspaceRef,
};
use cosh_gateway_contracts::error::{ContractError, ErrorCategory};
use cosh_gateway_contracts::ids::{ActorId, InputRequestId, InstallationId, RunId, TaskId};
use cosh_gateway_contracts::task::{
    CancelReason, CancellationStage, RuntimeUpdate, TaskEvent, TaskState,
};
use cosh_gateway_contracts::{
    capability::{
        ApprovalDecision, ApprovalRequest, BrokeredOperation, CapabilityRequest, DenialCode,
        RuntimeExecutionFence,
    },
    ids::{ApprovalId, ExecutionId},
    runtime::{
        BrokeredExecutionDelivery, BrokeredExecutionRef, BrokeredRequestAcknowledgement,
        RuntimeInputRequest, RuntimeInputResponse, RuntimePermissionDecision, RuntimePermissionRef,
        ToolSummary,
    },
};
use serde::{Deserialize, Serialize};

use crate::capability::DurableApprovalCoordinator;
use crate::storage::{
    ApprovalRecord, ApprovalState, BrokeredRequestRecord, BrokeredRuntimeDispatchKind,
    BrokeredRuntimeDispatchRecord, BrokeredRuntimeDispatchState, LeaseClaim, LeaseCommand,
    LedgerCommand, LedgerOutcome, OutboxClaim, ProviderPermissionDispatchDecision,
    ProviderPermissionDispatchState, RuntimeInputDispatchRecord, RuntimeInputDispatchState,
    RuntimeInputRequestRecord, RuntimeInputRequestState, SqliteTaskStore, StoreError, TaskCommit,
};

use super::{digest_json, GatewayDaemonError, TaskCoordinator, TaskView};

pub(super) const RUNTIME_START_SCHEMA_VERSION: u16 = 2;
const DEFAULT_LEASE_DURATION_MS: u64 = 180_000;
const DEFAULT_LEASE_RENEWAL_MARGIN_MS: u64 = 60_000;
const DEFAULT_RUNTIME_OPERATION_TIMEOUT_MS: u64 = 70_000;
const DEFAULT_RUNTIME_INPUT_TIMEOUT_MS: u64 = 15 * 60 * 1_000;

pub(super) fn runtime_start_delivery_kind() -> BoundedName {
    BoundedName::new("runtime_start").unwrap_or_else(|_| unreachable!())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeStartIntent {
    pub(super) schema_version: u16,
    pub(super) actor: ActorRef,
    pub(super) task_id: TaskId,
    pub(super) run_id: RunId,
    pub(super) runtime: RuntimeSelector,
    pub(super) intent: BoundedText,
    pub(super) target: TargetRef,
    pub(super) workspace: WorkspaceRef,
}

/// Immutable work description passed to an injected Runtime factory.
#[derive(Debug, Clone)]
pub struct ScheduledRun {
    /// Authenticated Task owner.
    pub actor: ActorRef,
    /// Task selected by the durable queue.
    pub task_id: TaskId,
    /// Fenced Run selected by the durable queue.
    pub run_id: RunId,
    /// Runtime and optional installed profile selected at ingress.
    pub runtime: RuntimeSelector,
    /// Original bounded Task intent retained in the private Outbox.
    pub intent: BoundedText,
    /// Governed execution target.
    pub target: TargetRef,
    /// Trusted public projection of the canonical workspace.
    pub workspace: WorkspaceRef,
    /// Current Run-lease generation.
    pub lease_generation: u64,
}

/// Non-blocking result from one injected Runtime handle poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimePoll {
    /// No public event is ready yet.
    Pending,
    /// One validated Runtime event advanced only the durable binding sequence.
    Observed {
        /// Monotonic Runtime event sequence.
        sequence: u64,
    },
    /// One bounded progress update is ready for durable recording.
    Update {
        /// Monotonic Runtime event sequence.
        sequence: u64,
        /// Safe Task progress projection.
        update: RuntimeUpdate,
    },
    /// Provider-native execution is paused on a durable approval decision.
    PermissionRequested {
        /// Exact callback identity fenced to the active Runtime generation.
        permission: RuntimePermissionRef,
        /// Trusted normalized capability request.
        request: Box<CapabilityRequest>,
        /// Provider-facing operation description sanitized for actor review.
        summary: ToolSummary,
    },
    /// A COSH-owned typed operation is paused before durable takeover.
    BrokeredExecutionRequested {
        /// Exact callback identity fenced to the active Runtime generation.
        brokered: BrokeredExecutionRef,
        /// Trusted normalized capability request.
        request: Box<CapabilityRequest>,
        /// Closed operation whose side effect remains outside the Runtime.
        operation: BrokeredOperation,
        /// Provider-facing operation description sanitized for actor review.
        summary: ToolSummary,
    },
    /// A side-effect-free Runtime question is paused on durable actor input.
    InputRequested {
        /// Monotonic Runtime event sequence committed with the request.
        sequence: u64,
        /// Exact bounded request presentation and correlation identity.
        request: RuntimeInputRequest,
    },
    /// The Runtime completed successfully.
    Succeeded,
    /// The Runtime completed with a safe bounded failure.
    Failed(ContractError),
    /// The Runtime acknowledged an earlier cancellation request.
    Cancelled,
}

/// Active provider-neutral Runtime owned by the scheduler.
pub trait RuntimeHandle: Send {
    /// Starts the prompt only after the Runtime binding is durable.
    ///
    /// # Errors
    ///
    /// Returns a bounded failure when the prompt cannot be admitted.
    fn begin(&mut self) -> Result<(), ContractError>;

    /// Polls at most one event without blocking.
    fn poll(&mut self) -> RuntimePoll;

    /// Requests cancellation and returns only after the Runtime acknowledges it.
    ///
    /// # Errors
    ///
    /// Returns a safe Runtime failure when cancellation cannot be acknowledged.
    fn shutdown(&mut self, reason: CancelReason) -> Result<(), ContractError>;

    /// Dispatches one previously persisted provider-native response.
    fn resolve_provider_permission(
        &mut self,
        permission: &RuntimePermissionRef,
        decision: RuntimePermissionDecision,
    ) -> Result<(), ContractError>;

    /// Dispatches durable ownership of one exact brokered callback.
    fn acknowledge_brokered_request(
        &mut self,
        _brokered: &BrokeredExecutionRef,
        _acknowledgement: BrokeredRequestAcknowledgement,
    ) -> Result<(), ContractError> {
        Err(runtime_handle_unsupported("brokered acknowledgement"))
    }

    /// Dispatches the terminal result of one exact brokered callback.
    fn deliver_brokered_result(
        &mut self,
        _brokered: &BrokeredExecutionRef,
        _delivery: BrokeredExecutionDelivery,
    ) -> Result<(), ContractError> {
        Err(runtime_handle_unsupported("brokered result delivery"))
    }

    /// Dispatches one exact durable input response to the waiting Runtime.
    fn resolve_input(
        &mut self,
        _request: &RuntimeInputRequest,
        _response: RuntimeInputResponse,
    ) -> Result<(), ContractError> {
        Err(runtime_handle_unsupported("input resolution"))
    }
}

fn runtime_handle_unsupported(operation: &'static str) -> ContractError {
    ContractError::new(
        "runtime_operation_unsupported",
        ErrorCategory::InvalidRequest,
        false,
        format!("The Runtime does not support {operation}"),
    )
    .unwrap_or_else(|_| unreachable!("static Runtime operation failure must remain bounded"))
}

/// Open Runtime session returned before its prompt is dispatched.
pub struct StartedRuntime {
    /// Fenced binding emitted by the opened Runtime session.
    pub binding: RuntimeBindingRef,
    /// Runtime handle whose prompt has not started yet.
    pub handle: Box<dyn RuntimeHandle>,
}

/// Injection boundary that opens one provider-neutral Runtime.
pub trait RuntimeFactory: Send {
    /// Opens a Runtime for an already-fenced durable Run without prompting it.
    ///
    /// # Errors
    ///
    /// Returns a safe Runtime failure when no handle can be started.
    fn open(&mut self, run: &ScheduledRun) -> Result<StartedRuntime, ContractError>;
}

impl<T: RuntimeFactory + ?Sized> RuntimeFactory for Box<T> {
    fn open(&mut self, run: &ScheduledRun) -> Result<StartedRuntime, ContractError> {
        (**self).open(run)
    }
}

/// Lease and blocking-operation bounds for one scheduler worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskSchedulerConfig {
    /// Lifetime of each acquired or renewed Run lease.
    pub lease_duration: Duration,
    /// Lease is renewed once its remaining lifetime reaches this margin.
    pub lease_renewal_margin: Duration,
    /// Maximum configured duration of one blocking Runtime operation.
    pub runtime_operation_timeout: Duration,
}

impl Default for TaskSchedulerConfig {
    fn default() -> Self {
        Self {
            lease_duration: Duration::from_millis(DEFAULT_LEASE_DURATION_MS),
            lease_renewal_margin: Duration::from_millis(DEFAULT_LEASE_RENEWAL_MARGIN_MS),
            runtime_operation_timeout: Duration::from_millis(DEFAULT_RUNTIME_OPERATION_TIMEOUT_MS),
        }
    }
}

impl TaskSchedulerConfig {
    fn validate(self) -> Result<ValidatedSchedulerConfig, GatewayDaemonError> {
        let lease_duration_ms = duration_ms(self.lease_duration, "scheduler lease duration")?;
        let lease_renewal_margin_ms =
            duration_ms(self.lease_renewal_margin, "scheduler lease renewal margin")?;
        let runtime_operation_timeout_ms = duration_ms(
            self.runtime_operation_timeout,
            "scheduler Runtime operation timeout",
        )?;
        if lease_renewal_margin_ms == 0
            || runtime_operation_timeout_ms == 0
            || lease_duration_ms
                <= lease_renewal_margin_ms.saturating_add(runtime_operation_timeout_ms)
        {
            return Err(GatewayDaemonError::Protocol(
                "scheduler lease duration must exceed the Runtime operation timeout plus renewal margin"
                    .to_owned(),
            ));
        }
        Ok(ValidatedSchedulerConfig {
            lease_duration_ms,
            lease_renewal_margin_ms,
            runtime_operation_timeout_ms,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct ValidatedSchedulerConfig {
    lease_duration_ms: u64,
    lease_renewal_margin_ms: u64,
    runtime_operation_timeout_ms: u64,
}

/// Observable result of one bounded scheduler iteration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedulerTick {
    /// No queued or active work changed durable state.
    Idle,
    /// A queued Run was fenced and its Runtime started.
    Started(TaskView),
    /// Runtime progress was committed.
    Progressed(TaskView),
    /// A Run and its Task reached a terminal state.
    Settled(TaskView),
}

struct ActiveRun {
    scheduled: ScheduledRun,
    lease: LeaseClaim,
    lease_expires_at_ms: u64,
    next_event_sequence: u64,
    abort_error: Option<ContractError>,
    binding: RuntimeBindingRef,
    terminal: Option<TerminalOutcome>,
    binding_closed: bool,
    task_settled: bool,
    pending_permission: Option<PendingPermission>,
    pending_brokered: Option<PendingBrokered>,
    pending_input: Option<RuntimeInputRequestRecord>,
    handle: Box<dyn RuntimeHandle>,
}

#[derive(Debug, Clone)]
struct PendingPermission {
    permission: RuntimePermissionRef,
    approval: ApprovalRequest,
}

#[derive(Debug, Clone)]
enum TerminalOutcome {
    Succeeded,
    Failed(ContractError),
    Cancelled,
}

enum RuntimeStartClaim {
    Empty,
    Claimed {
        outbox: OutboxClaim,
        intent: Box<RuntimeStartIntent>,
        lease: LeaseClaim,
    },
    Recovered(TaskView),
}

/// Minimal durable scheduler for one active Runtime per worker instance.
pub struct TaskScheduler<F> {
    coordinator: TaskCoordinator,
    worker_id: BoundedOpaque,
    config: ValidatedSchedulerConfig,
    factory: F,
    brokered_driver: Box<dyn BrokeredExecutionDriver>,
    active: Option<ActiveRun>,
    shutting_down: bool,
    #[cfg(test)]
    fail_next_brokered_result_completion: bool,
    #[cfg(test)]
    fail_next_terminal_lease_release: bool,
    #[cfg(test)]
    fail_next_input_dispatch_completion: bool,
    #[cfg(test)]
    fail_next_input_request_install: bool,
    #[cfg(test)]
    fail_next_input_unknown_cleanup: bool,
}

impl<F: RuntimeFactory> TaskScheduler<F> {
    /// Opens the durable Task database with an injected Runtime factory.
    ///
    /// # Errors
    ///
    /// Returns a storage or installation-identity error when durable state
    /// cannot be opened safely.
    pub fn open(
        database_path: impl AsRef<Path>,
        requested_installation_id: Option<InstallationId>,
        worker_id: BoundedOpaque,
        factory: F,
    ) -> Result<Self, GatewayDaemonError> {
        Self::open_with_config(
            database_path,
            requested_installation_id,
            worker_id,
            factory,
            TaskSchedulerConfig::default(),
        )
    }

    /// Opens durable state with explicit, validated lease timing bounds.
    pub fn open_with_config(
        database_path: impl AsRef<Path>,
        requested_installation_id: Option<InstallationId>,
        worker_id: BoundedOpaque,
        factory: F,
        config: TaskSchedulerConfig,
    ) -> Result<Self, GatewayDaemonError> {
        Ok(Self {
            coordinator: TaskCoordinator::open(database_path, requested_installation_id)?,
            worker_id,
            config: config.validate()?,
            factory,
            brokered_driver: Box::new(RejectingBrokeredExecutionDriver),
            active: None,
            shutting_down: false,
            #[cfg(test)]
            fail_next_brokered_result_completion: false,
            #[cfg(test)]
            fail_next_terminal_lease_release: false,
            #[cfg(test)]
            fail_next_input_dispatch_completion: false,
            #[cfg(test)]
            fail_next_input_request_install: false,
            #[cfg(test)]
            fail_next_input_unknown_cleanup: false,
        })
    }

    /// Installs the trusted brokered policy and execution boundary.
    ///
    /// The default driver rejects every brokered request, so callers must
    /// explicitly install a production driver before selecting that profile.
    pub fn with_brokered_execution_driver(
        mut self,
        driver: Box<dyn BrokeredExecutionDriver>,
    ) -> Self {
        self.brokered_driver = driver;
        self
    }

    #[cfg(test)]
    pub(super) fn fail_next_terminal_lease_release_for_test(&mut self) {
        self.fail_next_terminal_lease_release = true;
    }

    #[cfg(test)]
    fn fail_next_input_dispatch_completion_for_test(&mut self) {
        self.fail_next_input_dispatch_completion = true;
    }

    #[cfg(test)]
    fn fail_next_input_request_install_for_test(&mut self) {
        self.fail_next_input_request_install = true;
    }

    #[cfg(test)]
    fn fail_next_input_unknown_cleanup_for_test(&mut self) {
        self.fail_next_input_unknown_cleanup = true;
    }

    /// Performs one non-blocking claim, cancel, poll, or settlement step.
    ///
    /// # Errors
    ///
    /// Returns a storage, fencing, protocol, or Runtime-start error. Durable
    /// state is never advanced after a stale lease is detected.
    pub fn tick(&mut self, now_ms: u64) -> Result<SchedulerTick, GatewayDaemonError> {
        if self.active.is_some() {
            self.ensure_active_operation_budget(now_ms)?;
            return self.poll_active(now_ms);
        }
        if self.shutting_down {
            return Ok(SchedulerTick::Idle);
        }

        let lease_deadline = deadline(now_ms, self.config.lease_duration_ms)?;
        if let Some(view) =
            self.coordinator
                .recover_expired_active_run(&self.worker_id, now_ms, lease_deadline)?
        {
            return Ok(SchedulerTick::Settled(view));
        }
        let (claim, intent, lease) =
            match self
                .coordinator
                .claim_runtime_start(&self.worker_id, now_ms, lease_deadline)?
            {
                RuntimeStartClaim::Empty => return Ok(SchedulerTick::Idle),
                RuntimeStartClaim::Recovered(view) => return Ok(SchedulerTick::Settled(view)),
                RuntimeStartClaim::Claimed {
                    outbox,
                    intent,
                    lease,
                } => (outbox, *intent, lease),
            };
        let scheduled = ScheduledRun {
            actor: intent.actor,
            task_id: intent.task_id,
            run_id: intent.run_id,
            runtime: intent.runtime,
            intent: intent.intent,
            target: intent.target,
            workspace: intent.workspace,
            lease_generation: lease.generation,
        };
        match self.factory.open(&scheduled) {
            Ok(StartedRuntime {
                binding,
                mut handle,
            }) => {
                let opened_at_ms = refreshed_now_ms(now_ms)?;
                if opened_at_ms >= lease_deadline {
                    let _ = handle.shutdown(CancelReason::RuntimeShutdown);
                    return Err(stale_operation_error());
                }
                let mut lease = lease;
                let mut lease_expires_at_ms = lease_deadline;
                renew_for_operation(
                    &mut self.coordinator,
                    &scheduled.actor.actor_id,
                    &mut lease,
                    &mut lease_expires_at_ms,
                    self.config,
                    opened_at_ms,
                )?;
                self.coordinator
                    .persist_runtime_binding(&lease, &binding, opened_at_ms)?;
                self.coordinator.record_runtime_binding_sequence(
                    &lease,
                    &binding,
                    1,
                    opened_at_ms,
                )?;
                let bound_at_ms = refreshed_now_ms(opened_at_ms)?;
                if bound_at_ms >= lease_expires_at_ms {
                    let _ = handle.shutdown(CancelReason::RuntimeShutdown);
                    return Err(stale_operation_error());
                }
                if let Err(ack_error) = self.coordinator.store.complete_outbox(&claim, bound_at_ms)
                {
                    let abort_error = runtime_lost_error(
                        "runtime_start_unacknowledged",
                        "Runtime start could not be acknowledged durably",
                    )?;
                    let shutdown_acknowledged =
                        handle.shutdown(CancelReason::RuntimeShutdown).is_ok();
                    self.active = Some(ActiveRun {
                        scheduled,
                        lease,
                        lease_expires_at_ms,
                        next_event_sequence: 1,
                        abort_error: Some(abort_error.clone()),
                        binding,
                        terminal: None,
                        binding_closed: false,
                        task_settled: false,
                        pending_permission: None,
                        pending_brokered: None,
                        pending_input: None,
                        handle,
                    });
                    if shutdown_acknowledged {
                        let stopped_at_ms = refreshed_now_ms(bound_at_ms)?;
                        self.require_active_lease_time(stopped_at_ms)?;
                        self.finish_failed(abort_error, stopped_at_ms)?;
                    }
                    return Err(ack_error.into());
                }
                let task = self.coordinator.store.load_task(&scheduled.task_id)?;
                let view = TaskView::from(&task);
                self.active = Some(ActiveRun {
                    scheduled,
                    lease,
                    lease_expires_at_ms,
                    next_event_sequence: 1,
                    abort_error: None,
                    binding,
                    terminal: None,
                    binding_closed: false,
                    task_settled: false,
                    pending_permission: None,
                    pending_brokered: None,
                    pending_input: None,
                    handle,
                });
                self.ensure_active_operation_budget(bound_at_ms)?;
                let begin_result = self
                    .active
                    .as_mut()
                    .ok_or_else(no_active_run)?
                    .handle
                    .begin();
                let began_at_ms = refreshed_now_ms(bound_at_ms)?;
                self.require_active_lease_time(began_at_ms)?;
                if let Err(error) = begin_result {
                    return self.finish_failed(error, began_at_ms);
                }
                Ok(SchedulerTick::Started(view))
            }
            Err(error) => {
                let failed_at_ms = refreshed_now_ms(now_ms)?;
                if failed_at_ms >= lease_deadline {
                    return Err(stale_operation_error());
                }
                let view = self
                    .coordinator
                    .settle_failed(&lease, error, failed_at_ms)?;
                self.coordinator
                    .store
                    .complete_outbox(&claim, failed_at_ms)?;
                self.coordinator.release_lease(&lease, failed_at_ms)?;
                Ok(SchedulerTick::Settled(view))
            }
        }
    }

    /// Stops claiming work and converges the active Runtime under its lease.
    pub fn shutdown(&mut self, now_ms: u64) -> Result<SchedulerTick, GatewayDaemonError> {
        self.shutting_down = true;
        if self.active.is_none() {
            return Ok(SchedulerTick::Idle);
        }
        self.ensure_active_operation_budget(now_ms)?;
        if self
            .active
            .as_ref()
            .ok_or_else(no_active_run)?
            .terminal
            .is_some()
        {
            return self.finish_terminal(now_ms);
        }
        if let Some(abort_error) = self
            .active
            .as_ref()
            .ok_or_else(no_active_run)?
            .abort_error
            .clone()
        {
            let result = self
                .active
                .as_mut()
                .ok_or_else(no_active_run)?
                .handle
                .shutdown(CancelReason::RuntimeShutdown);
            let stopped_at_ms = refreshed_now_ms(now_ms)?;
            self.require_active_lease_time(stopped_at_ms)?;
            return match result {
                Ok(()) => self.finish_failed(abort_error, stopped_at_ms),
                Err(_) => Err(GatewayDaemonError::Protocol(
                    "Runtime shutdown after an earlier failure was not acknowledged".to_owned(),
                )),
            };
        }
        self.coordinator.request_runtime_shutdown(
            &self.active.as_ref().ok_or_else(no_active_run)?.lease,
            now_ms,
        )?;
        self.cancel_pending_input(now_ms)?;
        let result = self
            .active
            .as_mut()
            .ok_or_else(no_active_run)?
            .handle
            .shutdown(CancelReason::RuntimeShutdown);
        let stopped_at_ms = refreshed_now_ms(now_ms)?;
        self.require_active_lease_time(stopped_at_ms)?;
        match result {
            Ok(()) => self.finish_cancelled(stopped_at_ms),
            Err(error) => {
                self.active.as_mut().ok_or_else(no_active_run)?.abort_error = Some(error);
                Err(GatewayDaemonError::Protocol(
                    "Runtime shutdown was not acknowledged".to_owned(),
                ))
            }
        }
    }

    /// Resolves the only provider-native approval currently held by this worker.
    pub fn resolve_approval(
        &mut self,
        actor_id: &ActorId,
        idempotency_key: IdempotencyKey,
        approval_id: &ApprovalId,
        decision: ApprovalDecision,
        now_ms: u64,
    ) -> Result<SchedulerTick, GatewayDaemonError> {
        let approval = self.coordinator.store.load_approval_record(approval_id)?;
        if approval.actor_id != *actor_id {
            return Err(GatewayDaemonError::Unauthorized);
        }
        if approval.permission.is_none()
            && approval.target_identity_digest.is_some()
            && approval.runtime_fence.is_some()
        {
            return self.resolve_brokered_approval(
                actor_id,
                idempotency_key,
                approval,
                decision,
                now_ms,
            );
        }
        let expected_revision = approval_resolution_revision(&approval, decision)?;
        let permission = approval.permission.clone().ok_or_else(|| {
            GatewayDaemonError::Protocol(
                "approval is not bound to a provider-native callback".to_owned(),
            )
        })?;
        let resolution_command = LedgerCommand {
            actor_id: actor_id.clone(),
            idempotency_key,
            command_digest: digest_json(&(
                "resolve_provider_permission",
                approval_id,
                decision,
                expected_revision,
                &permission,
            ))?,
            committed_at_ms: now_ms,
        };
        if approval.state != ApprovalState::Pending {
            let replay_lease = LeaseClaim {
                task_id: approval.task_id.clone(),
                run_id: approval.run_id.clone(),
                lease_owner: BoundedOpaque::new("durable-replay")
                    .unwrap_or_else(|_| unreachable!()),
                generation: 0,
                revision: 0,
            };
            let replayed = self.coordinator.store.resolve_provider_permission(
                &resolution_command,
                approval_id,
                expected_revision,
                crate::storage::ApprovalResolution::Decide(decision),
                &permission,
                &replay_lease,
            )?;
            if !matches!(replayed, LedgerOutcome::Replayed(_)) {
                return Err(GatewayDaemonError::Protocol(
                    "approval replay unexpectedly changed durable state".to_owned(),
                ));
            }
            let dispatch = self
                .coordinator
                .store
                .load_provider_permission_dispatch_record(approval_id)?;
            if dispatch.actor_id != *actor_id
                || dispatch.task_id != approval.task_id
                || dispatch.run_id != approval.run_id
                || dispatch.permission != permission
                || dispatch.decision != provider_dispatch_decision(decision)
            {
                return Err(GatewayDaemonError::Unauthorized);
            }
            if dispatch.state == ProviderPermissionDispatchState::Delivered {
                let task = self.coordinator.store.load_task(&approval.task_id)?;
                return Ok(SchedulerTick::Progressed(TaskView::from(&task)));
            }
            if matches!(
                dispatch.state,
                ProviderPermissionDispatchState::Started | ProviderPermissionDispatchState::Unknown
            ) {
                self.coordinator
                    .store
                    .mark_provider_dispatches_unknown_for_run(&approval.run_id, now_ms)?;
                let active_matches = self.active.as_ref().is_some_and(|active| {
                    active.scheduled.task_id == approval.task_id
                        && active.scheduled.run_id == approval.run_id
                });
                if active_matches {
                    return self.fail_unknown_provider_dispatch(
                        runtime_lost_error(
                            "provider_permission_replay_unknown",
                            "Provider permission response delivery is indeterminate",
                        )?,
                        now_ms,
                    );
                }
                return Err(GatewayDaemonError::Protocol(
                    "provider permission response delivery is indeterminate".to_owned(),
                ));
            }
        }
        if self.active.is_none() {
            return Err(GatewayDaemonError::Protocol(
                "provider permission replay requires a live Runtime handle".to_owned(),
            ));
        }
        self.ensure_active_operation_budget(now_ms)?;
        let (lease, task_id, run_id, pending) = {
            let active = self.active.as_ref().ok_or_else(no_active_run)?;
            if &active.scheduled.actor.actor_id != actor_id {
                return Err(GatewayDaemonError::Unauthorized);
            }
            if let Some(pending) = &active.pending_permission {
                if &pending.approval.approval_id != approval_id {
                    return Err(GatewayDaemonError::Protocol(
                        "approval does not match the active Runtime callback".to_owned(),
                    ));
                }
            }
            (
                active.lease.clone(),
                active.scheduled.task_id.clone(),
                active.scheduled.run_id.clone(),
                active.pending_permission.clone(),
            )
        };
        if approval.task_id != task_id || approval.run_id != run_id {
            return Err(GatewayDaemonError::Unauthorized);
        }
        if let Some(pending) = &pending {
            if pending.permission != permission {
                return Err(GatewayDaemonError::Protocol(
                    "approval does not match the active Runtime callback".to_owned(),
                ));
            }
        }
        if now_ms >= approval.expires_at_ms {
            self.expire_active_provider_approval(approval_id, &permission, now_ms)?;
            return Err(GatewayDaemonError::Protocol(
                "approval is no longer resolvable".to_owned(),
            ));
        }
        let prepared = self.coordinator.store.resolve_provider_permission(
            &resolution_command,
            approval_id,
            expected_revision,
            crate::storage::ApprovalResolution::Decide(decision),
            &permission,
            &lease,
        )?;
        let prepared = match prepared {
            LedgerOutcome::Applied(prepared) => prepared,
            LedgerOutcome::Replayed(_) => self
                .coordinator
                .store
                .load_provider_permission_dispatch_record(approval_id)?,
        };
        if prepared.actor_id != *actor_id
            || prepared.task_id != task_id
            || prepared.run_id != run_id
            || prepared.permission != permission
            || prepared.decision != provider_dispatch_decision(decision)
        {
            return Err(GatewayDaemonError::Unauthorized);
        }
        match prepared.state {
            ProviderPermissionDispatchState::Delivered => {
                let task = self.coordinator.store.load_task(&task_id)?;
                return Ok(SchedulerTick::Progressed(TaskView::from(&task)));
            }
            ProviderPermissionDispatchState::Started | ProviderPermissionDispatchState::Unknown => {
                return self.fail_unknown_provider_dispatch(
                    runtime_lost_error(
                        "provider_permission_replay_unknown",
                        "Provider permission response delivery is indeterminate",
                    )?,
                    now_ms,
                );
            }
            ProviderPermissionDispatchState::Prepared => {}
        }
        let task = self.coordinator.store.load_task(&task_id)?;
        let view = if task.state() == TaskState::WaitingApproval {
            self.coordinator.record_approval_resolved(
                &lease,
                approval_id,
                decision,
                prepared.permission.event_sequence,
                now_ms,
            )?
        } else if task.state() == TaskState::Running {
            TaskView::from(&task)
        } else {
            return self.fail_unknown_provider_dispatch(
                runtime_lost_error(
                    "provider_permission_task_state_invalid",
                    "Provider permission response no longer matches an active Task",
                )?,
                now_ms,
            );
        };
        let start_command =
            provider_dispatch_command(actor_id, "start", approval_id, prepared.revision, now_ms)?;
        let started = match self.coordinator.store.start_provider_permission_dispatch(
            &start_command,
            approval_id,
            prepared.revision,
            &lease,
        )? {
            LedgerOutcome::Applied(started) => started,
            LedgerOutcome::Replayed(_) => {
                return self.fail_unknown_provider_dispatch(
                    runtime_lost_error(
                        "provider_permission_replay_unknown",
                        "Provider permission dispatch start was replayed",
                    )?,
                    now_ms,
                );
            }
        };
        let runtime_decision = match decision {
            ApprovalDecision::Approve => RuntimePermissionDecision::ProviderNativeAllowOnce,
            ApprovalDecision::Deny => RuntimePermissionDecision::Deny {
                code: DenialCode::ApprovalDenied,
                safe_message: BoundedText::new("The provider-native operation was denied")
                    .unwrap_or_else(|_| unreachable!()),
            },
        };
        let dispatch = self
            .active
            .as_mut()
            .ok_or_else(no_active_run)?
            .handle
            .resolve_provider_permission(&permission, runtime_decision);
        let dispatched_at_ms = refreshed_now_ms(now_ms)?;
        self.require_active_lease_time(dispatched_at_ms)?;
        if let Err(error) = dispatch {
            return self.fail_unknown_provider_dispatch(error, dispatched_at_ms);
        }
        let complete_command = provider_dispatch_command(
            actor_id,
            "complete",
            approval_id,
            started.revision,
            dispatched_at_ms,
        )?;
        if self
            .coordinator
            .store
            .complete_provider_permission_dispatch(&complete_command, approval_id, started.revision)
            .is_err()
        {
            return self.fail_unknown_provider_dispatch(
                runtime_lost_error(
                    "provider_permission_receipt_unknown",
                    "Provider accepted a response whose receipt could not be persisted",
                )?,
                dispatched_at_ms,
            );
        }
        self.active
            .as_mut()
            .ok_or_else(no_active_run)?
            .pending_permission = None;
        Ok(SchedulerTick::Progressed(view))
    }

    fn fail_unknown_provider_dispatch(
        &mut self,
        error: ContractError,
        now_ms: u64,
    ) -> Result<SchedulerTick, GatewayDaemonError> {
        let run_id = self
            .active
            .as_ref()
            .ok_or_else(no_active_run)?
            .scheduled
            .run_id
            .clone();
        self.coordinator
            .store
            .mark_provider_dispatches_unknown_for_run(&run_id, now_ms)?;
        let shutdown_acknowledged = self
            .active
            .as_mut()
            .ok_or_else(no_active_run)?
            .handle
            .shutdown(CancelReason::RuntimeShutdown)
            .is_ok();
        if !shutdown_acknowledged {
            self.active.as_mut().ok_or_else(no_active_run)?.abort_error = Some(error);
            return Err(GatewayDaemonError::Protocol(
                "Runtime cancellation after an indeterminate provider response was not acknowledged"
                    .to_owned(),
            ));
        }
        self.finish_failed(error, refreshed_now_ms(now_ms)?)
    }

    /// Persists and dispatches one exact actor response to a pending Runtime question.
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_input(
        &mut self,
        actor_id: &ActorId,
        idempotency_key: IdempotencyKey,
        task_id: &TaskId,
        request_id: &InputRequestId,
        response: RuntimeInputResponse,
        expected_task_revision: Option<u64>,
        now_ms: u64,
    ) -> Result<SchedulerTick, GatewayDaemonError> {
        let request = self
            .coordinator
            .store
            .load_runtime_input_request(request_id)?;
        if request.actor_id != *actor_id || request.task_id != *task_id {
            return Err(GatewayDaemonError::Unauthorized);
        }
        let task = self.coordinator.store.load_task(task_id)?;
        if task.owner_actor_id() != actor_id {
            return Err(GatewayDaemonError::Unauthorized);
        }
        let active_matches = self.active.as_ref().is_some_and(|active| {
            active.scheduled.actor.actor_id == *actor_id
                && active.scheduled.task_id == *task_id
                && active.scheduled.run_id == request.run_id
                && active
                    .pending_input
                    .as_ref()
                    .is_some_and(|pending| pending.request.request_id() == request_id)
        });
        if request.state == RuntimeInputRequestState::Pending && !active_matches {
            return Err(GatewayDaemonError::Protocol(
                "Runtime input resolution requires its live waiting handle".to_owned(),
            ));
        }
        if request.state == RuntimeInputRequestState::Pending && now_ms >= request.expires_at_ms {
            self.expire_active_input(now_ms)?;
            return Err(GatewayDaemonError::Protocol(
                "Runtime input request is no longer resolvable".to_owned(),
            ));
        }
        let expected_revision = expected_task_revision.unwrap_or(task.revision());
        let command = LedgerCommand {
            actor_id: actor_id.clone(),
            idempotency_key,
            command_digest: digest_json(&(
                "resolve_runtime_input",
                task_id,
                request_id,
                &response,
                expected_task_revision,
            ))?,
            committed_at_ms: now_ms,
        };
        let prepared = match self.coordinator.store.resolve_runtime_input(
            &command,
            request_id,
            expected_revision,
            &response,
        )? {
            LedgerOutcome::Applied(record) | LedgerOutcome::Replayed(record) => record,
        };
        if prepared.actor_id != *actor_id
            || prepared.task_id != *task_id
            || prepared.run_id != request.run_id
            || prepared.request_id != *request_id
            || prepared.response != response
        {
            return Err(GatewayDaemonError::Unauthorized);
        }
        match prepared.state {
            RuntimeInputDispatchState::Delivered => {
                return Ok(SchedulerTick::Progressed(TaskView::from(
                    &self.coordinator.store.load_task(task_id)?,
                )));
            }
            RuntimeInputDispatchState::Started | RuntimeInputDispatchState::Unknown => {
                if !active_matches {
                    return Err(GatewayDaemonError::Protocol(
                        "Runtime input response delivery is indeterminate".to_owned(),
                    ));
                }
                return self.fail_unknown_input_dispatch(&prepared, now_ms);
            }
            RuntimeInputDispatchState::Prepared => {}
        }
        if !active_matches {
            return Err(GatewayDaemonError::Protocol(
                "Prepared Runtime input has no matching live handle".to_owned(),
            ));
        }
        self.ensure_active_operation_budget(now_ms)?;
        let lease = self
            .active
            .as_ref()
            .ok_or_else(no_active_run)?
            .lease
            .clone();
        let start_command =
            runtime_input_command(actor_id, "start", request_id, prepared.revision, now_ms)?;
        let started = match self.coordinator.store.start_runtime_input_dispatch(
            &start_command,
            request_id,
            &prepared.response_digest,
            prepared.revision,
            &lease,
        )? {
            LedgerOutcome::Applied(started)
                if started.state == RuntimeInputDispatchState::Started =>
            {
                started
            }
            LedgerOutcome::Applied(_) => {
                return Err(GatewayDaemonError::Protocol(
                    "Runtime input dispatch start reached an invalid state".to_owned(),
                ));
            }
            LedgerOutcome::Replayed(replayed)
                if replayed.state == RuntimeInputDispatchState::Delivered =>
            {
                return Ok(SchedulerTick::Progressed(TaskView::from(
                    &self.coordinator.store.load_task(task_id)?,
                )));
            }
            LedgerOutcome::Replayed(replayed) => {
                return self.fail_unknown_input_dispatch(&replayed, now_ms);
            }
        };
        let dispatch = self
            .active
            .as_mut()
            .ok_or_else(no_active_run)?
            .handle
            .resolve_input(&request.request, started.response.clone());
        let dispatched_at_ms = refreshed_now_ms(now_ms)?;
        self.require_active_lease_time(dispatched_at_ms)?;
        if dispatch.is_err() {
            return self.fail_unknown_input_dispatch(&started, dispatched_at_ms);
        }
        #[cfg(test)]
        if std::mem::take(&mut self.fail_next_input_dispatch_completion) {
            return self.fail_unknown_input_dispatch(&started, dispatched_at_ms);
        }
        let complete_command = runtime_input_command(
            actor_id,
            "complete",
            request_id,
            started.revision,
            dispatched_at_ms,
        )?;
        let completed = self.coordinator.store.complete_runtime_input_dispatch(
            &complete_command,
            request_id,
            &started.response_digest,
            started.revision,
            &lease,
        );
        match completed {
            Ok(LedgerOutcome::Applied(record))
                if record.state == RuntimeInputDispatchState::Delivered => {}
            Ok(LedgerOutcome::Replayed(record))
                if record.state == RuntimeInputDispatchState::Delivered => {}
            Ok(_) => {
                return self.fail_unknown_input_dispatch(&started, dispatched_at_ms);
            }
            Err(_) => {
                let observed = self
                    .coordinator
                    .store
                    .load_runtime_input_dispatch(request_id);
                if !matches!(
                    observed,
                    Ok(RuntimeInputDispatchRecord {
                        state: RuntimeInputDispatchState::Delivered,
                        ..
                    })
                ) {
                    return self.fail_unknown_input_dispatch(&started, dispatched_at_ms);
                }
            }
        }
        self.active
            .as_mut()
            .ok_or_else(no_active_run)?
            .pending_input = None;
        Ok(SchedulerTick::Progressed(TaskView::from(
            &self.coordinator.store.load_task(task_id)?,
        )))
    }

    fn fail_unknown_input_dispatch(
        &mut self,
        started: &RuntimeInputDispatchRecord,
        now_ms: u64,
    ) -> Result<SchedulerTick, GatewayDaemonError> {
        let lease = self
            .active
            .as_ref()
            .ok_or_else(no_active_run)?
            .lease
            .clone();
        let command = runtime_input_command(
            &started.actor_id,
            "unknown",
            &started.request_id,
            started.revision,
            now_ms,
        )?;
        let marked = self.coordinator.store.mark_runtime_input_dispatch_unknown(
            &command,
            &started.request_id,
            &started.response_digest,
            started.revision,
            &lease,
        );
        let marked = match marked {
            Ok(LedgerOutcome::Applied(record)) | Ok(LedgerOutcome::Replayed(record)) => record,
            Err(error) => {
                let _ = self
                    .active
                    .as_mut()
                    .ok_or_else(no_active_run)?
                    .handle
                    .shutdown(CancelReason::RuntimeShutdown);
                return Err(error.into());
            }
        };
        if marked.state != RuntimeInputDispatchState::Unknown {
            return Err(GatewayDaemonError::Protocol(
                "Runtime input dispatch did not reach Unknown".to_owned(),
            ));
        }
        self.active
            .as_mut()
            .ok_or_else(no_active_run)?
            .pending_input = None;
        #[cfg(test)]
        if std::mem::take(&mut self.fail_next_input_unknown_cleanup) {
            return Err(GatewayDaemonError::Protocol(
                "injected failure before uncertain input Runtime cleanup".to_owned(),
            ));
        }
        self.finish_suspended_after_input(now_ms)
    }

    fn poll_active(&mut self, now_ms: u64) -> Result<SchedulerTick, GatewayDaemonError> {
        if self
            .active
            .as_ref()
            .ok_or_else(no_active_run)?
            .terminal
            .is_some()
        {
            return self.finish_terminal(now_ms);
        }
        let cancellation_requested = {
            let active = self.active.as_ref().ok_or_else(no_active_run)?;
            self.coordinator
                .store
                .run_cancellation_requested(&active.scheduled.task_id, &active.scheduled.run_id)?
        };
        if cancellation_requested {
            let cancellation_at_ms = refreshed_now_ms(now_ms)?;
            self.require_active_lease_time(cancellation_at_ms)?;
            self.cancel_pending_input(cancellation_at_ms)?;
            let cancel_result = self
                .active
                .as_mut()
                .ok_or_else(no_active_run)?
                .handle
                .shutdown(CancelReason::UserRequested);
            let cancelled_at_ms = refreshed_now_ms(now_ms)?;
            self.require_active_lease_time(cancelled_at_ms)?;
            return match cancel_result {
                Ok(()) => self.finish_cancelled(cancelled_at_ms),
                Err(error) => {
                    self.active.as_mut().ok_or_else(no_active_run)?.abort_error = Some(error);
                    Err(GatewayDaemonError::Protocol(
                        "Runtime cancellation was not acknowledged".to_owned(),
                    ))
                }
            };
        }
        if let Some(pending) = self
            .active
            .as_ref()
            .ok_or_else(no_active_run)?
            .pending_input
            .as_ref()
        {
            if now_ms < pending.expires_at_ms {
                return Ok(SchedulerTick::Idle);
            }
            return self.expire_active_input(now_ms);
        }
        if let Some(pending) = self
            .active
            .as_ref()
            .ok_or_else(no_active_run)?
            .pending_brokered
            .as_ref()
        {
            if now_ms < pending.approval.expires_at_ms {
                return Ok(SchedulerTick::Idle);
            }
            let approval = self
                .coordinator
                .store
                .load_approval_record(&pending.approval.approval_id)?;
            return self.expire_active_brokered_approval(&approval, now_ms);
        }
        if let Some(pending) = self
            .active
            .as_ref()
            .ok_or_else(no_active_run)?
            .pending_permission
            .as_ref()
        {
            if now_ms < pending.approval.expires_at_ms {
                return Ok(SchedulerTick::Idle);
            }
            let (permission, approval_id) = {
                let active = self.active.as_ref().ok_or_else(no_active_run)?;
                let pending = active
                    .pending_permission
                    .as_ref()
                    .ok_or_else(no_active_run)?;
                (
                    pending.permission.clone(),
                    pending.approval.approval_id.clone(),
                )
            };
            return self.expire_active_provider_approval(&approval_id, &permission, now_ms);
        }
        let abort_error = self
            .active
            .as_ref()
            .ok_or_else(no_active_run)?
            .abort_error
            .clone();
        if let Some(abort_error) = abort_error {
            let acknowledged = self
                .active
                .as_mut()
                .ok_or_else(no_active_run)?
                .handle
                .shutdown(CancelReason::RuntimeShutdown)
                .is_ok();
            if acknowledged {
                let stopped_at_ms = refreshed_now_ms(now_ms)?;
                self.require_active_lease_time(stopped_at_ms)?;
                return self.finish_failed(abort_error, stopped_at_ms);
            }
            return Err(GatewayDaemonError::Protocol(
                "Runtime cancellation after durable start failure is not acknowledged".to_owned(),
            ));
        }
        let poll = self
            .active
            .as_mut()
            .ok_or_else(no_active_run)?
            .handle
            .poll();
        let polled_at_ms = refreshed_now_ms(now_ms)?;
        self.require_active_lease_time(polled_at_ms)?;
        match poll {
            RuntimePoll::Pending => Ok(SchedulerTick::Idle),
            RuntimePoll::Observed { sequence } => {
                let active = self.active.as_ref().ok_or_else(no_active_run)?;
                self.coordinator.record_runtime_binding_sequence(
                    &active.lease,
                    &active.binding,
                    sequence,
                    polled_at_ms,
                )?;
                Ok(SchedulerTick::Idle)
            }
            RuntimePoll::Update { sequence, update } => {
                let active = self.active.as_mut().ok_or_else(no_active_run)?;
                let next_sequence = active.next_event_sequence.checked_add(1).ok_or_else(|| {
                    GatewayDaemonError::Protocol(
                        "Runtime event sequence exceeds the supported range".to_owned(),
                    )
                })?;
                self.coordinator.record_runtime_binding_sequence(
                    &active.lease,
                    &active.binding,
                    sequence,
                    polled_at_ms,
                )?;
                let view = self.coordinator.record_runtime_update(
                    &active.lease,
                    active.next_event_sequence,
                    update,
                    polled_at_ms,
                )?;
                active.next_event_sequence = next_sequence;
                Ok(SchedulerTick::Progressed(view))
            }
            RuntimePoll::PermissionRequested {
                permission,
                request,
                summary,
            } => {
                let active = self.active.as_ref().ok_or_else(no_active_run)?;
                if permission.binding_id != active.binding.binding_id
                    || permission.runtime_generation != active.binding.runtime_generation
                    || permission.run_id != active.scheduled.run_id
                    || request.actor.actor_id != active.scheduled.actor.actor_id
                    || request.task_id != active.scheduled.task_id
                    || request.run_id != active.scheduled.run_id
                    || request.target != active.scheduled.target
                {
                    return self.finish_failed(
                        runtime_lost_error(
                            "runtime_permission_binding_invalid",
                            "Runtime permission request did not match the active Run",
                        )?,
                        polled_at_ms,
                    );
                }
                self.coordinator.record_runtime_binding_sequence(
                    &active.lease,
                    &active.binding,
                    permission.event_sequence,
                    polled_at_ms,
                )?;
                let approval = ApprovalRequest {
                    approval_id: ApprovalId::new(),
                    request_id: request.request_id.clone(),
                    task_id: request.task_id.clone(),
                    run_id: request.run_id.clone(),
                    summary: summary.summary,
                    expires_at_ms: request.expires_at_ms,
                };
                let view = self.coordinator.record_provider_approval(
                    &self.active.as_ref().ok_or_else(no_active_run)?.lease,
                    &permission,
                    &request,
                    &approval,
                    polled_at_ms,
                )?;
                self.active
                    .as_mut()
                    .ok_or_else(no_active_run)?
                    .pending_permission = Some(PendingPermission {
                    permission,
                    approval,
                });
                Ok(SchedulerTick::Progressed(view))
            }
            RuntimePoll::BrokeredExecutionRequested {
                brokered,
                request,
                operation,
                summary,
            } => {
                self.admit_brokered_execution(brokered, *request, operation, summary, polled_at_ms)
            }
            RuntimePoll::InputRequested { sequence, request } => {
                let active = self.active.as_ref().ok_or_else(no_active_run)?;
                if request.run_id() != &active.scheduled.run_id
                    || active.pending_input.is_some()
                    || active.pending_permission.is_some()
                    || active.pending_brokered.is_some()
                {
                    return self.finish_failed(
                        runtime_lost_error(
                            "runtime_input_binding_invalid",
                            "Runtime input request did not match the active Run",
                        )?,
                        polled_at_ms,
                    );
                }
                let expires_at_ms = polled_at_ms
                    .checked_add(DEFAULT_RUNTIME_INPUT_TIMEOUT_MS)
                    .ok_or_else(|| {
                        GatewayDaemonError::Protocol(
                            "Runtime input deadline exceeds the supported range".to_owned(),
                        )
                    })?;
                let command = LedgerCommand {
                    actor_id: active.scheduled.actor.actor_id.clone(),
                    idempotency_key: IdempotencyKey::new(format!(
                        "scheduler-input-request-{}",
                        request.request_id().as_str()
                    ))
                    .map_err(|error| GatewayDaemonError::Protocol(error.to_string()))?,
                    command_digest: digest_json(&(
                        "record_runtime_input_request",
                        &request,
                        expires_at_ms,
                        &active.binding,
                        sequence,
                        active.lease.generation,
                    ))?,
                    committed_at_ms: polled_at_ms,
                };
                let record = match self.coordinator.store.record_runtime_input_request(
                    &command,
                    &request,
                    expires_at_ms,
                    &active.binding.binding_id,
                    &active.binding.runtime_instance_id,
                    active.binding.runtime_generation,
                    sequence,
                    &active.lease,
                )? {
                    LedgerOutcome::Applied(record) | LedgerOutcome::Replayed(record) => record,
                };
                let view = TaskView::from(
                    &self
                        .coordinator
                        .store
                        .load_task(&active.scheduled.task_id)?,
                );
                #[cfg(test)]
                if std::mem::take(&mut self.fail_next_input_request_install) {
                    return Err(GatewayDaemonError::Protocol(
                        "injected failure before pending input installation".to_owned(),
                    ));
                }
                self.active
                    .as_mut()
                    .ok_or_else(no_active_run)?
                    .pending_input = Some(record);
                Ok(SchedulerTick::Progressed(view))
            }
            RuntimePoll::Succeeded => self.finish_succeeded(polled_at_ms),
            RuntimePoll::Failed(error) => self.finish_failed(error, polled_at_ms),
            RuntimePoll::Cancelled => Err(GatewayDaemonError::Protocol(
                "Runtime reported cancellation without a durable request".to_owned(),
            )),
        }
    }

    fn cancel_pending_input(&mut self, now_ms: u64) -> Result<(), GatewayDaemonError> {
        let pending = if let Some(pending) = self
            .active
            .as_ref()
            .ok_or_else(no_active_run)?
            .pending_input
            .clone()
        {
            pending
        } else {
            let (actor_id, task_id, run_id) = {
                let active = self.active.as_ref().ok_or_else(no_active_run)?;
                (
                    active.scheduled.actor.actor_id.clone(),
                    active.scheduled.task_id.clone(),
                    active.scheduled.run_id.clone(),
                )
            };
            let task = self.coordinator.store.load_task(&task_id)?;
            let Some(request_id) = task.pending_input_request_id().cloned() else {
                return Ok(());
            };
            let pending = self
                .coordinator
                .store
                .load_runtime_input_request(&request_id)?;
            if pending.actor_id != actor_id
                || pending.task_id != task_id
                || pending.run_id != run_id
                || pending.state != RuntimeInputRequestState::Pending
            {
                return Err(GatewayDaemonError::Protocol(
                    "durable pending input does not match the active Run".to_owned(),
                ));
            }
            pending
        };
        let command = runtime_input_command(
            &pending.actor_id,
            "cancel",
            pending.request.request_id(),
            pending.revision,
            now_ms,
        )?;
        let cancelled = match self.coordinator.store.cancel_runtime_input_request(
            &command,
            pending.request.request_id(),
            pending.revision,
        )? {
            LedgerOutcome::Applied(record) | LedgerOutcome::Replayed(record) => record,
        };
        if cancelled.state != RuntimeInputRequestState::Cancelled {
            return Err(GatewayDaemonError::Protocol(
                "Runtime input cancellation did not reach its durable terminal state".to_owned(),
            ));
        }
        self.active
            .as_mut()
            .ok_or_else(no_active_run)?
            .pending_input = None;
        Ok(())
    }

    fn expire_active_input(&mut self, now_ms: u64) -> Result<SchedulerTick, GatewayDaemonError> {
        let pending = self
            .active
            .as_ref()
            .ok_or_else(no_active_run)?
            .pending_input
            .clone()
            .ok_or_else(no_active_run)?;
        let command = runtime_input_command(
            &pending.actor_id,
            "expire",
            pending.request.request_id(),
            pending.revision,
            now_ms,
        )?;
        let expired = match self.coordinator.store.expire_runtime_input_request(
            &command,
            pending.request.request_id(),
            pending.revision,
        )? {
            LedgerOutcome::Applied(record) | LedgerOutcome::Replayed(record) => record,
        };
        if expired.state != RuntimeInputRequestState::Expired {
            return Err(GatewayDaemonError::Protocol(
                "Runtime input expiry did not reach its durable terminal state".to_owned(),
            ));
        }
        self.active
            .as_mut()
            .ok_or_else(no_active_run)?
            .pending_input = None;
        self.finish_suspended_after_input(now_ms)
    }

    fn finish_suspended_after_input(
        &mut self,
        now_ms: u64,
    ) -> Result<SchedulerTick, GatewayDaemonError> {
        self.require_active_lease_time(now_ms)?;
        let stopped = self
            .active
            .as_mut()
            .ok_or_else(no_active_run)?
            .handle
            .shutdown(CancelReason::RuntimeShutdown)
            .is_ok();
        if !stopped {
            return Err(GatewayDaemonError::Protocol(
                "Runtime shutdown after input convergence was not acknowledged".to_owned(),
            ));
        }
        let stopped_at_ms = refreshed_now_ms(now_ms)?;
        self.require_active_lease_time(stopped_at_ms)?;
        let (actor_id, task_id, lease, binding) = {
            let active = self.active.as_ref().ok_or_else(no_active_run)?;
            (
                active.scheduled.actor.actor_id.clone(),
                active.scheduled.task_id.clone(),
                active.lease.clone(),
                active.binding.clone(),
            )
        };
        self.coordinator
            .close_runtime_binding(&actor_id, &lease, &binding, stopped_at_ms)?;
        let task = self.coordinator.store.load_task(&task_id)?;
        if task.state() != TaskState::Suspended {
            return Err(GatewayDaemonError::Protocol(
                "Runtime input convergence did not suspend its Task".to_owned(),
            ));
        }
        self.coordinator.release_lease(&lease, stopped_at_ms)?;
        self.active.take();
        Ok(SchedulerTick::Settled(TaskView::from(&task)))
    }

    fn expire_active_provider_approval(
        &mut self,
        approval_id: &ApprovalId,
        permission: &RuntimePermissionRef,
        now_ms: u64,
    ) -> Result<SchedulerTick, GatewayDaemonError> {
        let (actor_id, lease) = {
            let active = self.active.as_ref().ok_or_else(no_active_run)?;
            (
                active.scheduled.actor.actor_id.clone(),
                active.lease.clone(),
            )
        };
        let command = LedgerCommand {
            actor_id,
            idempotency_key: IdempotencyKey::new(format!(
                "scheduler-expire-provider-{}",
                approval_id.as_str()
            ))
            .map_err(|error| GatewayDaemonError::Protocol(error.to_string()))?,
            command_digest: digest_json(&(
                "expire_provider_approval",
                approval_id,
                permission,
                lease.generation,
            ))?,
            committed_at_ms: now_ms,
        };
        self.coordinator.store.expire_provider_approval(
            &command,
            approval_id,
            permission,
            &lease,
        )?;
        let expiry_error = runtime_lost_error(
            "provider_permission_expired",
            "The provider-native approval expired before it was resolved",
        )?;
        let shutdown_acknowledged = self
            .active
            .as_mut()
            .ok_or_else(no_active_run)?
            .handle
            .shutdown(CancelReason::RuntimeShutdown)
            .is_ok();
        if !shutdown_acknowledged {
            self.active.as_mut().ok_or_else(no_active_run)?.abort_error = Some(expiry_error);
            return Err(GatewayDaemonError::Protocol(
                "Runtime cancellation after approval expiry was not acknowledged".to_owned(),
            ));
        }
        let expired_at_ms = refreshed_now_ms(now_ms)?;
        self.require_active_lease_time(expired_at_ms)?;
        self.finish_failed(expiry_error, expired_at_ms)
    }

    fn ensure_active_operation_budget(&mut self, now_ms: u64) -> Result<(), GatewayDaemonError> {
        let active = self.active.as_mut().ok_or_else(no_active_run)?;
        renew_for_operation(
            &mut self.coordinator,
            &active.scheduled.actor.actor_id,
            &mut active.lease,
            &mut active.lease_expires_at_ms,
            self.config,
            now_ms,
        )
    }

    fn require_active_lease_time(&self, now_ms: u64) -> Result<(), GatewayDaemonError> {
        let active = self.active.as_ref().ok_or_else(no_active_run)?;
        if now_ms >= active.lease_expires_at_ms {
            Err(stale_operation_error())
        } else {
            Ok(())
        }
    }

    fn finish_succeeded(&mut self, now_ms: u64) -> Result<SchedulerTick, GatewayDaemonError> {
        self.active.as_mut().ok_or_else(no_active_run)?.terminal = Some(TerminalOutcome::Succeeded);
        self.finish_terminal(now_ms)
    }

    fn finish_failed(
        &mut self,
        error: ContractError,
        now_ms: u64,
    ) -> Result<SchedulerTick, GatewayDaemonError> {
        self.active.as_mut().ok_or_else(no_active_run)?.terminal =
            Some(TerminalOutcome::Failed(error));
        self.finish_terminal(now_ms)
    }

    fn finish_cancelled(&mut self, now_ms: u64) -> Result<SchedulerTick, GatewayDaemonError> {
        self.active.as_mut().ok_or_else(no_active_run)?.terminal = Some(TerminalOutcome::Cancelled);
        self.finish_terminal(now_ms)
    }

    fn finish_terminal(&mut self, now_ms: u64) -> Result<SchedulerTick, GatewayDaemonError> {
        self.require_active_lease_time(now_ms)?;
        let (actor_id, run_id, lease, binding, terminal, binding_closed, task_settled) = {
            let active = self.active.as_ref().ok_or_else(no_active_run)?;
            (
                active.scheduled.actor.actor_id.clone(),
                active.scheduled.run_id.clone(),
                active.lease.clone(),
                active.binding.clone(),
                active.terminal.clone().ok_or_else(no_active_run)?,
                active.binding_closed,
                active.task_settled,
            )
        };
        self.coordinator
            .store
            .cancel_pending_approvals_for_run(&run_id, now_ms)?;
        self.coordinator
            .store
            .mark_provider_dispatches_unknown_for_run(&run_id, now_ms)?;
        if !binding_closed {
            self.coordinator
                .close_runtime_binding(&actor_id, &lease, &binding, now_ms)?;
            self.active
                .as_mut()
                .ok_or_else(no_active_run)?
                .binding_closed = true;
        }
        let view = if task_settled {
            let task_id = &self
                .active
                .as_ref()
                .ok_or_else(no_active_run)?
                .scheduled
                .task_id;
            TaskView::from(&self.coordinator.store.load_task(task_id)?)
        } else {
            let view = match terminal {
                TerminalOutcome::Succeeded => self.coordinator.settle_succeeded(&lease, now_ms)?,
                TerminalOutcome::Failed(error) => {
                    self.coordinator.settle_failed(&lease, error, now_ms)?
                }
                TerminalOutcome::Cancelled => self.coordinator.settle_cancelled(&lease, now_ms)?,
            };
            self.active.as_mut().ok_or_else(no_active_run)?.task_settled = true;
            view
        };
        #[cfg(test)]
        if std::mem::take(&mut self.fail_next_terminal_lease_release) {
            return Err(GatewayDaemonError::Protocol(
                "injected failure before terminal Run lease release".to_owned(),
            ));
        }
        self.coordinator.release_lease(&lease, now_ms)?;
        self.active.take();
        Ok(SchedulerTick::Settled(view))
    }
}

impl TaskCoordinator {
    fn recover_expired_active_run(
        &mut self,
        worker_id: &BoundedOpaque,
        now_ms: u64,
        lease_expires_at_ms: u64,
    ) -> Result<Option<TaskView>, GatewayDaemonError> {
        let Some(expired) = self.store.load_expired_active_lease(now_ms)? else {
            return Ok(None);
        };
        let command = LeaseCommand {
            command: LedgerCommand {
                actor_id: expired.actor_id.clone(),
                idempotency_key: IdempotencyKey::new(format!(
                    "scheduler-recover-lease-{}-{}",
                    expired.run_id.as_str(),
                    expired.generation
                ))
                .map_err(|error| GatewayDaemonError::Protocol(error.to_string()))?,
                command_digest: digest_json(&(
                    "recover_run_lease",
                    &expired.task_id,
                    &expired.run_id,
                    worker_id,
                    expired.generation,
                    lease_expires_at_ms,
                ))?,
                committed_at_ms: now_ms,
            },
            task_id: expired.task_id,
            run_id: expired.run_id,
            lease_owner: worker_id.clone(),
            expires_at_ms: lease_expires_at_ms,
        };
        let record = match self.store.acquire_run_lease(&command)? {
            LedgerOutcome::Applied(record) | LedgerOutcome::Replayed(record) => record,
        };
        let actor_id = record.actor_id.clone();
        let claim = LeaseClaim {
            task_id: record.task_id,
            run_id: record.run_id,
            lease_owner: record.lease_owner,
            generation: record.generation,
            revision: record.revision,
        };
        self.store
            .mark_runtime_bindings_lost_for_run(&claim.run_id, now_ms)?;
        let input_recovery = LedgerCommand {
            actor_id,
            idempotency_key: IdempotencyKey::new(format!(
                "scheduler-recover-input-{}-{}",
                claim.run_id.as_str(),
                claim.generation
            ))
            .map_err(|error| GatewayDaemonError::Protocol(error.to_string()))?,
            command_digest: digest_json(&(
                "recover_runtime_input_dispatch_for_run",
                &claim.task_id,
                &claim.run_id,
                claim.generation,
            ))?,
            committed_at_ms: now_ms,
        };
        self.store.recover_runtime_input_dispatch_for_run(
            &input_recovery,
            &claim.run_id,
            &claim,
        )?;
        self.store
            .cancel_pending_approvals_for_run(&claim.run_id, now_ms)?;
        self.store
            .mark_provider_dispatches_unknown_for_run(&claim.run_id, now_ms)?;
        self.store
            .mark_brokered_dispatches_unknown_for_run(&claim.run_id, now_ms)?;
        self.store
            .recover_brokered_executions_for_run(&claim.run_id, now_ms)?;
        let recovered_task = self.store.load_task(&claim.task_id)?;
        if recovered_task.cancellation_requested()
            && recovered_task.active_run_can_be_cancelled(&claim.run_id)
        {
            let view = self.settle_cancelled(&claim, now_ms)?;
            self.release_lease(&claim, now_ms)?;
            return Ok(Some(view));
        }
        if recovered_task.state() == TaskState::Suspended {
            let view = TaskView::from(&recovered_task);
            self.release_lease(&claim, now_ms)?;
            return Ok(Some(view));
        }
        let view = self.settle_failed(
            &claim,
            runtime_lost_error(
                "runtime_lost",
                "Runtime ownership was lost across daemon restart",
            )?,
            now_ms,
        )?;
        self.release_lease(&claim, now_ms)?;
        Ok(Some(view))
    }

    fn claim_runtime_start(
        &mut self,
        worker_id: &BoundedOpaque,
        now_ms: u64,
        lease_expires_at_ms: u64,
    ) -> Result<RuntimeStartClaim, GatewayDaemonError> {
        let Some(claim) = self.store.claim_outbox(
            &runtime_start_delivery_kind(),
            worker_id,
            now_ms,
            lease_expires_at_ms,
        )?
        else {
            return Ok(RuntimeStartClaim::Empty);
        };
        let intent = serde_json::from_value::<RuntimeStartIntent>(claim.payload.clone())?;
        if intent.schema_version != RUNTIME_START_SCHEMA_VERSION || intent.task_id != claim.task_id
        {
            return Err(GatewayDaemonError::Protocol(
                "runtime start Outbox identity or schema mismatch".to_owned(),
            ));
        }
        let task = self.store.load_task(&intent.task_id)?;
        if task.owner_actor_id() != &intent.actor.actor_id
            || task.active_run_id() != Some(&intent.run_id)
            || task.target() != &intent.target
        {
            return Err(GatewayDaemonError::Protocol(
                "runtime start intent no longer matches its queued Task".to_owned(),
            ));
        }
        if !matches!(task.state(), TaskState::Queued | TaskState::Running) {
            self.store.complete_outbox(&claim, now_ms)?;
            return Ok(RuntimeStartClaim::Empty);
        }
        let lease =
            self.acquire_start_lease(&intent, &claim, worker_id, now_ms, lease_expires_at_ms)?;
        if task.state() == TaskState::Running {
            // The preceding worker crossed RunStarted but did not acknowledge
            // its Outbox delivery. The expired Run lease proves that no live
            // scheduler still owns the handle, so fail closed after takeover.
            self.store
                .mark_runtime_bindings_lost_for_run(&lease.run_id, now_ms)?;
            self.store
                .cancel_pending_approvals_for_run(&lease.run_id, now_ms)?;
            self.store
                .mark_provider_dispatches_unknown_for_run(&lease.run_id, now_ms)?;
            let view = self.settle_failed(
                &lease,
                runtime_lost_error(
                    "runtime_lost",
                    "Runtime ownership was lost before start acknowledgement",
                )?,
                now_ms,
            )?;
            self.store.complete_outbox(&claim, now_ms)?;
            self.release_lease(&lease, now_ms)?;
            return Ok(RuntimeStartClaim::Recovered(view));
        }
        let started = self.event(
            &intent.actor.actor_id,
            &intent.task_id,
            Some(&intent.run_id),
            task.revision().saturating_add(1),
            now_ms,
            TaskEvent::RunStarted {
                run_id: intent.run_id.clone(),
            },
        );
        self.store.commit_task(&TaskCommit {
            actor_id: intent.actor.actor_id.clone(),
            idempotency_key: internal_key("start", &claim),
            command_digest: digest_json(&("start_runtime", &intent.task_id, &intent.run_id))?,
            expected_revision: Some(task.revision()),
            events: vec![started],
            outbox: Vec::new(),
            committed_at_ms: now_ms,
        })?;
        Ok(RuntimeStartClaim::Claimed {
            outbox: claim,
            intent: Box::new(intent),
            lease,
        })
    }

    fn acquire_start_lease(
        &mut self,
        intent: &RuntimeStartIntent,
        claim: &OutboxClaim,
        worker_id: &BoundedOpaque,
        now_ms: u64,
        lease_expires_at_ms: u64,
    ) -> Result<LeaseClaim, GatewayDaemonError> {
        let lease_command = LeaseCommand {
            command: internal_ledger_command(
                &intent.actor.actor_id,
                "lease",
                claim,
                now_ms,
                &(
                    &intent.task_id,
                    &intent.run_id,
                    worker_id,
                    lease_expires_at_ms,
                ),
            )?,
            task_id: intent.task_id.clone(),
            run_id: intent.run_id.clone(),
            lease_owner: worker_id.clone(),
            expires_at_ms: lease_expires_at_ms,
        };
        let lease = match self.store.acquire_run_lease(&lease_command)? {
            LedgerOutcome::Applied(lease) | LedgerOutcome::Replayed(lease) => lease,
        };
        Ok(LeaseClaim {
            task_id: lease.task_id,
            run_id: lease.run_id,
            lease_owner: lease.lease_owner,
            generation: lease.generation,
            revision: lease.revision,
        })
    }

    fn renew_lease(
        &mut self,
        actor_id: &ActorId,
        claim: &LeaseClaim,
        expires_at_ms: u64,
        now_ms: u64,
    ) -> Result<LeaseClaim, GatewayDaemonError> {
        let command = LeaseCommand {
            command: LedgerCommand {
                actor_id: actor_id.clone(),
                idempotency_key: IdempotencyKey::new(format!(
                    "scheduler-renew-{}-{}",
                    claim.run_id.as_str(),
                    claim.revision
                ))
                .map_err(|error| GatewayDaemonError::Protocol(error.to_string()))?,
                command_digest: digest_json(&(
                    "renew_run_lease",
                    &claim.task_id,
                    &claim.run_id,
                    &claim.lease_owner,
                    claim.generation,
                    claim.revision,
                    expires_at_ms,
                    now_ms,
                ))?,
                committed_at_ms: now_ms,
            },
            task_id: claim.task_id.clone(),
            run_id: claim.run_id.clone(),
            lease_owner: claim.lease_owner.clone(),
            expires_at_ms,
        };
        let record = match self
            .store
            .renew_run_lease(&command, claim.generation, claim.revision)?
        {
            LedgerOutcome::Applied(record) | LedgerOutcome::Replayed(record) => record,
        };
        Ok(LeaseClaim {
            task_id: record.task_id,
            run_id: record.run_id,
            lease_owner: record.lease_owner,
            generation: record.generation,
            revision: record.revision,
        })
    }

    fn persist_runtime_binding(
        &mut self,
        claim: &LeaseClaim,
        binding: &RuntimeBindingRef,
        now_ms: u64,
    ) -> Result<TaskView, GatewayDaemonError> {
        self.require_current_lease(claim, now_ms)?;
        let task = self.store.load_task(&claim.task_id)?;
        let command = LedgerCommand {
            actor_id: task.owner_actor_id().clone(),
            idempotency_key: IdempotencyKey::new(format!(
                "scheduler-bind-ledger-{}-{}",
                claim.run_id.as_str(),
                claim.generation
            ))
            .map_err(|error| GatewayDaemonError::Protocol(error.to_string()))?,
            command_digest: digest_json(&(
                "bind_runtime",
                &claim.task_id,
                &claim.run_id,
                &claim.lease_owner,
                claim.generation,
                binding,
            ))?,
            committed_at_ms: now_ms,
        };
        self.store.bind_runtime(&command, binding, claim)?;
        let event = self.event(
            task.owner_actor_id(),
            &claim.task_id,
            Some(&claim.run_id),
            task.revision().saturating_add(1),
            now_ms,
            TaskEvent::RuntimeBound {
                run_id: claim.run_id.clone(),
                binding: binding.clone(),
            },
        );
        self.commit_internal(task.owner_actor_id(), claim, "bind", 0, vec![event], now_ms)
    }

    fn close_runtime_binding(
        &mut self,
        actor_id: &ActorId,
        claim: &LeaseClaim,
        binding: &RuntimeBindingRef,
        now_ms: u64,
    ) -> Result<(), GatewayDaemonError> {
        let command = LedgerCommand {
            actor_id: actor_id.clone(),
            idempotency_key: IdempotencyKey::new(format!(
                "scheduler-close-binding-{}-{}",
                claim.run_id.as_str(),
                claim.generation
            ))
            .map_err(|error| GatewayDaemonError::Protocol(error.to_string()))?,
            command_digest: digest_json(&(
                "close_runtime_binding",
                &binding.binding_id,
                binding.runtime_generation,
                claim.generation,
            ))?,
            committed_at_ms: now_ms,
        };
        self.store.close_runtime_binding(
            &command,
            &binding.binding_id,
            binding.runtime_generation,
            claim,
        )?;
        Ok(())
    }

    fn record_runtime_binding_sequence(
        &mut self,
        claim: &LeaseClaim,
        binding: &RuntimeBindingRef,
        sequence: u64,
        now_ms: u64,
    ) -> Result<(), GatewayDaemonError> {
        self.store.record_runtime_sequence(
            &binding.binding_id,
            &binding.runtime_instance_id,
            binding.runtime_generation,
            sequence,
            now_ms,
            claim,
        )?;
        Ok(())
    }

    fn request_runtime_shutdown(
        &mut self,
        claim: &LeaseClaim,
        now_ms: u64,
    ) -> Result<(), GatewayDaemonError> {
        if self
            .store
            .run_cancellation_requested(&claim.task_id, &claim.run_id)?
        {
            return Ok(());
        }
        self.require_current_lease(claim, now_ms)?;
        let task = self.store.load_task(&claim.task_id)?;
        let event = self.event(
            task.owner_actor_id(),
            &claim.task_id,
            Some(&claim.run_id),
            task.revision().saturating_add(1),
            now_ms,
            TaskEvent::CancellationRequested {
                run_id: claim.run_id.clone(),
                cause: CancelReason::RuntimeShutdown,
            },
        );
        self.commit_internal(
            task.owner_actor_id(),
            claim,
            "shutdown-request",
            0,
            vec![event],
            now_ms,
        )?;
        Ok(())
    }

    fn record_provider_approval(
        &mut self,
        claim: &LeaseClaim,
        permission: &RuntimePermissionRef,
        request: &CapabilityRequest,
        approval: &ApprovalRequest,
        now_ms: u64,
    ) -> Result<TaskView, GatewayDaemonError> {
        self.require_current_lease(claim, now_ms)?;
        let command = LedgerCommand {
            actor_id: request.actor.actor_id.clone(),
            idempotency_key: IdempotencyKey::new(format!(
                "scheduler-approval-{}",
                request.request_id.as_str()
            ))
            .map_err(|error| GatewayDaemonError::Protocol(error.to_string()))?,
            command_digest: digest_json(&(
                "record_provider_approval",
                request,
                approval,
                permission,
                claim.generation,
            ))?,
            committed_at_ms: now_ms,
        };
        DurableApprovalCoordinator::new(&mut self.store)
            .record_provider_pending(&command, request, approval, permission, claim)
            .map_err(|error| GatewayDaemonError::Protocol(error.to_string()))?;
        let task = self.store.load_task(&claim.task_id)?;
        let event = self.event(
            task.owner_actor_id(),
            &claim.task_id,
            Some(&claim.run_id),
            task.revision().saturating_add(1),
            now_ms,
            TaskEvent::ApprovalRequested {
                approval: approval.clone(),
            },
        );
        self.commit_internal(
            task.owner_actor_id(),
            claim,
            "approval-request",
            permission.event_sequence,
            vec![event],
            now_ms,
        )
    }

    fn record_approval_resolved(
        &mut self,
        claim: &LeaseClaim,
        approval_id: &ApprovalId,
        decision: ApprovalDecision,
        sequence: u64,
        now_ms: u64,
    ) -> Result<TaskView, GatewayDaemonError> {
        self.require_current_lease(claim, now_ms)?;
        let task = self.store.load_task(&claim.task_id)?;
        let event = self.event(
            task.owner_actor_id(),
            &claim.task_id,
            Some(&claim.run_id),
            task.revision().saturating_add(1),
            now_ms,
            TaskEvent::ApprovalResolved {
                approval_id: approval_id.clone(),
                decision,
            },
        );
        self.commit_internal(
            task.owner_actor_id(),
            claim,
            "approval-resolve",
            sequence,
            vec![event],
            now_ms,
        )
    }

    fn record_runtime_update(
        &mut self,
        claim: &LeaseClaim,
        sequence: u64,
        update: RuntimeUpdate,
        now_ms: u64,
    ) -> Result<TaskView, GatewayDaemonError> {
        self.require_current_lease(claim, now_ms)?;
        let task = self.store.load_task(&claim.task_id)?;
        let event = self.event(
            task.owner_actor_id(),
            &claim.task_id,
            Some(&claim.run_id),
            task.revision().saturating_add(1),
            now_ms,
            TaskEvent::RuntimeEventRecorded {
                run_id: claim.run_id.clone(),
                update,
            },
        );
        self.commit_internal(
            task.owner_actor_id(),
            claim,
            "update",
            sequence,
            vec![event],
            now_ms,
        )
    }

    fn settle_succeeded(
        &mut self,
        claim: &LeaseClaim,
        now_ms: u64,
    ) -> Result<TaskView, GatewayDaemonError> {
        self.require_current_lease(claim, now_ms)?;
        let task = self.store.load_task(&claim.task_id)?;
        let revision = task
            .revision()
            .checked_add(1)
            .ok_or_else(|| GatewayDaemonError::Protocol("Task revision overflow".to_owned()))?;
        let run = self.event(
            task.owner_actor_id(),
            &claim.task_id,
            Some(&claim.run_id),
            revision,
            now_ms,
            TaskEvent::RunSucceeded {
                run_id: claim.run_id.clone(),
            },
        );
        let completed = self.event(
            task.owner_actor_id(),
            &claim.task_id,
            Some(&claim.run_id),
            revision.saturating_add(1),
            now_ms,
            TaskEvent::TaskSucceeded,
        );
        self.commit_internal(
            task.owner_actor_id(),
            claim,
            "succeed",
            0,
            vec![run, completed],
            now_ms,
        )
    }

    fn settle_failed(
        &mut self,
        claim: &LeaseClaim,
        error: ContractError,
        now_ms: u64,
    ) -> Result<TaskView, GatewayDaemonError> {
        self.require_current_lease(claim, now_ms)?;
        let task = self.store.load_task(&claim.task_id)?;
        let revision = task
            .revision()
            .checked_add(1)
            .ok_or_else(|| GatewayDaemonError::Protocol("Task revision overflow".to_owned()))?;
        let run = self.event(
            task.owner_actor_id(),
            &claim.task_id,
            Some(&claim.run_id),
            revision,
            now_ms,
            TaskEvent::RunFailed {
                run_id: claim.run_id.clone(),
                error: error.clone(),
            },
        );
        let events = if error.retryable {
            vec![run]
        } else {
            let completed_revision = revision
                .checked_add(1)
                .ok_or_else(|| GatewayDaemonError::Protocol("Task revision overflow".to_owned()))?;
            let completed = self.event(
                task.owner_actor_id(),
                &claim.task_id,
                Some(&claim.run_id),
                completed_revision,
                now_ms,
                TaskEvent::TaskFailed { error },
            );
            vec![run, completed]
        };
        self.commit_internal(task.owner_actor_id(), claim, "fail", 0, events, now_ms)
    }

    fn settle_cancelled(
        &mut self,
        claim: &LeaseClaim,
        now_ms: u64,
    ) -> Result<TaskView, GatewayDaemonError> {
        self.require_current_lease(claim, now_ms)?;
        let task = self.store.load_task(&claim.task_id)?;
        let revision = task.revision().saturating_add(1);
        let run = self.event(
            task.owner_actor_id(),
            &claim.task_id,
            Some(&claim.run_id),
            revision,
            now_ms,
            TaskEvent::RunCancelled {
                run_id: claim.run_id.clone(),
                stage: CancellationStage::Runtime,
            },
        );
        let completed = self.event(
            task.owner_actor_id(),
            &claim.task_id,
            Some(&claim.run_id),
            revision.saturating_add(1),
            now_ms,
            TaskEvent::TaskCancelled,
        );
        self.commit_internal(
            task.owner_actor_id(),
            claim,
            "cancel",
            0,
            vec![run, completed],
            now_ms,
        )
    }

    fn commit_internal(
        &mut self,
        actor_id: &ActorId,
        claim: &LeaseClaim,
        operation: &str,
        sequence: u64,
        events: Vec<cosh_gateway_contracts::task::TaskEventEnvelope>,
        now_ms: u64,
    ) -> Result<TaskView, GatewayDaemonError> {
        let revision = events
            .first()
            .map_or(0, |event| event.revision.saturating_sub(1));
        self.store.commit_task(&TaskCommit {
            actor_id: actor_id.clone(),
            idempotency_key: IdempotencyKey::new(format!(
                "scheduler-{operation}-{}-{}-{sequence}",
                claim.run_id.as_str(),
                claim.generation
            ))
            .map_err(|error| GatewayDaemonError::Protocol(error.to_string()))?,
            command_digest: digest_json(&(
                operation,
                &claim.task_id,
                &claim.run_id,
                &claim.lease_owner,
                claim.generation,
                claim.revision,
                sequence,
                &events,
            ))?,
            expected_revision: Some(revision),
            events,
            outbox: Vec::new(),
            committed_at_ms: now_ms,
        })?;
        Ok(TaskView::from(&self.store.load_task(&claim.task_id)?))
    }

    fn require_current_lease(
        &self,
        claim: &LeaseClaim,
        now_ms: u64,
    ) -> Result<(), GatewayDaemonError> {
        let lease = self.store.load_run_lease(&claim.run_id)?;
        if lease.task_id != claim.task_id
            || lease.lease_owner != claim.lease_owner
            || lease.generation != claim.generation
            || lease.revision != claim.revision
            || lease.expires_at_ms <= now_ms
        {
            return Err(StoreError::GenerationFenced {
                expected: claim.generation,
                actual: lease.generation,
            }
            .into());
        }
        Ok(())
    }

    fn release_lease(&mut self, claim: &LeaseClaim, now_ms: u64) -> Result<(), GatewayDaemonError> {
        let task = self.store.load_task(&claim.task_id)?;
        let command = LedgerCommand {
            actor_id: task.owner_actor_id().clone(),
            idempotency_key: IdempotencyKey::new(format!(
                "scheduler-release-{}-{}",
                claim.run_id.as_str(),
                claim.generation
            ))
            .map_err(|error| GatewayDaemonError::Protocol(error.to_string()))?,
            command_digest: digest_json(&(
                "release_run_lease",
                &claim.task_id,
                &claim.run_id,
                &claim.lease_owner,
                claim.generation,
                claim.revision,
            ))?,
            committed_at_ms: now_ms,
        };
        self.store.release_run_lease(&command, claim)?;
        Ok(())
    }
}

fn internal_ledger_command(
    actor_id: &ActorId,
    operation: &str,
    claim: &OutboxClaim,
    committed_at_ms: u64,
    digest_value: &impl Serialize,
) -> Result<LedgerCommand, GatewayDaemonError> {
    Ok(LedgerCommand {
        actor_id: actor_id.clone(),
        idempotency_key: internal_key(operation, claim),
        command_digest: digest_json(digest_value)?,
        committed_at_ms,
    })
}

fn provider_dispatch_command(
    actor_id: &ActorId,
    operation: &str,
    approval_id: &ApprovalId,
    expected_revision: u64,
    committed_at_ms: u64,
) -> Result<LedgerCommand, GatewayDaemonError> {
    Ok(LedgerCommand {
        actor_id: actor_id.clone(),
        idempotency_key: IdempotencyKey::new(format!(
            "scheduler-provider-{operation}-{}-{expected_revision}",
            approval_id.as_str()
        ))
        .map_err(|error| GatewayDaemonError::Protocol(error.to_string()))?,
        command_digest: digest_json(&(operation, approval_id, expected_revision))?,
        committed_at_ms,
    })
}

fn runtime_input_command(
    actor_id: &ActorId,
    operation: &str,
    request_id: &InputRequestId,
    expected_revision: u64,
    committed_at_ms: u64,
) -> Result<LedgerCommand, GatewayDaemonError> {
    Ok(LedgerCommand {
        actor_id: actor_id.clone(),
        idempotency_key: IdempotencyKey::new(format!(
            "scheduler-input-{operation}-{}-{expected_revision}",
            request_id.as_str()
        ))
        .map_err(|error| GatewayDaemonError::Protocol(error.to_string()))?,
        command_digest: digest_json(&(operation, request_id, expected_revision))?,
        committed_at_ms,
    })
}

fn approval_resolution_revision(
    approval: &crate::storage::ApprovalRecord,
    decision: ApprovalDecision,
) -> Result<u64, GatewayDaemonError> {
    match (approval.state, decision) {
        (ApprovalState::Pending, _) => Ok(approval.revision),
        (ApprovalState::Approved, ApprovalDecision::Approve)
        | (ApprovalState::Denied, ApprovalDecision::Deny) => {
            approval.revision.checked_sub(1).ok_or_else(|| {
                GatewayDaemonError::Protocol(
                    "resolved approval has an invalid durable revision".to_owned(),
                )
            })
        }
        (ApprovalState::Approved, ApprovalDecision::Deny)
        | (ApprovalState::Denied, ApprovalDecision::Approve) => Err(GatewayDaemonError::Protocol(
            "approval was already resolved with a different decision".to_owned(),
        )),
        (ApprovalState::Expired | ApprovalState::Cancelled, _) => Err(
            GatewayDaemonError::Protocol("approval is no longer resolvable".to_owned()),
        ),
    }
}

fn provider_dispatch_decision(decision: ApprovalDecision) -> ProviderPermissionDispatchDecision {
    match decision {
        ApprovalDecision::Approve => ProviderPermissionDispatchDecision::AllowOnce,
        ApprovalDecision::Deny => ProviderPermissionDispatchDecision::Deny,
    }
}

fn internal_key(operation: &str, claim: &OutboxClaim) -> IdempotencyKey {
    IdempotencyKey::new(format!(
        "scheduler-{operation}-{}-{}",
        claim.delivery_id.as_str(),
        claim.attempt
    ))
    .unwrap_or_else(|_| unreachable!())
}

fn deadline(now_ms: u64, duration_ms: u64) -> Result<u64, GatewayDaemonError> {
    now_ms
        .checked_add(duration_ms)
        .ok_or_else(|| GatewayDaemonError::Protocol("scheduler lease deadline overflow".to_owned()))
}

fn duration_ms(duration: Duration, label: &str) -> Result<u64, GatewayDaemonError> {
    u64::try_from(duration.as_millis())
        .map_err(|_| GatewayDaemonError::Protocol(format!("{label} exceeds the supported range")))
}

fn refreshed_now_ms(previous_ms: u64) -> Result<u64, GatewayDaemonError> {
    Ok(super::now_ms()?.max(previous_ms))
}

fn renew_for_operation(
    coordinator: &mut TaskCoordinator,
    actor_id: &ActorId,
    claim: &mut LeaseClaim,
    lease_expires_at_ms: &mut u64,
    config: ValidatedSchedulerConfig,
    now_ms: u64,
) -> Result<(), GatewayDaemonError> {
    if now_ms >= *lease_expires_at_ms {
        return Err(stale_operation_error());
    }
    let required_remaining = config
        .runtime_operation_timeout_ms
        .checked_add(config.lease_renewal_margin_ms)
        .ok_or_else(|| {
            GatewayDaemonError::Protocol("scheduler Runtime operation budget overflow".to_owned())
        })?;
    if lease_expires_at_ms.saturating_sub(now_ms) <= required_remaining {
        let renewed_until = deadline(now_ms, config.lease_duration_ms)?;
        *claim = coordinator.renew_lease(actor_id, claim, renewed_until, now_ms)?;
        *lease_expires_at_ms = renewed_until;
    }
    Ok(())
}

fn stale_operation_error() -> GatewayDaemonError {
    StoreError::GenerationFenced {
        expected: 0,
        actual: 0,
    }
    .into()
}

fn no_active_run() -> GatewayDaemonError {
    GatewayDaemonError::Protocol("scheduler has no active Runtime".to_owned())
}

fn runtime_lost_error(code: &str, message: &str) -> Result<ContractError, GatewayDaemonError> {
    ContractError::new(code, ErrorCategory::RuntimeUnavailable, false, message)
        .map_err(|error| GatewayDaemonError::Protocol(error.to_string()))
}

#[cfg(test)]
mod tests;
