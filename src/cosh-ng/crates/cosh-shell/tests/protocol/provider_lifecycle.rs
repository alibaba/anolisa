use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cosh_shell::adapter::{AgentRunHandle, AgentRunPoll, ClaudeCodeAdapter, QwenCliAdapter};
use cosh_shell::types::{
    AgentEvent, AgentMode, AgentRequest, CommandBlock, CommandStatus, CoshApprovalMode, OutputRefs,
};

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

fn make_request(id: &str) -> AgentRequest {
    AgentRequest {
        id: id.to_string(),
        session_id: "session-1".to_string(),
        command_block: CommandBlock {
            id: "cmd-1".to_string(),
            session_id: "session-1".to_string(),
            command: "echo test".to_string(),
            origin: Default::default(),
            cwd: "/tmp".to_string(),
            end_cwd: "/tmp".to_string(),
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
