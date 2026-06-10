use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cosh_shell::adapter::{
    AgentRunPoll, ApprovalDecision, ApprovalResponse, ClaudeCodeAdapter, CoshTuiAdapter,
    QwenCliAdapter,
};
use cosh_shell::types::{AgentEvent, AgentRequest, CoshApprovalMode};

fn mock_cli_path(name: &str) -> String {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .join("tests")
        .join(name)
        .to_string_lossy()
        .to_string()
}

fn make_request(id: &str) -> AgentRequest {
    use cosh_shell::types::*;
    AgentRequest {
        id: id.to_string(),
        session_id: "test-session".to_string(),
        command_block: CommandBlock {
            id: "test-block".to_string(),
            session_id: "test-session".to_string(),
            command: "echo test".to_string(),
            cwd: "/tmp".to_string(),
            end_cwd: "/tmp".to_string(),
            started_at_ms: 0,
            ended_at_ms: 0,
            duration_ms: 0,
            exit_code: 1,
            status: CommandStatus::Failed,
            output: OutputRefs {
                terminal_output_ref: None,
                terminal_output_bytes: 0,
            },
        },
        context_blocks: vec![],
        context_hints: vec![],
        user_input: Some("test the file".to_string()),
        findings: vec![],
        mode: AgentMode::RecommendOnly,
        user_confirmed: true,
        hook_finding: None,
        recommended_skill: None,
    }
}

fn collect_events_until(
    handle: &cosh_shell::adapter::AgentRunHandle,
    timeout: Duration,
    predicate: impl Fn(&AgentEvent) -> bool,
) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if std::time::Instant::now() > deadline {
            break;
        }
        match handle.poll_event_timeout(Duration::from_millis(100)) {
            Ok(AgentRunPoll::Event(event)) => {
                let done = predicate(&event);
                events.push(event);
                if done {
                    break;
                }
            }
            Ok(AgentRunPoll::Timeout) => continue,
            Ok(AgentRunPoll::Finished) => break,
            Err(_) => break,
        }
    }
    events
}

#[allow(clippy::field_reassign_with_default)]
fn make_adapter(mock_script: &str) -> ClaudeCodeAdapter {
    let mut adapter = ClaudeCodeAdapter::default();
    adapter.program = mock_cli_path(mock_script);
    adapter.model = "mock".to_string();
    adapter.max_budget_usd = "1".to_string();
    adapter.allow_model_call = true;
    adapter
}

fn make_qwen_adapter(mock_script: &str) -> QwenCliAdapter {
    QwenCliAdapter {
        program: mock_cli_path(mock_script),
        allow_model_call: true,
        session_id: Arc::default(),
    }
}

#[test]
fn control_protocol_allow_round_trip() {
    let adapter = make_adapter("mock_control_cli.sh");
    let request = make_request("test-allow");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Ask);

    let events = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });

    let tool_req = events
        .iter()
        .find(|e| matches!(e, AgentEvent::ToolPermissionRequest { .. }));
    assert!(
        tool_req.is_some(),
        "expected ToolPermissionRequest, got: {events:?}"
    );

    if let AgentEvent::ToolPermissionRequest {
        request_id,
        tool_name,
        tool_use_id,
        ..
    } = tool_req.unwrap()
    {
        assert_eq!(request_id, "mock-req-001");
        assert_eq!(tool_name, "Bash");
        assert_eq!(tool_use_id, "toolu_mock001");

        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(tool_use_id.clone()),
                decision: ApprovalDecision::Allow,
            })
            .expect("respond_approval should succeed");
    }

    let remaining = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(
            event,
            AgentEvent::AgentCompleted { .. } | AgentEvent::AgentFailed { .. }
        )
    });

    let completed = remaining
        .iter()
        .find(|e| matches!(e, AgentEvent::AgentCompleted { .. }));
    assert!(
        completed.is_some(),
        "expected AgentCompleted after Allow, got: {remaining:?}"
    );
}

#[test]
fn control_protocol_rejects_wrong_tool_use_id() {
    let adapter = make_adapter("mock_control_cli.sh");
    let request = make_request("test-wrong-tool-use-id");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Ask);

    let events = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });
    let tool_req = events
        .iter()
        .find(|event| matches!(event, AgentEvent::ToolPermissionRequest { .. }));
    assert!(
        tool_req.is_some(),
        "expected ToolPermissionRequest, got: {events:?}"
    );

    if let AgentEvent::ToolPermissionRequest { request_id, .. } = tool_req.unwrap() {
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(request_id.clone()),
                decision: ApprovalDecision::Allow,
            })
            .expect("respond_approval should succeed");
    }

    let remaining = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::AgentFailed { .. })
    });
    assert!(
        remaining.iter().any(|event| matches!(
            event,
            AgentEvent::AgentFailed { error, .. }
                if error.contains("wrong toolUseID in allow response")
        )),
        "expected AgentFailed for wrong toolUseID, got: {remaining:?}"
    );
}

#[test]
fn control_protocol_deny_round_trip() {
    let adapter = make_adapter("mock_control_cli.sh");
    let request = make_request("test-deny");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Ask);

    let events = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });

    let tool_req = events
        .iter()
        .find(|e| matches!(e, AgentEvent::ToolPermissionRequest { .. }));
    assert!(tool_req.is_some(), "expected ToolPermissionRequest");

    if let AgentEvent::ToolPermissionRequest { request_id, .. } = tool_req.unwrap() {
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: None,
                decision: ApprovalDecision::Deny {
                    message: "User denied".to_string(),
                },
            })
            .expect("respond_approval should succeed");
    }

    let remaining = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(
            event,
            AgentEvent::AgentCompleted { .. } | AgentEvent::AgentFailed { .. }
        )
    });

    let completed = remaining
        .iter()
        .find(|e| matches!(e, AgentEvent::AgentCompleted { .. }));
    assert!(
        completed.is_some(),
        "expected AgentCompleted after Deny, got: {remaining:?}"
    );
}

#[test]
fn control_protocol_multi_tool_approval() {
    let adapter = make_adapter("mock_control_cli_multi.sh");
    let request = make_request("test-multi");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Ask);

    // First tool request
    let events1 = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });
    let req1 = events1
        .iter()
        .find(|e| matches!(e, AgentEvent::ToolPermissionRequest { .. }));
    assert!(req1.is_some(), "expected first ToolPermissionRequest");
    if let AgentEvent::ToolPermissionRequest {
        request_id,
        tool_name,
        tool_use_id,
        ..
    } = req1.unwrap()
    {
        assert_eq!(tool_name, "Read");
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(tool_use_id.clone()),
                decision: ApprovalDecision::Allow,
            })
            .unwrap();
    }

    // Second tool request
    let events2 = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });
    let req2 = events2
        .iter()
        .find(|e| matches!(e, AgentEvent::ToolPermissionRequest { .. }));
    assert!(req2.is_some(), "expected second ToolPermissionRequest");
    if let AgentEvent::ToolPermissionRequest {
        request_id,
        tool_name,
        tool_use_id,
        ..
    } = req2.unwrap()
    {
        assert_eq!(tool_name, "Bash");
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(tool_use_id.clone()),
                decision: ApprovalDecision::Allow,
            })
            .unwrap();
    }

    // Completion
    let remaining = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::AgentCompleted { .. })
    });
    assert!(
        remaining
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentCompleted { .. })),
        "expected AgentCompleted after multi-tool approval"
    );
}

#[test]
fn control_protocol_session_id_captured() {
    let adapter = make_adapter("mock_control_cli.sh");
    let session_state = Arc::clone(&adapter.session_id);
    let request = make_request("test-session");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Ask);

    let events = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });

    let captured = session_state.lock().unwrap().clone();
    assert_eq!(
        captured.as_deref(),
        Some("mock-session-001"),
        "session_id should be captured from init message"
    );

    if let Some(AgentEvent::ToolPermissionRequest {
        request_id,
        tool_use_id,
        ..
    }) = events
        .iter()
        .find(|e| matches!(e, AgentEvent::ToolPermissionRequest { .. }))
    {
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(tool_use_id.clone()),
                decision: ApprovalDecision::Allow,
            })
            .unwrap();
    }
    let _ = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::AgentCompleted { .. })
    });
}

#[test]
fn qwen_control_protocol_allow_round_trip() {
    let adapter = make_qwen_adapter("mock_control_cli.sh");
    let request = make_request("qwen-test-allow");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Ask);

    let events = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });

    let tool_req = events
        .iter()
        .find(|event| matches!(event, AgentEvent::ToolPermissionRequest { .. }));
    assert!(
        tool_req.is_some(),
        "expected Qwen ToolPermissionRequest, got: {events:?}"
    );

    if let AgentEvent::ToolPermissionRequest {
        request_id,
        tool_name,
        tool_use_id,
        ..
    } = tool_req.unwrap()
    {
        assert_eq!(request_id, "mock-req-001");
        assert_eq!(tool_name, "Bash");
        assert_eq!(tool_use_id, "toolu_mock001");

        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(tool_use_id.clone()),
                decision: ApprovalDecision::Allow,
            })
            .expect("respond_approval should succeed");
    }

    let remaining = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(
            event,
            AgentEvent::AgentCompleted { .. } | AgentEvent::AgentFailed { .. }
        )
    });
    assert!(
        remaining
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })),
        "expected Qwen AgentCompleted after Allow, got: {remaining:?}"
    );
}

#[test]
fn qwen_control_protocol_write_file_allow_preserves_input() {
    let adapter = make_qwen_adapter("mock_control_cli_write_file.sh");
    let request = make_request("qwen-test-write-file");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Ask);

    let events = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });

    let tool_req = events
        .iter()
        .find(|event| matches!(event, AgentEvent::ToolPermissionRequest { .. }));
    assert!(
        tool_req.is_some(),
        "expected Qwen write_file ToolPermissionRequest, got: {events:?}"
    );

    if let AgentEvent::ToolPermissionRequest {
        request_id,
        tool_name,
        tool_input,
        tool_use_id,
        ..
    } = tool_req.unwrap()
    {
        assert_eq!(request_id, "mock-req-write");
        assert_eq!(tool_name, "write_file");
        assert_eq!(tool_input["file_path"], "/tmp/cosh-write.html");
        assert_eq!(tool_input["content"], "<html>ok</html>");
        assert_eq!(tool_use_id, "toolu_write001");

        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(tool_use_id.clone()),
                decision: ApprovalDecision::Allow,
            })
            .expect("respond_approval should succeed");
    }

    let remaining = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(
            event,
            AgentEvent::AgentCompleted { .. } | AgentEvent::AgentFailed { .. }
        )
    });
    assert!(
        remaining
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })),
        "expected Qwen AgentCompleted after write_file Allow, got: {remaining:?}"
    );
    assert!(
        !remaining.iter().any(|event| matches!(
            event,
            AgentEvent::AgentFailed { error, .. }
                if error.contains("write_file args were cleared")
        )),
        "write_file allow should not clear provider input, got: {remaining:?}"
    );
}

#[test]
fn qwen_control_protocol_rejects_wrong_tool_use_id() {
    let adapter = make_qwen_adapter("mock_control_cli.sh");
    let request = make_request("qwen-test-wrong-tool-use-id");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Ask);

    let events = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });
    let tool_req = events
        .iter()
        .find(|event| matches!(event, AgentEvent::ToolPermissionRequest { .. }));
    assert!(
        tool_req.is_some(),
        "expected Qwen ToolPermissionRequest, got: {events:?}"
    );

    if let AgentEvent::ToolPermissionRequest { request_id, .. } = tool_req.unwrap() {
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(request_id.clone()),
                decision: ApprovalDecision::Allow,
            })
            .expect("respond_approval should succeed");
    }

    let remaining = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::AgentFailed { .. })
    });
    assert!(
        remaining.iter().any(|event| matches!(
            event,
            AgentEvent::AgentFailed { error, .. }
                if error.contains("wrong toolUseID in allow response")
        )),
        "expected Qwen AgentFailed for wrong toolUseID, got: {remaining:?}"
    );
}

#[test]
fn qwen_control_protocol_deny_round_trip() {
    let adapter = make_qwen_adapter("mock_control_cli.sh");
    let request = make_request("qwen-test-deny");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Ask);

    let events = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });

    let tool_req = events
        .iter()
        .find(|event| matches!(event, AgentEvent::ToolPermissionRequest { .. }));
    assert!(tool_req.is_some(), "expected Qwen ToolPermissionRequest");

    if let AgentEvent::ToolPermissionRequest { request_id, .. } = tool_req.unwrap() {
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: None,
                decision: ApprovalDecision::Deny {
                    message: "User denied".to_string(),
                },
            })
            .expect("respond_approval should succeed");
    }

    let remaining = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(
            event,
            AgentEvent::AgentCompleted { .. } | AgentEvent::AgentFailed { .. }
        )
    });
    assert!(
        remaining
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })),
        "expected Qwen AgentCompleted after Deny, got: {remaining:?}"
    );
}

#[test]
fn qwen_control_protocol_multi_tool_approval() {
    let adapter = make_qwen_adapter("mock_control_cli_multi.sh");
    let request = make_request("qwen-test-multi");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Ask);

    let events1 = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });
    let req1 = events1
        .iter()
        .find(|event| matches!(event, AgentEvent::ToolPermissionRequest { .. }));
    assert!(req1.is_some(), "expected first Qwen ToolPermissionRequest");
    if let AgentEvent::ToolPermissionRequest {
        request_id,
        tool_name,
        tool_use_id,
        ..
    } = req1.unwrap()
    {
        assert_eq!(tool_name, "Read");
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(tool_use_id.clone()),
                decision: ApprovalDecision::Allow,
            })
            .unwrap();
    }

    let events2 = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });
    let req2 = events2
        .iter()
        .find(|event| matches!(event, AgentEvent::ToolPermissionRequest { .. }));
    assert!(req2.is_some(), "expected second Qwen ToolPermissionRequest");
    if let AgentEvent::ToolPermissionRequest {
        request_id,
        tool_name,
        tool_use_id,
        ..
    } = req2.unwrap()
    {
        assert_eq!(tool_name, "Bash");
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(tool_use_id.clone()),
                decision: ApprovalDecision::Allow,
            })
            .unwrap();
    }

    let remaining = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::AgentCompleted { .. })
    });
    assert!(
        remaining
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })),
        "expected Qwen AgentCompleted after multi-tool approval"
    );
}

#[test]
fn qwen_control_protocol_session_id_captured() {
    let adapter = make_qwen_adapter("mock_control_cli.sh");
    let session_state = Arc::clone(&adapter.session_id);
    let request = make_request("qwen-test-session");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Ask);

    let events = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });

    let captured = session_state.lock().unwrap().clone();
    assert_eq!(
        captured.as_deref(),
        Some("mock-session-001"),
        "Qwen session_id should be captured from init message"
    );

    if let Some(AgentEvent::ToolPermissionRequest {
        request_id,
        tool_use_id,
        ..
    }) = events
        .iter()
        .find(|event| matches!(event, AgentEvent::ToolPermissionRequest { .. }))
    {
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(tool_use_id.clone()),
                decision: ApprovalDecision::Allow,
            })
            .unwrap();
    }
    let _ = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::AgentCompleted { .. })
    });
}

#[allow(clippy::field_reassign_with_default)]
fn make_cosh_tui_adapter(mock_script: &str) -> CoshTuiAdapter {
    let mut adapter = CoshTuiAdapter::default();
    adapter.program = mock_cli_path(mock_script);
    adapter
}

#[test]
fn cosh_tui_control_protocol_allow_round_trip() {
    let adapter = make_cosh_tui_adapter("mock_control_cli.sh");
    let request = make_request("tui-test-allow");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Ask);

    let events = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });

    let tool_req = events
        .iter()
        .find(|event| matches!(event, AgentEvent::ToolPermissionRequest { .. }));
    assert!(
        tool_req.is_some(),
        "expected CoshTui ToolPermissionRequest, got: {events:?}"
    );

    if let AgentEvent::ToolPermissionRequest {
        request_id,
        tool_name,
        tool_use_id,
        ..
    } = tool_req.unwrap()
    {
        assert_eq!(request_id, "mock-req-001");
        assert_eq!(tool_name, "Bash");
        assert_eq!(tool_use_id, "toolu_mock001");

        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(tool_use_id.clone()),
                decision: ApprovalDecision::Allow,
            })
            .expect("respond_approval should succeed");
    }

    let remaining = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(
            event,
            AgentEvent::AgentCompleted { .. } | AgentEvent::AgentFailed { .. }
        )
    });
    assert!(
        remaining
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })),
        "expected CoshTui AgentCompleted after Allow, got: {remaining:?}"
    );
}

#[test]
fn cosh_tui_control_protocol_deny_round_trip() {
    let adapter = make_cosh_tui_adapter("mock_control_cli.sh");
    let request = make_request("tui-test-deny");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Ask);

    let events = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });

    let tool_req = events
        .iter()
        .find(|event| matches!(event, AgentEvent::ToolPermissionRequest { .. }));
    assert!(tool_req.is_some(), "expected CoshTui ToolPermissionRequest");

    if let AgentEvent::ToolPermissionRequest { request_id, .. } = tool_req.unwrap() {
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: None,
                decision: ApprovalDecision::Deny {
                    message: "User denied".to_string(),
                },
            })
            .expect("respond_approval should succeed");
    }

    let remaining = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(
            event,
            AgentEvent::AgentCompleted { .. } | AgentEvent::AgentFailed { .. }
        )
    });
    assert!(
        remaining
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })),
        "expected CoshTui AgentCompleted after Deny, got: {remaining:?}"
    );
}

#[test]
fn cosh_tui_control_protocol_session_id_captured() {
    let adapter = make_cosh_tui_adapter("mock_control_cli.sh");
    let session_state = Arc::clone(&adapter.session_id);
    let request = make_request("tui-test-session");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Ask);

    let events = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });

    let captured = session_state.lock().unwrap().clone();
    assert_eq!(
        captured.as_deref(),
        Some("mock-session-001"),
        "CoshTui session_id should be captured from init message"
    );

    if let Some(AgentEvent::ToolPermissionRequest {
        request_id,
        tool_use_id,
        ..
    }) = events
        .iter()
        .find(|event| matches!(event, AgentEvent::ToolPermissionRequest { .. }))
    {
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(tool_use_id.clone()),
                decision: ApprovalDecision::Allow,
            })
            .unwrap();
    }
    let _ = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::AgentCompleted { .. })
    });
}

#[test]
fn cosh_tui_control_protocol_multi_tool_approval() {
    let adapter = make_cosh_tui_adapter("mock_control_cli_multi.sh");
    let request = make_request("tui-test-multi");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Ask);

    let events1 = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });
    let req1 = events1
        .iter()
        .find(|event| matches!(event, AgentEvent::ToolPermissionRequest { .. }));
    assert!(
        req1.is_some(),
        "expected first CoshTui ToolPermissionRequest"
    );
    if let AgentEvent::ToolPermissionRequest {
        request_id,
        tool_name,
        tool_use_id,
        ..
    } = req1.unwrap()
    {
        assert_eq!(tool_name, "Read");
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(tool_use_id.clone()),
                decision: ApprovalDecision::Allow,
            })
            .unwrap();
    }

    let events2 = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });
    let req2 = events2
        .iter()
        .find(|event| matches!(event, AgentEvent::ToolPermissionRequest { .. }));
    assert!(
        req2.is_some(),
        "expected second CoshTui ToolPermissionRequest"
    );
    if let AgentEvent::ToolPermissionRequest {
        request_id,
        tool_name,
        tool_use_id,
        ..
    } = req2.unwrap()
    {
        assert_eq!(tool_name, "Bash");
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(tool_use_id.clone()),
                decision: ApprovalDecision::Allow,
            })
            .unwrap();
    }

    let remaining = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::AgentCompleted { .. })
    });
    assert!(
        remaining
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })),
        "expected CoshTui AgentCompleted after multi-tool approval"
    );
}

#[test]
fn cosh_tui_persistent_two_sequential_runs() {
    let adapter = make_cosh_tui_adapter("mock_control_cli_persistent.sh");

    let request1 = make_request("tui-persistent-run-1");
    let handle1 = adapter.start_cancellable(request1, CoshApprovalMode::Trust);
    let events1 = collect_events_until(&handle1, Duration::from_secs(5), |event| {
        matches!(
            event,
            AgentEvent::AgentCompleted { .. } | AgentEvent::AgentFailed { .. }
        )
    });
    assert!(
        events1
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentCompleted { .. })),
        "first run should complete, got: {events1:?}"
    );
    drop(handle1);

    std::thread::sleep(Duration::from_millis(50));

    let request2 = make_request("tui-persistent-run-2");
    let handle2 = adapter.start_cancellable(request2, CoshApprovalMode::Trust);
    let events2 = collect_events_until(&handle2, Duration::from_secs(5), |event| {
        matches!(
            event,
            AgentEvent::AgentCompleted { .. } | AgentEvent::AgentFailed { .. }
        )
    });
    assert!(
        events2
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentCompleted { .. })),
        "second run should complete via persistent process, got: {events2:?}"
    );
}

#[test]
fn respond_approval_fails_without_sender() {
    let fake = cosh_shell::adapter::FakeAgentAdapter;
    let request = make_request("test-fake");
    let handle = cosh_shell::adapter::AdapterInstance::Fake(fake)
        .start_cancellable(request, CoshApprovalMode::Ask);

    let result = handle.respond_approval(ApprovalResponse {
        request_id: "test".to_string(),
        tool_use_id: Some("toolu_test".to_string()),
        decision: ApprovalDecision::Allow,
    });
    assert!(
        result.is_err(),
        "respond_approval should fail for FakeAdapter"
    );

    use cosh_shell::adapter::AgentAdapter;
    let fake = cosh_shell::adapter::FakeAgentAdapter;
    assert!(!fake.capabilities().control_protocol);
}
