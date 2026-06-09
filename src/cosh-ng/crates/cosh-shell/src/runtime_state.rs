use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Instant;

use cosh_shell::{agent_render::ApprovalPanelAction, GovernedEvent, HookEngine};

use super::activity_runtime::RuntimeActivityRow;
use super::agent_run_runtime::{ActiveAgentRun, PendingAgentRequest};
use super::question_runtime::RuntimeUserQuestion;
pub(super) use cosh_shell::types::CoshApprovalMode;

pub(super) struct AnalysisThrottle {
    recent: HashMap<String, (Instant, usize)>,
    cooldown_secs: u64,
}

impl Default for AnalysisThrottle {
    fn default() -> Self {
        Self {
            recent: HashMap::new(),
            cooldown_secs: 30,
        }
    }
}

impl AnalysisThrottle {
    pub(super) fn should_throttle(&mut self, command: &str) -> bool {
        self.should_throttle_at(command, Instant::now())
    }

    fn should_throttle_at(&mut self, command: &str, now: Instant) -> bool {
        let key = normalize_command(command);
        if let Some((window_started, count)) = self.recent.get_mut(&key) {
            if now.duration_since(*window_started).as_secs() < self.cooldown_secs {
                *count += 1;
                return true;
            }
        }
        self.recent.insert(key, (now, 1));
        false
    }
}

fn normalize_command(cmd: &str) -> String {
    cosh_shell::first_program_token(cmd).to_string()
}

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
    pub(super) approval_mode: CoshApprovalMode,
    pub(super) analysis_mode: AnalysisMode,
    pub(super) analysis_throttle: AnalysisThrottle,
    pub(super) needs_prompt_after_agent_run: bool,
    pub(super) trigger_pty_prompt: bool,
    pub(super) hook_engine: HookEngine,
    pub(super) disabled_hooks: HashSet<String>,
    pub(super) pending_consultation: Option<PendingConsultation>,
}

#[derive(Debug, Clone)]
pub(super) struct PendingConsultation {
    pub(super) card_id: String,
    pub(super) block_id: String,
    #[allow(dead_code)]
    pub(super) prompt_hint: String,
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

#[derive(Debug, Clone)]
pub(super) struct RuntimeCommandHookHint {
    pub(super) id: String,
    pub(super) command_block_id: String,
    pub(super) ended_at_ms: u64,
    pub(super) prompt_hint: String,
    pub(super) finding_markdown: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::AnalysisThrottle;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    fn throttle(cooldown_secs: u64) -> AnalysisThrottle {
        AnalysisThrottle {
            recent: HashMap::new(),
            cooldown_secs,
        }
    }

    #[test]
    fn analysis_throttle_uses_fixed_window_instead_of_sliding_forever() {
        let start = Instant::now();
        let mut throttle = throttle(30);

        assert!(!throttle.should_throttle_at("ps -aux", start));
        assert!(throttle.should_throttle_at("ps -aux", start + Duration::from_secs(1)));
        assert!(throttle.should_throttle_at("ps -aux", start + Duration::from_secs(29)));
        assert!(!throttle.should_throttle_at("ps -aux", start + Duration::from_secs(30)));
        assert!(throttle.should_throttle_at("ps -aux", start + Duration::from_secs(31)));
    }
}

#[derive(Debug, Clone)]
pub(super) struct RuntimeModePanel {
    pub(super) id: String,
    pub(super) selected_option: usize,
}

impl AnalysisMode {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Smart => "smart",
            Self::Auto => "auto",
            Self::Manual => "manual",
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
    pub(super) request_id: Option<String>,
    pub(super) tool_use_id: Option<String>,
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
    Blocked,
    Denied,
    Cancelled,
}

impl ApprovalRequestStatus {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Blocked => "blocked",
            Self::Denied => "denied",
            Self::Cancelled => "cancelled",
        }
    }
}
