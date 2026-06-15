use crate::hook_types::HookFinding;
use serde::{Deserialize, Serialize};

pub const COMMAND_OUTPUT_REF_MAX_BYTES: usize = 1024 * 1024;
pub const SESSION_OUTPUT_REF_MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellHandoffRequest {
    pub command: String,
    pub exact_preview: String,
    pub source: String,
    pub actor: String,
    pub approval_id: String,
    pub run_id: String,
    pub request_id: Option<String>,
    pub tool_use_id: Option<String>,
    pub created_at_ms: u64,
    pub preview_hash: String,
}

impl ShellHandoffRequest {
    pub fn new(
        command: impl Into<String>,
        exact_preview: impl Into<String>,
        source: impl Into<String>,
        actor: impl Into<String>,
        approval_id: impl Into<String>,
        run_id: impl Into<String>,
        created_at_ms: u64,
    ) -> Result<Self, String> {
        let exact_preview = exact_preview.into();
        let request = Self {
            command: command.into(),
            preview_hash: preview_hash(&exact_preview),
            exact_preview,
            source: source.into(),
            actor: actor.into(),
            approval_id: approval_id.into(),
            run_id: run_id.into(),
            request_id: None,
            tool_use_id: None,
            created_at_ms,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.command.trim().is_empty() {
            return Err("empty shell handoff command".to_string());
        }
        if self.command.contains('\0') {
            return Err("shell handoff command contains NUL byte".to_string());
        }
        if self.command.chars().any(|ch| matches!(ch, '\n' | '\r')) {
            return Err(
                "shell handoff command contains newline; multiline handoff is not enabled"
                    .to_string(),
            );
        }
        if self
            .command
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\t'))
        {
            return Err("shell handoff command contains blocked control character".to_string());
        }
        if self.exact_preview.is_empty() {
            return Err("shell handoff preview is empty".to_string());
        }
        if self.approval_id.trim().is_empty() {
            return Err("shell handoff approval id is empty".to_string());
        }
        if self.run_id.trim().is_empty() {
            return Err("shell handoff run id is empty".to_string());
        }
        Ok(())
    }

    pub fn pty_bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut bytes = self.command.as_bytes().to_vec();
        bytes.push(b'\n');
        Ok(bytes)
    }
}

fn preview_hash(value: &str) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("fnv1a64:{hash:016x}")
}

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
        #[serde(default)]
        tool_id: Option<String>,
        name: String,
        input: String,
    },
    UserQuestion {
        run_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_request_id: Option<String>,
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
    ToolPermissionRequest {
        run_id: String,
        request_id: String,
        tool_name: String,
        tool_input: serde_json::Value,
        tool_use_id: String,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CoshApprovalMode {
    Recommend,
    #[default]
    Auto,
    Trust,
}

impl CoshApprovalMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Recommend => "recommend",
            Self::Auto => "auto",
            Self::Trust => "trust",
        }
    }

    pub fn uses_control_protocol(self) -> bool {
        matches!(self, Self::Auto | Self::Trust)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GovernanceDecision {
    Display,
    Degraded,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GovernancePolicyDecision {
    #[default]
    DisplayOnly,
    NeedsUserApproval,
    ProviderApprovalResponse,
    HostAutoApproved,
    HostDenied,
    HostBlocked,
    AuditOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GovernedEvent {
    pub decision: GovernanceDecision,
    #[serde(default)]
    pub policy_decision: GovernancePolicyDecision,
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

#[cfg(test)]
mod tests {
    use super::ShellHandoffRequest;

    fn handoff(command: &str) -> Result<ShellHandoffRequest, String> {
        ShellHandoffRequest::new(
            command,
            format!("$ {command}"),
            "test",
            "user",
            "approval-1",
            "run-1",
            42,
        )
    }

    #[test]
    fn shell_handoff_rejects_empty_nul_newline_and_control_chars() {
        for command in [
            "",
            "printf '\0'",
            "printf one\nprintf two",
            "printf '\u{1b}[31mred'",
        ] {
            assert!(handoff(command).is_err(), "{command:?}");
        }
    }

    #[test]
    fn shell_handoff_allows_visible_command_and_tab_separator() {
        let request = handoff("printf\tok").expect("tab-separated command is visible input");

        assert_eq!(request.pty_bytes().unwrap(), b"printf\tok\n");
        assert_eq!(request.preview_hash, "fnv1a64:7d74cbb1a6f6fb27");
    }
}
