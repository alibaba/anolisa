use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::activity::runtime::RuntimeActivityRow;
use crate::agent::run::{ActiveAgentRun, PendingAgentRequest};
use crate::hooks::state::HookRuntimeState;
use crate::question::runtime::RuntimeUserQuestion;
use crate::runtime::events::ShellEventCursor;
use crate::runtime::evidence_requests::EvidenceRequestState;
use crate::runtime::evidence_state::EvidenceState;
use crate::runtime::provider_cancellation_artifacts::ProviderCancellationArtifactState;
use crate::runtime::provider_tool_state::ProviderToolState;
use crate::runtime::shell_handoff_state::ShellHandoffState;
pub(crate) use crate::runtime::state_prelude::CoshApprovalMode;
use crate::runtime::state_prelude::{
    first_program_token, ApprovalPanelAction, CommandBlock, GovernedEvent, I18n, Language,
};

pub(crate) struct AnalysisThrottle {
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
    pub(crate) fn should_throttle(&mut self, command: &str) -> bool {
        self.should_throttle_at(command, Instant::now())
    }
    pub(crate) fn should_throttle_at(&mut self, command: &str, now: Instant) -> bool {
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
    first_program_token(cmd).to_string()
}

#[derive(Default)]
pub(crate) struct InlineState {
    pub(crate) analyzed_blocks: HashSet<String>,
    pub(crate) queued_analysis_notices: HashSet<String>,
    pub(crate) canceled_blocks: HashSet<String>,
    pub(crate) rendered_failed_command_cards: HashSet<String>,
    pub(crate) rendered_startup_banner: bool,
    pub(crate) handled_intercepts: HashSet<String>,
    pub(crate) hooks: HookRuntimeState,
    pub(crate) handled_confirmations: HashSet<String>,
    pub(crate) handled_cancellations: HashSet<String>,
    pub(crate) handled_cancel_requests: HashSet<String>,
    pub(crate) handled_slash_commands: HashSet<String>,
    pub(crate) handled_details_actions: HashSet<String>,
    pub(crate) handled_selections: HashSet<String>,
    pub(crate) approvals: ApprovalState,
    pub(crate) auth: crate::auth::runtime::AuthState,
    pub(crate) questions: QuestionState,
    pub(crate) control: ControlState,
    pub(crate) activity: ActivityState,
    pub(crate) agent_run: AgentRunState,
    pub(crate) provider_cancellation_artifacts: ProviderCancellationArtifactState,
    pub(crate) evidence: EvidenceState,
    pub(crate) evidence_requests: EvidenceRequestState,
    pub(crate) session_blocks: Vec<CommandBlock>,
    pub(crate) shell_exited: bool,
    pub(crate) language: Language,
    pub(crate) approval_mode: CoshApprovalMode,
    pub(crate) analysis_mode: AnalysisMode,
    pub(crate) debug: bool,
    pub(crate) analysis_throttle: AnalysisThrottle,
    pub(crate) trigger_pty_prompt: bool,
    pub(crate) pending_shell_handoff_timeout_notice: Option<Duration>,
    pub(crate) continuity: ContinuityState,
}

#[derive(Default)]
pub(crate) struct ApprovalState {
    pub(crate) handled_actions: HashSet<String>,
    pub(crate) requests: Vec<RuntimeApprovalRequest>,
    pub(crate) focus: HashMap<String, ApprovalPanelAction>,
    pub(crate) expanded_cards: HashSet<String>,
    pub(crate) active_panel_id: Option<String>,
    pub(crate) active_panel_height: usize,
    pub(crate) journal: Vec<RuntimeApprovalJournalEntry>,
}

impl ApprovalState {
    pub(crate) fn next_request_id(&self) -> String {
        format!("req-{}", self.requests.len() + 1)
    }
    pub(crate) fn mark_foreground_shell_execution(
        &mut self,
        approval_id: &str,
        command_block_id: &str,
    ) {
        for request in &mut self.requests {
            if request.id == approval_id {
                request.execution_path = Some("foreground_shell_pty");
                request.command_block_id = Some(command_block_id.to_string());
                request.redaction_status = Some("ref_only");
            }
        }
        for entry in &mut self.journal {
            if entry.id == approval_id {
                entry.execution_path = Some("foreground_shell_pty");
                entry.command_block_id = Some(command_block_id.to_string());
                entry.redaction_status = Some("ref_only");
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct AgentRunState {
    pub(crate) active: Option<ActiveAgentRun>,
    pub(crate) queued_requests: VecDeque<PendingAgentRequest>,
    pub(crate) held_events: Vec<GovernedEvent>,
    pub(crate) needs_prompt_after_run: bool,
    pub(crate) native_prompt_after_run: bool,
    pub(crate) host_executed_shell_result_delivered: bool,
}

impl AgentRunState {
    pub(crate) fn queue_request(&mut self, pending: PendingAgentRequest) {
        if !pending.before_held_text {
            self.queued_requests.push_back(pending);
            return;
        }

        let insert_at = self
            .queued_requests
            .iter()
            .position(|queued| !queued.before_held_text)
            .unwrap_or(self.queued_requests.len());
        self.queued_requests.insert(insert_at, pending);
    }
}

#[derive(Default)]
pub(crate) struct ActivityState {
    pub(crate) rows: Vec<RuntimeActivityRow>,
    pub(crate) output_dir: Option<PathBuf>,
}

#[derive(Default)]
pub(crate) struct QuestionState {
    pub(crate) items: Vec<RuntimeUserQuestion>,
    pub(crate) pending_id: Option<String>,
    pub(crate) active_panel_id: Option<String>,
    pub(crate) active_panel_height: usize,
    pub(crate) handled_focus: HashSet<String>,
    pub(crate) handled_answers: HashSet<String>,
    pub(crate) handled_cancellations: HashSet<String>,
}

#[derive(Default)]
pub(crate) struct ControlState {
    pending_mode_panel: Option<RuntimeModePanel>,
    active_mode_panel_id: Option<String>,
    active_mode_panel_height: usize,
    handled_mode_actions: HashSet<String>,
    pending_config_panel: Option<RuntimeConfigPanel>,
    active_config_panel_id: Option<String>,
    active_config_panel_height: usize,
    pending_config_language_panel: Option<RuntimeConfigLanguagePanel>,
    active_config_language_panel_id: Option<String>,
    active_config_language_panel_height: usize,
    handled_config_actions: HashSet<String>,
    provider_tool: ProviderToolState,
    provider_shell_handoff_run_ids: HashSet<String>,
    interactive_shell_handoffs: Vec<PendingInteractiveShellHandoff>,
    shell_handoff: ShellHandoffState,
    selectable_commands: Vec<String>,
    selectable_after_event_index: Option<usize>,
    session_trusted_commands: HashSet<String>,
    event_cursor: ShellEventCursor,
}

impl ControlState {
    pub(crate) fn set_pending_mode_panel(&mut self, selected_option: usize) {
        self.pending_mode_panel = Some(RuntimeModePanel {
            id: format!("mode-{}", self.handled_mode_actions.len() + 1),
            selected_option,
        });
    }
    pub(crate) fn pending_mode_panel(&self) -> Option<&RuntimeModePanel> {
        self.pending_mode_panel.as_ref()
    }
    pub(crate) fn pending_mode_panel_mut(&mut self) -> Option<&mut RuntimeModePanel> {
        self.pending_mode_panel.as_mut()
    }
    pub(crate) fn clear_pending_mode_panel(&mut self) {
        self.pending_mode_panel = None;
    }
    pub(crate) fn claim_mode_action(&mut self, key: String) -> bool {
        self.handled_mode_actions.insert(key)
    }
    pub(crate) fn active_mode_panel_id(&self) -> Option<&str> {
        self.active_mode_panel_id.as_deref()
    }
    pub(crate) fn set_active_mode_panel(&mut self, id: String, height: usize) {
        self.active_mode_panel_id = Some(id);
        self.active_mode_panel_height = height;
    }
    pub(crate) fn active_mode_panel_height(&self) -> usize {
        self.active_mode_panel_height
    }
    pub(crate) fn clear_active_mode_panel(&mut self) {
        self.active_mode_panel_id = None;
        self.active_mode_panel_height = 0;
    }
    pub(crate) fn clear_active_mode_panel_id(&mut self) {
        self.active_mode_panel_id = None;
    }
    pub(crate) fn set_pending_config_panel(&mut self, panel: RuntimeConfigPanel) {
        self.pending_config_panel = Some(panel);
    }
    pub(crate) fn new_config_panel_id(&self) -> String {
        format!("config-{}", self.handled_config_actions.len() + 1)
    }
    pub(crate) fn pending_config_panel(&self) -> Option<&RuntimeConfigPanel> {
        self.pending_config_panel.as_ref()
    }
    pub(crate) fn pending_config_panel_mut(&mut self) -> Option<&mut RuntimeConfigPanel> {
        self.pending_config_panel.as_mut()
    }
    pub(crate) fn clear_pending_config_panel(&mut self) {
        self.pending_config_panel = None;
    }
    pub(crate) fn set_pending_config_language_panel(&mut self, selected_option: usize) {
        self.pending_config_language_panel = Some(RuntimeConfigLanguagePanel {
            id: format!("config-language-{}", self.handled_config_actions.len() + 1),
            selected_option,
        });
    }
    pub(crate) fn pending_config_language_panel(&self) -> Option<&RuntimeConfigLanguagePanel> {
        self.pending_config_language_panel.as_ref()
    }
    pub(crate) fn pending_config_language_panel_mut(
        &mut self,
    ) -> Option<&mut RuntimeConfigLanguagePanel> {
        self.pending_config_language_panel.as_mut()
    }
    pub(crate) fn clear_pending_config_language_panel(&mut self) {
        self.pending_config_language_panel = None;
    }
    pub(crate) fn claim_config_action(&mut self, key: String) -> bool {
        self.handled_config_actions.insert(key)
    }
    pub(crate) fn active_config_panel_id(&self) -> Option<&str> {
        self.active_config_panel_id.as_deref()
    }
    pub(crate) fn set_active_config_panel(&mut self, id: String, height: usize) {
        self.active_config_panel_id = Some(id);
        self.active_config_panel_height = height;
    }
    pub(crate) fn active_config_panel_height(&self) -> usize {
        self.active_config_panel_height
    }
    pub(crate) fn clear_active_config_panel(&mut self) {
        self.active_config_panel_id = None;
        self.active_config_panel_height = 0;
    }
    pub(crate) fn clear_active_config_panel_id(&mut self) {
        self.active_config_panel_id = None;
    }
    pub(crate) fn active_config_language_panel_id(&self) -> Option<&str> {
        self.active_config_language_panel_id.as_deref()
    }
    pub(crate) fn set_active_config_language_panel(&mut self, id: String, height: usize) {
        self.active_config_language_panel_id = Some(id);
        self.active_config_language_panel_height = height;
    }
    pub(crate) fn active_config_language_panel_height(&self) -> usize {
        self.active_config_language_panel_height
    }
    pub(crate) fn clear_active_config_language_panel(&mut self) {
        self.active_config_language_panel_id = None;
        self.active_config_language_panel_height = 0;
    }
    pub(crate) fn clear_active_config_language_panel_id(&mut self) {
        self.active_config_language_panel_id = None;
    }
    pub(crate) fn provider_tool(&self) -> &ProviderToolState {
        &self.provider_tool
    }
    pub(crate) fn provider_tool_mut(&mut self) -> &mut ProviderToolState {
        &mut self.provider_tool
    }
    pub(crate) fn provider_host_executed_shell_result_delivered(
        &self,
        request_id: &str,
        tool_use_id: Option<&str>,
    ) -> bool {
        self.provider_tool
            .host_executed_shell_result_delivered(request_id, tool_use_id)
    }
    pub(crate) fn claim_provider_shell_transcript_command(&mut self, tool_id: &str) -> bool {
        self.provider_tool.claim_shell_transcript_command(tool_id)
    }
    pub(crate) fn mark_provider_shell_transcript_output(&mut self, tool_id: &str) {
        self.provider_tool.mark_shell_transcript_output(tool_id);
    }
    pub(crate) fn mark_provider_shell_transcript_seen(&mut self, tool_id: &str) {
        self.provider_tool.mark_shell_transcript_seen(tool_id);
    }
    pub(crate) fn provider_shell_transcript_output_seen(&self, tool_id: &str) -> bool {
        self.provider_tool.shell_transcript_output_seen(tool_id)
    }
    pub(crate) fn provider_shell_transcript_seen(&self, tool_id: &str) -> bool {
        self.provider_tool.shell_transcript_seen(tool_id)
    }
    pub(crate) fn mark_provider_foreground_shell_command(&mut self, command: &str) -> bool {
        self.provider_tool.mark_foreground_shell_command(command)
    }
    pub(crate) fn provider_foreground_shell_command_seen(&self, command: &str) -> bool {
        self.provider_tool.foreground_shell_command_seen(command)
    }
    pub(crate) fn provider_tool_is_shell(&self, tool_id: &str) -> bool {
        self.provider_tool.is_shell_tool(tool_id)
    }
    pub(crate) fn provider_tool_is_control_permission_shell(&self, tool_id: &str) -> bool {
        self.provider_tool.is_control_permission_shell_tool(tool_id)
    }
    pub(crate) fn mark_provider_shell_handoff_run(&mut self, run_id: &str) {
        self.provider_shell_handoff_run_ids
            .insert(run_id.to_string());
    }
    pub(crate) fn provider_shell_handoff_run_seen(&self, run_id: &str) -> bool {
        self.provider_shell_handoff_run_ids.contains(run_id)
    }
    pub(crate) fn record_provider_tool_command_from_input(
        &mut self,
        run_id: &str,
        tool_id: &str,
        tool_input: &serde_json::Value,
    ) -> bool {
        self.provider_tool
            .record_command_from_input(run_id, tool_id, tool_input)
    }
    pub(crate) fn mark_provider_control_permission_shell_tool(&mut self, tool_id: &str) {
        self.provider_tool
            .mark_control_permission_shell_tool(tool_id);
    }
    pub(crate) fn record_provider_shell_command_from_tool_call(
        &mut self,
        run_id: &str,
        tool_id: &str,
        input: &str,
    ) -> bool {
        self.provider_tool
            .record_shell_command_from_tool_call(run_id, tool_id, input)
    }
    pub(crate) fn record_pending_provider_shell_command(
        &mut self,
        run_id: &str,
        command: &str,
    ) -> bool {
        self.provider_tool
            .record_pending_shell_command(run_id, command)
    }
    pub(crate) fn record_provider_tool_output_delta(
        &mut self,
        run_id: &str,
        tool_id: &str,
        stream: &str,
        text: &str,
    ) {
        self.provider_tool
            .record_output_delta(run_id, tool_id, stream, text);
    }
    pub(crate) fn shell_handoff(&self) -> &ShellHandoffState {
        &self.shell_handoff
    }
    pub(crate) fn shell_handoff_mut(&mut self) -> &mut ShellHandoffState {
        &mut self.shell_handoff
    }
    pub(crate) fn find_interactive_shell_handoff(
        &self,
        handoff_id: &str,
    ) -> Option<PendingInteractiveShellHandoff> {
        self.interactive_shell_handoffs
            .iter()
            .find(|handoff| handoff.id == handoff_id)
            .cloned()
    }
    pub(crate) fn queue_interactive_shell_handoff_for_tool_failure(
        &mut self,
        tool_id: &str,
        status: &str,
    ) -> Option<PendingInteractiveShellHandoff> {
        let command = self
            .provider_tool
            .interactive_failure_command(tool_id, status)?;
        if let Some(handoff) = self
            .interactive_shell_handoffs
            .iter()
            .find(|handoff| handoff.tool_id == tool_id)
            .cloned()
        {
            return Some(handoff);
        }

        let handoff = PendingInteractiveShellHandoff {
            id: format!("handoff-{}", self.interactive_shell_handoffs.len() + 1),
            run_id: command.run_id.clone(),
            tool_id: command.tool_id.clone(),
            command: command.command.clone(),
            exact_preview: format!("$ {}", command.command),
        };
        self.interactive_shell_handoffs.push(handoff.clone());
        Some(handoff)
    }
    pub(crate) fn interactive_shell_handoff_ids(&self) -> impl Iterator<Item = &str> {
        self.interactive_shell_handoffs
            .iter()
            .map(|handoff| handoff.id.as_str())
    }
    pub(crate) fn remember_selectable_commands(
        &mut self,
        commands: Vec<String>,
        after_event_index: Option<usize>,
    ) {
        self.selectable_commands = commands;
        self.selectable_after_event_index = after_event_index;
    }
    pub(crate) fn selectable_command(&self, index: usize) -> Option<&str> {
        self.selectable_commands.get(index).map(String::as_str)
    }
    pub(crate) fn selectable_command_count(&self) -> usize {
        self.selectable_commands.len()
    }
    pub(crate) fn selectable_commands_available_after(&self) -> Option<usize> {
        self.selectable_after_event_index
    }
    pub(crate) fn has_selectable_commands(&self) -> bool {
        !self.selectable_commands.is_empty()
    }
    pub(crate) fn trust_session_command(&mut self, key: String) {
        self.session_trusted_commands.insert(key);
    }
    pub(crate) fn session_trusted_commands(&self) -> &HashSet<String> {
        &self.session_trusted_commands
    }
    pub(crate) fn event_cursor(&self) -> ShellEventCursor {
        self.event_cursor
    }
    pub(crate) fn set_event_cursor(&mut self, cursor: ShellEventCursor) {
        self.event_cursor = cursor;
    }
}

#[derive(Default)]
pub(crate) struct ContinuityState {
    pub(crate) facts: ContinuityFacts,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingInteractiveShellHandoff {
    pub(crate) id: String,
    pub(crate) run_id: String,
    pub(crate) tool_id: String,
    pub(crate) command: String,
    pub(crate) exact_preview: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ContinuityFact {
    pub(crate) kind: ContinuityFactKind,
    pub(crate) text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContinuityFactKind {
    UserIntent,
    AgentResult,
}

#[derive(Debug, Clone)]
pub(crate) struct ContinuityFacts {
    pub(crate) items: VecDeque<ContinuityFact>,
    max_items: usize,
}

impl Default for ContinuityFacts {
    fn default() -> Self {
        Self {
            items: VecDeque::new(),
            max_items: 12,
        }
    }
}

impl ContinuityFacts {
    pub(crate) fn push(&mut self, kind: ContinuityFactKind, text: impl Into<String>) {
        let text = text.into();
        if text.trim().is_empty() {
            return;
        }
        self.items.push_back(ContinuityFact { kind, text });
        while self.items.len() > self.max_items {
            self.items.pop_front();
        }
    }
}

impl InlineState {
    pub(crate) fn with_raw_session_dir(path: &Path) -> Self {
        Self {
            activity: ActivityState {
                output_dir: Some(path.join("agent-output-refs")),
                ..ActivityState::default()
            },
            ..Self::default()
        }
    }
    pub(crate) fn i18n(&self) -> I18n {
        I18n::new(self.language)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AnalysisMode {
    #[default]
    Smart,
    Auto,
    Manual,
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeModePanel {
    pub(crate) id: String,
    pub(crate) selected_option: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeConfigPanel {
    pub(crate) id: String,
    pub(crate) setting: String,
    pub(crate) before_value: String,
    pub(crate) pending_value: String,
    pub(crate) config_path: PathBuf,
    pub(crate) selected_option: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeConfigLanguagePanel {
    pub(crate) id: String,
    pub(crate) selected_option: usize,
}

impl AnalysisMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Smart => "smart",
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeApprovalRequest {
    pub(crate) id: String,
    pub(crate) run_id: String,
    pub(crate) session_id: String,
    pub(crate) cwd: String,
    pub(crate) source: &'static str,
    pub(crate) provider_shell_request_kind: ProviderShellRequestKind,
    pub(crate) kind: ApprovalRequestKind,
    pub(crate) subject: String,
    pub(crate) preview: String,
    pub(crate) risk: &'static str,
    pub(crate) request_id: Option<String>,
    pub(crate) tool_use_id: Option<String>,
    pub(crate) tool_input: Option<serde_json::Value>,
    pub(crate) original_user_request: Option<String>,
    pub(crate) status: ApprovalRequestStatus,
    pub(crate) execution_path: Option<&'static str>,
    pub(crate) command_block_id: Option<String>,
    pub(crate) redaction_status: Option<&'static str>,
    pub(crate) assessment: Option<RuntimeCommandAssessmentSummary>,
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeCommandAssessmentSummary {
    pub(crate) impact: &'static str,
    pub(crate) execution: &'static str,
    pub(crate) confidence: &'static str,
    pub(crate) primary_reason: &'static str,
    pub(crate) reason_trace: String,
    pub(crate) auto_allow: Option<&'static str>,
    pub(crate) output_stability: &'static str,
    pub(crate) output_exposure: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderShellRequestKind {
    ControlPermission,
    StreamedToolCallFallback,
    LocalApproval,
}

impl ProviderShellRequestKind {
    pub(crate) fn is_control_permission(self) -> bool {
        matches!(self, Self::ControlPermission)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeApprovalJournalEntry {
    pub(crate) id: String,
    pub(crate) run_id: String,
    pub(crate) source: &'static str,
    pub(crate) kind: ApprovalRequestKind,
    pub(crate) subject: String,
    pub(crate) preview: String,
    pub(crate) preview_hash: String,
    pub(crate) risk: &'static str,
    pub(crate) request_id: Option<String>,
    pub(crate) tool_use_id: Option<String>,
    pub(crate) actor: &'static str,
    pub(crate) decision: ApprovalRequestStatus,
    pub(crate) execution_path: Option<&'static str>,
    pub(crate) command_block_id: Option<String>,
    pub(crate) redaction_status: Option<&'static str>,
    pub(crate) assessment: Option<RuntimeCommandAssessmentSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApprovalRequestKind {
    Tool,
    ShellCommand,
}

impl ApprovalRequestKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Tool => "tool request",
            Self::ShellCommand => "shell command request",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApprovalRequestStatus {
    Pending,
    Approved,
    Blocked,
    Denied,
    Cancelled,
}

impl ApprovalRequestStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Blocked => "blocked",
            Self::Denied => "denied",
            Self::Cancelled => "cancelled",
        }
    }
}
