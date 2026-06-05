use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use cosh_shell::{agent_render::ApprovalPanelAction, GovernedEvent, HookEngine};

use super::activity_runtime::RuntimeActivityRow;
use super::agent_run_runtime::{ActiveAgentRun, PendingAgentRequest};
use super::question_runtime::RuntimeUserQuestion;

#[derive(Default)]
pub(super) struct InlineState {
    pub(super) analyzed_blocks: HashSet<String>,
    pub(super) queued_analysis_notices: HashSet<String>,
    pub(super) canceled_blocks: HashSet<String>,
    pub(super) rendered_startup_banner: bool,
    pub(super) handled_intercepts: HashSet<String>,
    pub(super) handled_command_hooks: HashSet<String>,
    pub(super) rendered_command_hook_findings: HashSet<String>,
    pub(super) command_hook_hints: Vec<RuntimeCommandHookHint>,
    pub(super) handled_confirmations: HashSet<String>,
    pub(super) handled_cancellations: HashSet<String>,
    pub(super) handled_cancel_requests: HashSet<String>,
    pub(super) handled_slash_commands: HashSet<String>,
    pub(super) handled_selections: HashSet<String>,
    pub(super) handled_approval_actions: HashSet<String>,
    pub(super) approval_requests: Vec<RuntimeApprovalRequest>,
    pub(super) approval_focus: HashMap<String, ApprovalPanelAction>,
    pub(super) expanded_approval_cards: HashSet<String>,
    pub(super) active_approval_panel_id: Option<String>,
    pub(super) active_approval_panel_height: usize,
    pub(super) approval_journal: Vec<RuntimeApprovalJournalEntry>,
    pub(super) user_questions: Vec<RuntimeUserQuestion>,
    pub(super) pending_question_id: Option<String>,
    pub(super) active_question_panel_id: Option<String>,
    pub(super) active_question_panel_height: usize,
    pub(super) handled_question_focus: HashSet<String>,
    pub(super) handled_question_answers: HashSet<String>,
    pub(super) pending_mode_panel: Option<RuntimeModePanel>,
    pub(super) active_mode_panel_id: Option<String>,
    pub(super) active_mode_panel_height: usize,
    pub(super) handled_mode_actions: HashSet<String>,
    pub(super) activity_rows: Vec<RuntimeActivityRow>,
    pub(super) activity_output_dir: Option<PathBuf>,
    pub(super) held_agent_events: Vec<GovernedEvent>,
    pub(super) selectable_commands: Vec<String>,
    pub(super) selectable_after_event_index: Option<usize>,
    pub(super) active_run: Option<ActiveAgentRun>,
    pub(super) queued_agent_requests: VecDeque<PendingAgentRequest>,
    pub(super) shell_exited: bool,
    pub(super) approval_mode: ApprovalMode,
    pub(super) analysis_mode: AnalysisMode,
    pub(super) needs_prompt_after_agent_run: bool,
    pub(super) hook_engine: HookEngine,
}

impl InlineState {
    pub(super) fn with_raw_session_dir(path: &Path) -> Self {
        Self {
            activity_output_dir: Some(path.join("agent-output-refs")),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum AnalysisMode {
    #[default]
    Smart,
    Auto,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ApprovalMode {
    #[default]
    Ask,
    Auto,
}

#[derive(Debug, Clone)]
pub(super) struct RuntimeCommandHookHint {
    pub(super) id: String,
    pub(super) command_block_id: String,
    pub(super) ended_at_ms: u64,
    pub(super) prompt_hint: String,
    pub(super) finding_markdown: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct RuntimeModePanel {
    pub(super) id: String,
    pub(super) selected_option: usize,
}

impl ApprovalMode {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Auto => "auto",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct RuntimeApprovalRequest {
    pub(super) id: String,
    pub(super) run_id: String,
    pub(super) session_id: String,
    pub(super) cwd: String,
    pub(super) source: &'static str,
    pub(super) kind: ApprovalRequestKind,
    pub(super) subject: String,
    pub(super) preview: String,
    pub(super) risk: &'static str,
    pub(super) status: ApprovalRequestStatus,
}

#[derive(Debug, Clone)]
pub(super) struct RuntimeApprovalJournalEntry {
    pub(super) id: String,
    pub(super) run_id: String,
    pub(super) source: &'static str,
    pub(super) kind: ApprovalRequestKind,
    pub(super) subject: String,
    pub(super) preview: String,
    pub(super) risk: &'static str,
    pub(super) decision: ApprovalRequestStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ApprovalRequestKind {
    Tool,
    ShellCommand,
}

impl ApprovalRequestKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Tool => "tool request",
            Self::ShellCommand => "shell command request",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ApprovalRequestStatus {
    Pending,
    Approved,
    Denied,
    Cancelled,
}

impl ApprovalRequestStatus {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Cancelled => "cancelled",
        }
    }
}
