use std::sync::{Arc, Mutex};

use super::cosh_core::{CoshCoreAdapter, SessionRecoveryState, SessionRuntimeState};
use super::{AdapterError, AgentAdapter, AgentRunPoll};
use crate::types::{
    AgentEvent, AgentMode, AgentRequest, CommandBlock, CommandStatus, CoshApprovalMode, OutputRefs,
};

fn test_workspace_scope() -> String {
    std::fs::canonicalize(std::env::temp_dir())
        .expect("canonical test workspace")
        .to_string_lossy()
        .into_owned()
}

fn test_request() -> AgentRequest {
    let workspace_scope = test_workspace_scope();
    AgentRequest {
        id: "test".to_string(),
        session_id: "sess".to_string(),
        command_block: CommandBlock {
            id: "blk".to_string(),
            session_id: "sess".to_string(),
            command: "echo test".to_string(),
            origin: Default::default(),
            cwd: workspace_scope.clone(),
            end_cwd: workspace_scope,
            started_at_ms: 0,
            ended_at_ms: 0,
            duration_ms: 0,
            exit_code: 1,
            status: CommandStatus::Failed,
            output: OutputRefs {
                terminal_output_ref: None,
                terminal_output_bytes: 0,
            },
            shell_environment_generation: None,
        },
        context_blocks: vec![],
        context_hints: vec![],
        user_input: Some("test".to_string()),
        findings: vec![],
        mode: AgentMode::RecommendOnly,
        user_confirmed: true,
        hook_finding: None,
        recommended_skill: None,
    }
}

fn test_adapter() -> CoshCoreAdapter {
    CoshCoreAdapter {
        program: "cosh-core".to_string(),
        allow_model_call: false,
        session: Arc::default(),
    }
}

fn write_mock_core(label: &str, script: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("cosh-core-{label}-{}.sh", std::process::id()));
    std::fs::write(&path, script).expect("write mock cosh-core");
    let mut permissions = std::fs::metadata(&path)
        .expect("mock cosh-core metadata")
        .permissions();
    use std::os::unix::fs::PermissionsExt;
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).expect("chmod mock cosh-core");
    path
}

fn adapter_with_active_session(program: &std::path::Path) -> CoshCoreAdapter {
    CoshCoreAdapter {
        program: program.to_string_lossy().into_owned(),
        allow_model_call: true,
        session: Arc::new(Mutex::new(SessionRuntimeState::with_active(
            "00000000-0000-4000-8000-000000000000",
            test_workspace_scope(),
        ))),
    }
}

fn adapter_with_selected_session(program: &std::path::Path) -> CoshCoreAdapter {
    let adapter = adapter_with_active_session(program);
    if let Ok(mut session) = adapter.session.lock() {
        session.recovery.state = SessionRecoveryState::Selected;
        session.recovery.selected_session_id =
            Some("11111111-1111-4111-8111-111111111111".to_string());
        session.recovery.selected_workspace_scope = Some(test_workspace_scope());
    }
    adapter
}

fn assert_failed_selection_was_released(adapter: &CoshCoreAdapter) {
    let recovery = adapter.recovery_snapshot();
    assert_eq!(recovery.state, SessionRecoveryState::Failed);
    assert_eq!(recovery.selected_session_id, None);
    assert_eq!(recovery.selected_workspace_scope, None);
    assert_eq!(
        adapter.protected_session_ids(),
        vec!["00000000-0000-4000-8000-000000000000"]
    );
}

#[test]
fn prepare_invocation_headless_flag() {
    let inv = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Auto);
    assert_eq!(inv.program, "cosh-core");
    assert!(inv.args.contains(&"--headless".to_string()));
    assert!(inv
        .args
        .contains(&"--enable-shell-evidence-tool".to_string()));
}

#[test]
fn agent_request_does_not_serialize_internal_context_binding() {
    let request = test_request();

    let json = serde_json::to_string(&request).expect("serialize request");

    assert!(!json.contains("context_binding"), "{json}");
}

#[test]
fn prepare_invocation_approval_modes() {
    let recommend = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Recommend);
    assert!(recommend.args.contains(&"strict".to_string()));

    let auto = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Auto);
    assert!(auto.args.contains(&"auto".to_string()));

    let trust = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Trust);
    assert!(trust.args.contains(&"trust".to_string()));
}

#[test]
fn prepare_invocation_prompt_includes_cosh_shell_contract() {
    let inv = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Auto);

    assert!(inv
        .prompt
        .contains("Handle this natural-language shell prompt request"));
    assert!(inv.prompt.contains("cosh-shell Agent contract"));
    assert!(inv
        .prompt
        .contains("Always emit a provider permission request"));
    assert!(inv.prompt.contains("State the diagnostic conclusion first"));
    assert!(inv
        .prompt
        .contains("at most one primary recommendation command"));
}

#[test]
fn prepare_invocation_prompt_uses_shell_output_tool_mode() {
    let mut request = test_request();
    let mut context = request.command_block.clone();
    context.id = "cmd-1".to_string();
    context.session_id = "session-1".to_string();
    context.exit_code = 0;
    context.status = CommandStatus::Completed;
    context.output.terminal_output_ref = Some("/tmp/cosh-output.txt".to_string());
    context.output.terminal_output_bytes = 42;
    request.context_blocks = vec![context];

    let inv = test_adapter().prepare_invocation(&request, CoshApprovalMode::Auto);

    assert!(inv.prompt.contains("cosh_shell_evidence"), "{}", inv.prompt);
    assert!(
        inv.prompt.contains("action=list_commands"),
        "{}",
        inv.prompt
    );
    assert!(inv.prompt.contains("action=read_output"), "{}", inv.prompt);
    assert!(
        inv.prompt.contains("Use current tool results first"),
        "{}",
        inv.prompt
    );
    assert!(
        inv.prompt
            .contains("Use read_output only for older shell ledger output"),
        "{}",
        inv.prompt
    );
    assert!(
        inv.prompt.contains("activity recaps or command lists"),
        "{}",
        inv.prompt
    );
    assert!(
        inv.prompt.contains("output_available=false"),
        "{}",
        inv.prompt
    );
    assert!(inv.prompt.contains("output_bytes=0"), "{}", inv.prompt);
    assert!(
        inv.prompt
            .contains("call cosh_shell_evidence with action=list_commands"),
        "{}",
        inv.prompt
    );
    assert!(!inv.prompt.contains("```cosh-request"), "{}", inv.prompt);
    assert!(
        !inv.prompt.contains("```cosh-request\noutput"),
        "{}",
        inv.prompt
    );
}

#[test]
fn prepare_invocation_prompt_suppresses_shell_output_requests_in_recommend_mode() {
    let mut request = test_request();
    let mut context = request.command_block.clone();
    context.id = "cmd-1".to_string();
    context.session_id = "session-1".to_string();
    context.output.terminal_output_ref = Some("/tmp/cosh-output.txt".to_string());
    context.output.terminal_output_bytes = 42;
    request.context_blocks = vec![context];

    let inv = test_adapter().prepare_invocation(&request, CoshApprovalMode::Recommend);

    assert!(
        inv.prompt
            .contains("do not request shell output automatically"),
        "{}",
        inv.prompt
    );
    assert!(
        !inv.prompt.contains("cosh_shell_evidence"),
        "{}",
        inv.prompt
    );
    assert!(!inv.prompt.contains("```cosh-request"), "{}", inv.prompt);
}

#[test]
fn prepare_invocation_session_resume() {
    let adapter = CoshCoreAdapter {
        program: "cosh-core".to_string(),
        allow_model_call: false,
        session: Arc::new(Mutex::new(SessionRuntimeState::with_active(
            "prev-sess",
            test_workspace_scope(),
        ))),
    };
    let inv = adapter.prepare_invocation(&test_request(), CoshApprovalMode::Auto);
    assert!(inv.args.contains(&"--resume".to_string()));
    assert!(inv.args.contains(&"prev-sess".to_string()));
}

#[test]
fn prepare_invocation_does_not_resume_across_cwd_scope() {
    let adapter = CoshCoreAdapter {
        program: "cosh-core".to_string(),
        allow_model_call: false,
        session: Arc::new(Mutex::new(SessionRuntimeState::with_active(
            "prev-sess",
            "/other",
        ))),
    };
    let inv = adapter.prepare_invocation(&test_request(), CoshApprovalMode::Auto);
    assert!(!inv.args.contains(&"--resume".to_string()));
    assert!(!inv.args.contains(&"prev-sess".to_string()));
}

#[test]
fn prepare_invocation_ignores_failed_selected_session() {
    let adapter = test_adapter();
    if let Ok(mut session) = adapter.session.lock() {
        session.recovery.state = SessionRecoveryState::Failed;
        session.recovery.selected_session_id =
            Some("11111111-1111-4111-8111-111111111111".to_string());
        session.recovery.selected_workspace_scope = Some(test_workspace_scope());
    }

    let invocation = adapter.prepare_invocation(&test_request(), CoshApprovalMode::Auto);

    assert!(!invocation.args.contains(&"--resume".to_string()));
}

#[test]
fn prepare_invocation_uses_process_cwd_for_unknown_intercept_scope() {
    let mut request = test_request();
    request.command_block.cwd = "<unknown>".to_string();
    request.command_block.end_cwd = "<unknown>".to_string();

    let invocation = test_adapter().prepare_invocation(&request, CoshApprovalMode::Recommend);
    let workspace_index = invocation
        .args
        .iter()
        .position(|argument| argument == "--workspace")
        .expect("workspace argument");
    let expected = std::fs::canonicalize(std::env::current_dir().expect("current dir"))
        .expect("canonical current dir")
        .to_string_lossy()
        .into_owned();

    assert_eq!(invocation.args[workspace_index + 1], expected);
}

#[test]
fn capabilities_match_expected() {
    let adapter = test_adapter();
    let caps = adapter.capabilities();
    assert!(caps.text_stream);
    assert!(caps.session_resume);
    assert!(caps.tool_intent);
    assert!(caps.user_question);
    assert!(caps.cancellable);
    assert!(caps.control_protocol);
}

#[test]
fn list_sessions_returns_one_page_and_preserves_opaque_cursor() {
    let script =
        std::env::temp_dir().join(format!("cosh-core-session-pages-{}.sh", std::process::id()));
    std::fs::write(
        &script,
        r#"#!/bin/sh
request=$(cat)
case "$request" in
  *'"cursor":null'*)
    printf '%s\n' '{"ok":true,"data":{"action":"list","sessions":[{"session_id":"00000000-0000-4000-8000-000000000000","workspace_scope":"/tmp","created_at_ms":1,"updated_at_ms":3,"model":"mock","message_count":1,"first_prompt":"first","schema_version":1,"health":"ready"}],"next_cursor":"cursor-1"}}'
    ;;
  *'"cursor":"cursor-1"'*)
    printf '%s\n' '{"ok":true,"data":{"action":"list","sessions":[{"session_id":"11111111-1111-4111-8111-111111111111","workspace_scope":"/tmp","created_at_ms":1,"updated_at_ms":2,"model":"mock","message_count":1,"first_prompt":"second","schema_version":1,"health":"ready"}],"next_cursor":"cursor-2"}}'
    ;;
  *'"cursor":"cursor-2"'*)
    printf '%s\n' '{"ok":true,"data":{"action":"list","sessions":[{"session_id":"22222222-2222-4222-8222-222222222222","workspace_scope":"/tmp","created_at_ms":1,"updated_at_ms":1,"model":"mock","message_count":1,"first_prompt":"third","schema_version":1,"health":"ready"}],"next_cursor":null}}'
    ;;
esac
"#,
    )
    .expect("write paginated session mock");
    let mut permissions = std::fs::metadata(&script)
        .expect("paginated session mock metadata")
        .permissions();
    use std::os::unix::fs::PermissionsExt;
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).expect("chmod paginated session mock");
    let adapter = CoshCoreAdapter {
        program: script.to_string_lossy().into_owned(),
        ..test_adapter()
    };

    let first = adapter.list_sessions("/tmp").expect("first session page");
    let second = adapter
        .list_sessions_page("/tmp", 100, first.next_cursor.as_deref())
        .expect("second session page");
    let third = adapter
        .list_sessions_page("/tmp", 100, second.next_cursor.as_deref())
        .expect("third session page");
    let _ = std::fs::remove_file(&script);

    assert_eq!(
        first
            .sessions
            .iter()
            .chain(&second.sessions)
            .chain(&third.sessions)
            .map(|summary| summary.first_prompt.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("first"), Some("second"), Some("third")]
    );
    assert_eq!(first.next_cursor.as_deref(), Some("cursor-1"));
    assert_eq!(second.next_cursor.as_deref(), Some("cursor-2"));
    assert!(third.next_cursor.is_none());
}

#[test]
fn stream_parser_uses_neutral_status_messages() {
    let script =
        std::env::temp_dir().join(format!("cosh-tui-neutral-status-{}.sh", std::process::id()));
    std::fs::write(
        &script,
        r#"#!/bin/sh
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hidden reasoning"}}}'
printf '%s\n' '{"type":"result","subtype":"success","session_id":"s","is_error":false,"result":"done"}'
"#,
    )
    .expect("write mock cosh-tui");
    let mut permissions = std::fs::metadata(&script)
        .expect("mock cosh-tui metadata")
        .permissions();
    use std::os::unix::fs::PermissionsExt;
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).expect("chmod mock cosh-tui");

    let adapter = CoshCoreAdapter {
        program: script.to_string_lossy().to_string(),
        allow_model_call: true,
        session: Arc::default(),
    };
    let mut events = Vec::new();
    let result = adapter.run_stream(&test_request(), &mut |event| {
        events.push(event);
        Ok(())
    });
    let _ = std::fs::remove_file(&script);
    result.expect("run mock cosh-tui");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::StatusChanged { phase, message, .. }
            if phase == "thinking" && message == "thinking"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::AgentCompleted { summary, .. } if summary == "analysis completed"
    )));
    let debug = format!("{events:?}");
    assert!(!debug.contains("claude"), "{debug}");
    assert!(!debug.contains("co thinking"), "{debug}");
}

#[test]
fn selected_session_transitions_through_restoring_to_active() {
    let workspace_scope = test_workspace_scope();
    let script = std::env::temp_dir().join(format!(
        "cosh-core-session-recovery-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
if [ "$1" = "--session-control" ]; then
  cat >/dev/null
  printf '%s\n' '{{"ok":true,"data":{{"action":"validate","session":{{"session_id":"00000000-0000-4000-8000-000000000000","workspace_scope":"{workspace_scope}","created_at_ms":1,"updated_at_ms":2,"model":"mock","message_count":2,"first_prompt":"remember","schema_version":1,"health":"ready"}}}}}}'
  exit 0
fi
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-000000000000","model":"mock","tools":[]}}'
printf '%s\n' '{{"type":"result","subtype":"success","session_id":"00000000-0000-4000-8000-000000000000","is_error":false,"duration_ms":1,"result":"done"}}'
"#
        ),
    )
    .expect("write session recovery mock");
    let mut permissions = std::fs::metadata(&script)
        .expect("session recovery mock metadata")
        .permissions();
    use std::os::unix::fs::PermissionsExt;
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).expect("chmod session recovery mock");

    let adapter = CoshCoreAdapter {
        program: script.to_string_lossy().into_owned(),
        allow_model_call: true,
        session: Arc::default(),
    };
    let selected = adapter
        .select_session(&workspace_scope, "00000000-0000-4000-8000-000000000000")
        .expect("select persisted session");
    assert_eq!(selected.session_id, "00000000-0000-4000-8000-000000000000");
    assert_eq!(
        adapter.recovery_snapshot().state,
        SessionRecoveryState::Selected
    );

    let handle = adapter.start_cancellable(test_request(), CoshApprovalMode::Recommend);
    assert_eq!(
        adapter.recovery_snapshot().state,
        SessionRecoveryState::Restoring
    );
    while let AgentRunPoll::Event(_) | AgentRunPoll::Timeout = handle
        .poll_event_timeout(std::time::Duration::from_secs(2))
        .expect("poll session recovery")
    {}
    let _ = std::fs::remove_file(&script);

    assert_eq!(
        adapter.recovery_snapshot().state,
        SessionRecoveryState::Active
    );
    assert_eq!(
        adapter.committed_session_id().as_deref(),
        Some("00000000-0000-4000-8000-000000000000")
    );
}

#[test]
fn active_and_selected_sessions_are_protected_only_while_selection_is_live() {
    let adapter = CoshCoreAdapter {
        session: Arc::new(Mutex::new(SessionRuntimeState::with_active(
            "00000000-0000-4000-8000-000000000000",
            "/tmp",
        ))),
        ..test_adapter()
    };
    if let Ok(mut session) = adapter.session.lock() {
        session.recovery.state = SessionRecoveryState::Selected;
        session.recovery.selected_session_id =
            Some("11111111-1111-4111-8111-111111111111".to_string());
        session.recovery.selected_workspace_scope = Some("/tmp".to_string());
    }

    assert_eq!(
        adapter.protected_session_ids(),
        vec![
            "00000000-0000-4000-8000-000000000000",
            "11111111-1111-4111-8111-111111111111"
        ]
    );

    if let Ok(mut session) = adapter.session.lock() {
        session.recovery.state = SessionRecoveryState::Failed;
    }
    assert_eq!(
        adapter.protected_session_ids(),
        vec!["00000000-0000-4000-8000-000000000000"]
    );
}

#[test]
fn failed_session_selection_clears_previous_selection() {
    let script = write_mock_core(
        "selection-failure",
        r#"#!/bin/sh
cat >/dev/null
printf '%s\n' '{"ok":false,"error":{"code":"not_found","message":"session disappeared","recoverable":true,"hint":"Refresh and retry."}}'
"#,
    );
    let adapter = CoshCoreAdapter {
        program: script.to_string_lossy().into_owned(),
        ..test_adapter()
    };
    if let Ok(mut session) = adapter.session.lock() {
        session.recovery.state = SessionRecoveryState::Selected;
        session.recovery.selected_session_id =
            Some("00000000-0000-4000-8000-000000000000".to_string());
        session.recovery.selected_workspace_scope = Some("/tmp".to_string());
    }

    let result = adapter.select_session("/tmp", "11111111-1111-4111-8111-111111111111");
    let _ = std::fs::remove_file(&script);

    assert!(result.is_err());
    let recovery = adapter.recovery_snapshot();
    assert_eq!(recovery.state, SessionRecoveryState::Failed);
    assert_eq!(recovery.selected_session_id, None);
    assert_eq!(recovery.selected_workspace_scope, None);
}

#[test]
fn synchronous_status_sink_error_releases_restoring_selection() {
    let program = std::path::Path::new("/unused/cosh-core");
    let adapter = adapter_with_selected_session(program);

    let result = adapter.run_stream(&test_request(), &mut |_| {
        Err(AdapterError {
            message: "status sink failed".to_string(),
        })
    });

    assert_eq!(
        result.expect_err("status sink failure").message,
        "status sink failed"
    );
    assert_failed_selection_was_released(&adapter);
}

#[test]
fn synchronous_spawn_error_releases_restoring_selection() {
    let missing = std::env::temp_dir().join(format!(
        "missing-cosh-core-{}-{}",
        std::process::id(),
        "spawn"
    ));
    let adapter = adapter_with_selected_session(&missing);

    let result = adapter.run_stream(&test_request(), &mut |_| Ok(()));

    assert!(result
        .expect_err("spawn failure")
        .message
        .contains("failed to run cosh-core"));
    assert_failed_selection_was_released(&adapter);
}

#[test]
fn synchronous_stream_read_error_releases_restoring_selection() {
    let script = write_mock_core(
        "invalid-utf8-stream",
        r#"#!/bin/sh
printf '%s\n' '{"type":"system","subtype":"init","session_id":"11111111-1111-4111-8111-111111111111","model":"mock","tools":[]}'
printf '\377\n'
"#,
    );
    let adapter = adapter_with_selected_session(&script);

    let result = adapter.run_stream(&test_request(), &mut |_| Ok(()));

    let error = result.expect_err("stream read failure");
    assert!(
        error.message.contains("failed to read cosh-core stream"),
        "{}",
        error.message
    );
    assert_failed_selection_was_released(&adapter);
    let _ = std::fs::remove_file(&script);
}

#[test]
fn non_resumable_error_result_discards_active_session() {
    let script = write_mock_core(
        "non-resumable-error",
        r#"#!/bin/sh
printf '%s\n' '{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-000000000000","session_resumable":false,"model":"mock","tools":[]}'
printf '%s\n' '{"type":"result","subtype":"error","session_id":"00000000-0000-4000-8000-000000000000","is_error":true,"result":"failed"}'
"#,
    );
    let adapter = adapter_with_active_session(&script);

    adapter
        .run_stream(&test_request(), &mut |_| Ok(()))
        .expect("run non-resumable error result");
    let _ = std::fs::remove_file(&script);

    assert_eq!(adapter.committed_session_id(), None);
    assert_eq!(
        adapter.recovery_snapshot().state,
        SessionRecoveryState::None
    );
}

#[test]
fn non_resumable_nonzero_exit_discards_active_session() {
    let script = write_mock_core(
        "non-resumable-nonzero",
        r#"#!/bin/sh
printf '%s\n' '{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-000000000000","session_resumable":false,"model":"mock","tools":[]}'
printf '%s\n' 'provider failed' >&2
exit 7
"#,
    );
    let adapter = adapter_with_active_session(&script);

    adapter
        .run_stream(&test_request(), &mut |_| Ok(()))
        .expect("run non-resumable nonzero exit");
    let _ = std::fs::remove_file(&script);

    assert_eq!(adapter.committed_session_id(), None);
    assert_eq!(
        adapter.recovery_snapshot().state,
        SessionRecoveryState::None
    );
}
