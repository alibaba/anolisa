use super::*;
use crate::agent::run::{ActiveAgentRun, AgentRunOrigin};
use crate::evidence::stream::CoshRequestStreamFilter;

#[test]
fn direct_resume_refuses_to_select_while_agent_run_is_active() {
    let adapter = AdapterInstance::CoshCore(CoshCoreAdapter {
        program: "/must-not-be-started".to_string(),
        ..CoshCoreAdapter::default()
    });
    let mut state = InlineState::default();
    state.agent_run.active = Some(test_active_run());
    let mut output = Vec::new();

    select_session_in_scope(
        "/tmp",
        "00000000-0000-4000-8000-000000000000",
        &adapter,
        &mut state,
        &mut output,
    )
    .expect("render busy notice");

    let rendered = String::from_utf8(output).expect("UTF-8 notice");
    assert!(
        rendered.contains("Finish the active Agent run"),
        "{rendered}"
    );
    assert_eq!(
        match adapter {
            AdapterInstance::CoshCore(ref core) => core.recovery_snapshot().state,
            _ => unreachable!("test adapter is cosh-core"),
        },
        SessionRecoveryState::None
    );
}

fn test_active_run() -> ActiveAgentRun {
    let request = AgentRequest {
        id: "active-request".to_string(),
        session_id: "shell-session".to_string(),
        command_block: CommandBlock {
            id: "command".to_string(),
            session_id: "shell-session".to_string(),
            command: "echo active".to_string(),
            origin: Default::default(),
            cwd: "/tmp".to_string(),
            end_cwd: "/tmp".to_string(),
            started_at_ms: 1,
            ended_at_ms: 2,
            duration_ms: 1,
            exit_code: 1,
            status: CommandStatus::Failed,
            output: OutputRefs {
                terminal_output_ref: None,
                terminal_output_bytes: 0,
            },
            shell_environment_generation: None,
        },
        context_blocks: Vec::new(),
        context_hints: Vec::new(),
        user_input: Some("active".to_string()),
        findings: Vec::new(),
        mode: AgentMode::RecommendOnly,
        user_confirmed: true,
        hook_finding: None,
        recommended_skill: None,
    };
    let handle = AdapterInstance::Fake(FakeAgentAdapter)
        .start_cancellable(request.clone(), CoshApprovalMode::Recommend);
    let renderer = RatatuiInlineRenderer::for_terminal();
    ActiveAgentRun {
        request,
        origin: AgentRunOrigin::Standard,
        handle,
        provider_name: "fake",
        language: Language::EnUs,
        renderer: renderer.clone(),
        status_animation: renderer.status_animation(),
        markdown_stream: renderer.stream_markdown_agent(),
        governed_events: Vec::new(),
        deferred_events: Vec::new(),
        held_events: Vec::new(),
        cosh_request_filter: CoshRequestStreamFilter::default(),
        pending_cosh_requests: Vec::new(),
        pending_cosh_request_audits: Vec::new(),
        rendered_governed_event_count: 0,
        selectable_after_event_index: None,
        started_at: std::time::Instant::now(),
        last_activity_at: std::time::Instant::now(),
        last_heartbeat_at: std::time::Instant::now(),
        current_phase: String::new(),
        current_message: String::new(),
        has_visible_text_delta: false,
        completed: false,
        host_completed_tool_ids: Vec::new(),
        pending_hook_notifications: Vec::new(),
    }
}
