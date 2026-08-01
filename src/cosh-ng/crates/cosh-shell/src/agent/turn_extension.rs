//! User-approved continuation for Agent requests that exhaust their turn
//! budget after committing a resumable provider session.

use crate::agent::events::state_has_pending_interaction;
use crate::agent::run::{start_agent_run_with_origin, ActiveAgentRun, AgentStartIntent};
use crate::approval::journal::approval_audit_input;
use crate::approval::panel::render_current_approval_request;
use crate::runtime::prelude::*;

const CONTINUATION_PROMPT: &str = "Continue the current task from where the \
previous Agent request reached its turn limit. Preserve prior progress, do \
not repeat completed work, and finish the original user request.";

#[derive(Debug, Clone)]
pub(crate) struct PendingTurnExtension {
    pub(crate) approval: RuntimeApprovalRequest,
    pub(crate) request: AgentRequest,
    pub(crate) provider_session_id: String,
}

/// Records an extension candidate without rendering it. Automatic compaction
/// may need the idle boundary first; activation happens once that gate clears.
pub(crate) fn note_capped_run(
    state: &mut InlineState,
    active_run: &ActiveAgentRun,
    committed_provider_session: Option<String>,
) -> bool {
    if active_run.provider_name != crate::adapter::COSH_CORE_PROVIDER_NAME
        || active_run.origin != AgentRunOrigin::Standard
        || state.agent_run.pending_turn_extension.is_some()
        || !state.agent_run.queued_requests.is_empty()
        || state_has_pending_interaction(state)
    {
        return false;
    }

    let terminal_events = active_run
        .governed_events
        .iter()
        .map(|event| event.event.clone())
        .collect::<Vec<_>>();
    let Some(turns) = crate::adapter::max_turn_limit(&terminal_events) else {
        return false;
    };
    let Some(provider_session_id) = committed_provider_session else {
        return false;
    };

    let approval_id = state.approvals.next_request_id();
    let mut approval = turn_extension_approval(state, active_run, &approval_id, turns);
    if let Some(audit) = state.audit.as_mut() {
        approval.audit_ref = audit.record_approval_requested(approval_audit_input(&approval));
    }
    state.agent_run.pending_turn_extension = Some(PendingTurnExtension {
        request: continuation_request(&active_run.request, &approval_id),
        approval,
        provider_session_id,
    });
    true
}

/// Renders a recorded extension after compaction and explicit queued requests
/// have had their existing priority.
pub(crate) fn activate_pending_turn_extension<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<bool> {
    let Some(pending) = state.agent_run.pending_turn_extension.as_ref() else {
        return Ok(false);
    };
    if crate::slash::session::compaction_pending_or_active(state) {
        return Ok(false);
    }
    if state.agent_run.active.is_some() || !state.agent_run.queued_requests.is_empty() {
        state.agent_run.pending_turn_extension = None;
        return Ok(false);
    }
    if state_has_pending_interaction(state) {
        return Ok(false);
    }
    if !state
        .approvals
        .requests
        .iter()
        .any(|request| request.id == pending.approval.id)
    {
        state.approvals.requests.push(pending.approval.clone());
    }
    render_current_approval_request(state, output)?;
    Ok(true)
}

/// Applies a resolved extension exactly once. A changed provider binding
/// fails closed rather than continuing a different conversation.
pub(crate) fn resolve_turn_extension<W: Write>(
    request: &RuntimeApprovalRequest,
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
    event_index: Option<usize>,
) -> std::io::Result<()> {
    let Some(pending) = state.agent_run.pending_turn_extension.take() else {
        return render_unavailable(state, output);
    };
    if pending.approval.id != request.id {
        state.agent_run.pending_turn_extension = Some(pending);
        return render_unavailable(state, output);
    }
    if request.status != ApprovalRequestStatus::Approved {
        return Ok(());
    }
    if adapter.committed_session_id().as_deref() != Some(&pending.provider_session_id) {
        return render_unavailable(state, output);
    }

    start_agent_run_with_origin(
        &pending.request,
        request.origin,
        AgentStartIntent::UserInitiated,
        adapter,
        state,
        output,
        event_index,
    )
}

fn turn_extension_approval(
    state: &InlineState,
    active_run: &ActiveAgentRun,
    id: &str,
    turns: u32,
) -> RuntimeApprovalRequest {
    let turns = turns.to_string();
    RuntimeApprovalRequest {
        id: id.to_string(),
        audit_ref: None,
        run_id: active_run.request.id.clone(),
        origin: active_run.origin,
        session_id: active_run.request.session_id.clone(),
        cwd: active_run.request.command_block.cwd.clone(),
        source: "turn-budget",
        provider_shell_request_kind: ProviderShellRequestKind::LocalApproval,
        kind: ApprovalRequestKind::TurnExtension,
        subject: state
            .i18n()
            .t(MessageId::ApprovalTurnExtensionSubject)
            .to_string(),
        preview: state.i18n().format(
            MessageId::ApprovalTurnExtensionPreview,
            &[("turns", turns.as_str())],
        ),
        risk: "low",
        request_id: None,
        tool_use_id: None,
        tool_input: None,
        original_user_request: active_run.request.user_input.clone(),
        status: ApprovalRequestStatus::Pending,
        execution_path: None,
        command_block_id: None,
        redaction_status: None,
        assessment: None,
        hook_requires_approval: false,
        hook_warnings: Vec::new(),
    }
}

fn continuation_request(original: &AgentRequest, approval_id: &str) -> AgentRequest {
    let mut request = original.clone();
    let block_id = format!("turn-extension-{approval_id}");
    request.id = format!("agent-request-{block_id}");
    request.command_block.id = block_id;
    request.command_block.command = CONTINUATION_PROMPT.to_string();
    request.command_block.started_at_ms = 0;
    request.command_block.ended_at_ms = 0;
    request.command_block.duration_ms = 0;
    request.command_block.exit_code = 0;
    request.command_block.status = CommandStatus::Completed;
    request.command_block.output = OutputRefs {
        terminal_output_ref: None,
        terminal_output_bytes: 0,
    };
    request.context_blocks.clear();
    request.context_hints.clear();
    request.user_input = Some(CONTINUATION_PROMPT.to_string());
    request.findings.clear();
    request.user_confirmed = true;
    request.hook_finding = None;
    request.recommended_skill = None;
    request
}

fn render_unavailable<W: Write>(state: &InlineState, output: &mut W) -> std::io::Result<()> {
    RatatuiInlineRenderer::for_terminal()
        .with_language(state.language)
        .write_notice_panel(
            output,
            NoticePanelModel {
                title: state
                    .i18n()
                    .t(MessageId::ApprovalTurnExtensionUnavailableTitle),
                body: vec![state
                    .i18n()
                    .t(MessageId::ApprovalTurnExtensionUnavailableBody)
                    .to_string()],
                footer: None,
            },
        )
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn continuation_request_keeps_scope_and_clears_stale_context() {
        let original = request();
        let continued = continuation_request(&original, "req-1");

        assert_eq!(continued.session_id, original.session_id);
        assert_eq!(continued.command_block.cwd, original.command_block.cwd);
        assert_eq!(continued.mode, original.mode);
        assert_eq!(continued.user_input.as_deref(), Some(CONTINUATION_PROMPT));
        assert!(continued.context_blocks.is_empty());
        assert!(continued.context_hints.is_empty());
        assert!(continued.findings.is_empty());
    }

    #[test]
    fn capped_resumable_core_run_offers_one_matching_budget() {
        let mut state = InlineState::default();
        let active_run = active_run("localized turn limit", Some(5));

        assert!(note_capped_run(
            &mut state,
            &active_run,
            Some("provider-session".to_string()),
        ));

        let pending = state
            .agent_run
            .pending_turn_extension
            .as_ref()
            .expect("turn extension");
        assert_eq!(pending.approval.kind, ApprovalRequestKind::TurnExtension);
        assert!(pending.approval.preview.contains('5'));
        assert_eq!(pending.provider_session_id, "provider-session");
    }

    #[test]
    fn capped_run_without_committed_session_stays_terminal() {
        let mut state = InlineState::default();
        let active_run = active_run("localized turn limit", Some(5));

        assert!(!note_capped_run(&mut state, &active_run, None));
        assert!(state.agent_run.pending_turn_extension.is_none());
    }

    #[test]
    fn ordinary_core_failure_does_not_offer_turns() {
        let mut state = InlineState::default();
        let active_run = active_run("provider request failed", None);

        assert!(!note_capped_run(
            &mut state,
            &active_run,
            Some("provider-session".to_string()),
        ));
        assert!(state.agent_run.pending_turn_extension.is_none());
    }

    fn active_run(error: &str, max_turns: Option<u32>) -> ActiveAgentRun {
        let request = request();
        let adapter = AdapterInstance::Fake(FakeAgentAdapter);
        let handle = adapter.start_cancellable(request.clone(), CoshApprovalMode::Recommend);
        let renderer = RatatuiInlineRenderer::for_terminal();
        ActiveAgentRun {
            request,
            origin: AgentRunOrigin::Standard,
            handle,
            provider_name: "cosh-core",
            language: Language::EnUs,
            renderer: renderer.clone(),
            status_animation: renderer.status_animation(),
            markdown_stream: renderer.stream_markdown_agent(),
            governed_events: vec![GovernedEvent {
                decision: GovernanceDecision::Display,
                policy_decision: GovernancePolicyDecision::DisplayOnly,
                event: AgentEvent::AgentFailed {
                    run_id: "run-1".to_string(),
                    error: error.to_string(),
                    error_code: max_turns.map(|_| "max_turns".to_string()),
                    max_turns,
                },
                reason: "test".to_string(),
                display_text: error.to_string(),
                auto_execute: false,
            }],
            deferred_events: Vec::new(),
            held_events: Vec::new(),
            cosh_request_filter: crate::evidence::stream::CoshRequestStreamFilter::default(),
            pending_cosh_requests: Vec::new(),
            pending_cosh_request_audits: Vec::new(),
            pending_hook_notifications: Vec::new(),
            rendered_governed_event_count: 0,
            selectable_after_event_index: None,
            started_at: Instant::now(),
            last_activity_at: Instant::now(),
            last_heartbeat_at: Instant::now(),
            current_phase: String::new(),
            current_message: String::new(),
            has_visible_text_delta: false,
            completed: true,
            host_completed_tool_ids: Vec::new(),
        }
    }

    fn request() -> AgentRequest {
        AgentRequest {
            id: "agent-request-input-1".to_string(),
            session_id: "shell-session".to_string(),
            command_block: CommandBlock {
                id: "input-1".to_string(),
                session_id: "shell-session".to_string(),
                command: "finish the task".to_string(),
                origin: Default::default(),
                cwd: "/workspace".to_string(),
                end_cwd: "/workspace".to_string(),
                started_at_ms: 1,
                ended_at_ms: 1,
                duration_ms: 0,
                exit_code: 0,
                status: CommandStatus::Completed,
                output: OutputRefs {
                    terminal_output_ref: None,
                    terminal_output_bytes: 0,
                },
                shell_environment_generation: None,
                audit_identity: None,
            },
            context_blocks: Vec::new(),
            context_hints: vec!["stale".to_string()],
            user_input: Some("finish the task".to_string()),
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
            hook_finding: None,
            recommended_skill: None,
        }
    }
}
