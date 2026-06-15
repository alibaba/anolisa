use super::*;
use crate::agent::run::ActiveAgentRun;
use crate::approval::handoff::{
    approval_shell_handoff_validation_message, command_matches_trust_key,
    fallback_bash_execution_path, trust_key_from_command, ApprovedBashExecutionPath,
};
use crate::approval::requests::{approval_request_from_governed_event, record_approval_requests};
use crate::approval::resolution::{
    apply_approval_decision, approval_outcome_for_request, approval_resolution_agent_request,
};
use cosh_shell::adapter::ApprovalDecision;
use cosh_shell::types::{GovernanceDecision, GovernancePolicyDecision};
use std::time::{Duration, Instant};

#[test]
fn trust_key_from_command_normalizes_full_command() {
    assert_eq!(
        trust_key_from_command("git status").as_deref(),
        Some("git status")
    );
    assert_eq!(
        trust_key_from_command("npm   test").as_deref(),
        Some("npm test")
    );
    assert_eq!(trust_key_from_command("ls").as_deref(), Some("ls"));
    assert_eq!(trust_key_from_command("git -v").as_deref(), Some("git -v"));
}

#[test]
fn trust_key_from_command_strips_dollar_prefix() {
    assert_eq!(
        trust_key_from_command("$ git status").as_deref(),
        Some("git status")
    );
    assert_eq!(
        trust_key_from_command("$ npm test").as_deref(),
        Some("npm test")
    );
    assert_eq!(
        trust_key_from_command("$ ls -la").as_deref(),
        Some("ls -la")
    );
}

#[test]
fn approved_bash_foreground_handoff_matrix() {
    for command in [
        "pwd",
        "git status --short",
        "sudo id",
        "/usr/bin/sudo id",
        "LANG=C sudo id",
        "sudo -n true",
        "sudo -S true",
        "ssh host",
        "ssh -t host 'top'",
        "ssh -T git@github.com",
        "vim Cargo.toml",
        "less Cargo.toml",
        "less --help",
        "top -b -n1",
        "top",
        "python -c 'print(1)'",
        "python",
        "docker exec -it c sh",
        "kubectl exec -it pod -- sh",
        "local-unknown-tool --maybe",
    ] {
        assert_eq!(
            fallback_bash_execution_path(command),
            ApprovedBashExecutionPath::ForegroundShellPty,
            "{command}"
        );
    }
}

#[test]
fn approved_bash_blocks_empty_nul_newline_and_nonprinting_controls() {
    for command in [
        "",
        "printf '\\0'\0",
        "printf one\nprintf two",
        "printf '\u{1b}[31mred'",
    ] {
        assert_eq!(
            fallback_bash_execution_path(command),
            ApprovedBashExecutionPath::Blocked,
            "{command:?}"
        );
    }
}

#[test]
fn approved_bash_allows_visible_tab_separator() {
    assert_eq!(
        fallback_bash_execution_path("printf\tok"),
        ApprovedBashExecutionPath::ForegroundShellPty
    );
}

#[test]
fn rejected_tool_call_is_not_reinterpreted_as_approvable() {
    let state = InlineState::default();
    let blocked = GovernedEvent {
        decision: GovernanceDecision::Rejected,
        policy_decision: GovernancePolicyDecision::HostBlocked,
        event: AgentEvent::ToolCall {
            run_id: "run-1".to_string(),
            tool_id: None,
            name: "Bash".to_string(),
            input: "touch /tmp/should-not-run".to_string(),
        },
        reason: "blocked by governance".to_string(),
        display_text: "blocked".to_string(),
        auto_execute: false,
    };
    let needs_approval = GovernedEvent {
        policy_decision: GovernancePolicyDecision::NeedsUserApproval,
        decision: GovernanceDecision::Display,
        display_text: "approval required".to_string(),
        reason: "needs user approval".to_string(),
        auto_execute: false,
        event: AgentEvent::ToolCall {
            run_id: "run-1".to_string(),
            tool_id: None,
            name: "Bash".to_string(),
            input: "git status".to_string(),
        },
    };

    assert!(approval_request_from_governed_event(&state, &blocked, None, false).is_none());
    assert!(approval_request_from_governed_event(&state, &needs_approval, None, false).is_some());
}

#[test]
fn provider_shell_permission_approval_records_foreground_metadata() {
    let mut state = InlineState::default();
    state.approvals.requests.push(provider_tool_request(
        "run_shell_command",
        Some(serde_json::json!({ "command": "echo provider-shell" })),
    ));

    let decision = apply_approval_decision(&mut state, 0, ApprovalCommandKind::Approve)
        .expect("approval decision");
    assert_eq!(decision.request.status, ApprovalRequestStatus::Approved);
    assert_eq!(
        decision.request.execution_path,
        Some("foreground_shell_pty")
    );
    assert_eq!(decision.request.redaction_status, Some("ref_only"));

    queue_approved_shell_handoff(&mut state, &decision.request);
    assert!(!state.control.shell_handoff().approved_is_empty());
}

#[test]
fn duplicate_provider_permission_tool_use_id_is_not_recorded_twice() {
    let mut state = InlineState::default();
    let first = governed_provider_tool_permission("ctrl-1", "toolu-1");
    let duplicate = governed_provider_tool_permission("ctrl-2", "toolu-1");

    let ids = record_approval_requests(&mut state, &[first, duplicate], None, false);

    assert_eq!(ids, vec!["req-1"]);
    assert_eq!(state.approvals.requests.len(), 1);
    assert_eq!(
        state.approvals.requests[0].request_id.as_deref(),
        Some("ctrl-1")
    );
    assert_eq!(
        state.approvals.requests[0].tool_use_id.as_deref(),
        Some("toolu-1")
    );
}

#[test]
fn streamed_tool_fallback_handoff_strips_control_request_id() {
    let mut state = InlineState::default();
    let mut request = provider_tool_request(
        "run_shell_command",
        Some(serde_json::json!({ "command": "echo fallback" })),
    );
    request.provider_shell_request_kind = ProviderShellRequestKind::StreamedToolCallFallback;
    request.status = ApprovalRequestStatus::Approved;
    request.execution_path = Some("foreground_shell_pty");

    queue_approved_shell_handoff(&mut state, &request);
    let handoff = state
        .control
        .shell_handoff_mut()
        .emit_next_approved()
        .expect("handoff");

    assert_eq!(handoff.command, "echo fallback");
    assert_eq!(handoff.tool_use_id.as_deref(), Some("toolu-1"));
    assert!(handoff.request_id.is_none());
}

#[test]
fn provider_shell_permission_missing_command_is_blocked() {
    let mut state = InlineState::default();
    state.approvals.requests.push(provider_tool_request(
        "run_shell_command",
        Some(serde_json::json!({ "not_command": "echo no" })),
    ));

    let decision = apply_approval_decision(&mut state, 0, ApprovalCommandKind::Approve)
        .expect("approval decision");
    assert_eq!(decision.request.status, ApprovalRequestStatus::Blocked);
    assert_eq!(decision.request.execution_path, Some("blocked"));
    assert!(!decision.run_approved_tool);
    assert_eq!(
        approval_outcome_for_request(&state, &decision.request),
        ApprovalOutcome::ProviderApprovalResponse
    );
    let response = provider_approval_response(&decision.request, "ctrl-1");
    assert!(matches!(
        response.decision,
        ApprovalDecision::Deny { ref message }
            if message.contains("blocked this Bash tool request")
    ));
    let agent_request = approval_resolution_agent_request(&decision.request);
    let input = agent_request.user_input.expect("approval result input");
    assert!(input.contains("Decision: blocked by cosh-shell"), "{input}");
    assert!(input.contains("Status: not_executed"), "{input}");
    assert!(input.contains("No command ran."), "{input}");
}

#[test]
fn provider_shell_permission_multiline_command_is_blocked() {
    let mut state = InlineState::default();
    state.approvals.requests.push(provider_tool_request(
        "Bash",
        Some(serde_json::json!({ "command": "printf one\nprintf two" })),
    ));

    let decision = apply_approval_decision(&mut state, 0, ApprovalCommandKind::Approve)
        .expect("approval decision");
    assert_eq!(decision.request.status, ApprovalRequestStatus::Blocked);
    assert_eq!(decision.request.execution_path, Some("blocked"));
    assert!(!decision.run_approved_tool);
    queue_approved_shell_handoff(&mut state, &decision.request);
    assert!(state.control.shell_handoff().approved_is_empty());
}

#[test]
fn provider_tool_call_visibility_only_when_control_protocol_is_active() {
    let mut state = InlineState::default();
    let governed = GovernedEvent {
        decision: GovernanceDecision::Display,
        policy_decision: GovernancePolicyDecision::NeedsUserApproval,
        event: AgentEvent::ToolCall {
            run_id: "run-1".to_string(),
            tool_id: None,
            name: "run_shell_command".to_string(),
            input: r#"{"command":"echo should-not-handoff"}"#.to_string(),
        },
        reason: "tool call visible".to_string(),
        display_text: "tool call visible".to_string(),
        auto_execute: false,
    };

    let ids = record_approval_requests(&mut state, &[governed], None, true);
    assert!(ids.is_empty());
    assert!(state.approvals.requests.is_empty());
    assert!(state.control.shell_handoff().approved_is_empty());
}

#[test]
fn readonly_provider_tool_call_never_creates_pending_approval() {
    let mut state = InlineState::default();
    let governed = GovernedEvent {
        decision: GovernanceDecision::Display,
        policy_decision: GovernancePolicyDecision::NeedsUserApproval,
        event: AgentEvent::ToolCall {
            run_id: "run-1".to_string(),
            tool_id: Some("tool-1".to_string()),
            name: "glob".to_string(),
            input: r#"{"pattern":"**/README.md"}"#.to_string(),
        },
        reason: "provider tool call visible".to_string(),
        display_text: "provider tool call visible".to_string(),
        auto_execute: false,
    };

    let ids = record_approval_requests(&mut state, &[governed], None, false);
    assert!(ids.is_empty());
    assert!(state.approvals.requests.is_empty());
}

#[test]
fn non_shell_provider_permission_approval_stays_provider_owned() {
    let mut state = InlineState::default();
    state.approvals.requests.push(provider_tool_request(
        "Read",
        Some(serde_json::json!({ "file_path": "Cargo.toml" })),
    ));

    let decision = apply_approval_decision(&mut state, 0, ApprovalCommandKind::Approve)
        .expect("approval decision");
    assert_eq!(decision.request.status, ApprovalRequestStatus::Approved);
    assert_eq!(
        approval_outcome_for_request(&state, &decision.request),
        ApprovalOutcome::ProviderApprovalResponse
    );
    let response = provider_approval_response(&decision.request, "ctrl-1");
    assert!(matches!(response.decision, ApprovalDecision::Allow));
}

#[test]
fn provider_approval_response_refreshes_active_run_idle_clock() {
    let mut active_run = active_run_for_approval_test();
    active_run.last_activity_at = Instant::now() - Duration::from_secs(60);
    let mut request = provider_tool_request(
        "Read",
        Some(serde_json::json!({ "file_path": "Cargo.toml" })),
    );
    request.status = ApprovalRequestStatus::Cancelled;
    let response = provider_approval_response(&request, "ctrl-1");

    assert!(respond_active_run_approval(&mut active_run, response));
    assert!(active_run.last_activity_at.elapsed() < Duration::from_secs(2));
}

#[test]
fn shell_handoff_validation_message_uses_active_language() {
    let zh = cosh_shell::I18n::new(cosh_shell::Language::ZhCn);
    let text = approval_shell_handoff_validation_message(
        &zh,
        "shell handoff command contains newline; multiline handoff is not enabled",
    );

    assert!(text.contains("换行"), "{text}");
    assert!(!text.contains("multiline handoff is not enabled"), "{text}");

    let unknown = approval_shell_handoff_validation_message(&zh, "custom validation");
    assert_eq!(unknown, "custom validation");
}

fn provider_tool_request(
    tool_name: &str,
    tool_input: Option<serde_json::Value>,
) -> RuntimeApprovalRequest {
    RuntimeApprovalRequest {
        id: "req-1".to_string(),
        run_id: "run-1".to_string(),
        session_id: "sess-1".to_string(),
        cwd: "/tmp".to_string(),
        source: "control-protocol",
        provider_shell_request_kind: ProviderShellRequestKind::ControlPermission,
        kind: ApprovalRequestKind::Tool,
        subject: tool_name.to_string(),
        preview: tool_input
            .as_ref()
            .and_then(|input| input.get("command"))
            .and_then(|value| value.as_str())
            .map(|command| format!("$ {command}"))
            .unwrap_or_else(|| "Cargo.toml".to_string()),
        risk: "medium",
        request_id: Some("ctrl-1".to_string()),
        tool_use_id: Some("toolu-1".to_string()),
        tool_input,
        original_user_request: None,
        status: ApprovalRequestStatus::Pending,
        execution_path: Some("provider_control_protocol"),
        command_block_id: None,
        redaction_status: None,
    }
}

fn active_run_for_approval_test() -> ActiveAgentRun {
    let request = AgentRequest {
        id: "request-1".to_string(),
        session_id: "session-1".to_string(),
        command_block: CommandBlock {
            id: "cmd-1".to_string(),
            session_id: "session-1".to_string(),
            command: "approval test".to_string(),
            cwd: "/tmp".to_string(),
            end_cwd: "/tmp".to_string(),
            started_at_ms: 1,
            ended_at_ms: 2,
            duration_ms: 1,
            exit_code: 0,
            status: CommandStatus::Completed,
            output: OutputRefs {
                terminal_output_ref: None,
                terminal_output_bytes: 0,
            },
        },
        context_blocks: Vec::new(),
        context_hints: Vec::new(),
        user_input: Some("approval test".to_string()),
        findings: Vec::new(),
        mode: AgentMode::RecommendOnly,
        user_confirmed: true,
        hook_finding: None,
        recommended_skill: None,
    };
    let adapter = AdapterInstance::Fake(FakeAgentAdapter);
    let handle = adapter.start_cancellable(request.clone(), CoshApprovalMode::Recommend);
    let renderer = RatatuiInlineRenderer::for_terminal();
    ActiveAgentRun {
        request,
        handle,
        provider_name: "fake",
        language: cosh_shell::Language::EnUs,
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

fn governed_provider_tool_permission(request_id: &str, tool_use_id: &str) -> GovernedEvent {
    GovernedEvent {
        policy_decision: GovernancePolicyDecision::NeedsUserApproval,
        decision: GovernanceDecision::Display,
        display_text: "approval required".to_string(),
        reason: "needs user approval".to_string(),
        auto_execute: false,
        event: AgentEvent::ToolPermissionRequest {
            run_id: "run-1".to_string(),
            request_id: request_id.to_string(),
            tool_name: "run_shell_command".to_string(),
            tool_input: serde_json::json!({ "command": "df -h" }),
            tool_use_id: tool_use_id.to_string(),
        },
    }
}

#[test]
fn trust_key_from_command_empty_input() {
    assert_eq!(trust_key_from_command(""), None);
}

#[test]
fn command_matches_trust_key_basic() {
    let mut trusted = HashSet::new();
    trusted.insert("npm test".to_string());
    trusted.insert("git status".to_string());

    assert!(command_matches_trust_key("npm test", &trusted));
    assert!(command_matches_trust_key("git status", &trusted));
    assert!(!command_matches_trust_key("npm test --watch", &trusted));
    assert!(!command_matches_trust_key("git status --short", &trusted));
    assert!(!command_matches_trust_key(
        "git status && touch /tmp/x",
        &trusted
    ));
    assert!(!command_matches_trust_key("cargo build", &trusted));
}

#[test]
fn command_matches_trust_key_empty_set() {
    let trusted = HashSet::new();
    assert!(!command_matches_trust_key("npm test", &trusted));
}
