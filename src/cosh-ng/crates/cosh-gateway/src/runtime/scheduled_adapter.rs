//! Adapts one provider-neutral Agent Runtime port to the durable scheduler.

use std::time::{Duration, Instant};

use cosh_gateway_contracts::{
    common::{BoundedText, ContentPart, RuntimeBindingRef, WorkspaceRef},
    error::{ContractError, ErrorCategory},
    ids::{InputRequestId, RuntimeBindingId, TurnId},
    runtime::{
        AgentRuntimeCommand, AgentRuntimeEvent, BrokeredExecutionDelivery, BrokeredExecutionRef,
        BrokeredRequestAcknowledgement, RuntimeEventEnvelope, RuntimeInputRequest,
        RuntimeInputResponse, RuntimePermissionDecision, RuntimePermissionRef, TurnOutcome,
    },
};

use crate::daemon::{RuntimeFactory, RuntimeHandle, RuntimePoll, ScheduledRun, StartedRuntime};
use cosh_gateway_contracts::task::CancelReason;

use super::{AgentRuntimePort, AgentRuntimePortError};

const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(70);
const EVENT_POLL_TIMEOUT: Duration = Duration::from_millis(10);

/// A newly created Runtime port and its trusted workspace projection.
///
/// Profile resolution, process identity, actor authentication, and permission
/// normalization remain owned by the injected factory. The scheduler adapter
/// never derives those security-sensitive values from a Task's display data.
pub struct ScheduledRuntimePort {
    port: Box<dyn AgentRuntimePort>,
    workspace: WorkspaceRef,
}

impl ScheduledRuntimePort {
    /// Binds a Runtime port to the trusted workspace accepted by `OpenSession`.
    #[must_use]
    pub fn new(port: Box<dyn AgentRuntimePort>, workspace: WorkspaceRef) -> Self {
        Self { port, workspace }
    }
}

/// Injection boundary that resolves a scheduled Run into a concrete port.
///
/// Production implementations are responsible for validating the Runtime
/// selector and target, allocating fenced identities, and installing a safe
/// permission normalizer before returning the port.
pub trait AgentRuntimePortFactory: Send {
    /// Creates one unopened Runtime port for an already-fenced scheduled Run.
    ///
    /// # Errors
    ///
    /// Returns a bounded failure when the profile, target, identity, workspace,
    /// or supervised Runtime cannot be resolved safely.
    fn create(&mut self, run: &ScheduledRun) -> Result<ScheduledRuntimePort, ContractError>;
}

impl<T: AgentRuntimePortFactory + ?Sized> AgentRuntimePortFactory for Box<T> {
    fn create(&mut self, run: &ScheduledRun) -> Result<ScheduledRuntimePort, ContractError> {
        (**self).create(run)
    }
}

/// Scheduler factory backed by an injected provider-neutral Runtime factory.
pub struct ScheduledAgentRuntimeFactory<F> {
    port_factory: F,
    command_timeout: Duration,
}

impl<F> ScheduledAgentRuntimeFactory<F> {
    /// Creates an adapter with the conservative Runtime command timeout.
    #[must_use]
    pub fn new(port_factory: F) -> Self {
        Self {
            port_factory,
            command_timeout: DEFAULT_COMMAND_TIMEOUT,
        }
    }

    /// Overrides the deadline shared by open, prompt, permission, and close.
    ///
    /// Zero is rejected when the first Runtime is started.
    #[must_use]
    pub fn with_command_timeout(mut self, command_timeout: Duration) -> Self {
        self.command_timeout = command_timeout;
        self
    }
}

impl<F: AgentRuntimePortFactory> RuntimeFactory for ScheduledAgentRuntimeFactory<F> {
    fn open(&mut self, run: &ScheduledRun) -> Result<StartedRuntime, ContractError> {
        if self.command_timeout.is_zero() {
            return Err(contract_error(
                "runtime_deadline_invalid",
                ErrorCategory::InvalidRequest,
                false,
                "The Runtime command deadline is invalid",
            ));
        }

        let ScheduledRuntimePort {
            mut port,
            workspace,
        } = self.port_factory.create(run)?;
        if workspace != run.workspace {
            return Err(contract_error(
                "runtime_workspace_projection_mismatch",
                ErrorCategory::InvalidRequest,
                false,
                "The resolved Runtime workspace does not match the admitted workspace",
            ));
        }
        let binding_id = port.binding_id().clone();
        let open_deadline = deadline(self.command_timeout);
        port.dispatch(
            AgentRuntimeCommand::OpenSession {
                task_id: run.task_id.clone(),
                run_id: run.run_id.clone(),
                workspace,
            },
            open_deadline,
        )
        .map_err(map_port_error)?;

        let opened = port.next_event(open_deadline).map_err(map_port_error)?;
        validate_event(run, &binding_id, 1, &opened)?;
        let binding = match opened.event {
            AgentRuntimeEvent::SessionOpened { binding }
                if binding.binding_id == binding_id
                    && binding.task_id == run.task_id
                    && binding.run_id == run.run_id
                    && binding.runtime_generation == run.lease_generation =>
            {
                binding
            }
            _ => {
                return Err(contract_error(
                    "runtime_event_order_invalid",
                    ErrorCategory::Internal,
                    false,
                    "The Runtime emitted an invalid session event",
                ));
            }
        };

        let handle = ScheduledAgentRuntimeHandle {
            port,
            actor_id: run.actor.actor_id.clone(),
            task_id: run.task_id.clone(),
            run_id: run.run_id.clone(),
            target: run.target.clone(),
            binding_id,
            binding: binding.clone(),
            turn_id: TurnId::new(),
            intent: run.intent.clone(),
            last_sequence: 1,
            turn_started: false,
            prompt_started: false,
            pending_permission: None,
            pending_brokered: None,
            pending_input: None,
            command_timeout: self.command_timeout,
            terminal: false,
        };
        Ok(StartedRuntime {
            binding,
            handle: Box::new(handle),
        })
    }
}

struct ScheduledAgentRuntimeHandle {
    port: Box<dyn AgentRuntimePort>,
    actor_id: cosh_gateway_contracts::ids::ActorId,
    task_id: cosh_gateway_contracts::ids::TaskId,
    run_id: cosh_gateway_contracts::ids::RunId,
    target: cosh_gateway_contracts::common::TargetRef,
    binding_id: RuntimeBindingId,
    binding: RuntimeBindingRef,
    turn_id: TurnId,
    intent: BoundedText,
    last_sequence: u64,
    turn_started: bool,
    prompt_started: bool,
    pending_permission: Option<RuntimePermissionRef>,
    pending_brokered: Option<PendingBrokeredCallback>,
    pending_input: Option<InputRequestId>,
    command_timeout: Duration,
    terminal: bool,
}

struct PendingBrokeredCallback {
    reference: BrokeredExecutionRef,
    acknowledged: bool,
    dispatch_indeterminate: bool,
}

impl ScheduledAgentRuntimeHandle {
    fn poll_event(&mut self) -> RuntimePoll {
        let event = match self.port.next_event(deadline(EVENT_POLL_TIMEOUT)) {
            Ok(event) => event,
            Err(AgentRuntimePortError::Deadline { .. }) => return RuntimePoll::Pending,
            Err(error) => return self.fail(map_port_error(error)),
        };
        let expected_sequence = match self.last_sequence.checked_add(1) {
            Some(sequence) => sequence,
            None => {
                return self.fail(contract_error(
                    "runtime_event_sequence_exhausted",
                    ErrorCategory::Internal,
                    false,
                    "The Runtime event sequence exceeded its supported range",
                ));
            }
        };
        if let Err(error) = validate_event_ids(
            &self.actor_id,
            &self.task_id,
            &self.run_id,
            &self.binding_id,
            expected_sequence,
            &event,
        ) {
            return self.fail(error);
        }
        self.last_sequence = expected_sequence;

        match event.event {
            AgentRuntimeEvent::TurnStarted { turn_id }
                if turn_id == self.turn_id && !self.turn_started =>
            {
                self.turn_started = true;
                RuntimePoll::Observed {
                    sequence: self.last_sequence,
                }
            }
            AgentRuntimeEvent::MessageChunk {
                content: ContentPart::Text { text },
                ..
            } if self.turn_started => RuntimePoll::Update {
                sequence: self.last_sequence,
                update: cosh_gateway_contracts::task::RuntimeUpdate::Progress { summary: text },
            },
            AgentRuntimeEvent::MessageChunk { .. }
            | AgentRuntimeEvent::ToolCallObserved { .. }
            | AgentRuntimeEvent::ToolInvocationUpdated { .. }
            | AgentRuntimeEvent::UsageUpdated { .. }
                if self.turn_started =>
            {
                RuntimePoll::Observed {
                    sequence: self.last_sequence,
                }
            }
            AgentRuntimeEvent::PermissionRequested { .. } => self.fail(contract_error(
                "runtime_permission_authority_missing",
                ErrorCategory::Internal,
                false,
                "The Runtime permission event omitted its execution authority",
            )),
            AgentRuntimeEvent::InputRequested { request }
                if self.turn_started
                    && request.run_id() == &self.run_id
                    && request.turn_id() == &self.turn_id
                    && self.pending_input.is_none() =>
            {
                self.pending_input = Some(request.request_id().clone());
                RuntimePoll::InputRequested {
                    sequence: self.last_sequence,
                    request,
                }
            }
            AgentRuntimeEvent::ExecutionPermissionRequested {
                turn_id,
                tool_use_id,
                request,
                summary,
            } if self.turn_started
                && turn_id == self.turn_id
                && request.task_id == self.task_id
                && request.run_id == self.run_id
                && request.actor.actor_id == self.actor_id
                && request.target == self.target =>
            {
                let permission = RuntimePermissionRef {
                    binding_id: self.binding_id.clone(),
                    runtime_generation: self.binding.runtime_generation,
                    event_sequence: self.last_sequence,
                    run_id: self.run_id.clone(),
                    turn_id,
                    tool_use_id,
                    request_id: request.request_id.clone(),
                };
                self.pending_permission = Some(permission.clone());
                RuntimePoll::PermissionRequested {
                    permission,
                    request: Box::new(request),
                    summary,
                }
            }
            AgentRuntimeEvent::BrokeredExecutionRequested {
                turn_id,
                tool_use_id,
                request,
                operation,
                summary,
            } if self.turn_started
                && turn_id == self.turn_id
                && request.task_id == self.task_id
                && request.run_id == self.run_id
                && request.actor.actor_id == self.actor_id
                && request.target == self.target
                && self.pending_permission.is_none()
                && self.pending_brokered.is_none() =>
            {
                let brokered = BrokeredExecutionRef {
                    binding_id: self.binding_id.clone(),
                    runtime_generation: self.binding.runtime_generation,
                    event_sequence: self.last_sequence,
                    run_id: self.run_id.clone(),
                    turn_id,
                    tool_use_id,
                    request_id: request.request_id.clone(),
                    operation: operation.clone(),
                };
                self.pending_brokered = Some(PendingBrokeredCallback {
                    reference: brokered.clone(),
                    acknowledged: false,
                    dispatch_indeterminate: false,
                });
                RuntimePoll::BrokeredExecutionRequested {
                    brokered,
                    request: Box::new(request),
                    operation,
                    summary,
                }
            }
            AgentRuntimeEvent::Completed { turn_id, outcome }
                if self.turn_started && turn_id == self.turn_id =>
            {
                self.settle_turn(outcome)
            }
            AgentRuntimeEvent::TransportFailed { error } => {
                self.terminal = true;
                RuntimePoll::Failed(error)
            }
            _ => self.fail(contract_error(
                "runtime_event_order_invalid",
                ErrorCategory::Internal,
                false,
                "The Runtime emitted an event outside its active lifecycle",
            )),
        }
    }

    fn settle_turn(&mut self, outcome: TurnOutcome) -> RuntimePoll {
        let poll = match outcome {
            TurnOutcome::Completed => RuntimePoll::Succeeded,
            TurnOutcome::LimitReached { .. } => RuntimePoll::Failed(contract_error(
                "runtime_turn_limit_reached",
                ErrorCategory::RuntimeUnavailable,
                false,
                "The Agent turn stopped after reaching a configured limit",
            )),
            TurnOutcome::Refused => RuntimePoll::Failed(contract_error(
                "runtime_turn_refused",
                ErrorCategory::RuntimeUnavailable,
                false,
                "The Agent refused the scheduled task",
            )),
            TurnOutcome::Cancelled => RuntimePoll::Failed(contract_error(
                "runtime_turn_cancelled_unsolicited",
                ErrorCategory::Cancelled,
                false,
                "The Agent cancelled the turn without a durable cancellation request",
            )),
            TurnOutcome::Failed { error } => RuntimePoll::Failed(error),
        };
        match poll {
            RuntimePoll::Succeeded => match self.close() {
                Ok(()) => poll,
                Err(error) => RuntimePoll::Failed(error),
            },
            RuntimePoll::Failed(_) => {
                // Preserve the known turn result; close diagnostics are
                // separately governed and cannot make that result less known.
                let _ = self.close();
                poll
            }
            RuntimePoll::Pending
            | RuntimePoll::Observed { .. }
            | RuntimePoll::Update { .. }
            | RuntimePoll::PermissionRequested { .. }
            | RuntimePoll::BrokeredExecutionRequested { .. }
            | RuntimePoll::InputRequested { .. }
            | RuntimePoll::Cancelled => {
                unreachable!("turn settlement must be terminal")
            }
        }
    }

    fn close(&mut self) -> Result<(), ContractError> {
        if self.terminal {
            return Ok(());
        }
        self.port
            .dispatch(
                AgentRuntimeCommand::Close {
                    binding: self.binding.clone(),
                },
                deadline(self.command_timeout),
            )
            .map_err(map_port_error)?;
        self.terminal = true;
        Ok(())
    }

    fn fail(&mut self, error: ContractError) -> RuntimePoll {
        let _ = self.close();
        RuntimePoll::Failed(error)
    }
}

impl RuntimeHandle for ScheduledAgentRuntimeHandle {
    fn begin(&mut self) -> Result<(), ContractError> {
        if self.terminal || self.prompt_started {
            return Err(contract_error(
                "runtime_begin_state_invalid",
                ErrorCategory::Conflict,
                false,
                "The Runtime prompt cannot start in its current state",
            ));
        }
        if let Err(error) = self.port.dispatch(
            AgentRuntimeCommand::Prompt {
                run_id: self.run_id.clone(),
                turn_id: self.turn_id.clone(),
                input: vec![ContentPart::Text {
                    text: self.intent.clone(),
                }],
            },
            deadline(self.command_timeout),
        ) {
            let mapped = map_port_error(error);
            let _ = self.close();
            return Err(mapped);
        }
        self.prompt_started = true;
        Ok(())
    }

    fn poll(&mut self) -> RuntimePoll {
        if self.terminal || !self.prompt_started {
            return RuntimePoll::Failed(contract_error(
                "runtime_already_terminal",
                ErrorCategory::Conflict,
                false,
                "The Runtime handle is already terminal",
            ));
        }
        if self.pending_permission.is_some()
            || self.pending_brokered.is_some()
            || self.pending_input.is_some()
        {
            return RuntimePoll::Pending;
        }
        self.poll_event()
    }

    fn shutdown(&mut self, reason: CancelReason) -> Result<(), ContractError> {
        if self.terminal {
            return Ok(());
        }
        if !self.prompt_started {
            return self.close();
        }
        let shutdown_deadline = deadline(self.command_timeout);
        self.port
            .dispatch(
                AgentRuntimeCommand::Cancel {
                    run_id: self.run_id.clone(),
                    turn_id: self.turn_id.clone(),
                    cause: reason,
                },
                shutdown_deadline,
            )
            .map_err(map_port_error)?;
        // ACP and Core cancellation dispatches already wait for process
        // settlement. Close is therefore cleanup-only and may legitimately
        // report that the port is terminal after cancellation was acknowledged.
        let _ = self.port.dispatch(
            AgentRuntimeCommand::Close {
                binding: self.binding.clone(),
            },
            shutdown_deadline,
        );
        self.terminal = true;
        Ok(())
    }

    fn resolve_provider_permission(
        &mut self,
        permission: &RuntimePermissionRef,
        decision: RuntimePermissionDecision,
    ) -> Result<(), ContractError> {
        if self.pending_permission.as_ref() != Some(permission) {
            return Err(contract_error(
                "runtime_permission_identity_invalid",
                ErrorCategory::Conflict,
                false,
                "The provider permission response does not match the pending callback",
            ));
        }
        self.port
            .dispatch(
                AgentRuntimeCommand::ResolvePermission {
                    request_id: permission.request_id.clone(),
                    decision,
                },
                deadline(self.command_timeout),
            )
            .map_err(map_port_error)?;
        self.pending_permission = None;
        Ok(())
    }

    fn resolve_input(
        &mut self,
        request: &RuntimeInputRequest,
        response: RuntimeInputResponse,
    ) -> Result<(), ContractError> {
        if self.pending_input.as_ref() != Some(request.request_id())
            || request.run_id() != &self.run_id
            || request.turn_id() != &self.turn_id
        {
            return Err(contract_error(
                "runtime_input_identity_invalid",
                ErrorCategory::Conflict,
                false,
                "The input response does not match the pending Runtime request",
            ));
        }
        self.port
            .dispatch(
                AgentRuntimeCommand::ResolveInput {
                    request_id: request.request_id().clone(),
                    run_id: request.run_id().clone(),
                    turn_id: request.turn_id().clone(),
                    response,
                },
                deadline(self.command_timeout),
            )
            .map_err(map_port_error)?;
        self.pending_input = None;
        Ok(())
    }

    fn acknowledge_brokered_request(
        &mut self,
        brokered: &BrokeredExecutionRef,
        acknowledgement: BrokeredRequestAcknowledgement,
    ) -> Result<(), ContractError> {
        let pending = self.pending_brokered.as_mut().ok_or_else(|| {
            contract_error(
                "runtime_brokered_identity_invalid",
                ErrorCategory::Conflict,
                false,
                "The brokered acknowledgement does not match a pending callback",
            )
        })?;
        if pending.reference != *brokered
            || pending.reference.request_id != acknowledgement.request_id
            || pending.acknowledged
            || pending.dispatch_indeterminate
        {
            return Err(contract_error(
                "runtime_brokered_identity_invalid",
                ErrorCategory::Conflict,
                false,
                "The brokered acknowledgement does not match the pending callback",
            ));
        }
        pending.dispatch_indeterminate = true;
        let dispatch = self.port.dispatch(
            AgentRuntimeCommand::AcknowledgeBrokeredRequest { acknowledgement },
            deadline(self.command_timeout),
        );
        match dispatch {
            Ok(()) => {
                let pending = self
                    .pending_brokered
                    .as_mut()
                    .unwrap_or_else(|| unreachable!("pending callback cannot disappear"));
                pending.acknowledged = true;
                pending.dispatch_indeterminate = false;
                Ok(())
            }
            Err(error) => {
                let error = map_port_error(error);
                let _ = self.close();
                Err(error)
            }
        }
    }

    fn deliver_brokered_result(
        &mut self,
        brokered: &BrokeredExecutionRef,
        delivery: BrokeredExecutionDelivery,
    ) -> Result<(), ContractError> {
        let pending = self.pending_brokered.as_mut().ok_or_else(|| {
            contract_error(
                "runtime_brokered_identity_invalid",
                ErrorCategory::Conflict,
                false,
                "The brokered result does not match a pending callback",
            )
        })?;
        if pending.reference != *brokered
            || pending.reference.request_id != delivery.request_id
            || !pending.acknowledged
            || pending.dispatch_indeterminate
        {
            return Err(contract_error(
                "runtime_brokered_identity_invalid",
                ErrorCategory::Conflict,
                false,
                "The brokered result does not match the pending callback",
            ));
        }
        pending.dispatch_indeterminate = true;
        let dispatch = self.port.dispatch(
            AgentRuntimeCommand::DeliverBrokeredResult { delivery },
            deadline(self.command_timeout),
        );
        match dispatch {
            Ok(()) => {
                self.pending_brokered = None;
                Ok(())
            }
            Err(error) => {
                let error = map_port_error(error);
                let _ = self.close();
                Err(error)
            }
        }
    }
}

fn validate_event(
    run: &ScheduledRun,
    binding_id: &RuntimeBindingId,
    expected_sequence: u64,
    event: &RuntimeEventEnvelope,
) -> Result<(), ContractError> {
    validate_event_ids(
        &run.actor.actor_id,
        &run.task_id,
        &run.run_id,
        binding_id,
        expected_sequence,
        event,
    )
}

fn validate_event_ids(
    actor_id: &cosh_gateway_contracts::ids::ActorId,
    task_id: &cosh_gateway_contracts::ids::TaskId,
    run_id: &cosh_gateway_contracts::ids::RunId,
    binding_id: &RuntimeBindingId,
    expected_sequence: u64,
    event: &RuntimeEventEnvelope,
) -> Result<(), ContractError> {
    let correlation = &event.header.correlation;
    if event.header.validate_version().is_err()
        || event
            .header
            .validate_schema(cosh_gateway_contracts::common::ContractSchema::RuntimeEvent)
            .is_err()
        || &event.binding_id != binding_id
        || event.sequence != expected_sequence
        || correlation.actor_id.as_ref() != Some(actor_id)
        || correlation.task_id.as_ref() != Some(task_id)
        || correlation.run_id.as_ref() != Some(run_id)
        || correlation.runtime_binding_id.as_ref() != Some(binding_id)
    {
        return Err(contract_error(
            "runtime_event_identity_invalid",
            ErrorCategory::Internal,
            false,
            "The Runtime emitted an event with invalid identity or ordering",
        ));
    }
    Ok(())
}

fn deadline(timeout: Duration) -> Instant {
    Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(Instant::now)
}

fn map_port_error(error: AgentRuntimePortError) -> ContractError {
    match error {
        AgentRuntimePortError::Deadline { .. } => contract_error(
            "runtime_deadline_exceeded",
            ErrorCategory::Transport,
            true,
            "The Runtime operation exceeded its deadline",
        ),
        AgentRuntimePortError::Transport => contract_error(
            "runtime_transport_failed",
            ErrorCategory::Transport,
            true,
            "The Runtime transport failed",
        ),
        AgentRuntimePortError::Unsupported { .. } => contract_error(
            "runtime_operation_unsupported",
            ErrorCategory::RuntimeUnavailable,
            false,
            "The Runtime does not support a required operation",
        ),
        AgentRuntimePortError::InvalidState { .. }
        | AgentRuntimePortError::IdentityMismatch
        | AgentRuntimePortError::WorkspaceMismatch
        | AgentRuntimePortError::Protocol => contract_error(
            "runtime_protocol_failed",
            ErrorCategory::Internal,
            false,
            "The Runtime violated its lifecycle contract",
        ),
        AgentRuntimePortError::Terminal => contract_error(
            "runtime_terminal_unexpected",
            ErrorCategory::RuntimeUnavailable,
            false,
            "The Runtime became terminal before completing the task",
        ),
    }
}

fn contract_error(
    code: &'static str,
    category: ErrorCategory,
    retryable: bool,
    message: &'static str,
) -> ContractError {
    ContractError::new(code, category, retryable, message)
        .unwrap_or_else(|_| unreachable!("static Runtime error must remain valid"))
}

#[cfg(test)]
#[path = "scheduled_adapter/tests.rs"]
mod tests;
