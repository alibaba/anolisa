//! Decision subject types: `SubjectRef`, `ActionKind`, `ResourceRef` (§3.1).
//! `session_id` is the global subject; agent/user identities are upgraded
//! progressively through the binding table without blocking the decision path.

use serde::{Deserialize, Serialize};

use crate::primitives::Digest;

/// Session identifier: the global decision subject in P0.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub String);

/// Conversation identifier within a session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConversationId(pub String);

/// Agent identity, available once the binding table has been upgraded.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentId(pub String);

/// User identity, available once the binding table has been upgraded.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserId(pub String);

/// Skill reference pinned by content hash to detect drift.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SkillRef {
    /// Skill name.
    pub name: String,
    /// Skill version string.
    pub version: String,
    /// Content hash of the skill package.
    pub content_hash: Digest,
}

/// Process reference obtained from the session registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessRef {
    /// Process id.
    pub pid: u32,
    /// Process start time, disambiguating pid reuse.
    pub start_time: u64,
    /// cgroup id the process belongs to.
    pub cgroup_id: u64,
}

/// Global decision subject (§3.1). Identity is built lazily: in P0 the
/// `session_id` is the subject and the optional identities are filled in as
/// the binding table upgrades, never blocking the decision path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectRef {
    /// Global subject key.
    pub session_id: SessionId,
    /// Conversation within the session, when known.
    pub conversation_id: Option<ConversationId>,
    /// Agent identity, available after binding-table upgrade.
    pub agent_id: Option<AgentId>,
    /// User identity, available after binding-table upgrade.
    pub user_id: Option<UserId>,
    /// Skill on whose behalf the action is taken.
    pub skill: Option<SkillRef>,
    /// Backing OS process, from the session registry.
    pub process: Option<ProcessRef>,
}

/// Action families subject to adjudication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    /// Tool invocation by the agent.
    ToolCall,
    /// LLM API call.
    LlmCall,
    /// Skill activation or upgrade.
    SkillActivation,
    /// Credential use through the gateway.
    CredentialUse,
    /// Process execution (system layer).
    ProcessExec,
    /// File access (system layer).
    FileAccess,
    /// Network connection (system layer).
    NetConnect,
    /// IPC connection (system layer).
    IpcConnect,
}

/// Resource families an action targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    /// Named tool.
    Tool,
    /// Filesystem path.
    Path,
    /// Executable file.
    Executable,
    /// Network endpoint.
    Endpoint,
    /// Credential handle.
    Credential,
    /// Skill package.
    Skill,
}

/// Normalized resource reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceRef {
    /// Resource family.
    pub kind: ResourceKind,
    /// Normalized identifier: tool name, absolute path, `host:port`, ...
    pub identifier: String,
    /// Tool schema digest / executable digest, guarding against tool drift.
    pub digest: Option<Digest>,
}
