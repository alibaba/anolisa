//! Event model: `EventEnvelope`, `AgentEvent` payload families and the
//! `EventTrust` provenance levels P0-P3 (§3.2, Table 4). Trust level bounds
//! which effects an event may drive: deny and above require >= P2.

use serde::{Deserialize, Serialize};

use crate::primitives::{DecisionId, Digest, Timestamp};
use crate::subject::{SessionId, SkillRef};

/// Event identifier (ULID). Monotonic only within a single source; no global
/// ordering across sources — per-session processing order is the serial-queue
/// arrival order (§6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventId(pub ulid::Ulid);

/// LLM call identifier within a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LlmCallId(pub String);

/// Tool call identifier within a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolCallId(pub String);

/// Trajectory span the event belongs to (session/conversation/llm call/tool
/// call layering).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventSpan {
    /// Conversation, when known.
    pub conversation_id: Option<crate::subject::ConversationId>,
    /// Enclosing LLM call, when known.
    pub llm_call_id: Option<LlmCallId>,
    /// Enclosing tool call, when known.
    pub tool_call_id: Option<ToolCallId>,
}

/// Producer of the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSource {
    /// Agent-runtime hook.
    Hook,
    /// AgentSecCore daemon.
    Daemon,
    /// Kernel event stream (interface A).
    KernelA,
    /// Taint profile push (interface B).
    TaintB,
    /// Out-of-band control plane (CLI).
    Cli,
}

/// Provenance assurance level (Table 4). Ordered: `P0 < P1 < P2 < P3`.
/// P0/P1 sources may only drive audit/step_up; deny and above require >= P2;
/// P3 is the precondition for automatic execution of irreversible actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EventTrust {
    /// Temporal inference only (ordering of application-layer events).
    P0,
    /// Agent-runtime self-declaration (hook-reported dependencies).
    P1,
    /// Trusted-component association (session registry, taint profile).
    P2,
    /// Structured capability/dataflow (credential gateway, broker-derived).
    P3,
}

/// Uniform event envelope wrapping a typed payload (§3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    /// Envelope schema version; currently 1.
    pub schema_version: u16,
    /// Source-monotonic event id.
    pub event_id: EventId,
    /// Session the event belongs to.
    pub session_id: SessionId,
    /// Trajectory span coordinates.
    pub span: EventSpan,
    /// Producer of the event.
    pub source: EventSource,
    /// Provenance assurance level (Table 4).
    pub trust: EventTrust,
    /// Monotonic clock reading in nanoseconds; audit timeline only, never
    /// used for decision ordering (§6.1).
    pub ts_mono_ns: u64,
    /// Wall-clock timestamp.
    pub ts_wall: Timestamp,
    /// Typed payload.
    pub payload: AgentEvent,
}

/// Typed event payloads: application layer (hook/daemon), system layer
/// (interface A), taint pushes (interface B) and control-plane actions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEvent {
    /// Conversation message.
    Message(MessageEvent),
    /// LLM API call.
    LlmCall(LlmCallEvent),
    /// Tool invocation.
    ToolCall(ToolCallEvent),
    /// Tool output.
    ToolOutput(ToolOutputEvent),
    /// Skill activation.
    SkillActivation(SkillActivationEvent),
    /// Credential use through the gateway.
    CredentialUse(CredentialUseEvent),
    /// Process execution (interface A).
    ProcessExec(OsProcessEvent),
    /// File access (interface A).
    FileAccess(OsFileEvent),
    /// Network connection (interface A).
    NetConnect(OsNetEvent),
    /// Taint alert push (interface B).
    TaintAlert(TaintAlertEvent),
    /// Out-of-band control action.
    ControlAction(ControlActionEvent),
}

/// Conversation message payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageEvent {
    /// Message role (`user`, `assistant`, `system`, `tool`).
    pub role: String,
    /// Digest of the message content.
    pub content_digest: Digest,
    /// Reference to externally stored content, if retained.
    pub content_ref: Option<String>,
}

/// Token usage of an LLM call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmUsage {
    /// Prompt tokens consumed.
    pub input_tokens: u64,
    /// Completion tokens produced.
    pub output_tokens: u64,
}

/// LLM call payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmCallEvent {
    /// Model identifier.
    pub model: String,
    /// Digest of the full prompt.
    pub prompt_digest: Digest,
    /// Token usage, when reported.
    pub usage: Option<LlmUsage>,
}

/// Tool invocation payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallEvent {
    /// Tool name.
    pub tool: String,
    /// Parsed tool arguments.
    pub args: serde_json::Value,
    /// Digest of the canonical argument encoding.
    pub args_digest: Digest,
    /// Digest of the tool schema, guarding against tool drift.
    pub tool_schema_digest: Option<Digest>,
}

/// Tool output payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutputEvent {
    /// Tool name.
    pub tool: String,
    /// Reference to externally stored content, if retained.
    pub content_ref: Option<String>,
    /// Digest of the output content.
    pub content_digest: Digest,
    /// Content labels (e.g. `untrusted_web`).
    pub labels: Vec<String>,
}

/// Skill activation payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillActivationEvent {
    /// Activated skill.
    pub skill_ref: SkillRef,
    /// Skill Ledger trust state at activation time.
    pub ledger_state: String,
    /// Decision that authorized the activation, when adjudicated.
    pub decision_ref: Option<DecisionId>,
}

/// Credential use payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialUseEvent {
    /// Credential handle (never the secret itself).
    pub credential_ref: String,
    /// Endpoint the credential is presented to.
    pub target_endpoint: String,
    /// Access method (e.g. HTTP verb).
    pub method: String,
}

/// Process execution payload (interface A).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OsProcessEvent {
    /// Executable path.
    pub exe_path: String,
    /// Digest of the canonical argv encoding.
    pub argv_digest: Digest,
    /// cgroup id of the process.
    pub cgroup_id: u64,
    /// Parent process id.
    pub parent_pid: u32,
}

/// File access mode for [`OsFileEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAccessMode {
    /// Read access.
    Read,
    /// Write access.
    Write,
    /// Execute access.
    Execute,
}

/// File access payload (interface A).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OsFileEvent {
    /// Accessed path.
    pub path: String,
    /// Access mode.
    pub mode: FileAccessMode,
    /// cgroup id of the accessing process.
    pub cgroup_id: u64,
}

/// Network connection payload (interface A).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OsNetEvent {
    /// Destination address.
    pub dst: String,
    /// Destination port.
    pub port: u16,
    /// Transport protocol.
    pub protocol: String,
    /// cgroup id of the connecting process.
    pub cgroup_id: u64,
    /// TLS SNI, when observed.
    pub sni: Option<String>,
}

/// Taint alert payload (interface B push).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaintAlertEvent {
    /// Tainted process id.
    pub pid: u32,
    /// Taint labels.
    pub labels: Vec<String>,
    /// Provenance chain of the taint.
    pub provenance_chain: Vec<String>,
}

/// Control-plane action kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlActionKind {
    /// Freeze the session (cgroup freezer).
    Freeze,
    /// Kill the session process tree (operator-confirmed).
    Kill,
    /// Switch the active policy revision.
    PolicySwitch,
}

/// Control-plane action payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlActionEvent {
    /// Action taken.
    pub action: ControlActionKind,
    /// Operator who initiated the action.
    pub operator: String,
    /// Stated reason for the action.
    pub reason: String,
}
