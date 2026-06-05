use crate::hook_types::HookFinding;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellEventKind {
    ShellStarted,
    ShellReady,
    UserInputIntercepted,
    CommandStarted,
    CommandCompleted,
    CommandFailed,
    ShellExited,
    ComponentFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellEvent {
    pub kind: ShellEventKind,
    pub session_id: String,
    pub command_id: Option<String>,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub end_cwd: Option<String>,
    pub exit_code: Option<i32>,
    pub started_at_ms: Option<u64>,
    pub ended_at_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub terminal_output_ref: Option<String>,
    pub terminal_output_bytes: Option<u64>,
    pub input: Option<String>,
    pub component: Option<String>,
    pub message: Option<String>,
}

impl ShellEvent {
    pub fn command_started(
        session_id: impl Into<String>,
        command_id: impl Into<String>,
        command: impl Into<String>,
        cwd: impl Into<String>,
        started_at_ms: u64,
    ) -> Self {
        Self {
            kind: ShellEventKind::CommandStarted,
            session_id: session_id.into(),
            command_id: Some(command_id.into()),
            command: Some(command.into()),
            cwd: Some(cwd.into()),
            end_cwd: None,
            exit_code: None,
            started_at_ms: Some(started_at_ms),
            ended_at_ms: None,
            duration_ms: None,
            terminal_output_ref: None,
            terminal_output_bytes: None,
            input: None,
            component: None,
            message: None,
        }
    }

    pub fn command_finished(
        kind: ShellEventKind,
        session_id: impl Into<String>,
        command_id: impl Into<String>,
        exit_code: i32,
        ended_at_ms: u64,
        terminal_output_ref: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            session_id: session_id.into(),
            command_id: Some(command_id.into()),
            command: None,
            cwd: None,
            end_cwd: None,
            exit_code: Some(exit_code),
            started_at_ms: None,
            ended_at_ms: Some(ended_at_ms),
            duration_ms: None,
            terminal_output_ref: Some(terminal_output_ref.into()),
            terminal_output_bytes: Some(0),
            input: None,
            component: None,
            message: None,
        }
    }

    pub fn user_input_intercepted(session_id: impl Into<String>, input: impl Into<String>) -> Self {
        Self {
            kind: ShellEventKind::UserInputIntercepted,
            session_id: session_id.into(),
            command_id: None,
            command: None,
            cwd: None,
            end_cwd: None,
            exit_code: None,
            started_at_ms: None,
            ended_at_ms: None,
            duration_ms: None,
            terminal_output_ref: None,
            terminal_output_bytes: None,
            input: Some(input.into()),
            component: None,
            message: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputRefs {
    pub terminal_output_ref: Option<String>,
    pub terminal_output_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandBlock {
    pub id: String,
    pub session_id: String,
    pub command: String,
    pub cwd: String,
    pub end_cwd: String,
    pub started_at_ms: u64,
    pub ended_at_ms: u64,
    pub duration_ms: u64,
    pub exit_code: i32,
    pub status: CommandStatus,
    pub output: OutputRefs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    NonZeroExit,
    CommandNotFound,
    PermissionDenied,
    ServiceFailed,
    MissingOutput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub command_block_id: String,
    pub kind: FindingKind,
    pub severity: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterventionDecision {
    Suggest,
    AskAgent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Intervention {
    pub id: String,
    pub finding_id: String,
    pub command_block_id: String,
    pub decision: InterventionDecision,
    pub guidance: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    AnalysisOnly,
    RecommendOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRequest {
    pub id: String,
    pub session_id: String,
    pub command_block: CommandBlock,
    #[serde(default)]
    pub context_blocks: Vec<CommandBlock>,
    #[serde(default)]
    pub context_hints: Vec<String>,
    pub user_input: Option<String>,
    pub findings: Vec<Finding>,
    pub mode: AgentMode,
    pub user_confirmed: bool,
    #[serde(default)]
    pub hook_finding: Option<HookFinding>,
    #[serde(default)]
    pub recommended_skill: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuestionSelectionMode {
    #[default]
    Single,
    Multiple,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    StatusChanged {
        run_id: String,
        phase: String,
        message: String,
    },
    TextDelta {
        run_id: String,
        text: String,
    },
    Recommendation {
        run_id: String,
        summary: String,
        commands: Vec<String>,
        auto_execute: bool,
    },
    ToolCall {
        run_id: String,
        name: String,
        input: String,
    },
    UserQuestion {
        run_id: String,
        question: String,
        options: Vec<String>,
        allow_free_text: bool,
        #[serde(default)]
        selection_mode: QuestionSelectionMode,
    },
    Action {
        run_id: String,
        command: String,
    },
    SkillLoadStarted {
        run_id: String,
        skill: String,
        reason: String,
    },
    SkillLoadCompleted {
        run_id: String,
        skill: String,
        summary: String,
    },
    SkillLoadFailed {
        run_id: String,
        skill: String,
        error: String,
    },
    ToolOutputDelta {
        run_id: String,
        tool_id: String,
        stream: String,
        text: String,
    },
    ToolCompleted {
        run_id: String,
        tool_id: String,
        status: String,
    },
    AgentCompleted {
        run_id: String,
        summary: String,
    },
    AgentFailed {
        run_id: String,
        error: String,
    },
    AgentCancelled {
        run_id: String,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GovernanceDecision {
    Display,
    Degraded,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GovernedEvent {
    pub decision: GovernanceDecision,
    pub event: AgentEvent,
    pub reason: String,
    pub display_text: String,
    pub auto_execute: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRecord {
    pub id: String,
    pub subject: String,
    pub decision: GovernanceDecision,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    pub recommend_only: bool,
    pub permission_callback_available: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            recommend_only: true,
            permission_callback_available: false,
        }
    }
}
