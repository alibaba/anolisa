use std::time::Instant;

use crate::agent::continuation::provider_mode_for_agent_run;
use crate::agent::poll::poll_active_agent_run;
use crate::agent::skill_context::finalize_agent_request_skill_context;
use crate::evidence::request::ParsedCoshRequest;
use crate::evidence::stream::{CoshRequestAuditRecord, CoshRequestStreamFilter};
use crate::recommendation::personal_integration::record_started_agent_request;
use crate::runtime::prelude::*;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum AgentRunOrigin {
    #[default]
    Standard,
    InsightPrompt,
    AutoFailure,
}

impl AgentRunOrigin {
    pub(crate) fn is_insight_triggered(self) -> bool {
        matches!(self, Self::InsightPrompt | Self::AutoFailure)
    }
}

pub(crate) struct ActiveAgentRun {
    pub(crate) request: AgentRequest,
    pub(crate) origin: AgentRunOrigin,
    pub(crate) handle: AgentRunHandle,
    pub(crate) provider_name: &'static str,
    pub(crate) language: Language,
    pub(crate) renderer: RatatuiInlineRenderer,
    pub(crate) status_animation: AgentStatusAnimation,
    pub(crate) markdown_stream: MarkdownStreamBlock,
    pub(crate) governed_events: Vec<GovernedEvent>,
    pub(crate) deferred_events: Vec<GovernedEvent>,
    pub(crate) held_events: Vec<GovernedEvent>,
    pub(crate) cosh_request_filter: CoshRequestStreamFilter,
    pub(crate) pending_cosh_requests: Vec<ParsedCoshRequest>,
    pub(crate) pending_cosh_request_audits: Vec<CoshRequestAuditRecord>,
    pub(crate) pending_hook_notifications: Vec<PendingHookNotification>,
    pub(crate) rendered_governed_event_count: usize,
    pub(crate) selectable_after_event_index: Option<usize>,
    pub(crate) started_at: Instant,
    pub(crate) last_activity_at: Instant,
    pub(crate) last_heartbeat_at: Instant,
    pub(crate) current_phase: String,
    pub(crate) current_message: String,
    pub(crate) has_visible_text_delta: bool,
    pub(crate) completed: bool,
    pub(crate) host_completed_tool_ids: Vec<String>,
}

impl ActiveAgentRun {
    pub(crate) fn prepare_structured_surface<W: Write>(
        &mut self,
        output: &mut W,
    ) -> std::io::Result<bool> {
        self.status_animation.clear(output)?;
        let finished = self.markdown_stream.finish(output, None)?;
        if finished {
            self.has_visible_text_delta = false;
        }
        Ok(finished)
    }

    pub(crate) fn mark_host_completed_tool(&mut self, tool_id: &str) {
        if tool_id.trim().is_empty() {
            return;
        }
        if !self
            .host_completed_tool_ids
            .iter()
            .any(|existing| existing == tool_id)
        {
            self.host_completed_tool_ids.push(tool_id.to_string());
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PendingHookNotification {
    pub(crate) tool_use_id: Option<String>,
    pub(crate) hook_name: String,
    pub(crate) message: String,
    pub(crate) decision: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingAgentRequest {
    pub(crate) request: AgentRequest,
    pub(crate) origin: AgentRunOrigin,
    pub(crate) selectable_after_event_index: Option<usize>,
    pub(crate) before_held_text: bool,
}

pub(crate) fn start_agent_run<W: Write>(
    request: &AgentRequest,
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
    selectable_after_event_index: Option<usize>,
) -> std::io::Result<()> {
    start_agent_run_with_origin(
        request,
        AgentRunOrigin::Standard,
        adapter,
        state,
        output,
        selectable_after_event_index,
    )
}

pub(crate) fn start_agent_run_with_origin<W: Write>(
    request: &AgentRequest,
    origin: AgentRunOrigin,
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
    selectable_after_event_index: Option<usize>,
) -> std::io::Result<()> {
    start_agent_run_with_queue_policy(
        request,
        origin,
        adapter,
        state,
        output,
        selectable_after_event_index,
        false,
    )
}

fn start_agent_run_with_queue_policy<W: Write>(
    request: &AgentRequest,
    origin: AgentRunOrigin,
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
    selectable_after_event_index: Option<usize>,
    before_held_text: bool,
) -> std::io::Result<()> {
    if state.agent_run.active.is_some() {
        queue_agent_request(
            state,
            PendingAgentRequest {
                request: request.clone(),
                origin,
                selectable_after_event_index,
                before_held_text,
            },
        );
        return Ok(());
    }

    let renderer = RatatuiInlineRenderer::for_terminal().with_language(state.language);
    let markdown_stream = renderer.stream_markdown_agent();
    let mut status_animation = renderer.status_animation();
    if status_animation.is_enabled() {
        status_animation.render(output, state.i18n().t(MessageId::AgentThinking))?;
    } else {
        renderer.write_loading_text(output, state.i18n().t(MessageId::AgentThinking))?;
    }
    output.flush()?;

    let mut request = request.clone();
    state.startup_health.poll_ready();
    attach_continuity_prompt_hint(&mut request, state);
    finalize_agent_request_skill_context(&mut request, state.startup_health.report.as_ref());
    enforce_insight_context_budget(&mut request);
    let provider_mode = provider_mode_for_agent_run(&request, state.approval_mode);
    let handle = adapter.start_cancellable(request.clone(), provider_mode);
    record_started_agent_request(state, &request);
    let now = Instant::now();
    let i18n = state.i18n();
    state.agent_run.host_executed_shell_result_delivered = false;
    state.shell_evidence.clear_recent_shell_tool_outputs();
    state.agent_run.active = Some(ActiveAgentRun {
        request,
        origin,
        handle,
        provider_name: adapter.name(),
        language: state.language,
        renderer,
        status_animation,
        markdown_stream,
        governed_events: Vec::new(),
        deferred_events: Vec::new(),
        held_events: Vec::new(),
        cosh_request_filter: CoshRequestStreamFilter::default(),
        pending_cosh_requests: Vec::new(),
        pending_cosh_request_audits: Vec::new(),
        pending_hook_notifications: Vec::new(),
        rendered_governed_event_count: 0,
        selectable_after_event_index,
        started_at: now,
        last_activity_at: now,
        last_heartbeat_at: now,
        current_phase: i18n.t(MessageId::AgentStatusStarting).to_string(),
        current_message: i18n.t(MessageId::AgentStatusWaitingBackend).to_string(),
        has_visible_text_delta: false,
        completed: false,
        host_completed_tool_ids: Vec::new(),
    });
    poll_active_agent_run(state, output, adapter)
}

fn queue_agent_request(state: &mut InlineState, pending: PendingAgentRequest) {
    state.agent_run.queue_request(pending);
}

fn attach_continuity_prompt_hint(request: &mut AgentRequest, state: &InlineState) {
    let Some(input) = request.user_input.as_deref() else {
        return;
    };
    let Some(hint) = continuity_prompt_hint(state, input) else {
        return;
    };
    if !request
        .context_hints
        .iter()
        .any(|existing| existing == &hint)
    {
        request.context_hints.push(hint);
    }
}

fn enforce_insight_context_budget(request: &mut AgentRequest) {
    if !request
        .context_hints
        .iter()
        .any(|hint| hint.starts_with("insight_evidence\n"))
    {
        return;
    }

    while serialized_context_hint_bytes(&request.context_hints)
        > crate::insight::evidence::PROVIDER_CONTEXT_MAX_BYTES
    {
        if let Some(index) = request
            .context_hints
            .iter()
            .rposition(|hint| !hint.starts_with("insight_evidence\n"))
        {
            request.context_hints.remove(index);
            continue;
        }
        if request.context_hints.len() > 1 {
            request.context_hints.pop();
            continue;
        }
        let hint = &mut request.context_hints[0];
        let mut end = crate::insight::evidence::PROVIDER_CONTEXT_MAX_BYTES.min(hint.len());
        while !hint.is_char_boundary(end) {
            end -= 1;
        }
        hint.truncate(end);
    }
}

fn serialized_context_hint_bytes(hints: &[String]) -> usize {
    hints.iter().map(String::len).sum::<usize>() + hints.len().saturating_sub(1)
}

pub(crate) fn stop_active_agent_run_without_rendering<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    state.agent_run.held_events.clear();
    let Some(mut active_run) = state.agent_run.active.take() else {
        return Ok(());
    };

    active_run.handle.cancel();
    active_run.status_animation.clear(output)?;
    active_run.held_events.clear();
    active_run.deferred_events.clear();
    active_run.cosh_request_filter.clear();
    active_run.pending_cosh_requests.clear();
    active_run.pending_cosh_request_audits.clear();
    output.flush()?;
    Ok(())
}

pub(super) fn has_queued_run_before_held_text(state: &InlineState) -> bool {
    state
        .agent_run
        .queued_requests
        .iter()
        .any(|pending| pending.before_held_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::skill_context::finalize_agent_request_skill_context;
    use crate::diagnostics::health::{
        HealthFact, HealthFactCategory, HealthFactSource, HealthFactValue, HealthScanReport,
        HealthSeverity,
    };
    use crate::types::STARTUP_HEALTH_FOLLOW_UP_BINDING_HINT;

    #[test]
    fn only_insight_origins_use_strict_result_presentation() {
        assert!(!AgentRunOrigin::Standard.is_insight_triggered());
        assert!(AgentRunOrigin::InsightPrompt.is_insight_triggered());
        assert!(AgentRunOrigin::AutoFailure.is_insight_triggered());
    }

    #[test]
    fn final_insight_context_never_exceeds_provider_budget() {
        let mut request = test_agent_request();
        request.context_hints = vec![
            "x".repeat(crate::insight::evidence::PROVIDER_CONTEXT_MAX_BYTES),
            "insight_evidence\ntarget_facts:\ncommand_id=cmd-1".to_string(),
        ];

        finalize_agent_request_skill_context(&mut request, None);
        enforce_insight_context_budget(&mut request);

        assert!(
            serialized_context_hint_bytes(&request.context_hints)
                <= crate::insight::evidence::PROVIDER_CONTEXT_MAX_BYTES
        );
        assert_eq!(request.context_hints.len(), 1);
        assert!(request.context_hints[0].starts_with("insight_evidence\n"));
    }

    #[test]
    fn malformed_oversized_insight_payload_is_utf8_safely_bounded() {
        let mut request = test_agent_request();
        request.context_hints = vec![format!(
            "insight_evidence\n{}",
            "界".repeat(crate::insight::evidence::PROVIDER_CONTEXT_MAX_BYTES)
        )];

        enforce_insight_context_budget(&mut request);

        assert!(
            serialized_context_hint_bytes(&request.context_hints)
                <= crate::insight::evidence::PROVIDER_CONTEXT_MAX_BYTES
        );
        assert!(request.context_hints[0].is_char_boundary(request.context_hints[0].len()));
    }

    #[test]
    fn health_context_hint_is_not_attached_to_free_form_request() {
        let report = test_health_report();
        let mut request = test_agent_request();

        finalize_agent_request_skill_context(&mut request, Some(&report));

        assert!(!request
            .context_hints
            .iter()
            .any(|hint| hint.starts_with("health_scan ")));
    }

    #[test]
    fn health_context_hint_is_attached_to_startup_health_follow_up() {
        let report = test_health_report();
        let mut request = test_agent_request();
        request
            .context_hints
            .push(STARTUP_HEALTH_FOLLOW_UP_BINDING_HINT.to_string());

        finalize_agent_request_skill_context(&mut request, Some(&report));

        let hint = request
            .context_hints
            .iter()
            .find(|hint| hint.starts_with("health_scan "))
            .expect("health context hint");
        assert!(hint.contains("scan_id=health-1"), "{hint}");
        assert!(hint.contains("overall_severity=warning"), "{hint}");
        assert!(hint.contains("bounded_facts_only=true"), "{hint}");
        assert!(hint.contains("no_collector_stdout=true"), "{hint}");
        assert!(!hint.contains("/tmp/cosh"), "{hint}");
    }

    #[test]
    fn health_context_hint_dedupes_existing_health_hint() {
        let report = test_health_report();
        let mut request = test_agent_request();
        request
            .context_hints
            .push(STARTUP_HEALTH_FOLLOW_UP_BINDING_HINT.to_string());
        request
            .context_hints
            .push("health_scan scan_id=existing".to_string());

        finalize_agent_request_skill_context(&mut request, Some(&report));

        assert_eq!(
            request
                .context_hints
                .iter()
                .filter(|hint| hint.starts_with("health_scan "))
                .count(),
            1
        );
    }

    fn test_health_report() -> HealthScanReport {
        let mut report = HealthScanReport::new("health-1", 0);
        report.overall_severity = HealthSeverity::Warning;
        report.facts.push(HealthFact {
            id: "memory.available_ratio".to_string(),
            category: HealthFactCategory::Memory,
            key: "memory.available_ratio".to_string(),
            value: HealthFactValue::Float(0.08),
            unit: None,
            source: HealthFactSource::Fixture,
            elapsed_ms: 0,
        });
        report
    }

    fn test_agent_request() -> AgentRequest {
        AgentRequest {
            id: "agent-request-health".to_string(),
            session_id: "session-1".to_string(),
            command_block: CommandBlock {
                id: "cmd-1".to_string(),
                session_id: "session-1".to_string(),
                command: "分析一下这台机器内存风险".to_string(),
                origin: CommandOrigin::UserInteractive,
                cwd: "/repo".to_string(),
                end_cwd: "/repo".to_string(),
                started_at_ms: 1,
                ended_at_ms: 2,
                duration_ms: 1,
                exit_code: 0,
                status: CommandStatus::Completed,
                output: OutputRefs {
                    terminal_output_ref: None,
                    terminal_output_bytes: 0,
                },
                shell_environment_generation: None,
            },
            context_blocks: Vec::new(),
            context_hints: Vec::new(),
            user_input: Some("分析一下这台机器内存风险".to_string()),
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
            hook_finding: None,
            recommended_skill: None,
        }
    }
}
