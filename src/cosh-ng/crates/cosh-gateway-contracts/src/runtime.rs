//! Neutral commands and events for Agent Runtime bridges.

use serde::{de, Deserialize, Deserializer, Serialize};
use thiserror::Error;

use crate::{
    capability::{BrokeredOperation, CapabilityRequest, DenialCode},
    common::{
        BoundedName, BoundedText, ContentPart, ContractHeader, ContractSchema, RuntimeBindingRef,
        WorkspaceRef,
    },
    error::ContractError,
    ids::{
        ApprovalId, CheckpointId, ExecutionId, InputRequestId, RequestId, RunId, RuntimeBindingId,
        RuntimeMessageId, TaskId, ToolUseId, TurnId,
    },
    task::CancelReason,
};

/// Maximum number of choices carried by one Runtime input request.
pub const MAX_RUNTIME_INPUT_OPTIONS: usize = 32;
/// Maximum aggregate UTF-8 bytes across a question and all choice presentation.
pub const MAX_RUNTIME_INPUT_REQUEST_TEXT_BYTES: usize = 16 * 1024;
/// Maximum number of selected choice indices in one Runtime input response.
pub const MAX_RUNTIME_INPUT_SELECTIONS: usize = 32;

/// Invalid bounded Runtime input request or response.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RuntimeInputError {
    /// A non-free-text question has no selectable answer.
    #[error("input request must allow free text or provide at least one option")]
    NoAnswerMode,
    /// Multiple selection requires at least two choices.
    #[error("multi-select input request must provide at least two options")]
    InvalidMultiSelect,
    /// The option list exceeds the contract count bound.
    #[error("input request exceeds the {max} option limit")]
    TooManyOptions {
        /// Maximum accepted option count.
        max: usize,
    },
    /// The complete presentation exceeds its aggregate UTF-8 bound.
    #[error("input request presentation exceeds the {max_bytes}-byte limit")]
    RequestTextTooLarge {
        /// Maximum aggregate UTF-8 byte count.
        max_bytes: usize,
    },
    /// Choice labels must be unique so indices and display remain unambiguous.
    #[error("input request contains duplicate option labels")]
    DuplicateOption,
    /// An option response must select at least one choice.
    #[error("input response must select at least one option")]
    EmptySelection,
    /// The selection list exceeds the contract count bound.
    #[error("input response exceeds the {max} selection limit")]
    TooManySelections {
        /// Maximum accepted selection count.
        max: usize,
    },
    /// Repeated indices could otherwise be misread as multiple answers.
    #[error("input response contains duplicate option indices")]
    DuplicateSelection,
}

/// One bounded user-presentable choice for a Runtime input request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeInputOption {
    label: BoundedText,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<BoundedText>,
}

impl RuntimeInputOption {
    /// Creates one bounded choice.
    #[must_use]
    pub fn new(label: BoundedText, description: Option<BoundedText>) -> Self {
        Self { label, description }
    }

    /// Returns the user-visible choice label.
    #[must_use]
    pub fn label(&self) -> &BoundedText {
        &self.label
    }

    /// Returns the optional user-visible explanation.
    #[must_use]
    pub fn description(&self) -> Option<&BoundedText> {
        self.description.as_ref()
    }
}

/// Exact bounded question emitted by one Runtime turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeInputRequest {
    request_id: InputRequestId,
    run_id: RunId,
    turn_id: TurnId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_use_id: Option<ToolUseId>,
    question: BoundedText,
    options: Vec<RuntimeInputOption>,
    allow_free_text: bool,
    multi_select: bool,
}

impl RuntimeInputRequest {
    /// Builds a request after enforcing count, aggregate byte, and answer-mode bounds.
    pub fn new(
        request_id: InputRequestId,
        run_id: RunId,
        turn_id: TurnId,
        tool_use_id: Option<ToolUseId>,
        question: BoundedText,
        options: Vec<RuntimeInputOption>,
        allow_free_text: bool,
        multi_select: bool,
    ) -> Result<Self, RuntimeInputError> {
        if options.len() > MAX_RUNTIME_INPUT_OPTIONS {
            return Err(RuntimeInputError::TooManyOptions {
                max: MAX_RUNTIME_INPUT_OPTIONS,
            });
        }
        if !allow_free_text && options.is_empty() {
            return Err(RuntimeInputError::NoAnswerMode);
        }
        if multi_select && options.len() < 2 {
            return Err(RuntimeInputError::InvalidMultiSelect);
        }
        let text_bytes = options
            .iter()
            .try_fold(question.as_str().len(), |total, option| {
                total
                    .checked_add(option.label().as_str().len())?
                    .checked_add(option.description().map_or(0, |value| value.as_str().len()))
            });
        if text_bytes.is_none_or(|bytes| bytes > MAX_RUNTIME_INPUT_REQUEST_TEXT_BYTES) {
            return Err(RuntimeInputError::RequestTextTooLarge {
                max_bytes: MAX_RUNTIME_INPUT_REQUEST_TEXT_BYTES,
            });
        }
        if options.iter().enumerate().any(|(index, option)| {
            options[..index]
                .iter()
                .any(|prior| prior.label() == option.label())
        }) {
            return Err(RuntimeInputError::DuplicateOption);
        }
        Ok(Self {
            request_id,
            run_id,
            turn_id,
            tool_use_id,
            question,
            options,
            allow_free_text,
            multi_select,
        })
    }

    /// Returns the independently allocated request identity.
    #[must_use]
    pub fn request_id(&self) -> &InputRequestId {
        &self.request_id
    }

    /// Returns the owning Run.
    #[must_use]
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// Returns the exact prompt turn waiting for input.
    #[must_use]
    pub fn turn_id(&self) -> &TurnId {
        &self.turn_id
    }

    /// Returns the optional observed tool identity.
    #[must_use]
    pub fn tool_use_id(&self) -> Option<&ToolUseId> {
        self.tool_use_id.as_ref()
    }

    /// Returns the bounded user-visible question.
    #[must_use]
    pub fn question(&self) -> &BoundedText {
        &self.question
    }

    /// Returns the bounded selectable choices.
    #[must_use]
    pub fn options(&self) -> &[RuntimeInputOption] {
        &self.options
    }

    /// Returns whether bounded free text is accepted.
    #[must_use]
    pub fn allows_free_text(&self) -> bool {
        self.allow_free_text
    }

    /// Returns whether more than one choice may be selected.
    #[must_use]
    pub fn allows_multiple(&self) -> bool {
        self.multi_select
    }
}

impl<'de> Deserialize<'de> for RuntimeInputRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WireRequest {
            request_id: InputRequestId,
            run_id: RunId,
            turn_id: TurnId,
            #[serde(default)]
            tool_use_id: Option<ToolUseId>,
            question: BoundedText,
            options: Vec<RuntimeInputOption>,
            allow_free_text: bool,
            multi_select: bool,
        }

        let wire = WireRequest::deserialize(deserializer)?;
        Self::new(
            wire.request_id,
            wire.run_id,
            wire.turn_id,
            wire.tool_use_id,
            wire.question,
            wire.options,
            wire.allow_free_text,
            wire.multi_select,
        )
        .map_err(de::Error::custom)
    }
}

/// Non-empty bounded unique indices selected from a Runtime input request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct RuntimeInputSelections(Vec<u16>);

impl RuntimeInputSelections {
    /// Builds a bounded selection list.
    pub fn new(selections: Vec<u16>) -> Result<Self, RuntimeInputError> {
        if selections.is_empty() {
            return Err(RuntimeInputError::EmptySelection);
        }
        if selections.len() > MAX_RUNTIME_INPUT_SELECTIONS {
            return Err(RuntimeInputError::TooManySelections {
                max: MAX_RUNTIME_INPUT_SELECTIONS,
            });
        }
        if selections
            .iter()
            .enumerate()
            .any(|(index, selection)| selections[..index].iter().any(|prior| prior == selection))
        {
            return Err(RuntimeInputError::DuplicateSelection);
        }
        Ok(Self(selections))
    }

    /// Returns selected zero-based option indices.
    #[must_use]
    pub fn as_slice(&self) -> &[u16] {
        &self.0
    }
}

impl<'de> Deserialize<'de> for RuntimeInputSelections {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(Vec::<u16>::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

/// Bounded answer supplied for one exact Runtime input request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "response", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeInputResponse {
    /// Bounded free-text input.
    Text {
        /// Text delivered only to the waiting Runtime request.
        text: BoundedText,
    },
    /// One or more zero-based indices into the request's choices.
    Options {
        /// Non-empty bounded unique selections.
        selections: RuntimeInputSelections,
    },
}

/// Runtime-facing result for a provider-native permission callback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum RuntimePermissionDecision {
    /// Permit the provider to execute its own tool exactly once.
    ///
    /// This is observation-only authority. It must never create or consume a
    /// COSH execution permit because the side effect remains provider-owned.
    ProviderNativeAllowOnce,
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
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
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
        /// COSH-owned identity for this prompt turn.
        turn_id: TurnId,
        /// Neutral content parts.
        input: Vec<ContentPart>,
    },
    /// Return a broker decision to a pending Runtime request.
    ResolvePermission {
        /// Capability request being resolved.
        request_id: RequestId,
        /// Provider-native decision translated for the Runtime.
        decision: RuntimePermissionDecision,
    },
    /// Confirms that Gateway durably took ownership of a brokered request.
    ///
    /// This acknowledgement carries neither a policy decision nor executable
    /// authority. The final outcome arrives through `DeliverBrokeredResult`.
    AcknowledgeBrokeredRequest {
        /// Durable takeover acknowledgement for the pending Runtime request.
        acknowledgement: BrokeredRequestAcknowledgement,
    },
    /// Delivers the terminal COSH-owned outcome of a brokered request.
    DeliverBrokeredResult {
        /// Typed result correlated to the original Runtime request.
        delivery: BrokeredExecutionDelivery,
    },
    /// Resolves one exact pending Runtime input request.
    ///
    /// The Task plane's durable `SubmitInput` command is the only intended
    /// source of this Runtime-local callback.
    ResolveInput {
        /// Independently allocated request being resolved.
        request_id: InputRequestId,
        /// Run that owns the request.
        run_id: RunId,
        /// Prompt turn waiting for the response.
        turn_id: TurnId,
        /// Bounded answer validated against the pending request.
        response: RuntimeInputResponse,
    },
    /// Request cancellation of an active Agent turn.
    Cancel {
        /// Run to cancel.
        run_id: RunId,
        /// Active turn to cancel.
        turn_id: TurnId,
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

/// Receipt proving Gateway durably took ownership of a brokered Runtime request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokeredRequestAcknowledgement {
    /// Capability request now owned by Gateway.
    pub request_id: RequestId,
    /// Durable approval created before the Runtime may release its callback.
    pub approval_id: ApprovalId,
}

/// Terminal delivery for one brokered Runtime request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokeredExecutionDelivery {
    /// Capability request whose callback receives this result.
    pub request_id: RequestId,
    /// Typed terminal outcome; no permit is exposed to the Runtime.
    pub outcome: BrokeredExecutionOutcome,
}

/// Provider-neutral terminal outcome of a COSH-brokered execution request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum BrokeredExecutionOutcome {
    /// Policy or approval denied the request before any effect was executed.
    Denied {
        /// Stable denial classification.
        code: DenialCode,
        /// Redacted explanation safe to return to the Runtime.
        safe_message: BoundedText,
    },
    /// The COSH execution target completed the typed operation successfully.
    Succeeded {
        /// Governed execution that produced the result.
        execution_id: ExecutionId,
        /// Operation-specific bounded result.
        result: BrokeredOperationResult,
    },
    /// The COSH execution target produced a known terminal failure.
    Failed {
        /// Governed execution that failed.
        execution_id: ExecutionId,
        /// Safe failure that excludes raw target output.
        error: ContractError,
    },
    /// Recovery cannot prove whether the side effect completed.
    Uncertain {
        /// Governed execution requiring reconciliation.
        execution_id: ExecutionId,
        /// Safe reason that does not expose target internals.
        error: ContractError,
    },
}

/// Typed successful result returned by a COSH execution target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", content = "result", rename_all = "snake_case")]
pub enum BrokeredOperationResult {
    /// Result of creating a checkpoint for the bound workspace.
    WorkspaceCheckpointCreateV1(WorkspaceCheckpointCreateV1Result),
}

/// Result of a brokered workspace checkpoint creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCheckpointCreateV1Result {
    /// Broker-allocated checkpoint identity from the request.
    pub checkpoint_id: CheckpointId,
    /// Target-reported checkpoint creation outcome.
    pub outcome: WorkspaceCheckpointCreateV1Outcome,
}

/// Target-reported outcome for a workspace checkpoint creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WorkspaceCheckpointCreateV1Outcome {
    /// The target created a new snapshot.
    Created {
        /// Opaque bounded snapshot identity allocated by the checkpoint target.
        snapshot_id: crate::common::BoundedOpaque,
    },
    /// The target safely skipped creation without producing a snapshot.
    Skipped {
        /// Redacted bounded reason for the skip.
        reason: BoundedText,
    },
}

/// Describes where a tool side effect is ultimately enforced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionAuthority {
    /// COSH observes an Agent-native execution but cannot enforce permit consumption.
    ProviderNativeObserved,
    /// A COSH execution target validates and consumes a broker-issued permit.
    CoshBrokered,
}

/// Exact Runtime identity of one pending permission callback.
///
/// Resolution callers must reproduce every field. A request identity alone is
/// insufficient because it does not fence a restarted Runtime or a later turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimePermissionRef {
    /// Fenced Runtime binding that emitted the callback.
    pub binding_id: RuntimeBindingId,
    /// Runtime generation copied from the durable binding.
    pub runtime_generation: u64,
    /// Monotonic event sequence carrying the callback.
    pub event_sequence: u64,
    /// Run that owns the callback.
    pub run_id: RunId,
    /// Prompt turn waiting for the decision.
    pub turn_id: TurnId,
    /// Stable COSH tool identity when the callback belongs to a tool snapshot.
    pub tool_use_id: Option<ToolUseId>,
    /// COSH capability request being resolved.
    pub request_id: RequestId,
}

/// Exact Runtime identity of one pending COSH-brokered execution callback.
///
/// The typed operation is part of the fence, so a callback cannot be rebound
/// to another checkpoint identity after Gateway durably accepts it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokeredExecutionRef {
    /// Fenced Runtime binding that emitted the request.
    pub binding_id: RuntimeBindingId,
    /// Runtime generation copied from the durable binding.
    pub runtime_generation: u64,
    /// Monotonic event sequence carrying the request.
    pub event_sequence: u64,
    /// Run that owns the request.
    pub run_id: RunId,
    /// Prompt turn waiting for takeover and a terminal result.
    pub turn_id: TurnId,
    /// Stable COSH tool identity when the request belongs to a tool snapshot.
    pub tool_use_id: Option<ToolUseId>,
    /// COSH capability request being brokered.
    pub request_id: RequestId,
    /// Closed typed operation and its broker-allocated identity.
    pub operation: BrokeredOperation,
}

/// Provider-neutral execution status for one observed tool invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolInvocationStatus {
    /// Input is still being prepared or permission is pending.
    Pending,
    /// The provider reports that execution is in progress.
    InProgress,
    /// The provider reports successful completion.
    Completed,
    /// The provider reports failed completion.
    Failed,
}

impl ToolInvocationStatus {
    /// Returns whether no later state mutation is valid for this invocation.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

/// Stable COSH projection of one ACP or provider tool invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInvocationSnapshot {
    /// Prompt turn that owns the invocation.
    pub turn_id: TurnId,
    /// Stable COSH identity retained across provider updates.
    pub tool_use_id: ToolUseId,
    /// Monotonic revision within this invocation.
    pub revision: u64,
    /// Redacted provider-neutral presentation.
    pub summary: ToolSummary,
    /// Latest provider-reported execution status.
    pub status: ToolInvocationStatus,
    /// Boundary at which a side effect is enforced.
    pub authority: ExecutionAuthority,
}

/// Limit that ended an Agent turn before normal completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnLimit {
    /// The provider reached its token budget.
    Tokens,
    /// The provider reached its request budget for the turn.
    Requests,
}

/// Terminal result of one prompt turn, independent from Task or Run settlement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum TurnOutcome {
    /// The Agent completed the turn normally.
    Completed,
    /// A configured limit stopped the turn before normal completion.
    LimitReached {
        /// Limit that stopped the turn.
        limit: TurnLimit,
    },
    /// The Agent refused to process the turn.
    Refused,
    /// The Agent acknowledged cancellation of the turn.
    Cancelled,
    /// The turn ended with a bounded provider or Runtime failure.
    Failed {
        /// Safe failure that does not expose provider payloads.
        error: ContractError,
    },
}

/// Terminal result reported by a legacy Runtime for an entire Run.
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
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentRuntimeEvent {
    /// A provider or ACP session was opened and fenced.
    SessionOpened {
        /// New active binding.
        binding: RuntimeBindingRef,
    },
    /// One explicitly identified prompt turn was accepted by the Runtime.
    TurnStarted {
        /// Turn that became active.
        turn_id: TurnId,
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
    /// A tool invocation was created or advanced within an explicit turn.
    ToolInvocationUpdated {
        /// Stable latest projection of the invocation.
        snapshot: ToolInvocationSnapshot,
    },
    /// Runtime requested permission for a capability.
    PermissionRequested {
        /// Neutral capability request evaluated by the broker.
        request: CapabilityRequest,
    },
    /// Runtime requested permission for a provider-owned side effect.
    ///
    /// This variant is always observation-only. Brokered effects use
    /// `BrokeredExecutionRequested`, so no authority selector is accepted.
    ExecutionPermissionRequested {
        /// Turn owning the tool invocation.
        turn_id: TurnId,
        /// Stable tool identity when a prior update established one.
        tool_use_id: Option<ToolUseId>,
        /// Agent-provided, bounded presentation that carries no authority.
        summary: ToolSummary,
        /// Neutral capability request evaluated by the broker.
        request: CapabilityRequest,
    },
    /// Runtime requested a typed operation whose effect is owned by COSH.
    BrokeredExecutionRequested {
        /// Turn owning the brokered invocation.
        turn_id: TurnId,
        /// Stable tool identity when a prior update established one.
        tool_use_id: Option<ToolUseId>,
        /// Agent-provided, bounded presentation that carries no authority.
        summary: ToolSummary,
        /// Neutral capability request evaluated by the broker.
        request: CapabilityRequest,
        /// Closed typed operation executed only by a COSH target.
        operation: BrokeredOperation,
    },
    /// Runtime asked Gateway to durably coordinate bounded user input.
    InputRequested {
        /// Exact request, Run, Turn, presentation, and response constraints.
        request: RuntimeInputRequest,
    },
    /// Runtime reported cumulative token usage.
    UsageUpdated {
        /// Current cumulative usage.
        usage: RuntimeUsage,
    },
    /// Runtime turn reached a terminal outcome.
    Completed {
        /// Turn that reached a terminal result.
        turn_id: TurnId,
        /// Turn result that must not directly settle the owning Task.
        outcome: TurnOutcome,
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        AgentRuntimeEvent, ExecutionAuthority, RuntimePermissionDecision, ToolInvocationSnapshot,
        ToolInvocationStatus, ToolSummary, TurnLimit, TurnOutcome,
    };
    use crate::{
        common::{BoundedName, BoundedText},
        ids::{ToolUseId, TurnId},
    };

    #[test]
    fn turn_limits_are_not_serialized_as_run_success() {
        let event = AgentRuntimeEvent::Completed {
            turn_id: TurnId::new(),
            outcome: TurnOutcome::LimitReached {
                limit: TurnLimit::Tokens,
            },
        };

        let value = serde_json::to_value(event).expect("turn event serializes");
        assert_eq!(value["event"], "completed");
        assert_eq!(value["outcome"]["outcome"], "limit_reached");
        assert_eq!(value["outcome"]["limit"], "tokens");
        assert_ne!(value["outcome"], json!({"outcome": "succeeded"}));
    }

    #[test]
    fn tool_snapshot_records_observation_only_authority() {
        let event = AgentRuntimeEvent::ToolInvocationUpdated {
            snapshot: ToolInvocationSnapshot {
                turn_id: TurnId::new(),
                tool_use_id: ToolUseId::new(),
                revision: 1,
                summary: ToolSummary {
                    name: BoundedName::new("execute").expect("test name is bounded"),
                    summary: BoundedText::new("Run tests").expect("test text is bounded"),
                },
                status: ToolInvocationStatus::Pending,
                authority: ExecutionAuthority::ProviderNativeObserved,
            },
        };

        let value = serde_json::to_value(event).expect("tool event serializes");
        assert_eq!(value["snapshot"]["authority"], "provider_native_observed");
        assert_ne!(value["snapshot"]["authority"], "cosh_brokered");
    }

    #[test]
    fn provider_native_allow_never_serializes_a_cosh_permit() {
        let value = serde_json::to_value(RuntimePermissionDecision::ProviderNativeAllowOnce)
            .expect("provider decision serializes");
        assert_eq!(value["decision"], "provider_native_allow_once");
        assert!(value.get("permit_id").is_none());
    }
}
