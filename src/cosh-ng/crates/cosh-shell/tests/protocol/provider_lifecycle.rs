use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cosh_shell::adapter::{
    AdapterError, AgentAdapter, AgentRunHandle, AgentRunPoll, ClaudeCodeAdapter, CoshCoreAdapter,
    QwenCliAdapter, SessionRecoveryState, SessionRuntimeState,
};
use cosh_shell::types::{
    AgentEvent, AgentMode, AgentRequest, CommandBlock, CommandStatus, CoshApprovalMode, OutputRefs,
};

fn test_workspace_scope() -> String {
    fs::canonicalize(std::env::temp_dir())
        .expect("canonical test workspace")
        .to_string_lossy()
        .into_owned()
}

fn test_workspace_child(name: &str) -> String {
    Path::new(&test_workspace_scope())
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn mock_provider_script(name: &str, body: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "cosh-provider-lifecycle-{name}-{}-{nonce}.sh",
        std::process::id()
    ));
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write mock provider");
    let mut permissions = fs::metadata(&path)
        .expect("mock provider metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("chmod mock provider");
    path
}

fn qwen_adapter(program: &Path, session_id: Arc<Mutex<Option<String>>>) -> QwenCliAdapter {
    QwenCliAdapter {
        program: program.display().to_string(),
        allow_model_call: true,
        session_id,
    }
}

fn claude_adapter(program: &Path) -> ClaudeCodeAdapter {
    ClaudeCodeAdapter {
        program: program.display().to_string(),
        model: "mock".to_string(),
        max_budget_usd: "1".to_string(),
        allow_model_call: true,
        session_id: Arc::new(Mutex::new(None)),
    }
}

fn cosh_core_restore_adapter(program: &Path) -> CoshCoreAdapter {
    let mut session = SessionRuntimeState::with_active(
        "00000000-0000-4000-8000-000000000000",
        test_workspace_scope(),
    );
    session.recovery.state = SessionRecoveryState::Selected;
    session.recovery.selected_session_id = Some("11111111-1111-4111-8111-111111111111".to_string());
    session.recovery.selected_workspace_scope = Some(test_workspace_scope());
    CoshCoreAdapter {
        program: program.display().to_string(),
        allow_model_call: true,
        session: Arc::new(Mutex::new(session)),
    }
}

fn cosh_core_active_adapter(program: &Path) -> CoshCoreAdapter {
    CoshCoreAdapter {
        program: program.display().to_string(),
        allow_model_call: true,
        session: Arc::new(Mutex::new(SessionRuntimeState::with_active(
            "00000000-0000-4000-8000-000000000000",
            test_workspace_scope(),
        ))),
    }
}

fn cosh_core_active_with_unrelated_selection(program: &Path) -> CoshCoreAdapter {
    let mut session = SessionRuntimeState::with_active(
        "00000000-0000-4000-8000-000000000000",
        test_workspace_child("workspace-b"),
    );
    session.recovery.state = SessionRecoveryState::Selected;
    session.recovery.selected_session_id = Some("11111111-1111-4111-8111-111111111111".to_string());
    session.recovery.selected_workspace_scope = Some(test_workspace_child("workspace-a"));
    CoshCoreAdapter {
        program: program.display().to_string(),
        allow_model_call: true,
        session: Arc::new(Mutex::new(session)),
    }
}

fn make_request(id: &str) -> AgentRequest {
    let workspace_scope = test_workspace_scope();
    AgentRequest {
        id: id.to_string(),
        session_id: "session-1".to_string(),
        command_block: CommandBlock {
            id: "cmd-1".to_string(),
            session_id: "session-1".to_string(),
            command: "echo test".to_string(),
            origin: Default::default(),
            cwd: workspace_scope.clone(),
            end_cwd: workspace_scope,
            started_at_ms: 0,
            ended_at_ms: 1,
            duration_ms: 1,
            exit_code: 1,
            status: CommandStatus::Failed,
            output: OutputRefs {
                terminal_output_ref: None,
                terminal_output_bytes: 0,
            },
            shell_environment_generation: None,
            audit_identity: None,
        },
        context_blocks: Vec::new(),
        context_hints: Vec::new(),
        user_input: Some("test provider lifecycle".to_string()),
        findings: Vec::new(),
        mode: AgentMode::RecommendOnly,
        user_confirmed: true,
        hook_finding: None,
        recommended_skill: None,
    }
}

fn collect_events_until(
    handle: &AgentRunHandle,
    timeout: Duration,
    predicate: impl Fn(&AgentEvent) -> bool,
) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match handle.poll_event_timeout(Duration::from_millis(100)) {
            Ok(AgentRunPoll::Event(event)) => {
                let done = predicate(&event);
                events.push(event);
                if done {
                    break;
                }
            }
            Ok(AgentRunPoll::Timeout) => {}
            Ok(AgentRunPoll::Finished) | Err(_) => break,
        }
    }
    events
}

fn collect_events_until_finished(handle: &AgentRunHandle, timeout: Duration) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    let deadline = Instant::now() + timeout;
    loop {
        assert!(
            Instant::now() < deadline,
            "provider did not finish; events: {events:?}"
        );
        match handle.poll_event_timeout(Duration::from_millis(100)) {
            Ok(AgentRunPoll::Event(event)) => events.push(event),
            Ok(AgentRunPoll::Timeout) => {}
            Ok(AgentRunPoll::Finished) => return events,
            Err(error) => panic!("provider event stream failed: {}", error.message),
        }
    }
}

fn assert_restore_identity_failure(events: &[AgentEvent]) {
    assert!(
        events.iter().any(
            |event| matches!(event, AgentEvent::AgentFailed { error, .. }
                if error.contains("identity mismatch"))
        ),
        "expected identity mismatch failure, got: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })),
        "restore failure must suppress AgentCompleted: {events:?}"
    );
}

fn assert_provider_failure_is_preserved(events: &[AgentEvent]) {
    assert!(
        events.iter().any(
            |event| matches!(event, AgentEvent::AgentFailed { error, .. }
                if error == "Reached maximum budget ($0.05)")
        ),
        "expected original provider failure, got: {events:?}"
    );
    assert!(
        !events.iter().any(
            |event| matches!(event, AgentEvent::AgentFailed { error, .. }
                if error.contains("provider session did not complete"))
        ),
        "generic restore failure replaced provider detail: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })),
        "provider failure must not emit AgentCompleted: {events:?}"
    );
}

fn assert_selected_structured_failure(
    adapter: &CoshCoreAdapter,
    code: &str,
    message: &str,
    hint_fragment: &str,
) {
    assert_eq!(
        adapter.committed_session_id().as_deref(),
        Some("00000000-0000-4000-8000-000000000000")
    );
    let recovery = adapter.recovery_snapshot();
    assert_eq!(recovery.state, SessionRecoveryState::Failed);
    assert_eq!(recovery.selected_session_id, None);
    let error = recovery.last_error.as_ref().expect("typed session failure");
    assert_eq!(error.code, code);
    assert_eq!(error.message, message);
    assert!(error
        .hint
        .as_deref()
        .is_some_and(|hint| hint.contains(hint_fragment)));
}

fn assert_recorded_process_is_gone(pid_file: &Path) {
    let pid: i32 = fs::read_to_string(pid_file)
        .expect("read mock provider pid")
        .trim()
        .parse()
        .expect("parse mock provider pid");
    let result = unsafe { nix::libc::kill(pid, 0) };
    let error = std::io::Error::last_os_error();
    assert_eq!(result, -1, "mock provider PID {pid} is still alive");
    assert_eq!(
        error.raw_os_error(),
        Some(nix::libc::ESRCH),
        "unexpected PID probe error for {pid}: {error}"
    );
}

fn assert_recorded_process_is_not_running(pid_file: &Path) {
    let pid = fs::read_to_string(pid_file)
        .expect("read mock provider pid")
        .trim()
        .to_string();
    let status = fs::read_to_string(format!("/proc/{pid}/status"));
    if let Ok(status) = status {
        assert!(
            status
                .lines()
                .any(|line| { line.starts_with("State:\tZ") || line.starts_with("State:\tX") }),
            "mock provider descendant PID {pid} is still running: {status}"
        );
    }
}

#[test]
fn qwen_provider_lifecycle_cancellable_process_emits_cancelled_event() {
    let script = mock_provider_script("qwen-sleep", "exec /bin/sleep 10");
    let adapter = qwen_adapter(&script, Arc::new(Mutex::new(None)));
    let handle =
        adapter.start_cancellable(make_request("qwen-cancel"), CoshApprovalMode::Recommend);

    let starting = collect_events_until(
        &handle,
        Duration::from_secs(2),
        |event| matches!(event, AgentEvent::StatusChanged { phase, .. } if phase == "starting"),
    );
    assert!(
        starting.iter().any(
            |event| matches!(event, AgentEvent::StatusChanged { phase, .. } if phase == "starting")
        ),
        "expected starting event, got: {starting:?}"
    );

    handle.cancel();

    let cancelled = collect_events_until(&handle, Duration::from_secs(3), |event| {
        matches!(event, AgentEvent::AgentCancelled { .. })
    });
    let _ = fs::remove_file(script);
    assert!(
        cancelled
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCancelled { .. })),
        "expected AgentCancelled after cancel, got: {cancelled:?}"
    );
}

#[test]
fn claude_provider_lifecycle_cancellable_process_emits_cancelled_event() {
    let script = mock_provider_script("claude-sleep", "exec /bin/sleep 10");
    let adapter = claude_adapter(&script);
    let handle =
        adapter.start_cancellable(make_request("claude-cancel"), CoshApprovalMode::Recommend);

    let starting = collect_events_until(
        &handle,
        Duration::from_secs(2),
        |event| matches!(event, AgentEvent::StatusChanged { phase, .. } if phase == "starting"),
    );
    assert!(
        starting.iter().any(
            |event| matches!(event, AgentEvent::StatusChanged { phase, .. } if phase == "starting")
        ),
        "expected starting event, got: {starting:?}"
    );

    handle.cancel();

    let cancelled = collect_events_until(&handle, Duration::from_secs(3), |event| {
        matches!(event, AgentEvent::AgentCancelled { .. })
    });
    let _ = fs::remove_file(script);
    assert!(
        cancelled
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCancelled { .. })),
        "expected AgentCancelled after cancel, got: {cancelled:?}"
    );
}

#[test]
fn qwen_provider_lifecycle_commits_session_only_after_successful_completion() {
    let script = mock_provider_script(
        "qwen-success",
        "printf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-ok\",\"model\":\"qwen\"}'\nprintf '%s\\n' '{\"type\":\"result\",\"session_id\":\"sess-ok\",\"result\":\"done\"}'",
    );
    let committed = Arc::new(Mutex::new(None));
    let adapter = qwen_adapter(&script, Arc::clone(&committed));
    let handle =
        adapter.start_cancellable(make_request("qwen-success"), CoshApprovalMode::Recommend);

    let completed = collect_events_until(&handle, Duration::from_secs(3), |event| {
        matches!(event, AgentEvent::AgentCompleted { .. })
    });
    let _ = fs::remove_file(script);
    assert!(
        completed
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })),
        "expected AgentCompleted, got: {completed:?}"
    );
    assert_eq!(
        committed.lock().expect("committed session").as_deref(),
        Some("sess-ok")
    );
}

#[test]
fn qwen_provider_lifecycle_does_not_commit_session_after_provider_failure() {
    let script = mock_provider_script(
        "qwen-failure",
        "printf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-bad\",\"model\":\"qwen\"}'\nexit 2",
    );
    let committed = Arc::new(Mutex::new(Some("sess-prev".to_string())));
    let adapter = qwen_adapter(&script, Arc::clone(&committed));
    let handle =
        adapter.start_cancellable(make_request("qwen-failure"), CoshApprovalMode::Recommend);

    let failed = collect_events_until(&handle, Duration::from_secs(3), |event| {
        matches!(event, AgentEvent::AgentFailed { .. })
    });
    let _ = fs::remove_file(script);
    assert!(
        failed
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentFailed { .. })),
        "expected AgentFailed, got: {failed:?}"
    );
    assert_eq!(
        committed.lock().expect("committed session").as_deref(),
        Some("sess-prev")
    );
}

#[test]
fn cosh_core_sync_restore_identity_failure_replaces_completed_event() {
    let script = mock_provider_script(
        "cosh-core-sync-identity-mismatch",
        r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"22222222-2222-4222-8222-222222222222","model":"mock","tools":[]}'
printf '%s\n' '{"type":"result","subtype":"success","session_id":"22222222-2222-4222-8222-222222222222","is_error":false,"duration_ms":1,"result":"done"}'"#,
    );
    let adapter = cosh_core_restore_adapter(&script);

    let events = adapter
        .run(&make_request("cosh-core-sync-identity-mismatch"))
        .expect("run mismatched restore");

    assert_restore_identity_failure(&events);
    assert_eq!(
        adapter.committed_session_id().as_deref(),
        Some("00000000-0000-4000-8000-000000000000")
    );
    assert_eq!(
        adapter.recovery_snapshot().state,
        SessionRecoveryState::Failed
    );
    let _ = fs::remove_file(script);
}

#[test]
fn cosh_core_async_restore_identity_failure_replaces_completed_event() {
    let script = mock_provider_script(
        "cosh-core-async-identity-mismatch",
        r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"22222222-2222-4222-8222-222222222222","model":"mock","tools":[]}'
printf '%s\n' '{"type":"result","subtype":"success","session_id":"22222222-2222-4222-8222-222222222222","is_error":false,"duration_ms":1,"result":"done"}'"#,
    );

    for (label, mode) in [
        ("recommend", CoshApprovalMode::Recommend),
        ("control", CoshApprovalMode::Auto),
    ] {
        let adapter = cosh_core_restore_adapter(&script);
        let handle = adapter.start_cancellable(
            make_request(&format!("cosh-core-{label}-identity-mismatch")),
            mode,
        );

        let events = collect_events_until_finished(&handle, Duration::from_secs(3));

        assert_restore_identity_failure(&events);
        assert_eq!(
            adapter.committed_session_id().as_deref(),
            Some("00000000-0000-4000-8000-000000000000")
        );
        assert_eq!(
            adapter.recovery_snapshot().state,
            SessionRecoveryState::Failed
        );
    }
    let _ = fs::remove_file(script);
}

#[test]
fn active_resume_identity_mismatch_is_rejected_for_every_runner() {
    let script = mock_provider_script(
        "cosh-core-active-identity-mismatch",
        r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"22222222-2222-4222-8222-222222222222","model":"mock","tools":[]}'
printf '%s\n' '{"type":"result","subtype":"success","session_id":"22222222-2222-4222-8222-222222222222","is_error":false,"duration_ms":1,"result":"done"}'"#,
    );

    let sync = cosh_core_active_adapter(&script);
    let sync_events = sync
        .run(&make_request("cosh-core-sync-active-identity-mismatch"))
        .expect("run mismatched active resume");
    assert_restore_identity_failure(&sync_events);
    assert_eq!(sync.committed_session_id(), None);
    assert_eq!(sync.recovery_snapshot().state, SessionRecoveryState::Failed);

    for (label, mode) in [
        ("recommend", CoshApprovalMode::Recommend),
        ("control", CoshApprovalMode::Auto),
    ] {
        let adapter = cosh_core_active_adapter(&script);
        let handle = adapter.start_cancellable(
            make_request(&format!("cosh-core-{label}-active-identity-mismatch")),
            mode,
        );
        let events = collect_events_until_finished(&handle, Duration::from_secs(3));

        assert_restore_identity_failure(&events);
        assert_eq!(adapter.committed_session_id(), None);
        assert_eq!(
            adapter.recovery_snapshot().state,
            SessionRecoveryState::Failed
        );
    }
    let _ = fs::remove_file(script);
}

#[test]
fn cosh_core_sync_restore_preserves_provider_result_error() {
    let script = mock_provider_script(
        "cosh-core-sync-result-error",
        r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"11111111-1111-4111-8111-111111111111","model":"mock","tools":[]}'
printf '%s\n' '{"type":"result","subtype":"error_max_budget_usd","session_id":"11111111-1111-4111-8111-111111111111","is_error":true,"errors":["Reached maximum budget ($0.05)"]}'"#,
    );
    let adapter = cosh_core_restore_adapter(&script);

    let events = adapter
        .run(&make_request("cosh-core-sync-result-error"))
        .expect("run provider result error");

    assert_provider_failure_is_preserved(&events);
    assert_eq!(
        adapter.recovery_snapshot().state,
        SessionRecoveryState::Failed
    );
    let _ = fs::remove_file(script);
}

#[test]
fn cosh_core_async_restore_preserves_provider_result_error() {
    let script = mock_provider_script(
        "cosh-core-async-result-error",
        r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"11111111-1111-4111-8111-111111111111","model":"mock","tools":[]}'
printf '%s\n' '{"type":"result","subtype":"error_max_budget_usd","session_id":"11111111-1111-4111-8111-111111111111","is_error":true,"errors":["Reached maximum budget ($0.05)"]}'"#,
    );

    for (label, mode) in [
        ("recommend", CoshApprovalMode::Recommend),
        ("control", CoshApprovalMode::Auto),
    ] {
        let adapter = cosh_core_restore_adapter(&script);
        let handle = adapter.start_cancellable(
            make_request(&format!("cosh-core-{label}-result-error")),
            mode,
        );

        let events = collect_events_until_finished(&handle, Duration::from_secs(3));

        assert_provider_failure_is_preserved(&events);
        assert_eq!(
            adapter.recovery_snapshot().state,
            SessionRecoveryState::Failed
        );
    }
    let _ = fs::remove_file(script);
}

#[test]
fn cosh_core_active_load_failure_releases_resume_for_every_runner() {
    let script = mock_provider_script(
        "cosh-core-active-not-found",
        r#"printf '%s\n' '{"type":"result","subtype":"error","session_id":"00000000-0000-4000-8000-000000000000","is_error":true,"errors":["session recovery failed [not_found]: session not found"],"session_error_code":"not_found","session_error_phase":"load"}'"#,
    );

    let sync = cosh_core_active_adapter(&script);
    let sync_events = sync
        .run(&make_request("cosh-core-sync-active-not-found"))
        .expect("run sync active resume failure");
    assert!(sync_events.iter().any(
        |event| matches!(event, AgentEvent::AgentFailed { error, .. }
            if error.contains("[not_found]"))
    ));
    assert_eq!(sync.committed_session_id(), None);
    assert!(sync.protected_session_ids().is_empty());
    assert_eq!(sync.recovery_snapshot().state, SessionRecoveryState::Failed);
    assert_eq!(
        sync.recovery_snapshot()
            .last_error
            .as_ref()
            .map(|error| error.code.as_str()),
        Some("not_found")
    );

    for (label, mode) in [
        ("recommend", CoshApprovalMode::Recommend),
        ("control", CoshApprovalMode::Auto),
    ] {
        let adapter = cosh_core_active_adapter(&script);
        let handle = adapter.start_cancellable(
            make_request(&format!("cosh-core-{label}-active-not-found")),
            mode,
        );
        let events = collect_events_until_finished(&handle, Duration::from_secs(3));

        assert!(events.iter().any(
            |event| matches!(event, AgentEvent::AgentFailed { error, .. }
                if error.contains("[not_found]"))
        ));
        assert_eq!(adapter.committed_session_id(), None);
        assert!(adapter.protected_session_ids().is_empty());
        assert_eq!(
            adapter.recovery_snapshot().state,
            SessionRecoveryState::Failed
        );
    }
    let _ = fs::remove_file(script);
}

#[test]
fn active_load_failure_preserves_unrelated_selection_for_every_runner() {
    let script = mock_provider_script(
        "cosh-core-active-not-found-with-selection",
        r#"printf '%s\n' '{"type":"result","subtype":"error","session_id":"00000000-0000-4000-8000-000000000000","is_error":true,"errors":["session recovery failed"],"session_error_code":"not_found","session_error_phase":"load"}'"#,
    );
    let mut request = make_request("cosh-core-sync-active-not-found-with-selection");
    request.command_block.cwd = test_workspace_child("workspace-b");
    request.command_block.end_cwd = test_workspace_child("workspace-b");

    let sync = cosh_core_active_with_unrelated_selection(&script);
    let _ = sync.run(&request).expect("run sync active resume failure");
    assert_eq!(sync.committed_session_id(), None);
    assert_eq!(
        sync.recovery_snapshot().state,
        SessionRecoveryState::Selected
    );
    assert_eq!(
        sync.recovery_snapshot().selected_session_id.as_deref(),
        Some("11111111-1111-4111-8111-111111111111")
    );

    for (label, mode) in [
        ("recommend", CoshApprovalMode::Recommend),
        ("control", CoshApprovalMode::Auto),
    ] {
        let adapter = cosh_core_active_with_unrelated_selection(&script);
        let mut request = request.clone();
        request.id = format!("cosh-core-{label}-active-not-found-with-selection");
        let handle = adapter.start_cancellable(request, mode);
        let _ = collect_events_until_finished(&handle, Duration::from_secs(3));

        assert_eq!(adapter.committed_session_id(), None);
        assert_eq!(
            adapter.recovery_snapshot().state,
            SessionRecoveryState::Selected
        );
        assert_eq!(
            adapter.recovery_snapshot().selected_session_id.as_deref(),
            Some("11111111-1111-4111-8111-111111111111")
        );
    }
    let _ = fs::remove_file(script);
}

#[test]
fn disable_resume_hint_preserves_selection_for_every_runner() {
    let script = mock_provider_script(
        "cosh-core-disable-resume",
        r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"22222222-2222-4222-8222-222222222222","model":"mock","tools":[]}'
printf '%s\n' '{"type":"result","subtype":"success","session_id":"22222222-2222-4222-8222-222222222222","is_error":false,"duration_ms":1,"result":"done"}'"#,
    );
    let mut request = make_request("cosh-core-sync-disable-resume");
    request
        .context_hints
        .push("disable provider resume for shell handoff fallback".to_string());

    let sync = cosh_core_restore_adapter(&script);
    let sync_events = sync.run(&request).expect("run sync without resume");
    assert!(sync_events
        .iter()
        .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })));
    assert_eq!(
        sync.recovery_snapshot().state,
        SessionRecoveryState::Selected
    );
    assert_eq!(
        sync.recovery_snapshot().selected_session_id.as_deref(),
        Some("11111111-1111-4111-8111-111111111111")
    );
    assert_eq!(
        sync.committed_session_id().as_deref(),
        Some("22222222-2222-4222-8222-222222222222")
    );

    for (label, mode) in [
        ("recommend", CoshApprovalMode::Recommend),
        ("control", CoshApprovalMode::Auto),
    ] {
        let adapter = cosh_core_restore_adapter(&script);
        let mut request = request.clone();
        request.id = format!("cosh-core-{label}-disable-resume");
        let handle = adapter.start_cancellable(request, mode);
        let events = collect_events_until_finished(&handle, Duration::from_secs(3));

        assert!(events
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })));
        assert!(!events
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentFailed { .. })));
        assert_eq!(
            adapter.recovery_snapshot().state,
            SessionRecoveryState::Selected
        );
        assert_eq!(
            adapter.recovery_snapshot().selected_session_id.as_deref(),
            Some("11111111-1111-4111-8111-111111111111")
        );
    }
    let _ = fs::remove_file(script);
}

#[test]
fn disabled_resume_non_resumable_turn_preserves_unattempted_ids_for_every_runner() {
    let script = mock_provider_script(
        "cosh-core-disable-non-resumable",
        r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"22222222-2222-4222-8222-222222222222","session_resumable":false,"model":"mock","tools":[]}'
printf '%s\n' '{"type":"result","subtype":"success","session_id":"22222222-2222-4222-8222-222222222222","is_error":false,"duration_ms":1,"result":"done"}'"#,
    );
    let mut request = make_request("cosh-core-sync-disable-non-resumable");
    request
        .context_hints
        .push("disable provider resume for shell handoff fallback".to_string());

    let sync = cosh_core_restore_adapter(&script);
    let sync_events = sync.run(&request).expect("run sync non-resumable turn");
    assert!(sync_events
        .iter()
        .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })));
    assert_eq!(
        sync.committed_session_id().as_deref(),
        Some("00000000-0000-4000-8000-000000000000")
    );
    assert_eq!(
        sync.recovery_snapshot().selected_session_id.as_deref(),
        Some("11111111-1111-4111-8111-111111111111")
    );

    for (label, mode) in [
        ("recommend", CoshApprovalMode::Recommend),
        ("control", CoshApprovalMode::Auto),
    ] {
        let adapter = cosh_core_restore_adapter(&script);
        let mut request = request.clone();
        request.id = format!("cosh-core-{label}-disable-non-resumable");
        let handle = adapter.start_cancellable(request, mode);
        let events = collect_events_until_finished(&handle, Duration::from_secs(3));

        assert!(events
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })));
        assert_eq!(
            adapter.committed_session_id().as_deref(),
            Some("00000000-0000-4000-8000-000000000000")
        );
        assert_eq!(
            adapter.recovery_snapshot().selected_session_id.as_deref(),
            Some("11111111-1111-4111-8111-111111111111")
        );
    }
    let _ = fs::remove_file(script);
}

#[test]
fn ordinary_active_provider_failure_keeps_committed_resume() {
    let script = mock_provider_script(
        "cosh-core-active-budget-error",
        r#"printf '%s\n' '{"type":"result","subtype":"error","session_id":"00000000-0000-4000-8000-000000000000","is_error":true,"errors":["Reached maximum budget ($0.05)"]}'"#,
    );
    let adapter = cosh_core_active_adapter(&script);

    let events = adapter
        .run(&make_request("cosh-core-active-budget-error"))
        .expect("run ordinary provider failure");

    assert_provider_failure_is_preserved(&events);
    assert_eq!(
        adapter.committed_session_id().as_deref(),
        Some("00000000-0000-4000-8000-000000000000")
    );
    assert_eq!(
        adapter.recovery_snapshot().state,
        SessionRecoveryState::Active
    );
    let _ = fs::remove_file(script);
}

#[test]
fn active_persistence_failure_releases_resume_for_every_runner() {
    let script = mock_provider_script(
        "cosh-core-active-persist-conflict",
        r#"printf '%s\n' '{"type":"result","subtype":"error","session_id":"00000000-0000-4000-8000-000000000000","is_error":true,"errors":["session persistence failed [conflict]: session changed concurrently"],"session_error_code":"conflict","session_error_phase":"persist"}'"#,
    );

    let sync = cosh_core_active_adapter(&script);
    let sync_events = sync
        .run(&make_request("cosh-core-sync-active-persist-conflict"))
        .expect("run active persistence failure");
    assert!(sync_events.iter().any(
        |event| matches!(event, AgentEvent::AgentFailed { error, .. }
            if error.contains("persistence failed [conflict]"))
    ));
    assert_eq!(sync.committed_session_id(), None);
    assert_eq!(sync.recovery_snapshot().state, SessionRecoveryState::Failed);
    assert_eq!(
        sync.recovery_snapshot()
            .last_error
            .as_ref()
            .map(|error| error.code.as_str()),
        Some("conflict")
    );

    for (label, mode) in [
        ("recommend", CoshApprovalMode::Recommend),
        ("control", CoshApprovalMode::Auto),
    ] {
        let adapter = cosh_core_active_adapter(&script);
        let handle = adapter.start_cancellable(
            make_request(&format!("cosh-core-{label}-active-persist-conflict")),
            mode,
        );
        let events = collect_events_until_finished(&handle, Duration::from_secs(3));

        assert!(events.iter().any(
            |event| matches!(event, AgentEvent::AgentFailed { error, .. }
                if error.contains("persistence failed [conflict]"))
        ));
        assert_eq!(adapter.committed_session_id(), None);
        assert_eq!(
            adapter.recovery_snapshot().state,
            SessionRecoveryState::Failed
        );
    }
    let _ = fs::remove_file(script);
}

#[test]
fn selected_structured_failures_preserve_metadata_for_every_runner() {
    for (label, code, phase, message, hint_fragment) in [
        (
            "load",
            "scope_mismatch",
            "load",
            "session recovery failed [scope_mismatch]: selected workspace changed",
            "Refresh the session list",
        ),
        (
            "persist",
            "conflict",
            "persist",
            "session persistence failed [conflict]: selected session changed",
            "Resolve the persistence failure",
        ),
    ] {
        let script = mock_provider_script(
            &format!("cosh-core-selected-{label}-failure"),
            &format!(
                "printf '%s\\n' '{{\"type\":\"result\",\"subtype\":\"error\",\
                 \"session_id\":\"11111111-1111-4111-8111-111111111111\",\
                 \"is_error\":true,\"errors\":[\"{message}\"],\
                 \"session_error_code\":\"{code}\",\"session_error_phase\":\"{phase}\"}}'"
            ),
        );

        let sync = cosh_core_restore_adapter(&script);
        let sync_events = sync
            .run(&make_request(&format!("cosh-core-sync-selected-{label}")))
            .expect("run selected structured failure");
        assert!(sync_events.iter().any(
            |event| matches!(event, AgentEvent::AgentFailed { error, .. } if error == message)
        ));
        assert_selected_structured_failure(&sync, code, message, hint_fragment);

        for (runner, mode) in [
            ("recommend", CoshApprovalMode::Recommend),
            ("control", CoshApprovalMode::Auto),
        ] {
            let adapter = cosh_core_restore_adapter(&script);
            let handle = adapter.start_cancellable(
                make_request(&format!("cosh-core-{runner}-selected-{label}")),
                mode,
            );
            let events = collect_events_until_finished(&handle, Duration::from_secs(3));

            assert!(events.iter().any(
                |event| matches!(event, AgentEvent::AgentFailed { error, .. } if error == message)
            ));
            assert_selected_structured_failure(&adapter, code, message, hint_fragment);
        }
        let _ = fs::remove_file(script);
    }
}

#[test]
fn cancelled_async_runners_apply_already_parsed_session_failure() {
    let script = mock_provider_script(
        "cosh-core-persist-conflict-then-cancel",
        r#"printf '%s\n' '{"type":"result","subtype":"error","session_id":"00000000-0000-4000-8000-000000000000","is_error":true,"errors":["session persistence failed [conflict]: parsed before cancellation"],"session_error_code":"conflict","session_error_phase":"persist"}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-000000000000","model":"mock","tools":[]}'
exec sleep 30"#,
    );

    for (label, mode) in [
        ("recommend", CoshApprovalMode::Recommend),
        ("control", CoshApprovalMode::Auto),
    ] {
        let adapter = cosh_core_active_adapter(&script);
        let handle = adapter.start_cancellable(
            make_request(&format!("cosh-core-{label}-persist-then-cancel")),
            mode,
        );
        let initialized = collect_events_until(
            &handle,
            Duration::from_secs(3),
            |event| matches!(event, AgentEvent::StatusChanged { phase, .. } if phase == "initialized"),
        );
        assert!(
            initialized.iter().any(
                |event| matches!(event, AgentEvent::StatusChanged { phase, .. }
                    if phase == "initialized")
            ),
            "{label} runner did not parse the marker after the structured failure: {initialized:?}"
        );

        handle.cancel();
        let cancelled = collect_events_until(&handle, Duration::from_secs(3), |event| {
            matches!(event, AgentEvent::AgentCancelled { .. })
        });

        assert!(
            cancelled
                .iter()
                .any(|event| matches!(event, AgentEvent::AgentCancelled { .. })),
            "{label} runner did not preserve cancellation semantics: {cancelled:?}"
        );
        assert_eq!(adapter.committed_session_id(), None);
        let recovery = adapter.recovery_snapshot();
        assert_eq!(recovery.state, SessionRecoveryState::Failed);
        assert_eq!(
            recovery
                .last_error
                .as_ref()
                .map(|error| error.code.as_str()),
            Some("conflict")
        );
    }
    let _ = fs::remove_file(script);
}

#[test]
fn cancelled_selected_runners_preserve_already_parsed_session_failure() {
    for (failure, code, phase, message, hint_fragment) in [
        (
            "load",
            "not_found",
            "load",
            "session recovery failed [not_found]: selected session disappeared",
            "Refresh the session list",
        ),
        (
            "persist",
            "conflict",
            "persist",
            "session persistence failed [conflict]: selected session changed",
            "Resolve the persistence failure",
        ),
    ] {
        let script = mock_provider_script(
            &format!("cosh-core-selected-{failure}-then-cancel"),
            &format!(
                r#"printf '%s\n' '{{"type":"result","subtype":"error","session_id":"11111111-1111-4111-8111-111111111111","is_error":true,"errors":["{message}"],"session_error_code":"{code}","session_error_phase":"{phase}"}}'
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"11111111-1111-4111-8111-111111111111","model":"mock","tools":[]}}'
exec sleep 30"#
            ),
        );

        for (runner, mode) in [
            ("recommend", CoshApprovalMode::Recommend),
            ("control", CoshApprovalMode::Auto),
        ] {
            let adapter = cosh_core_restore_adapter(&script);
            let handle = adapter.start_cancellable(
                make_request(&format!("cosh-core-{runner}-selected-{failure}-cancel")),
                mode,
            );
            let initialized = collect_events_until(
                &handle,
                Duration::from_secs(3),
                |event| matches!(event, AgentEvent::StatusChanged { phase, .. } if phase == "initialized"),
            );
            assert!(
                initialized.iter().any(
                    |event| matches!(event, AgentEvent::StatusChanged { phase, .. }
                        if phase == "initialized")
                ),
                "{runner} runner did not parse selected {failure} failure: {initialized:?}"
            );

            handle.cancel();
            let cancelled = collect_events_until(&handle, Duration::from_secs(3), |event| {
                matches!(event, AgentEvent::AgentCancelled { .. })
            });
            assert!(
                cancelled
                    .iter()
                    .any(|event| matches!(event, AgentEvent::AgentCancelled { .. })),
                "{runner} runner did not preserve cancellation: {cancelled:?}"
            );
            assert_selected_structured_failure(&adapter, code, message, hint_fragment);
        }
        let _ = fs::remove_file(script);
    }
}

#[test]
fn structured_session_failure_survives_nonzero_exit_for_every_runner() {
    let script = mock_provider_script(
        "cosh-core-active-persist-conflict-exit-one",
        r#"printf '%s\n' '{"type":"result","subtype":"error","session_id":"00000000-0000-4000-8000-000000000000","is_error":true,"errors":["session persistence failed [conflict]: retained detail"],"session_error_code":"conflict","session_error_phase":"persist"}'
exit 1"#,
    );

    let sync = cosh_core_active_adapter(&script);
    let sync_events = sync
        .run(&make_request("cosh-core-sync-persist-exit-one"))
        .expect("run structured nonzero persistence failure");
    assert!(sync_events.iter().any(
        |event| matches!(event, AgentEvent::AgentFailed { error, .. }
            if error == "session persistence failed [conflict]: retained detail")
    ));
    assert_eq!(sync.committed_session_id(), None);

    for (label, mode) in [
        ("recommend", CoshApprovalMode::Recommend),
        ("control", CoshApprovalMode::Auto),
    ] {
        let adapter = cosh_core_active_adapter(&script);
        let handle = adapter.start_cancellable(
            make_request(&format!("cosh-core-{label}-persist-exit-one")),
            mode,
        );
        let events = collect_events_until_finished(&handle, Duration::from_secs(3));

        assert!(events.iter().any(
            |event| matches!(event, AgentEvent::AgentFailed { error, .. }
                if error == "session persistence failed [conflict]: retained detail")
        ));
        assert_eq!(adapter.committed_session_id(), None);
        assert_eq!(
            adapter
                .recovery_snapshot()
                .last_error
                .as_ref()
                .map(|error| error.code.as_str()),
            Some("conflict")
        );
    }
    let _ = fs::remove_file(script);
}

#[test]
fn cosh_core_pending_question_nonzero_exit_reports_only_protocol_failure() {
    let script = mock_provider_script(
        "cosh-core-question-then-nonzero",
        r#"read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-000000000000","model":"mock"}'
read -r user_message
printf '%s\n' '{"type":"control_request","request_id":"ask-pending","request":{"subtype":"ask_user","question":"Choose","options":[{"label":"One"}],"allow_free_text":false,"multi_select":false}}'
printf '%s\n' 'provider stderr must stay hidden' >&2
exit 7"#,
    );
    let adapter = cosh_core_active_adapter(&script);
    let handle = adapter.start_cancellable(
        make_request("cosh-core-question-nonzero"),
        CoshApprovalMode::Auto,
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut events = Vec::new();
    let mut errors = Vec::new();
    loop {
        assert!(Instant::now() < deadline, "provider did not finish");
        match handle.poll_event_timeout(Duration::from_millis(100)) {
            Ok(AgentRunPoll::Event(event)) => events.push(event),
            Ok(AgentRunPoll::Timeout) => {}
            Ok(AgentRunPoll::Finished) => break,
            Err(error) => errors.push(error.message),
        }
    }

    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::UserQuestion { .. })),
        "question was not emitted: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentFailed { .. })),
        "generic process failure must not precede protocol failure: {events:?}"
    );
    assert_eq!(
        errors,
        vec!["cosh-core-question-protocol:premature-completion"]
    );
    assert!(
        !format!("{events:?}{errors:?}").contains("provider stderr must stay hidden"),
        "provider stderr leaked through the protocol failure"
    );
    let _ = fs::remove_file(script);
}

#[test]
fn structured_session_failure_is_finalized_before_read_error_for_every_runner() {
    let script = mock_provider_script(
        "cosh-core-persist-conflict-invalid-utf8",
        r#"printf '%s\n' '{"type":"result","subtype":"error","session_id":"00000000-0000-4000-8000-000000000000","is_error":true,"errors":["session persistence failed [conflict]: retained before read failure"],"session_error_code":"conflict","session_error_phase":"persist"}'
printf '\377\n'
exec sleep 30"#,
    );

    let sync = cosh_core_active_adapter(&script);
    let mut sync_events = Vec::new();
    let sync_error = sync
        .run_stream(
            &make_request("cosh-core-sync-persist-read-error"),
            &mut |event| {
                sync_events.push(event);
                Ok(())
            },
        )
        .expect_err("invalid UTF-8 must remain a transport error");
    assert!(sync_events.iter().any(
        |event| matches!(event, AgentEvent::AgentFailed { error, .. }
            if error == "session persistence failed [conflict]: retained before read failure")
    ));
    assert!(sync_error
        .message
        .contains("failed to read cosh-core stream"));
    assert_eq!(sync.committed_session_id(), None);

    for (label, mode) in [
        ("recommend", CoshApprovalMode::Recommend),
        ("control", CoshApprovalMode::Auto),
    ] {
        let adapter = cosh_core_active_adapter(&script);
        let handle = adapter.start_cancellable(
            make_request(&format!("cosh-core-{label}-persist-read-error")),
            mode,
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut saw_structured_failure = false;
        let mut saw_transport_error_after_failure = false;
        loop {
            assert!(Instant::now() < deadline, "{label} runner did not finish");
            match handle.poll_event_timeout(Duration::from_millis(100)) {
                Ok(AgentRunPoll::Event(AgentEvent::AgentFailed { error, .. }))
                    if error
                        == "session persistence failed [conflict]: retained before read failure" =>
                {
                    saw_structured_failure = true;
                }
                Ok(AgentRunPoll::Event(_)) | Ok(AgentRunPoll::Timeout) => {}
                Ok(AgentRunPoll::Finished) => break,
                Err(error) => {
                    assert!(
                        saw_structured_failure,
                        "{label} transport error arrived before structured failure: {}",
                        error.message
                    );
                    assert!(
                        error.message.contains("failed to read cosh-core stream"),
                        "{}",
                        error.message
                    );
                    saw_transport_error_after_failure = true;
                }
            }
        }
        assert!(saw_structured_failure, "{label} lost structured failure");
        assert!(
            saw_transport_error_after_failure,
            "{label} lost transport failure"
        );
        assert_eq!(adapter.committed_session_id(), None);
        assert_eq!(
            adapter
                .recovery_snapshot()
                .last_error
                .as_ref()
                .map(|error| error.code.as_str()),
            Some("conflict")
        );
    }
    let _ = fs::remove_file(script);
}

#[test]
fn ordinary_provider_error_marker_cannot_release_active_resume() {
    let script = mock_provider_script(
        "cosh-core-active-marker-error",
        r#"printf '%s\n' '{"type":"result","subtype":"error","session_id":"00000000-0000-4000-8000-000000000000","is_error":true,"errors":["model output mentioned [not_found] without a session error code"]}'"#,
    );
    let adapter = cosh_core_active_adapter(&script);

    let events = adapter
        .run(&make_request("cosh-core-active-marker-error"))
        .expect("run ordinary marker provider failure");

    assert!(events.iter().any(
        |event| matches!(event, AgentEvent::AgentFailed { error, .. }
            if error.contains("[not_found]"))
    ));
    assert_eq!(
        adapter.committed_session_id().as_deref(),
        Some("00000000-0000-4000-8000-000000000000")
    );
    assert_eq!(
        adapter.recovery_snapshot().state,
        SessionRecoveryState::Active
    );
    let _ = fs::remove_file(script);
}

#[test]
fn cosh_core_sync_reaps_descendant_that_inherits_output_pipes() {
    let pid_file = std::env::temp_dir().join(format!(
        "cosh-core-sync-descendant-{}.pid",
        std::process::id()
    ));
    let script = mock_provider_script(
        "cosh-core-sync-descendant",
        &format!(
            r#"printf '%s\n' '{{"type":"system","subtype":"init","session_id":"11111111-1111-4111-8111-111111111111","model":"mock","tools":[]}}'
printf '%s\n' '{{"type":"result","subtype":"success","session_id":"11111111-1111-4111-8111-111111111111","is_error":false,"duration_ms":1,"result":"done"}}'
sleep 30 &
printf '%s\n' "$!" > "{}"
exit 0"#,
            pid_file.display()
        ),
    );
    let adapter = cosh_core_restore_adapter(&script);
    let started = Instant::now();

    let events = adapter
        .run(&make_request("cosh-core-sync-descendant"))
        .expect("sync runner must reap inherited pipes");

    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })));
    assert_recorded_process_is_not_running(&pid_file);
    let _ = fs::remove_file(pid_file);
    let _ = fs::remove_file(script);
}

#[test]
fn cosh_core_sync_sink_error_terminates_and_reaps_provider() {
    let pid_file =
        std::env::temp_dir().join(format!("cosh-core-sink-child-{}.pid", std::process::id()));
    let script = mock_provider_script(
        "cosh-core-sink-child",
        &format!(
            r#"printf '%s\n' "$$" > "{}"
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"11111111-1111-4111-8111-111111111111","model":"mock","tools":[]}}'
trap '' TERM
exec sleep 30"#,
            pid_file.display()
        ),
    );
    let adapter = cosh_core_restore_adapter(&script);

    let result = adapter.run_stream(&make_request("cosh-core-sink-child"), &mut |event| {
        if matches!(
            event,
            AgentEvent::StatusChanged { ref phase, .. } if phase == "initialized"
        ) {
            return Err(AdapterError {
                message: "provider sink failed".to_string(),
            });
        }
        Ok(())
    });

    assert_eq!(
        result.expect_err("provider sink failure").message,
        "provider sink failed"
    );
    assert_recorded_process_is_gone(&pid_file);
    assert_eq!(
        adapter.recovery_snapshot().state,
        SessionRecoveryState::Failed
    );
    let _ = fs::remove_file(pid_file);
    let _ = fs::remove_file(script);
}

#[test]
fn cosh_core_sync_read_error_terminates_and_reaps_provider() {
    let pid_file =
        std::env::temp_dir().join(format!("cosh-core-read-child-{}.pid", std::process::id()));
    let script = mock_provider_script(
        "cosh-core-read-child",
        &format!(
            r#"printf '%s\n' "$$" > "{}"
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"11111111-1111-4111-8111-111111111111","model":"mock","tools":[]}}'
printf '\377\n'
trap '' TERM
exec sleep 30"#,
            pid_file.display()
        ),
    );
    let adapter = cosh_core_restore_adapter(&script);

    let error = adapter
        .run_stream(&make_request("cosh-core-read-child"), &mut |_| Ok(()))
        .expect_err("stream read failure");

    assert!(
        error.message.contains("failed to read cosh-core stream"),
        "{}",
        error.message
    );
    assert_recorded_process_is_gone(&pid_file);
    assert_eq!(
        adapter.recovery_snapshot().state,
        SessionRecoveryState::Failed
    );
    let _ = fs::remove_file(pid_file);
    let _ = fs::remove_file(script);
}

#[test]
fn cosh_core_async_read_error_terminates_and_reaps_provider() {
    for (label, mode) in [
        ("recommend", CoshApprovalMode::Recommend),
        ("control", CoshApprovalMode::Auto),
    ] {
        let pid_file = std::env::temp_dir().join(format!(
            "cosh-core-async-read-{label}-{}.pid",
            std::process::id()
        ));
        let script = mock_provider_script(
            &format!("cosh-core-async-read-{label}"),
            &format!(
                r#"printf '%s\n' "$$" > "{}"
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"11111111-1111-4111-8111-111111111111","model":"mock","tools":[]}}'
printf '\377\n'
trap '' TERM
exec sleep 30"#,
                pid_file.display()
            ),
        );
        let adapter = cosh_core_restore_adapter(&script);
        let handle =
            adapter.start_cancellable(make_request(&format!("cosh-core-async-read-{label}")), mode);
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut observed_error = None;
        loop {
            assert!(Instant::now() < deadline, "async reader did not finish");
            match handle.poll_event_timeout(Duration::from_millis(100)) {
                Ok(AgentRunPoll::Event(_)) | Ok(AgentRunPoll::Timeout) => {}
                Ok(AgentRunPoll::Finished) => break,
                Err(error) => observed_error = Some(error),
            }
        }

        let error = observed_error.expect("stream read error");
        assert!(
            error.message.contains("failed to read cosh-core stream"),
            "{}",
            error.message
        );
        assert_recorded_process_is_gone(&pid_file);
        assert_eq!(
            adapter.recovery_snapshot().state,
            SessionRecoveryState::Failed
        );
        let _ = fs::remove_file(pid_file);
        let _ = fs::remove_file(script);
    }
}

#[test]
fn cosh_core_cancelled_non_resumable_restore_preserves_previous_active_session() {
    let script = mock_provider_script(
        "cosh-core-non-resumable-cancel",
        r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"11111111-1111-4111-8111-111111111111","session_resumable":false,"model":"mock","tools":[]}'
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"ready"}]}}'
exec sleep 30"#,
    );
    let adapter = cosh_core_restore_adapter(&script);
    let handle = adapter.start_cancellable(
        make_request("cosh-core-non-resumable-cancel"),
        CoshApprovalMode::Recommend,
    );

    let ready = collect_events_until(
        &handle,
        Duration::from_secs(3),
        |event| matches!(event, AgentEvent::TextDelta { text, .. } if text == "ready"),
    );
    assert!(
        ready
            .iter()
            .any(|event| matches!(event, AgentEvent::TextDelta { text, .. } if text == "ready")),
        "provider init was not observed before cancellation: {ready:?}"
    );

    handle.cancel();
    let _ = collect_events_until_finished(&handle, Duration::from_secs(3));

    assert_eq!(
        adapter.committed_session_id().as_deref(),
        Some("00000000-0000-4000-8000-000000000000")
    );
    let recovery = adapter.recovery_snapshot();
    assert_eq!(recovery.state, SessionRecoveryState::Failed);
    assert_eq!(recovery.selected_session_id, None);
    assert_eq!(recovery.selected_workspace_scope, None);
    let _ = fs::remove_file(script);
}
