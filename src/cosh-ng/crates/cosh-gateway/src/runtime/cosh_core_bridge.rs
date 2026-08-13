//! Gateway-owned bridge from private cosh-core JSONL to neutral Runtime events.
//!
//! Ownership note: lifecycle and mapping stay together for the first bridge
//! slice. New private message families must first extract pure mapping state
//! into `runtime/cosh_core_bridge/mapping.rs` instead of extending this file.

use std::collections::{btree_map::Entry, BTreeMap, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cosh_gateway_contracts::{
    common::{
        BoundedName, BoundedOpaque, BoundedText, ContentPart, ContractHeader, ContractSchema,
        Correlation, Digest, RuntimeBindingRef, WorkspaceRef,
    },
    error::{ContractError, ErrorCategory},
    external::{ExternalRef, ExternalRefKind},
    ids::{
        ActorId, AgentSessionId, InstallationId, MessageId, RunId, RuntimeBindingId,
        RuntimeInstanceId, RuntimeMessageId, TaskId, ToolUseId,
    },
    runtime::{
        AgentRuntimeCommand, AgentRuntimeEvent, RunOutcome, RuntimeEventEnvelope, ToolSummary,
    },
};

use super::{
    AgentRuntimePort, AgentRuntimePortError, CoshCoreContentBlockInfo, CoshCoreContentDelta,
    CoshCoreControlRequest, CoshCoreJsonlCodec, CoshCoreObservation, CoshCoreStreamEvent,
    CoshCoreUserTurn, RuntimeFrameRead, RuntimeLaunchSpec, RuntimeState, RuntimeSupervisor,
};

const READ_POLL_INTERVAL: Duration = Duration::from_millis(20);
const MAX_TOOL_USES_PER_TURN: usize = 1024;

/// COSH-owned identities and provider scope for one Core process generation.
#[derive(Debug, Clone)]
pub struct CoshCoreBridgeIdentity {
    /// Durable Gateway installation.
    pub installation_id: InstallationId,
    /// Authenticated actor propagated into public correlation when known.
    pub actor_id: Option<ActorId>,
    /// Task owning this bridge.
    pub task_id: TaskId,
    /// Run owning this bridge generation.
    pub run_id: RunId,
    /// Logical Agent session allocated by COSH.
    pub agent_session_id: AgentSessionId,
    /// Fenced binding allocated by COSH.
    pub binding_id: RuntimeBindingId,
    /// Supervised process identity allocated by COSH.
    pub runtime_instance_id: RuntimeInstanceId,
    /// Monotonic process generation used to reject stale output.
    pub runtime_generation: u64,
    /// Trusted provider namespace, such as `cosh-core`.
    pub provider_authority: BoundedName,
    /// Digest of the complete provider-session parent scope.
    pub provider_scope_digest: Digest,
}

/// Immutable launch, identity, and deadline settings for a Core bridge.
#[derive(Clone)]
pub struct CoshCoreBridgeConfig {
    /// Direct supervised cosh-core launch specification.
    pub launch: RuntimeLaunchSpec,
    /// Public workspace scope expected by `OpenSession`.
    pub workspace: WorkspaceRef,
    /// COSH-owned lifecycle identities.
    pub identity: CoshCoreBridgeIdentity,
    /// Maximum private JSONL frame size.
    pub max_frame_bytes: usize,
    /// Maximum lifetime of one active turn.
    pub prompt_timeout: Duration,
    /// TERM grace before KILL escalation.
    pub shutdown_grace: Duration,
}

impl CoshCoreBridgeConfig {
    /// Builds a configuration with conservative local deadlines.
    #[must_use]
    pub fn new(
        launch: RuntimeLaunchSpec,
        workspace: WorkspaceRef,
        identity: CoshCoreBridgeIdentity,
    ) -> Self {
        let max_frame_bytes = launch.stdout_line_limit;
        Self {
            launch,
            workspace,
            identity,
            max_frame_bytes,
            prompt_timeout: Duration::from_secs(30 * 60),
            shutdown_grace: Duration::from_secs(2),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BridgeState {
    Created,
    Opening,
    SessionOpenedPending,
    SessionOpen,
    PromptActive,
    Terminal,
}

impl BridgeState {
    fn name(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Opening => "opening",
            Self::SessionOpenedPending => "session-opened-pending",
            Self::SessionOpen => "session-open",
            Self::PromptActive => "prompt-active",
            Self::Terminal => "terminal",
        }
    }
}

/// One supervised private Core process exposed through `AgentRuntimePort`.
pub struct CoshCoreBridge {
    supervisor: RuntimeSupervisor,
    codec: CoshCoreJsonlCodec,
    config: CoshCoreBridgeConfig,
    state: BridgeState,
    binding: Option<RuntimeBindingRef>,
    provider_session_id: Option<String>,
    pending_events: VecDeque<RuntimeEventEnvelope>,
    sequence: u64,
    current_message: Option<RuntimeMessageId>,
    tool_ids: BTreeMap<String, ToolUseId>,
    prompt_deadline: Option<Instant>,
    terminal_delivered: bool,
}

impl CoshCoreBridge {
    /// Launches one direct Core child without admitting user input.
    ///
    /// # Errors
    ///
    /// Returns a redacted transport error when validation or spawn fails.
    pub fn launch(config: CoshCoreBridgeConfig) -> Result<Self, AgentRuntimePortError> {
        if config.prompt_timeout.is_zero() || config.shutdown_grace.is_zero() {
            return Err(AgentRuntimePortError::Deadline {
                operation: "configuration",
            });
        }
        let initialize_request_id = format!("init-{}", config.identity.runtime_instance_id);
        let codec = CoshCoreJsonlCodec::new(initialize_request_id, config.max_frame_bytes)
            .map_err(|_| AgentRuntimePortError::Protocol)?;
        let mut supervisor = RuntimeSupervisor::new();
        supervisor
            .launch(&config.launch)
            .map_err(|_| AgentRuntimePortError::Transport)?;
        Ok(Self {
            supervisor,
            codec,
            config,
            state: BridgeState::Created,
            binding: None,
            provider_session_id: None,
            pending_events: VecDeque::new(),
            sequence: 0,
            current_message: None,
            tool_ids: BTreeMap::new(),
            prompt_deadline: None,
            terminal_delivered: false,
        })
    }

    fn open_session(
        &mut self,
        task_id: TaskId,
        run_id: RunId,
        workspace: WorkspaceRef,
        deadline: Instant,
    ) -> Result<(), AgentRuntimePortError> {
        self.require_state(BridgeState::Created, "open_session")?;
        self.require_run(&task_id, &run_id)?;
        if workspace != self.config.workspace {
            return Err(AgentRuntimePortError::WorkspaceMismatch);
        }
        self.require_time(deadline, "open_session")?;
        let result = (|| {
            let frame = self
                .codec
                .initialize_frame(true)
                .map_err(|_| AgentRuntimePortError::Protocol)?;
            self.supervisor
                .write_frame(&frame)
                .map_err(|_| AgentRuntimePortError::Transport)?;
            self.state = BridgeState::Opening;
            self.wait_until_session_open(deadline)
        })();
        if result.is_err() {
            self.fail_transport("core_session_open_failed");
        }
        result
    }

    fn wait_until_session_open(&mut self, deadline: Instant) -> Result<(), AgentRuntimePortError> {
        loop {
            self.require_time(deadline, "open_session")?;
            let observation = self.read_observation(deadline, "open_session")?;
            match observation {
                CoshCoreObservation::Initialized(_) => self
                    .supervisor
                    .mark_ready()
                    .map_err(|_| AgentRuntimePortError::Transport)?,
                CoshCoreObservation::System(message) if message.subtype == "init" => {
                    let provider_session_id = message
                        .provider_session_id
                        .ok_or(AgentRuntimePortError::Protocol)?;
                    self.bind_provider_session(provider_session_id)?;
                    self.state = BridgeState::SessionOpenedPending;
                    return Ok(());
                }
                CoshCoreObservation::ControlRequest(envelope)
                    if matches!(
                        envelope.request,
                        CoshCoreControlRequest::AuthRequired { .. }
                    ) =>
                {
                    return Err(AgentRuntimePortError::Unsupported {
                        operation: "core authentication bootstrap",
                    });
                }
                _ => return Err(AgentRuntimePortError::Protocol),
            }
        }
    }

    fn prompt(
        &mut self,
        run_id: RunId,
        input: Vec<ContentPart>,
        deadline: Instant,
    ) -> Result<(), AgentRuntimePortError> {
        self.require_state(BridgeState::SessionOpen, "prompt")?;
        if run_id != self.config.identity.run_id {
            return Err(AgentRuntimePortError::IdentityMismatch);
        }
        self.require_time(deadline, "prompt")?;
        let content = prompt_text(input, self.config.max_frame_bytes)?;
        let result = self
            .codec
            .user_frame(&CoshCoreUserTurn {
                raw_user_input: Some(content.clone()),
                content,
                provider_session_id: self.provider_session_id.clone(),
                shell_context: None,
            })
            .map_err(|_| AgentRuntimePortError::Protocol)
            .and_then(|frame| {
                self.supervisor
                    .write_frame(&frame)
                    .map_err(|_| AgentRuntimePortError::Transport)
            });
        if let Err(error) = result {
            self.fail_transport("core_prompt_write_failed");
            return Err(error);
        }
        self.state = BridgeState::PromptActive;
        self.prompt_deadline = Instant::now().checked_add(self.config.prompt_timeout);
        if self.prompt_deadline.is_none() {
            self.fail_transport("core_prompt_deadline_invalid");
            return Err(AgentRuntimePortError::Deadline {
                operation: "prompt",
            });
        }
        Ok(())
    }

    fn cancel(&mut self, run_id: RunId, deadline: Instant) -> Result<(), AgentRuntimePortError> {
        if run_id != self.config.identity.run_id {
            return Err(AgentRuntimePortError::IdentityMismatch);
        }
        self.require_time(deadline, "cancel")?;
        if self.state == BridgeState::Terminal {
            return Ok(());
        }
        self.require_state(BridgeState::PromptActive, "cancel")?;
        let result = self
            .codec
            .interrupt_frame("gateway-interrupt")
            .map_err(|_| AgentRuntimePortError::Protocol)
            .and_then(|frame| {
                self.supervisor
                    .write_frame(&frame)
                    .map_err(|_| AgentRuntimePortError::Transport)
            });
        if let Err(error) = result {
            self.fail_transport("core_cancel_write_failed");
            return Err(error);
        }
        self.settle(AgentRuntimeEvent::Completed {
            outcome: RunOutcome::Cancelled,
        });
        self.shutdown_process();
        Ok(())
    }

    fn close(
        &mut self,
        binding: RuntimeBindingRef,
        deadline: Instant,
    ) -> Result<(), AgentRuntimePortError> {
        self.require_time(deadline, "close")?;
        if self.binding.as_ref() != Some(&binding) {
            return Err(AgentRuntimePortError::IdentityMismatch);
        }
        if self.state == BridgeState::Terminal {
            return Ok(());
        }
        if matches!(
            self.state,
            BridgeState::SessionOpen | BridgeState::PromptActive
        ) {
            if let Ok(frame) = self.codec.shutdown_frame("gateway-shutdown") {
                let _ = self.supervisor.write_frame(&frame);
            }
        }
        if self.state == BridgeState::PromptActive {
            self.settle(AgentRuntimeEvent::Completed {
                outcome: RunOutcome::Cancelled,
            });
        } else {
            self.state = BridgeState::Terminal;
        }
        self.shutdown_process();
        Ok(())
    }

    fn read_next_event(
        &mut self,
        deadline: Instant,
    ) -> Result<RuntimeEventEnvelope, AgentRuntimePortError> {
        if let Some(event) = self.pending_events.pop_front() {
            return self.deliver(event);
        }
        if self.state == BridgeState::Terminal {
            return Err(AgentRuntimePortError::Terminal);
        }
        if self.state != BridgeState::PromptActive {
            return Err(AgentRuntimePortError::InvalidState {
                operation: "next_event",
                state: self.state.name(),
            });
        }

        loop {
            let turn_deadline = self.prompt_deadline.unwrap_or(deadline).min(deadline);
            if Instant::now() >= turn_deadline {
                self.fail_transport("core_prompt_deadline");
                return self
                    .pending_events
                    .pop_front()
                    .ok_or(AgentRuntimePortError::Terminal)
                    .and_then(|event| self.deliver(event));
            }
            let observation = match self.read_observation(turn_deadline, "next_event") {
                Ok(observation) => observation,
                Err(AgentRuntimePortError::Deadline { .. })
                    if deadline < self.prompt_deadline.unwrap_or(deadline) =>
                {
                    return Err(AgentRuntimePortError::Deadline {
                        operation: "next_event",
                    });
                }
                Err(_) => {
                    self.fail_transport("core_transport_failed");
                    return self
                        .pending_events
                        .pop_front()
                        .ok_or(AgentRuntimePortError::Terminal)
                        .and_then(|event| self.deliver(event));
                }
            };
            match self.map_observation(observation) {
                Ok(Some(event)) => return self.deliver(event),
                Ok(None) => {}
                Err(_) => {
                    self.fail_transport("core_protocol_failed");
                    return self
                        .pending_events
                        .pop_front()
                        .ok_or(AgentRuntimePortError::Terminal)
                        .and_then(|event| self.deliver(event));
                }
            }
        }
    }

    fn read_observation(
        &mut self,
        deadline: Instant,
        operation: &'static str,
    ) -> Result<CoshCoreObservation, AgentRuntimePortError> {
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AgentRuntimePortError::Deadline { operation });
            }
            match self
                .supervisor
                .read_frame_timeout(remaining.min(READ_POLL_INTERVAL))
                .map_err(|_| AgentRuntimePortError::Transport)?
            {
                RuntimeFrameRead::Frame(frame) => {
                    return self
                        .codec
                        .decode_frame(frame.as_bytes())
                        .map_err(|_| AgentRuntimePortError::Protocol);
                }
                RuntimeFrameRead::Eof => {
                    return Ok(self
                        .codec
                        .finish_stdout()
                        .unwrap_or(CoshCoreObservation::ProtocolEndedWithoutResult));
                }
                RuntimeFrameRead::TimedOut => {
                    if self
                        .supervisor
                        .poll_terminal()
                        .map_err(|_| AgentRuntimePortError::Transport)?
                        .is_some()
                    {
                        return Ok(CoshCoreObservation::ProtocolEndedWithoutResult);
                    }
                }
            }
        }
    }

    fn map_observation(
        &mut self,
        observation: CoshCoreObservation,
    ) -> Result<Option<RuntimeEventEnvelope>, AgentRuntimePortError> {
        match observation {
            CoshCoreObservation::Stream(event) => self.map_stream(event),
            CoshCoreObservation::System(message) => {
                if let Some(provider_session_id) = message.provider_session_id {
                    self.require_provider_session(&provider_session_id)?;
                }
                Ok(None)
            }
            CoshCoreObservation::Assistant(message) => {
                self.require_provider_session(&message.provider_session_id)?;
                Ok(None)
            }
            CoshCoreObservation::ToolResults {
                provider_session_id,
                ..
            } => {
                self.require_provider_session(&provider_session_id)?;
                Ok(None)
            }
            CoshCoreObservation::Result(result) => {
                if self.current_message.is_some() {
                    return Err(AgentRuntimePortError::Protocol);
                }
                if let Some(provider_session_id) = result.provider_session_id.as_deref() {
                    self.require_provider_session(provider_session_id)?;
                }
                let event = if result.is_error {
                    AgentRuntimeEvent::Completed {
                        outcome: RunOutcome::Failed {
                            error: safe_error(
                                "core_turn_failed",
                                ErrorCategory::RuntimeUnavailable,
                                false,
                                "The Agent runtime reported a failed turn",
                            ),
                        },
                    }
                } else {
                    AgentRuntimeEvent::Completed {
                        outcome: RunOutcome::Succeeded,
                    }
                };
                self.settle(event);
                self.shutdown_process();
                Ok(self.pending_events.pop_front())
            }
            CoshCoreObservation::ProtocolEndedWithoutResult => {
                Err(AgentRuntimePortError::Transport)
            }
            CoshCoreObservation::ControlRequest(_)
            | CoshCoreObservation::ControlResponse(_)
            | CoshCoreObservation::RegistryResponse { .. }
            | CoshCoreObservation::Initialized(_) => Err(AgentRuntimePortError::Protocol),
        }
    }

    fn map_stream(
        &mut self,
        event: CoshCoreStreamEvent,
    ) -> Result<Option<RuntimeEventEnvelope>, AgentRuntimePortError> {
        match event {
            CoshCoreStreamEvent::MessageStart => {
                if self.current_message.is_some() {
                    return Err(AgentRuntimePortError::Protocol);
                }
                self.current_message = Some(RuntimeMessageId::new());
                Ok(None)
            }
            CoshCoreStreamEvent::ContentBlockStart {
                content_block: CoshCoreContentBlockInfo::ToolUse { id, name },
                ..
            } => {
                if self.current_message.is_none() {
                    return Err(AgentRuntimePortError::Protocol);
                }
                let name = BoundedName::new(name).map_err(|_| AgentRuntimePortError::Protocol)?;
                if self.tool_ids.len() >= MAX_TOOL_USES_PER_TURN {
                    return Err(AgentRuntimePortError::Protocol);
                }
                let tool_use_id = match self.tool_ids.entry(id) {
                    Entry::Vacant(entry) => entry.insert(ToolUseId::new()).clone(),
                    Entry::Occupied(_) => return Err(AgentRuntimePortError::Protocol),
                };
                Ok(Some(
                    self.event(AgentRuntimeEvent::ToolCallObserved {
                        tool_use_id,
                        summary: ToolSummary {
                            name,
                            summary: BoundedText::new("Agent runtime declared a tool call")
                                .map_err(|_| AgentRuntimePortError::Protocol)?,
                        },
                    }),
                ))
            }
            CoshCoreStreamEvent::ContentBlockDelta {
                delta: CoshCoreContentDelta::TextDelta { text },
                ..
            } => {
                if text.is_empty() {
                    return Ok(None);
                }
                let message_id = self
                    .current_message
                    .clone()
                    .ok_or(AgentRuntimePortError::Protocol)?;
                let text = BoundedText::new(text).map_err(|_| AgentRuntimePortError::Protocol)?;
                Ok(Some(self.event(AgentRuntimeEvent::MessageChunk {
                    message_id,
                    content: ContentPart::Text { text },
                })))
            }
            CoshCoreStreamEvent::MessageStop => {
                if self.current_message.is_none() {
                    return Err(AgentRuntimePortError::Protocol);
                }
                self.current_message = None;
                Ok(None)
            }
            CoshCoreStreamEvent::ContentBlockStart { .. }
            | CoshCoreStreamEvent::ContentBlockDelta { .. }
            | CoshCoreStreamEvent::ContentBlockStop { .. } => {
                if self.current_message.is_none() {
                    Err(AgentRuntimePortError::Protocol)
                } else {
                    Ok(None)
                }
            }
        }
    }

    fn bind_provider_session(
        &mut self,
        provider_session_id: String,
    ) -> Result<(), AgentRuntimePortError> {
        if self.provider_session_id.is_some() {
            return Err(AgentRuntimePortError::Protocol);
        }
        let external_session = ExternalRef {
            kind: ExternalRefKind::ProviderSession,
            authority: self.config.identity.provider_authority.clone(),
            scope_digest: self.config.identity.provider_scope_digest.clone(),
            value: BoundedOpaque::new(provider_session_id.clone())
                .map_err(|_| AgentRuntimePortError::Protocol)?,
        };
        let binding = RuntimeBindingRef {
            binding_id: self.config.identity.binding_id.clone(),
            task_id: self.config.identity.task_id.clone(),
            run_id: self.config.identity.run_id.clone(),
            agent_session_id: self.config.identity.agent_session_id.clone(),
            runtime_instance_id: self.config.identity.runtime_instance_id.clone(),
            runtime_generation: self.config.identity.runtime_generation,
            external_session,
        };
        self.provider_session_id = Some(provider_session_id);
        self.binding = Some(binding.clone());
        let event = self.event(AgentRuntimeEvent::SessionOpened { binding });
        self.pending_events.push_back(event);
        Ok(())
    }

    fn require_provider_session(
        &self,
        provider_session_id: &str,
    ) -> Result<(), AgentRuntimePortError> {
        if self.provider_session_id.as_deref() == Some(provider_session_id) {
            Ok(())
        } else {
            Err(AgentRuntimePortError::IdentityMismatch)
        }
    }

    fn event(&mut self, event: AgentRuntimeEvent) -> RuntimeEventEnvelope {
        self.sequence = self.sequence.saturating_add(1);
        let mut correlation = Correlation::new(self.config.identity.installation_id.clone());
        correlation.actor_id = self.config.identity.actor_id.clone();
        correlation.task_id = Some(self.config.identity.task_id.clone());
        correlation.run_id = Some(self.config.identity.run_id.clone());
        correlation.agent_session_id = Some(self.config.identity.agent_session_id.clone());
        correlation.runtime_binding_id = Some(self.config.identity.binding_id.clone());
        RuntimeEventEnvelope {
            header: ContractHeader::new(
                ContractSchema::RuntimeEvent,
                MessageId::new(),
                now_ms(),
                correlation,
            ),
            binding_id: self.config.identity.binding_id.clone(),
            sequence: self.sequence,
            event,
        }
    }

    fn settle(&mut self, event: AgentRuntimeEvent) {
        if self.state == BridgeState::Terminal {
            return;
        }
        let event = self.event(event);
        self.pending_events.push_back(event);
        self.state = BridgeState::Terminal;
        self.prompt_deadline = None;
    }

    fn fail_transport(&mut self, code: &'static str) {
        self.settle(AgentRuntimeEvent::TransportFailed {
            error: safe_error(
                code,
                ErrorCategory::Transport,
                false,
                "The Agent runtime transport failed",
            ),
        });
        self.shutdown_process();
    }

    fn shutdown_process(&mut self) {
        if matches!(
            self.supervisor.state(),
            RuntimeState::Initializing | RuntimeState::Ready | RuntimeState::Stopping
        ) && self
            .supervisor
            .shutdown(self.config.shutdown_grace)
            .is_err()
        {
            // Dropping the old supervisor synchronously kills and reaps its
            // direct child before a terminal event can be delivered.
            drop(std::mem::take(&mut self.supervisor));
        }
    }

    fn deliver(
        &mut self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, AgentRuntimePortError> {
        if matches!(event.event, AgentRuntimeEvent::SessionOpened { .. })
            && self.state == BridgeState::SessionOpenedPending
        {
            self.state = BridgeState::SessionOpen;
        }
        if matches!(
            event.event,
            AgentRuntimeEvent::Completed { .. } | AgentRuntimeEvent::TransportFailed { .. }
        ) {
            if self.terminal_delivered {
                return Err(AgentRuntimePortError::Terminal);
            }
            self.terminal_delivered = true;
        }
        Ok(event)
    }

    fn require_state(
        &self,
        expected: BridgeState,
        operation: &'static str,
    ) -> Result<(), AgentRuntimePortError> {
        if self.state == expected {
            Ok(())
        } else {
            Err(AgentRuntimePortError::InvalidState {
                operation,
                state: self.state.name(),
            })
        }
    }

    fn require_run(&self, task_id: &TaskId, run_id: &RunId) -> Result<(), AgentRuntimePortError> {
        if task_id == &self.config.identity.task_id && run_id == &self.config.identity.run_id {
            Ok(())
        } else {
            Err(AgentRuntimePortError::IdentityMismatch)
        }
    }

    fn require_time(
        &self,
        deadline: Instant,
        operation: &'static str,
    ) -> Result<(), AgentRuntimePortError> {
        if Instant::now() < deadline {
            Ok(())
        } else {
            Err(AgentRuntimePortError::Deadline { operation })
        }
    }
}

impl AgentRuntimePort for CoshCoreBridge {
    fn binding_id(&self) -> &RuntimeBindingId {
        &self.config.identity.binding_id
    }

    fn dispatch(
        &mut self,
        command: AgentRuntimeCommand,
        deadline: Instant,
    ) -> Result<(), AgentRuntimePortError> {
        match command {
            AgentRuntimeCommand::OpenSession {
                task_id,
                run_id,
                workspace,
            } => self.open_session(task_id, run_id, workspace, deadline),
            AgentRuntimeCommand::Prompt { run_id, input } => self.prompt(run_id, input, deadline),
            AgentRuntimeCommand::Cancel { run_id, .. } => self.cancel(run_id, deadline),
            AgentRuntimeCommand::Close { binding } => self.close(binding, deadline),
            AgentRuntimeCommand::ResumeSession { .. } => Err(AgentRuntimePortError::Unsupported {
                operation: "resume_session",
            }),
            AgentRuntimeCommand::ResolvePermission { .. } => {
                Err(AgentRuntimePortError::Unsupported {
                    operation: "resolve_permission",
                })
            }
        }
    }

    fn next_event(
        &mut self,
        deadline: Instant,
    ) -> Result<RuntimeEventEnvelope, AgentRuntimePortError> {
        self.read_next_event(deadline)
    }
}

impl Drop for CoshCoreBridge {
    fn drop(&mut self) {
        self.shutdown_process();
    }
}

fn prompt_text(
    input: Vec<ContentPart>,
    maximum_bytes: usize,
) -> Result<String, AgentRuntimePortError> {
    let mut parts = Vec::with_capacity(input.len());
    let mut total_bytes = 0usize;
    for part in input {
        match part {
            ContentPart::Text { text } => {
                total_bytes = total_bytes
                    .checked_add(text.as_str().len())
                    .and_then(|total| total.checked_add(usize::from(!parts.is_empty())))
                    .ok_or(AgentRuntimePortError::Protocol)?;
                if total_bytes > maximum_bytes {
                    return Err(AgentRuntimePortError::Protocol);
                }
                parts.push(text.as_str().to_owned());
            }
            ContentPart::ResourceLink { .. } => {
                return Err(AgentRuntimePortError::Unsupported {
                    operation: "resource prompt",
                });
            }
        }
    }
    if parts.is_empty() {
        return Err(AgentRuntimePortError::Protocol);
    }
    Ok(parts.join("\n"))
}

fn safe_error(
    code: &'static str,
    category: ErrorCategory,
    retryable: bool,
    message: &'static str,
) -> ContractError {
    // Static values are kept within contract bounds and stable code syntax.
    ContractError::new(code, category, retryable, message)
        .unwrap_or_else(|_| unreachable!("static contract error must remain valid"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
