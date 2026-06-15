use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cosh_shell::adapter::{
    ApprovalDecision, ApprovalResponse, FakeAgentAdapter, HostExecutedShellMetadata,
    HostExecutedShellResult,
};
use cosh_shell::types::{AgentEvent, CoshApprovalMode};
use cosh_shell::{AdapterInstance, AgentAdapter};

#[path = "support/control_protocol.rs"]
mod support_control_protocol;

use support_control_protocol::{
    collect_events_until, make_adapter, make_cosh_tui_adapter, make_qwen_adapter, make_request,
};

fn wait_for_session_id(
    session_state: &Arc<Mutex<Option<String>>>,
    expected: &str,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if session_state.lock().unwrap().as_deref() == Some(expected) {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn control_protocol_allow_round_trip() {
    let adapter = make_adapter("mock_control_cli.sh");
    let request = make_request("test-allow");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Auto);

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
        tool_input,
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
                tool_input: Some(tool_input.clone()),
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
fn control_protocol_claude_allow_uses_tool_input() {
    let adapter = make_adapter("mock_control_cli.sh");
    let request = make_request("test-claude-updated-input");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Auto);

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

    if let AgentEvent::ToolPermissionRequest {
        request_id,
        tool_input,
        ..
    } = tool_req.unwrap()
    {
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some("intentionally-ignored-by-claude".to_string()),
                tool_input: Some(tool_input.clone()),
                decision: ApprovalDecision::Allow,
            })
            .expect("respond_approval should succeed");
    }

    let remaining = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::AgentCompleted { .. })
    });
    assert!(
        remaining
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })),
        "expected AgentCompleted when updatedInput is present, got: {remaining:?}"
    );
}

#[test]
fn control_protocol_deny_round_trip() {
    let adapter = make_adapter("mock_control_cli.sh");
    let request = make_request("test-deny");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Auto);

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
                tool_input: None,
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
fn control_protocol_cancel_while_waiting_for_approval_finishes() {
    let adapter = make_adapter("mock_control_cli.sh");
    let request = make_request("test-cancel-while-waiting");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Auto);

    let events = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::ToolPermissionRequest { .. })),
        "expected ToolPermissionRequest before cancel, got: {events:?}"
    );

    handle.cancel();

    let remaining = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::AgentCancelled { .. })
    });
    assert!(
        remaining
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCancelled { .. })),
        "expected AgentCancelled after pending approval cancel, got: {remaining:?}"
    );
}

#[test]
fn control_protocol_multi_tool_approval() {
    let adapter = make_adapter("mock_control_cli_multi.sh");
    let request = make_request("test-multi");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Auto);

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
        tool_input,
        tool_use_id,
        ..
    } = req1.unwrap()
    {
        assert_eq!(tool_name, "Read");
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(tool_use_id.clone()),
                tool_input: Some(tool_input.clone()),
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
        tool_input,
        tool_use_id,
        ..
    } = req2.unwrap()
    {
        assert_eq!(tool_name, "Bash");
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(tool_use_id.clone()),
                tool_input: Some(tool_input.clone()),
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
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Auto);

    let events = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });

    if let Some(AgentEvent::ToolPermissionRequest {
        request_id,
        tool_input,
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
                tool_input: Some(tool_input.clone()),
                decision: ApprovalDecision::Allow,
            })
            .unwrap();
    }
    let _ = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::AgentCompleted { .. })
    });
    let captured = session_state.lock().unwrap().clone();
    assert_eq!(
        captured.as_deref(),
        Some("mock-session-001"),
        "session_id should be committed after completion"
    );
}

#[test]
fn qwen_recommend_uses_stream_prompt_with_closed_stdin() {
    let adapter = make_qwen_adapter("mock_qwen_stream_cli.sh");
    let session_state = Arc::clone(&adapter.session_id);
    let request = make_request("qwen-test-stream");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Recommend);

    let events = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(
            event,
            AgentEvent::AgentCompleted { .. } | AgentEvent::AgentFailed { .. }
        )
    });

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::TextDelta { text, .. } if text.contains("qwen stream completed")
        ) || matches!(
            event,
            AgentEvent::AgentCompleted { summary, .. } if summary.contains("co analysis completed")
        )),
        "expected qwen stream completion, got: {events:?}"
    );
    assert!(
        !events.iter().any(|event| matches!(
            event,
            AgentEvent::AgentFailed { error, .. } if error.contains("stdin was not closed")
        )),
        "qwen stream should not inherit raw shell stdin, got: {events:?}"
    );
    assert!(
        wait_for_session_id(&session_state, "mock-qwen-stream", Duration::from_secs(1)),
        "session_id should be committed after qwen stream completion"
    );
    assert!(
        handle
            .respond_approval(ApprovalResponse {
                request_id: "unused".to_string(),
                tool_use_id: None,
                tool_input: None,
                decision: ApprovalDecision::Deny {
                    message: "unused".to_string(),
                },
            })
            .is_err(),
        "qwen stream mode should not expose a control approval channel"
    );
}

#[test]
fn qwen_auto_uses_control_approval_channel() {
    let adapter = make_qwen_adapter("mock_qwen_control_cli.sh");
    let session_state = Arc::clone(&adapter.session_id);
    let request = make_request("qwen-test-auto-stream");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Auto);

    let events = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });
    let tool_req = events
        .iter()
        .find(|event| matches!(event, AgentEvent::ToolPermissionRequest { .. }));
    assert!(tool_req.is_some(), "expected qwen ToolPermissionRequest");
    assert_eq!(
        session_state.lock().unwrap().as_deref(),
        None,
        "session id must not be committed while provider is stopped at a permission request"
    );
    if let AgentEvent::ToolPermissionRequest {
        request_id,
        tool_use_id,
        ..
    } = tool_req.unwrap()
    {
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(tool_use_id.clone()),
                tool_input: None,
                decision: ApprovalDecision::Allow,
            })
            .expect("qwen approval response should succeed");
    }

    let remaining = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::AgentCompleted { .. })
    });
    assert!(
        remaining
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })),
        "expected qwen control completion, got: {remaining:?}"
    );
    assert!(
        wait_for_session_id(&session_state, "mock-qwen-control", Duration::from_secs(1)),
        "session_id should be committed after qwen control completion"
    );
}

#[test]
fn cosh_tui_auto_uses_control_approval_channel() {
    let adapter = make_cosh_tui_adapter("mock_qwen_control_cli.sh");
    let session_state = Arc::clone(&adapter.session_id);
    let request = make_request("cosh-tui-test-auto-stream");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Auto);

    let events = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });
    let tool_req = events
        .iter()
        .find(|event| matches!(event, AgentEvent::ToolPermissionRequest { .. }));
    assert!(
        tool_req.is_some(),
        "expected cosh-tui ToolPermissionRequest"
    );

    if let AgentEvent::ToolPermissionRequest {
        request_id,
        tool_use_id,
        ..
    } = tool_req.unwrap()
    {
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(tool_use_id.clone()),
                tool_input: None,
                decision: ApprovalDecision::Allow,
            })
            .expect("cosh-tui approval response should succeed");
    }

    let remaining = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::AgentCompleted { .. })
    });
    assert!(
        remaining
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })),
        "expected cosh-tui control completion, got: {remaining:?}"
    );
    assert!(
        wait_for_session_id(&session_state, "mock-qwen-control", Duration::from_secs(1)),
        "session_id should be committed after cosh-tui control completion"
    );
}

#[test]
fn cosh_tui_host_executed_shell_result_uses_control_response() {
    let adapter = make_cosh_tui_adapter("mock_cosh_tui_host_executed_cli.sh");
    let session_state = Arc::clone(&adapter.session_id);
    let request = make_request("cosh-tui-test-host-executed");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Auto);

    let events = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });
    let tool_req = events
        .iter()
        .find(|event| matches!(event, AgentEvent::ToolPermissionRequest { .. }))
        .expect("expected cosh-tui ToolPermissionRequest");

    if let AgentEvent::ToolPermissionRequest {
        request_id,
        tool_use_id,
        ..
    } = tool_req
    {
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(tool_use_id.clone()),
                tool_input: None,
                decision: ApprovalDecision::HostExecutedShell {
                    result: Box::new(HostExecutedShellResult {
                        llm_content:
                            "ShellCommandCompleted evidence\ncommand: df -h\nstatus: completed"
                                .to_string(),
                        return_display: Some("df -h completed".to_string()),
                        metadata: HostExecutedShellMetadata {
                            command: "df -h".to_string(),
                            status: "completed".to_string(),
                            exit_code: 0,
                            signal: None,
                            cwd: "/tmp".to_string(),
                            end_cwd: "/tmp".to_string(),
                            duration_ms: 12,
                            output_ref: Some("terminal-output://test/cmd-1".to_string()),
                            redaction_status: "bounded".to_string(),
                            approval_id: Some("approval-1".to_string()),
                            tool_use_id: Some(tool_use_id.clone()),
                        },
                    }),
                },
            })
            .expect("cosh-tui host-executed approval response should succeed");
    }

    let remaining = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::AgentCompleted { .. })
    });
    assert!(
        remaining
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })),
        "expected cosh-tui host-executed completion, got: {remaining:?}"
    );
    assert!(
        wait_for_session_id(
            &session_state,
            "mock-cosh-tui-host-executed",
            Duration::from_secs(1)
        ),
        "session_id should be committed after cosh-tui host-executed completion"
    );
    let capabilities = handle.control_capabilities();
    assert!(capabilities.provider_initialize_seen);
    assert!(capabilities.can_handle_can_use_tool);
    assert!(capabilities.can_handle_host_executed_shell_tool_result);
}

#[test]
fn cosh_tui_multi_host_executed_shell_results_stay_in_same_control_turn() {
    let adapter = make_cosh_tui_adapter("mock_cosh_tui_host_executed_multi_cli.sh");
    let session_state = Arc::clone(&adapter.session_id);
    let request = make_request("cosh-tui-test-host-executed-multi");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Auto);

    let first = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });
    respond_host_executed(&handle, &first, "df -h", "completed");

    let second = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::ToolPermissionRequest { .. })
    });
    assert!(
        second.iter().any(|event| matches!(
            event,
            AgentEvent::TextDelta { text, .. } if text.contains("First host executed result received.")
        )),
        "expected provider to continue same turn after first host result, got: {second:?}"
    );
    respond_host_executed(&handle, &second, "du -sh .", "completed");

    let remaining = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::AgentCompleted { .. })
    });
    assert!(
        remaining.iter().any(|event| matches!(
            event,
            AgentEvent::TextDelta { text, .. } if text.contains("Second host executed result received.")
        )),
        "expected provider to continue after second host result, got: {remaining:?}"
    );
    assert!(
        remaining
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })),
        "expected cosh-tui multi host-executed completion, got: {remaining:?}"
    );
    assert!(
        wait_for_session_id(
            &session_state,
            "mock-cosh-tui-host-executed-multi",
            Duration::from_secs(1)
        ),
        "session_id should be committed after multi host-executed completion"
    );
}

#[test]
fn cosh_tui_analysis_continuation_denies_reentrant_shell_request() {
    let adapter = make_cosh_tui_adapter("mock_cosh_tui_analysis_continuation_shell_request.sh");
    let session_state = Arc::clone(&adapter.session_id);
    let mut request = make_request("cosh-tui-analysis-continuation-deny");
    request.user_input =
        Some("ShellCommandCompleted evidence\ncommand: df -h\nstatus: completed".to_string());
    request
        .context_hints
        .push("analysis-only continuation after foreground shell handoff".to_string());
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Auto);

    let events = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(
            event,
            AgentEvent::AgentCompleted { .. } | AgentEvent::AgentFailed { .. }
        )
    });
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::TextDelta { text, .. }
                if text.contains("Cosh-tui analysis continuation shell request was denied.")
        )),
        "expected cosh-tui provider to receive deny response, got: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })),
        "expected cosh-tui analysis continuation completion, got: {events:?}"
    );
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, AgentEvent::ToolPermissionRequest { .. })),
        "analysis-only shell request should be denied inside adapter, got: {events:?}"
    );
    assert!(
        wait_for_session_id(
            &session_state,
            "mock-cosh-tui-analysis-continuation",
            Duration::from_secs(1)
        ),
        "session_id should be committed after cosh-tui analysis continuation completion"
    );
}

fn respond_host_executed(
    handle: &cosh_shell::adapter::AgentRunHandle,
    events: &[AgentEvent],
    command: &str,
    status: &str,
) {
    let tool_req = events
        .iter()
        .find(|event| matches!(event, AgentEvent::ToolPermissionRequest { .. }))
        .expect("expected ToolPermissionRequest");

    if let AgentEvent::ToolPermissionRequest {
        request_id,
        tool_use_id,
        ..
    } = tool_req
    {
        handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: Some(tool_use_id.clone()),
                tool_input: None,
                decision: ApprovalDecision::HostExecutedShell {
                    result: Box::new(HostExecutedShellResult {
                        llm_content: format!(
                            "ShellCommandCompleted evidence\ncommand: {command}\nstatus: {status}"
                        ),
                        return_display: Some(format!("{command} {status}")),
                        metadata: HostExecutedShellMetadata {
                            command: command.to_string(),
                            status: status.to_string(),
                            exit_code: 0,
                            signal: None,
                            cwd: "/tmp".to_string(),
                            end_cwd: "/tmp".to_string(),
                            duration_ms: 12,
                            output_ref: Some("terminal-output://test/cmd-1".to_string()),
                            redaction_status: "bounded".to_string(),
                            approval_id: Some("approval-1".to_string()),
                            tool_use_id: Some(tool_use_id.clone()),
                        },
                    }),
                },
            })
            .expect("host-executed approval response should succeed");
    }
}

#[test]
fn qwen_control_protocol_records_initialize_capabilities() {
    let adapter = make_qwen_adapter("mock_qwen_control_capabilities.sh");
    let request = make_request("qwen-test-capabilities");
    let handle = adapter.start_cancellable(request, CoshApprovalMode::Auto);

    let events = collect_events_until(&handle, Duration::from_secs(5), |event| {
        matches!(event, AgentEvent::AgentCompleted { .. })
    });

    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentCompleted { .. })),
        "expected qwen control completion, got: {events:?}"
    );
    let capabilities = handle.control_capabilities();
    assert!(capabilities.provider_initialize_seen);
    assert!(capabilities.can_handle_can_use_tool);
    assert!(capabilities.can_handle_host_executed_shell_tool_result);
}

#[test]
fn respond_approval_fails_without_sender() {
    let fake = FakeAgentAdapter;
    let request = make_request("test-fake");
    let handle = AdapterInstance::Fake(fake).start_cancellable(request, CoshApprovalMode::Auto);

    let result = handle.respond_approval(ApprovalResponse {
        request_id: "test".to_string(),
        tool_use_id: Some("toolu_test".to_string()),
        tool_input: None,
        decision: ApprovalDecision::Allow,
    });
    assert!(
        result.is_err(),
        "respond_approval should fail for FakeAdapter"
    );

    let fake = FakeAgentAdapter;
    assert!(!fake.capabilities().control_protocol);
}
