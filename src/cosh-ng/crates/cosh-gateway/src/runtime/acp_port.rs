//! Provider-neutral Runtime port backed by one supervised ACP v1 session.
//!
//! Ownership note: lifecycle and mapping stay together for the first adapter
//! slice. New ACP update families must first extract the pure mapping state
//! into `runtime/acp_port/mapping.rs` instead of extending this file.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cosh_gateway_contracts::{
    capability::CapabilityRequest,
    common::{
        ActorRef, BoundedName, BoundedOpaque, BoundedText, ContentPart, ContractHeader,
        ContractSchema, Correlation, Digest, RuntimeBindingRef, WorkspaceRef, MAX_TEXT_BYTES,
    },
    error::{ContractError, ErrorCategory},
    external::{ExternalRef, ExternalRefKind},
    ids::{
        AgentSessionId, InstallationId, MessageId, RequestId, RunId, RuntimeBindingId,
        RuntimeInstanceId, RuntimeMessageId, TaskId, ToolUseId,
    },
    runtime::{
        AgentRuntimeCommand, AgentRuntimeEvent, RunOutcome, RuntimeEventEnvelope,
        RuntimePermissionDecision, ToolSummary,
    },
};

use super::{
    AcpSessionDriver, AcpSessionDriverConfig, AcpSessionDriverError, AcpSessionEvent,
    AcpSessionTerminalKind, AcpV1Observation, AcpV1PermissionDecision, AcpV1PermissionOptionKind,
    AcpV1PermissionRequest, AcpV1RequestId, AcpV1StopReason, AgentRuntimePort,
    AgentRuntimePortError,
};
use sha2::{Digest as ShaDigest, Sha256};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// COSH-owned identities and ACP connection scope for one process generation.
#[derive(Debug, Clone)]
pub struct AcpAgentRuntimeIdentity {
    /// Durable Gateway installation.
    pub installation_id: InstallationId,
    /// Authenticated actor whose policy context governs permissions.
    pub actor: ActorRef,
    /// Task owning this runtime.
    pub task_id: TaskId,
    /// Run owning this runtime generation.
    pub run_id: RunId,
    /// Logical Agent session allocated by COSH.
    pub agent_session_id: AgentSessionId,
    /// Fenced binding allocated by COSH.
    pub binding_id: RuntimeBindingId,
    /// Supervised process identity allocated by COSH.
    pub runtime_instance_id: RuntimeInstanceId,
    /// Monotonic process generation used to reject stale output.
    pub runtime_generation: u64,
    /// Trusted ACP adapter authority.
    pub adapter_authority: BoundedName,
    /// Digest of the complete ACP connection parent scope.
    pub connection_scope_digest: Digest,
}

/// Immutable launch, identity, and workspace settings for the ACP port.
#[derive(Clone)]
pub struct AcpAgentRuntimeConfig {
    /// Supervised ACP session configuration.
    pub session: AcpSessionDriverConfig,
    /// Public workspace scope expected by `OpenSession`.
    pub workspace: WorkspaceRef,
    /// COSH-owned lifecycle identities.
    pub identity: AcpAgentRuntimeIdentity,
}

/// Trusted context supplied while normalizing an ACP permission callback.
#[derive(Debug, Clone)]
pub struct AcpPermissionContext {
    /// Authenticated actor bound to the runtime.
    pub actor: ActorRef,
    /// Task owning the callback.
    pub task_id: TaskId,
    /// Run owning the callback.
    pub run_id: RunId,
}

/// Trusted boundary that canonicalizes an untrusted ACP tool call.
///
/// Implementations must resolve the target and operation from trusted local
/// configuration. Copying Agent-provided labels into authorization fields is
/// unsafe and violates this contract.
pub trait AcpPermissionNormalizer: Send {
    /// Produces a bounded request for Capability Broker evaluation.
    ///
    /// # Errors
    ///
    /// Returns a stable port error when the callback cannot be canonicalized
    /// without trusting Agent-controlled authorization data.
    fn normalize(
        &mut self,
        request: &AcpV1PermissionRequest,
        context: &AcpPermissionContext,
    ) -> Result<CapabilityRequest, AgentRuntimePortError>;
}

trait AcpSessionBackend: Send {
    fn initialize(&self) -> Result<(), AcpSessionDriverError>;
    fn open_session(&self) -> Result<(), AcpSessionDriverError>;
    fn prompt(&self, text: String) -> Result<(), AcpSessionDriverError>;
    fn answer_permission(
        &self,
        request_id: AcpV1RequestId,
        decision: AcpV1PermissionDecision,
    ) -> Result<(), AcpSessionDriverError>;
    fn receive_timeout(
        &self,
        timeout: Duration,
    ) -> Result<AcpSessionEvent, std::sync::mpsc::RecvTimeoutError>;
    fn cancel(&self) -> Result<(), AcpSessionDriverError>;
    fn shutdown(&self) -> Result<(), AcpSessionDriverError>;
}

impl AcpSessionBackend for AcpSessionDriver {
    fn initialize(&self) -> Result<(), AcpSessionDriverError> {
        self.initialize()
    }
    fn open_session(&self) -> Result<(), AcpSessionDriverError> {
        self.open_session()
    }
    fn prompt(&self, text: String) -> Result<(), AcpSessionDriverError> {
        self.prompt(text)
    }
    fn answer_permission(
        &self,
        request_id: AcpV1RequestId,
        decision: AcpV1PermissionDecision,
    ) -> Result<(), AcpSessionDriverError> {
        self.answer_permission(request_id, decision)
    }
    fn receive_timeout(
        &self,
        timeout: Duration,
    ) -> Result<AcpSessionEvent, std::sync::mpsc::RecvTimeoutError> {
        self.receive_timeout(timeout)
    }
    fn cancel(&self) -> Result<(), AcpSessionDriverError> {
        self.control().cancel()
    }
    fn shutdown(&self) -> Result<(), AcpSessionDriverError> {
        self.shutdown()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortState {
    Created,
    SessionOpenedPending,
    SessionOpen,
    PromptActive,
    Terminal,
}

impl PortState {
    fn name(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::SessionOpenedPending => "session-opened-pending",
            Self::SessionOpen => "session-open",
            Self::PromptActive => "prompt-active",
            Self::Terminal => "terminal",
        }
    }
}

#[derive(Clone)]
struct PendingPermission {
    acp_request_id: AcpV1RequestId,
    allow_once: Option<String>,
    reject_once: Option<String>,
}

/// One supervised ACP process exposed through `AgentRuntimePort`.
pub struct AcpAgentRuntime {
    backend: Box<dyn AcpSessionBackend>,
    normalizer: Box<dyn AcpPermissionNormalizer>,
    config: AcpAgentRuntimeConfig,
    state: PortState,
    binding: Option<RuntimeBindingRef>,
    provider_session: Option<String>,
    events: VecDeque<RuntimeEventEnvelope>,
    sequence: u64,
    messages: BTreeMap<String, RuntimeMessageId>,
    tools: BTreeMap<String, ToolUseId>,
    permissions: BTreeMap<RequestId, PendingPermission>,
    terminal_delivered: bool,
}

impl AcpAgentRuntime {
    /// Launches one ACP adapter without admitting a prompt or side effect.
    pub fn launch(
        config: AcpAgentRuntimeConfig,
        normalizer: Box<dyn AcpPermissionNormalizer>,
    ) -> Result<Self, AgentRuntimePortError> {
        let driver = AcpSessionDriver::launch(config.session.clone()).map_err(map_driver_error)?;
        Ok(Self::with_backend(config, normalizer, Box::new(driver)))
    }

    fn with_backend(
        config: AcpAgentRuntimeConfig,
        normalizer: Box<dyn AcpPermissionNormalizer>,
        backend: Box<dyn AcpSessionBackend>,
    ) -> Self {
        Self {
            backend,
            normalizer,
            config,
            state: PortState::Created,
            binding: None,
            provider_session: None,
            events: VecDeque::new(),
            sequence: 0,
            messages: BTreeMap::new(),
            tools: BTreeMap::new(),
            permissions: BTreeMap::new(),
            terminal_delivered: false,
        }
    }

    fn open(
        &mut self,
        task_id: TaskId,
        run_id: RunId,
        workspace: WorkspaceRef,
        deadline: Instant,
    ) -> Result<(), AgentRuntimePortError> {
        self.require_state(PortState::Created, "open_session")?;
        self.require_run(&task_id, &run_id)?;
        if workspace != self.config.workspace {
            return Err(AgentRuntimePortError::WorkspaceMismatch);
        }
        self.require_time(deadline, "open_session")?;
        let result = self
            .backend
            .initialize()
            .and_then(|()| self.backend.open_session())
            .map_err(map_driver_error);
        if result.is_err() || Instant::now() >= deadline {
            self.fail_and_shutdown("acp_session_open_failed")?;
            return result.and(Err(AgentRuntimePortError::Deadline {
                operation: "open_session",
            }));
        }
        loop {
            if Instant::now() >= deadline {
                self.fail_and_shutdown("acp_session_open_failed")?;
                return Err(AgentRuntimePortError::Deadline {
                    operation: "open_session",
                });
            }
            match self.backend.receive_timeout(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(EVENT_POLL_INTERVAL),
            ) {
                Ok(AcpSessionEvent::Observation(AcpV1Observation::Initialized { .. })) => {}
                Ok(AcpSessionEvent::Observation(AcpV1Observation::SessionOpened {
                    session_id,
                })) => {
                    self.bind_session(session_id)?;
                    self.state = PortState::SessionOpenedPending;
                    return Ok(());
                }
                Ok(AcpSessionEvent::Terminal(_))
                | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    self.settle(AgentRuntimeEvent::TransportFailed {
                        error: safe_error(
                            "acp_session_open_failed",
                            ErrorCategory::Transport,
                            false,
                            "The ACP runtime transport failed",
                        ),
                    });
                    return Err(AgentRuntimePortError::Transport);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Ok(AcpSessionEvent::Observation(_)) => {
                    self.fail_and_shutdown("acp_session_open_failed")?;
                    return Err(AgentRuntimePortError::Protocol);
                }
            }
        }
    }

    fn prompt(
        &mut self,
        run_id: RunId,
        input: Vec<ContentPart>,
        deadline: Instant,
    ) -> Result<(), AgentRuntimePortError> {
        self.require_state(PortState::SessionOpen, "prompt")?;
        self.require_run(&self.config.identity.task_id.clone(), &run_id)?;
        self.require_time(deadline, "prompt")?;
        self.backend
            .prompt(prompt_text(input)?)
            .map_err(map_driver_error)?;
        if Instant::now() >= deadline {
            self.backend.cancel().map_err(map_driver_error)?;
            self.await_terminal(deadline, "prompt")?;
            self.settle(AgentRuntimeEvent::TransportFailed {
                error: safe_error(
                    "acp_prompt_deadline",
                    ErrorCategory::Transport,
                    false,
                    "The ACP runtime transport failed",
                ),
            });
            return Err(AgentRuntimePortError::Deadline {
                operation: "prompt",
            });
        }
        self.state = PortState::PromptActive;
        Ok(())
    }

    fn resolve(
        &mut self,
        request_id: RequestId,
        decision: RuntimePermissionDecision,
        deadline: Instant,
    ) -> Result<(), AgentRuntimePortError> {
        self.require_state(PortState::PromptActive, "resolve_permission")?;
        self.require_time(deadline, "resolve_permission")?;
        let pending = self
            .permissions
            .get(&request_id)
            .cloned()
            .ok_or(AgentRuntimePortError::IdentityMismatch)?;
        let selected = match decision {
            RuntimePermissionDecision::Permit { .. } => pending.allow_once,
            RuntimePermissionDecision::Deny { .. } => pending.reject_once,
        };
        let missing_one_shot = selected.is_none();
        let acp_decision = selected.map_or(AcpV1PermissionDecision::Cancelled, |option_id| {
            AcpV1PermissionDecision::Selected { option_id }
        });
        if let Err(error) = self
            .backend
            .answer_permission(pending.acp_request_id, acp_decision)
            .map_err(map_driver_error)
        {
            self.fail_and_reap("acp_permission_failed", deadline)?;
            return Err(error);
        }
        self.permissions.remove(&request_id);
        if missing_one_shot {
            return Err(AgentRuntimePortError::Unsupported {
                operation: "one-shot permission option",
            });
        }
        self.require_time(deadline, "resolve_permission")
    }

    fn cancel(&mut self, run_id: RunId, deadline: Instant) -> Result<(), AgentRuntimePortError> {
        self.require_run(&self.config.identity.task_id.clone(), &run_id)?;
        self.require_time(deadline, "cancel")?;
        self.require_state(PortState::PromptActive, "cancel")?;
        self.backend.cancel().map_err(map_driver_error)?;
        self.await_terminal(deadline, "cancel")?;
        self.settle(AgentRuntimeEvent::Completed {
            outcome: RunOutcome::Cancelled,
        });
        Ok(())
    }

    fn next(&mut self, deadline: Instant) -> Result<RuntimeEventEnvelope, AgentRuntimePortError> {
        if let Some(event) = self.events.pop_front() {
            return self.deliver(event);
        }
        if self.state == PortState::Terminal {
            return Err(AgentRuntimePortError::Terminal);
        }
        if self.state != PortState::PromptActive {
            return Err(AgentRuntimePortError::InvalidState {
                operation: "next_event",
                state: self.state.name(),
            });
        }
        loop {
            self.require_time(deadline, "next_event")?;
            match self.backend.receive_timeout(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(EVENT_POLL_INTERVAL),
            ) {
                Ok(AcpSessionEvent::Observation(observation)) => {
                    match self.map_observation(observation) {
                        Ok(Some(event)) => return self.deliver(event),
                        Ok(None) => {}
                        Err(error) => {
                            self.fail_and_reap("acp_protocol_failed", deadline)?;
                            return self
                                .events
                                .pop_front()
                                .ok_or(error)
                                .and_then(|event| self.deliver(event));
                        }
                    }
                }
                Ok(AcpSessionEvent::Terminal(terminal)) => {
                    match terminal.kind {
                        AcpSessionTerminalKind::Cancelled => {
                            self.settle(AgentRuntimeEvent::Completed {
                                outcome: RunOutcome::Cancelled,
                            })
                        }
                        AcpSessionTerminalKind::Failed | AcpSessionTerminalKind::Shutdown => self
                            .settle(AgentRuntimeEvent::TransportFailed {
                                error: safe_error(
                                    "acp_transport_failed",
                                    ErrorCategory::Transport,
                                    false,
                                    "The ACP runtime transport failed",
                                ),
                            }),
                    }
                    return self
                        .events
                        .pop_front()
                        .ok_or(AgentRuntimePortError::Terminal)
                        .and_then(|event| self.deliver(event));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(AgentRuntimePortError::Transport);
                }
            }
        }
    }

    fn map_observation(
        &mut self,
        observation: AcpV1Observation,
    ) -> Result<Option<RuntimeEventEnvelope>, AgentRuntimePortError> {
        match observation {
            AcpV1Observation::SessionUpdate { session_id, update } => {
                self.require_session(&session_id)?;
                self.map_update(&update)
            }
            AcpV1Observation::PermissionRequested(request) => {
                self.require_session(&request.session_id)?;
                let context = AcpPermissionContext {
                    actor: self.config.identity.actor.clone(),
                    task_id: self.config.identity.task_id.clone(),
                    run_id: self.config.identity.run_id.clone(),
                };
                let normalized = self.normalizer.normalize(&request, &context)?;
                if normalized.task_id != context.task_id
                    || normalized.run_id != context.run_id
                    || normalized.actor != context.actor
                    || self.permissions.contains_key(&normalized.request_id)
                {
                    return Err(AgentRuntimePortError::IdentityMismatch);
                }
                let allow_once = request
                    .options
                    .iter()
                    .find(|o| o.kind == AcpV1PermissionOptionKind::AllowOnce)
                    .map(|o| o.option_id.clone());
                let reject_once = request
                    .options
                    .iter()
                    .find(|o| o.kind == AcpV1PermissionOptionKind::RejectOnce)
                    .map(|o| o.option_id.clone());
                self.permissions.insert(
                    normalized.request_id.clone(),
                    PendingPermission {
                        acp_request_id: request.request_id,
                        allow_once,
                        reject_once,
                    },
                );
                Ok(Some(self.event(AgentRuntimeEvent::PermissionRequested {
                    request: normalized,
                })))
            }
            AcpV1Observation::PromptFinished {
                session_id,
                stop_reason,
            } => {
                self.require_session(&session_id)?;
                let outcome = match stop_reason {
                    AcpV1StopReason::EndTurn
                    | AcpV1StopReason::MaxTokens
                    | AcpV1StopReason::MaxTurnRequests => RunOutcome::Succeeded,
                    AcpV1StopReason::Cancelled => RunOutcome::Cancelled,
                    AcpV1StopReason::Refusal | AcpV1StopReason::Unsupported => RunOutcome::Failed {
                        error: safe_error(
                            "acp_turn_failed",
                            ErrorCategory::RuntimeUnavailable,
                            false,
                            "The ACP Agent did not complete the turn",
                        ),
                    },
                };
                self.backend.shutdown().map_err(map_driver_error)?;
                self.settle(AgentRuntimeEvent::Completed { outcome });
                Ok(self.events.pop_front())
            }
            AcpV1Observation::RequestFailed { .. } => {
                self.backend.shutdown().map_err(map_driver_error)?;
                self.settle(AgentRuntimeEvent::Completed {
                    outcome: RunOutcome::Failed {
                        error: safe_error(
                            "acp_request_failed",
                            ErrorCategory::RuntimeUnavailable,
                            false,
                            "The ACP Agent request failed",
                        ),
                    },
                });
                Ok(self.events.pop_front())
            }
            AcpV1Observation::TransportClosed => Err(AgentRuntimePortError::Transport),
            AcpV1Observation::Initialized { .. }
            | AcpV1Observation::UnsupportedClientRequest { .. }
            | AcpV1Observation::UnsupportedNotification { .. } => Ok(None),
            AcpV1Observation::SessionOpened { .. } => Err(AgentRuntimePortError::Protocol),
        }
    }

    fn map_update(
        &mut self,
        update: &serde_json::Value,
    ) -> Result<Option<RuntimeEventEnvelope>, AgentRuntimePortError> {
        match update
            .get("sessionUpdate")
            .and_then(serde_json::Value::as_str)
        {
            Some("agent_message_chunk") => {
                let text = update
                    .get("content")
                    .and_then(|v| v.get("text"))
                    .and_then(serde_json::Value::as_str)
                    .ok_or(AgentRuntimePortError::Protocol)?;
                if text.is_empty() {
                    return Ok(None);
                }
                let external = update
                    .get("messageId")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("default")
                    .to_owned();
                let message_id = self.messages.entry(external).or_default().clone();
                let text = BoundedText::new(text).map_err(|_| AgentRuntimePortError::Protocol)?;
                Ok(Some(self.event(AgentRuntimeEvent::MessageChunk {
                    message_id,
                    content: ContentPart::Text { text },
                })))
            }
            Some("tool_call") => {
                let external = update
                    .get("toolCallId")
                    .and_then(serde_json::Value::as_str)
                    .ok_or(AgentRuntimePortError::Protocol)?
                    .to_owned();
                if self.tools.contains_key(&external) {
                    return Err(AgentRuntimePortError::Protocol);
                }
                let title = update
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Agent tool call");
                let name = update
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("agent_tool");
                let tool_use_id = ToolUseId::new();
                self.tools.insert(external, tool_use_id.clone());
                Ok(Some(self.event(AgentRuntimeEvent::ToolCallObserved {
                    tool_use_id,
                    summary: ToolSummary {
                        name:
                            BoundedName::new(name).map_err(|_| AgentRuntimePortError::Protocol)?,
                        summary:
                            BoundedText::new(title).map_err(|_| AgentRuntimePortError::Protocol)?,
                    },
                })))
            }
            Some(_) => Ok(None),
            None => Err(AgentRuntimePortError::Protocol),
        }
    }

    fn bind_session(&mut self, session_id: String) -> Result<(), AgentRuntimePortError> {
        if self.provider_session.is_some() {
            return Err(AgentRuntimePortError::Protocol);
        }
        let session_digest = Sha256::digest(session_id.as_bytes());
        let external_session = ExternalRef {
            kind: ExternalRefKind::AcpSession,
            authority: self.config.identity.adapter_authority.clone(),
            scope_digest: self.config.identity.connection_scope_digest.clone(),
            value: BoundedOpaque::new(format!("sha256:{session_digest:x}"))
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
        self.provider_session = Some(session_id);
        self.binding = Some(binding.clone());
        let event = self.event(AgentRuntimeEvent::SessionOpened { binding });
        self.events.push_back(event);
        Ok(())
    }

    fn event(&mut self, event: AgentRuntimeEvent) -> RuntimeEventEnvelope {
        self.sequence = self.sequence.saturating_add(1);
        let mut correlation = Correlation::new(self.config.identity.installation_id.clone());
        correlation.actor_id = Some(self.config.identity.actor.actor_id.clone());
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
        if self.state != PortState::Terminal {
            let event = self.event(event);
            self.events.push_back(event);
            self.state = PortState::Terminal;
            self.permissions.clear();
        }
    }
    fn fail_and_reap(
        &mut self,
        code: &'static str,
        deadline: Instant,
    ) -> Result<(), AgentRuntimePortError> {
        self.backend.cancel().map_err(map_driver_error)?;
        self.await_terminal(deadline, "runtime settlement")?;
        self.settle(AgentRuntimeEvent::TransportFailed {
            error: safe_error(
                code,
                ErrorCategory::Transport,
                false,
                "The ACP runtime transport failed",
            ),
        });
        Ok(())
    }
    fn fail_and_shutdown(&mut self, code: &'static str) -> Result<(), AgentRuntimePortError> {
        self.backend.shutdown().map_err(map_driver_error)?;
        self.settle(AgentRuntimeEvent::TransportFailed {
            error: safe_error(
                code,
                ErrorCategory::Transport,
                false,
                "The ACP runtime transport failed",
            ),
        });
        Ok(())
    }
    fn await_terminal(
        &self,
        deadline: Instant,
        operation: &'static str,
    ) -> Result<(), AgentRuntimePortError> {
        loop {
            self.require_time(deadline, operation)?;
            match self.backend.receive_timeout(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(EVENT_POLL_INTERVAL),
            ) {
                Ok(AcpSessionEvent::Terminal(_)) => return Ok(()),
                Ok(AcpSessionEvent::Observation(_)) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(AgentRuntimePortError::Transport);
                }
            }
        }
    }
    fn deliver(
        &mut self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, AgentRuntimePortError> {
        if matches!(event.event, AgentRuntimeEvent::SessionOpened { .. })
            && self.state == PortState::SessionOpenedPending
        {
            self.state = PortState::SessionOpen;
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
        expected: PortState,
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
    fn require_run(&self, task: &TaskId, run: &RunId) -> Result<(), AgentRuntimePortError> {
        if task == &self.config.identity.task_id && run == &self.config.identity.run_id {
            Ok(())
        } else {
            Err(AgentRuntimePortError::IdentityMismatch)
        }
    }
    fn require_session(&self, session: &str) -> Result<(), AgentRuntimePortError> {
        if self.provider_session.as_deref() == Some(session) {
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

impl AgentRuntimePort for AcpAgentRuntime {
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
            } => self.open(task_id, run_id, workspace, deadline),
            AgentRuntimeCommand::Prompt { run_id, input } => self.prompt(run_id, input, deadline),
            AgentRuntimeCommand::ResolvePermission {
                request_id,
                decision,
            } => self.resolve(request_id, decision, deadline),
            AgentRuntimeCommand::Cancel { run_id, .. } => self.cancel(run_id, deadline),
            AgentRuntimeCommand::Close { binding } => {
                self.require_time(deadline, "close")?;
                if self.binding.as_ref() != Some(&binding) {
                    return Err(AgentRuntimePortError::IdentityMismatch);
                }
                if self.state != PortState::Terminal {
                    self.backend.shutdown().map_err(map_driver_error)?;
                    self.state = PortState::Terminal;
                }
                Ok(())
            }
            AgentRuntimeCommand::ResumeSession { .. } => Err(AgentRuntimePortError::Unsupported {
                operation: "resume_session",
            }),
        }
    }
    fn next_event(
        &mut self,
        deadline: Instant,
    ) -> Result<RuntimeEventEnvelope, AgentRuntimePortError> {
        self.next(deadline)
    }
}

fn prompt_text(input: Vec<ContentPart>) -> Result<String, AgentRuntimePortError> {
    let mut output = String::new();
    for part in input {
        match part {
            ContentPart::Text { text } => {
                let separator = usize::from(!output.is_empty());
                let next_len = output
                    .len()
                    .checked_add(separator)
                    .and_then(|length| length.checked_add(text.as_str().len()))
                    .ok_or(AgentRuntimePortError::Protocol)?;
                if next_len > MAX_TEXT_BYTES {
                    return Err(AgentRuntimePortError::Protocol);
                }
                if separator == 1 {
                    output.push('\n');
                }
                output.push_str(text.as_str());
            }
            ContentPart::ResourceLink { .. } => {
                return Err(AgentRuntimePortError::Unsupported {
                    operation: "resource prompt",
                })
            }
        }
    }
    if output.is_empty() {
        Err(AgentRuntimePortError::Protocol)
    } else {
        Ok(output)
    }
}
fn map_driver_error(error: AcpSessionDriverError) -> AgentRuntimePortError {
    match error {
        AcpSessionDriverError::Deadline { operation } => {
            AgentRuntimePortError::Deadline { operation }
        }
        AcpSessionDriverError::InvalidState { operation, state } => {
            AgentRuntimePortError::InvalidState { operation, state }
        }
        AcpSessionDriverError::Bridge(_)
        | AcpSessionDriverError::ActorUnavailable
        | AcpSessionDriverError::CancellationPending
        | AcpSessionDriverError::ObservationBackpressure
        | AcpSessionDriverError::Cancelled => AgentRuntimePortError::Transport,
    }
}
fn safe_error(
    code: &'static str,
    category: ErrorCategory,
    retryable: bool,
    message: &'static str,
) -> ContractError {
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
#[path = "acp_port/tests.rs"]
mod tests;
