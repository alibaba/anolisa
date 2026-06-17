use std::time::Duration;

use crate::agent::run::ActiveAgentRun;
use crate::runtime::prelude::*;

const SHELL_HANDOFF_CONTINUATION_HINT: &str =
    "analysis-only continuation after foreground shell handoff";
const SHELL_HANDOFF_RECOVERY_OWNER_HINT: &str = "shell handoff recovery owner:";
const DISABLE_PROVIDER_RESUME_HINT: &str = "disable provider resume for shell handoff fallback";
const SHELL_HANDOFF_FIRST_TEXT_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) fn run_request_is_analysis_only_continuation(
    run_request: Option<&AgentRequest>,
) -> bool {
    run_request.is_some_and(|request| {
        request.mode == AgentMode::RecommendOnly
            && request
                .context_hints
                .iter()
                .any(|hint| hint.contains(SHELL_HANDOFF_CONTINUATION_HINT))
    })
}

pub(crate) fn provider_mode_for_agent_run(
    request: &AgentRequest,
    shell_mode: CoshApprovalMode,
) -> CoshApprovalMode {
    if run_request_is_analysis_only_continuation(Some(request)) {
        CoshApprovalMode::Recommend
    } else {
        shell_mode
    }
}

fn run_request_is_shell_handoff_recovery_continuation(request: &AgentRequest) -> bool {
    run_request_is_analysis_only_continuation(Some(request))
        && request
            .context_hints
            .iter()
            .any(|hint| hint.contains(SHELL_HANDOFF_RECOVERY_OWNER_HINT))
}

pub(crate) fn render_fresh_turn_recovery_notice<W: Write>(
    state: &InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    RatatuiInlineRenderer::for_terminal()
        .with_language(state.language)
        .write_notice_panel(
            output,
            NoticePanelModel {
                title: state.i18n().t(MessageId::AgentRecoveryTitle),
                body: vec![
                    state
                        .i18n()
                        .t(MessageId::AgentRecoveryFreshTurnBody)
                        .to_string(),
                    state
                        .i18n()
                        .t(MessageId::AgentRecoveryContinuityBody)
                        .to_string(),
                ],
                footer: None,
            },
        )
}

pub(crate) fn shell_handoff_resume_fallback_request(
    active_run: &ActiveAgentRun,
) -> Option<AgentRequest> {
    let failed = active_run.governed_events.iter().any(|event| {
        matches!(
            &event.event,
            AgentEvent::AgentFailed { .. } | AgentEvent::AgentCancelled { .. }
        )
    });
    if !failed {
        return None;
    }

    shell_handoff_resume_fallback_request_without_failure(active_run)
}

fn shell_handoff_resume_fallback_request_without_failure(
    active_run: &ActiveAgentRun,
) -> Option<AgentRequest> {
    if !run_request_is_shell_handoff_recovery_continuation(&active_run.request) {
        return None;
    }
    if active_run
        .request
        .context_hints
        .iter()
        .any(|hint| hint.contains(DISABLE_PROVIDER_RESUME_HINT))
    {
        return None;
    }

    let mut request = active_run.request.clone();
    request.id = format!("{}-fresh", request.id);
    request.command_block.id = format!("{}-fresh", request.command_block.id);
    request
        .context_hints
        .push(DISABLE_PROVIDER_RESUME_HINT.to_string());
    request
        .context_hints
        .push("fresh-turn fallback after shell handoff continuation resume failure".to_string());
    Some(request)
}

pub(crate) fn shell_handoff_first_text_fallback_request(
    active_run: &ActiveAgentRun,
) -> Option<AgentRequest> {
    if active_run.has_visible_text_delta {
        return None;
    }
    if active_run.started_at.elapsed() < SHELL_HANDOFF_FIRST_TEXT_TIMEOUT {
        return None;
    }
    shell_handoff_resume_fallback_request_without_failure(active_run)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn analysis_only_continuation_guard_requires_mode_and_hint() {
        let mut request = test_request();
        request.mode = AgentMode::RecommendOnly;
        request.context_hints = vec![SHELL_HANDOFF_CONTINUATION_HINT.to_string()];
        assert!(run_request_is_analysis_only_continuation(Some(&request)));

        request.context_hints.clear();
        assert!(!run_request_is_analysis_only_continuation(Some(&request)));

        request.mode = AgentMode::AnalysisOnly;
        request.context_hints = vec![SHELL_HANDOFF_CONTINUATION_HINT.to_string()];
        assert!(!run_request_is_analysis_only_continuation(Some(&request)));
        assert!(!run_request_is_analysis_only_continuation(None));
    }

    #[test]
    fn shell_handoff_resume_fallback_retries_once_without_resume() {
        let mut request = test_request();
        request.context_hints = vec![
            SHELL_HANDOFF_CONTINUATION_HINT.to_string(),
            format!("{SHELL_HANDOFF_RECOVERY_OWNER_HINT} req-1/toolu-1"),
        ];
        let mut active_run = test_active_run(request);
        active_run.governed_events.push(GovernedEvent {
            decision: GovernanceDecision::Display,
            policy_decision: GovernancePolicyDecision::AuditOnly,
            event: AgentEvent::AgentFailed {
                run_id: "run-1".to_string(),
                error: "Agent timed out: resume failed".to_string(),
            },
            reason: "failed".to_string(),
            display_text: "failed".to_string(),
            auto_execute: false,
        });

        let fallback =
            shell_handoff_resume_fallback_request(&active_run).expect("fallback request");
        assert_eq!(fallback.id, "request-1-fresh");
        assert_eq!(fallback.command_block.id, "block-1-fresh");
        assert!(fallback
            .context_hints
            .iter()
            .any(|hint| hint.contains(DISABLE_PROVIDER_RESUME_HINT)));

        let mut retry = test_active_run(fallback);
        retry.governed_events.push(GovernedEvent {
            decision: GovernanceDecision::Display,
            policy_decision: GovernancePolicyDecision::AuditOnly,
            event: AgentEvent::AgentFailed {
                run_id: "run-2".to_string(),
                error: "Agent timed out again".to_string(),
            },
            reason: "failed".to_string(),
            display_text: "failed".to_string(),
            auto_execute: false,
        });
        assert!(shell_handoff_resume_fallback_request(&retry).is_none());
    }

    #[test]
    fn shell_handoff_resume_fallback_ignores_successful_continuation() {
        let mut request = test_request();
        request.context_hints = vec![
            SHELL_HANDOFF_CONTINUATION_HINT.to_string(),
            format!("{SHELL_HANDOFF_RECOVERY_OWNER_HINT} req-1/toolu-1"),
        ];
        let active_run = test_active_run(request);
        assert!(shell_handoff_resume_fallback_request(&active_run).is_none());
    }

    #[test]
    fn shell_handoff_first_text_timeout_retries_without_resume() {
        let mut request = test_request();
        request.context_hints = vec![
            SHELL_HANDOFF_CONTINUATION_HINT.to_string(),
            format!("{SHELL_HANDOFF_RECOVERY_OWNER_HINT} req-1/toolu-1"),
        ];
        let mut active_run = test_active_run(request);
        active_run.started_at = Instant::now() - SHELL_HANDOFF_FIRST_TEXT_TIMEOUT;

        let fallback =
            shell_handoff_first_text_fallback_request(&active_run).expect("fallback request");
        assert_eq!(fallback.id, "request-1-fresh");
        assert!(fallback
            .context_hints
            .iter()
            .any(|hint| hint.contains(DISABLE_PROVIDER_RESUME_HINT)));
    }

    #[test]
    fn shell_handoff_first_text_timeout_ignores_visible_text() {
        let mut request = test_request();
        request.context_hints = vec![
            SHELL_HANDOFF_CONTINUATION_HINT.to_string(),
            format!("{SHELL_HANDOFF_RECOVERY_OWNER_HINT} req-1/toolu-1"),
        ];
        let mut active_run = test_active_run(request);
        active_run.started_at = Instant::now() - SHELL_HANDOFF_FIRST_TEXT_TIMEOUT;
        active_run.has_visible_text_delta = true;

        assert!(shell_handoff_first_text_fallback_request(&active_run).is_none());
    }

    #[test]
    fn shell_handoff_first_text_timeout_requires_recovery_owner() {
        let mut request = test_request();
        request.context_hints = vec![SHELL_HANDOFF_CONTINUATION_HINT.to_string()];
        let mut active_run = test_active_run(request);
        active_run.started_at = Instant::now() - SHELL_HANDOFF_FIRST_TEXT_TIMEOUT;

        assert!(shell_handoff_first_text_fallback_request(&active_run).is_none());
    }

    #[test]
    fn shell_handoff_continuation_uses_recommend_provider_mode() {
        let mut request = test_request();
        request.context_hints = vec![SHELL_HANDOFF_CONTINUATION_HINT.to_string()];

        assert_eq!(
            provider_mode_for_agent_run(&request, CoshApprovalMode::Auto),
            CoshApprovalMode::Recommend
        );

        request.context_hints.clear();
        assert_eq!(
            provider_mode_for_agent_run(&request, CoshApprovalMode::Auto),
            CoshApprovalMode::Auto
        );
    }

    fn test_active_run(request: AgentRequest) -> ActiveAgentRun {
        let adapter = AdapterInstance::Fake(FakeAgentAdapter);
        let handle = adapter.start_cancellable(request.clone(), CoshApprovalMode::Recommend);
        let renderer = RatatuiInlineRenderer::for_terminal();
        ActiveAgentRun {
            request,
            handle,
            provider_name: "fake",
            language: Language::EnUs,
            renderer: renderer.clone(),
            status_animation: renderer.status_animation(),
            markdown_stream: renderer.stream_markdown_agent(),
            governed_events: Vec::new(),
            deferred_events: Vec::new(),
            held_events: Vec::new(),
            cosh_request_filter: crate::evidence::stream::CoshRequestStreamFilter::default(),
            pending_cosh_requests: Vec::new(),
            pending_cosh_request_audits: Vec::new(),
            rendered_governed_event_count: 0,
            selectable_after_event_index: None,
            started_at: Instant::now(),
            last_activity_at: Instant::now(),
            last_heartbeat_at: Instant::now(),
            current_phase: String::new(),
            current_message: String::new(),
            has_visible_text_delta: false,
            completed: false,
        }
    }

    fn test_request() -> AgentRequest {
        AgentRequest {
            id: "request-1".to_string(),
            session_id: "session-1".to_string(),
            command_block: CommandBlock {
                id: "block-1".to_string(),
                session_id: "session-1".to_string(),
                command: "continuation".to_string(),
                origin: Default::default(),
                cwd: "/tmp".to_string(),
                end_cwd: "/tmp".to_string(),
                started_at_ms: 0,
                ended_at_ms: 0,
                duration_ms: 0,
                exit_code: 0,
                status: CommandStatus::Completed,
                output: OutputRefs {
                    terminal_output_ref: None,
                    terminal_output_bytes: 0,
                },
            },
            context_blocks: Vec::new(),
            context_hints: Vec::new(),
            user_input: Some("continuation".to_string()),
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
            hook_finding: None,
            recommended_skill: None,
        }
    }
}
