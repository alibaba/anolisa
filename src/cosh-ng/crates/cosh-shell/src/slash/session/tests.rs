use super::*;
use crate::adapter::{SessionHealth, SessionSummary};
use crate::agent::run::{ActiveAgentRun, AgentRunOrigin};
use crate::evidence::stream::CoshRequestStreamFilter;

const SESSION_ID: &str = "00000000-0000-4000-8000-000000000000";
const SESSION_USAGE: &str =
    "Usage: /session [status|list|resume <id>|clear <id>...|clear --all|compact [status|cancel]]";
const SESSION_UNAVAILABLE: &str = "Session recovery requires the cosh-core backend.";

#[test]
fn malformed_session_commands_render_usage_instead_of_selecting() {
    for arguments in [
        "status extra",
        "list extra",
        "--all",
        "resume 00000000-0000-4000-8000-000000000000 extra",
        "clear",
        "compact status extra",
        "-reserved",
    ] {
        let rendered = render_session_arguments(arguments);
        assert!(
            rendered.contains(SESSION_USAGE),
            "{arguments:?} did not render usage: {rendered}"
        );
        assert!(
            !rendered.contains(SESSION_UNAVAILABLE),
            "{arguments:?} entered session recovery: {rendered}"
        );
    }
}

#[test]
fn valid_resume_and_clear_all_keep_session_recovery_routes() {
    for arguments in [SESSION_ID, "resume 00000000-0000-4000-8000-000000000000"] {
        let rendered = render_session_arguments(arguments);
        assert!(
            rendered.contains(SESSION_UNAVAILABLE),
            "{arguments:?} did not enter session recovery: {rendered}"
        );
        assert!(
            !rendered.contains(SESSION_USAGE),
            "{arguments:?} unexpectedly rendered usage: {rendered}"
        );
    }

    let rendered = render_session_arguments("clear --all");
    assert!(rendered.contains(SESSION_UNAVAILABLE), "{rendered}");
    assert!(!rendered.contains(SESSION_USAGE), "{rendered}");
}

#[test]
fn resume_without_id_keeps_picker_contract() {
    let rendered = render_session_arguments("resume");
    assert!(rendered.contains(SESSION_UNAVAILABLE), "{rendered}");
    assert!(!rendered.contains(SESSION_USAGE), "{rendered}");
}

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

#[test]
fn picker_panel_shows_short_ids_marked_count_and_key_semantics() {
    let adapter = AdapterInstance::Fake(FakeAgentAdapter);
    let mut state = InlineState {
        language: Language::EnUs,
        ..InlineState::default()
    };
    let panel_id = state.control.session_mut().new_panel_id();
    let mut selected_for_clear = HashSet::new();
    selected_for_clear.insert(SESSION_ID.to_string());
    state
        .control
        .session_mut()
        .set_pending_panel(RuntimeSessionPanel {
            id: panel_id,
            workspace_scope: "/tmp".to_string(),
            sessions: vec![SessionSummary {
                session_id: SESSION_ID.to_string(),
                workspace_scope: "/tmp".to_string(),
                created_at_ms: 1,
                updated_at_ms: 1,
                model: Some("mock".to_string()),
                message_count: 2,
                first_prompt: Some("first prompt".to_string()),
                schema_version: Some(1),
                health: SessionHealth::Ready,
            }],
            next_cursor: None,
            selected_option: 0,
            selected_for_clear,
            clear_confirmation_ids: Vec::new(),
            protected_clear_ids: Vec::new(),
            phase: RuntimeSessionPanelPhase::Browse,
        });
    let mut output = Vec::new();

    render_current_session_panel(&adapter, &mut state, &mut output).expect("render picker panel");

    // Collapse renderer wrapping so contract assertions stay width-agnostic.
    let rendered = String::from_utf8(output).expect("UTF-8 picker panel");
    let flat = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(flat.contains("[x] 00000000… · first prompt"), "{rendered}");
    assert!(flat.contains("1/1 · 1 marked"), "{rendered}");
    assert!(flat.contains("Enter resume"), "{rendered}");
    assert!(flat.contains("Space toggle clear mark"), "{rendered}");
    assert!(flat.contains("d review clear"), "{rendered}");
    assert!(!flat.contains("Space mark for clear"), "{rendered}");
}

fn render_session_arguments(arguments: &str) -> String {
    let adapter = AdapterInstance::Fake(FakeAgentAdapter);
    let mut state = InlineState {
        language: Language::EnUs,
        ..InlineState::default()
    };
    let mut output = Vec::new();

    render_session_command(arguments, &[], &adapter, &mut state, &mut output)
        .expect("render session command");
    String::from_utf8(output).expect("UTF-8 session panel")
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
