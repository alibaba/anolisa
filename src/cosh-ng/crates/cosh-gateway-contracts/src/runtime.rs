//! Neutral commands and events for Agent Runtime bridges.

use serde::{de, Deserialize, Deserializer, Serialize};

use crate::{
    capability::{CapabilityRequest, DenialCode},
    common::{
        BoundedName, BoundedText, ContentPart, ContractHeader, ContractSchema, RuntimeBindingRef,
        WorkspaceRef,
    },
    error::ContractError,
    ids::{PermitId, RequestId, RunId, RuntimeBindingId, RuntimeMessageId, TaskId, ToolUseId},
    task::CancelReason,
};

/// Runtime-facing result of a capability decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum RuntimePermissionDecision {
    /// Policy granted a permit bound to the request.
    Permit {
        /// Permit issued by the capability broker.
        permit_id: PermitId,
    },
    /// Policy denied the Runtime request.
    Deny {
        /// Stable reason for denial.
        code: DenialCode,
        /// Redacted explanation safe to send to the Runtime.
        safe_message: BoundedText,
    },
}

/// Command issued through the neutral Agent Runtime port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum AgentRuntimeCommand {
    /// Open a new provider or ACP session for a Task Run.
    OpenSession {
        /// Task owning the session.
        task_id: TaskId,
        /// Run opening the session.
        run_id: RunId,
        /// Workspace scope exposed to the Runtime.
        workspace: WorkspaceRef,
    },
    /// Resume an existing fenced session binding.
    ResumeSession {
        /// Task owning the session.
        task_id: TaskId,
        /// Run resuming the session.
        run_id: RunId,
        /// Existing fenced binding.
        binding: RuntimeBindingRef,
    },
    /// Send bounded content to an active Agent turn.
    Prompt {
        /// Run receiving the input.
        run_id: RunId,
        /// Neutral content parts.
        input: Vec<ContentPart>,
    },
    /// Return a broker decision to a pending Runtime request.
    ResolvePermission {
        /// Capability request being resolved.
        request_id: RequestId,
        /// Broker decision translated for the Runtime.
        decision: RuntimePermissionDecision,
    },
    /// Request cancellation of an active Agent turn.
    Cancel {
        /// Run to cancel.
        run_id: RunId,
        /// Stable cancellation cause.
        cause: CancelReason,
    },
    /// Close a Runtime session binding.
    Close {
        /// Binding to close.
        binding: RuntimeBindingRef,
    },
}

/// Bounded token accounting reported by an Agent Runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeUsage {
    /// Input tokens consumed during the Run.
    pub input_tokens: u64,
    /// Output tokens produced during the Run.
    pub output_tokens: u64,
}

/// Redacted description of a Runtime-observed tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSummary {
    /// Provider-independent tool name.
    pub name: BoundedName,
    /// Safe bounded description suitable for presentation.
    pub summary: BoundedText,
}

/// Terminal result reported by an Agent Runtime turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RunOutcome {
    /// Runtime turn completed successfully.
    Succeeded,
    /// Runtime turn completed with a bounded failure.
    Failed {
        /// Safe Runtime failure.
        error: ContractError,
    },
    /// Runtime acknowledged cancellation.
    Cancelled,
}

/// Event emitted by a provider, Core, or ACP Runtime bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AgentRuntimeEvent {
    /// A provider or ACP session was opened and fenced.
    SessionOpened {
        /// New active binding.
        binding: RuntimeBindingRef,
    },
    /// A bounded streaming content part was observed.
    MessageChunk {
        /// Runtime message receiving the chunk.
        message_id: RuntimeMessageId,
        /// Neutral content part.
        content: ContentPart,
    },
    /// Runtime reported a tool call without authorizing a side effect.
    ToolCallObserved {
        /// COSH-owned tool observation identity.
        tool_use_id: ToolUseId,
        /// Redacted tool summary.
        summary: ToolSummary,
    },
    /// Runtime requested permission for a capability.
    PermissionRequested {
        /// Neutral capability request evaluated by the broker.
        request: CapabilityRequest,
    },
    /// Runtime reported cumulative token usage.
    UsageUpdated {
        /// Current cumulative usage.
        usage: RuntimeUsage,
    },
    /// Runtime turn reached a terminal outcome.
    Completed {
        /// Terminal turn result.
        outcome: RunOutcome,
    },
    /// Runtime transport failed before a domain result was known.
    TransportFailed {
        /// Safe bounded transport error.
        error: ContractError,
    },
}

/// Versioned envelope for commands sent to an Agent Runtime bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeCommandEnvelope {
    /// Versioned envelope metadata.
    pub header: ContractHeader,
    /// Neutral Runtime command.
    pub command: AgentRuntimeCommand,
}

impl RuntimeCommandEnvelope {
    /// Rejects a header that does not declare the Runtime command schema.
    pub fn validate_schema(&self) -> Result<(), crate::common::EnvelopeSchemaError> {
        self.header.validate_schema(ContractSchema::RuntimeCommand)
    }
}

impl<'de> Deserialize<'de> for RuntimeCommandEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireEnvelope {
            header: ContractHeader,
            command: AgentRuntimeCommand,
        }

        let wire = WireEnvelope::deserialize(deserializer)?;
        let envelope = Self {
            header: wire.header,
            command: wire.command,
        };
        envelope.validate_schema().map_err(de::Error::custom)?;
        Ok(envelope)
    }
}

/// Versioned event emitted by one fenced Agent Runtime binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeEventEnvelope {
    /// Versioned envelope metadata.
    pub header: ContractHeader,
    /// Fenced binding that produced the event.
    pub binding_id: RuntimeBindingId,
    /// Monotonic sequence assigned within the binding.
    pub sequence: u64,
    /// Neutral Runtime event.
    pub event: AgentRuntimeEvent,
}

impl RuntimeEventEnvelope {
    /// Rejects a header that does not declare the Runtime event schema.
    pub fn validate_schema(&self) -> Result<(), crate::common::EnvelopeSchemaError> {
        self.header.validate_schema(ContractSchema::RuntimeEvent)
    }
}

impl<'de> Deserialize<'de> for RuntimeEventEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireEnvelope {
            header: ContractHeader,
            binding_id: RuntimeBindingId,
            sequence: u64,
            event: AgentRuntimeEvent,
        }

        let wire = WireEnvelope::deserialize(deserializer)?;
        let envelope = Self {
            header: wire.header,
            binding_id: wire.binding_id,
            sequence: wire.sequence,
            event: wire.event,
        };
        envelope.validate_schema().map_err(de::Error::custom)?;
        Ok(envelope)
    }
}
