use std::time::Duration;

#[test]
fn mock_cosh_tui_ask_user_roundtrip() {
    let mock_script = r#"#!/usr/bin/env python3
import sys, json

def emit(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()

def read_line():
    line = sys.stdin.readline()
    if not line:
        return None
    return json.loads(line.strip())

# Wait for initialize
msg = read_line()
assert msg and msg.get("type") == "control_request", f"expected initialize, got {msg}"

# Send system init response
emit({"type": "system", "subtype": "init", "session_id": "mock-sess", "model": "mock-model", "tools": ["ask_user_question"]})

# Wait for user message
msg = read_line()
assert msg and msg.get("type") == "user", f"expected user message, got {msg}"

# Simulate stream: tool_use for ask_user_question
emit({"type": "stream_event", "event": {"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "call_ask_1", "name": "ask_user_question"}}})
emit({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"question\":\"Which color?\",\"options\":[{\"label\":\"Red\"},{\"label\":\"Blue\"}],\"allow_free_text\":true}"}}})
emit({"type": "stream_event", "event": {"type": "content_block_stop", "index": 0}})
emit({"type": "stream_event", "event": {"type": "message_stop"}})

# Simulate assistant snapshot (with tool_use block)
emit({"type": "assistant", "session_id": "mock-sess", "message": {"id": "msg1", "type": "message", "role": "assistant", "model": "mock-model", "content": [{"type": "tool_use", "id": "call_ask_1", "name": "ask_user_question", "input": {"question": "Which color?", "options": [{"label": "Red"}, {"label": "Blue"}], "allow_free_text": True}}]}})

# Emit the AskUser control_request
emit({"type": "control_request", "request_id": "req-0", "request": {"subtype": "ask_user", "question": "Which color?", "options": [{"label": "Red"}, {"label": "Blue"}], "allow_free_text": True, "multi_select": False}})

# Wait for the answer
while True:
    msg = read_line()
    if msg is None:
        emit({"type": "result", "is_error": True, "errors": ["stdin closed before answer"]})
        sys.exit(1)
    if msg.get("type") == "control_response":
        answer = msg.get("response", {}).get("response", {}).get("answer", "")
        break

# Emit result with the answer
emit({"type": "stream_event", "event": {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}})
emit({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": f"You chose: {answer}"}}})
emit({"type": "stream_event", "event": {"type": "content_block_stop", "index": 0}})
emit({"type": "stream_event", "event": {"type": "message_stop"}})
emit({"type": "result", "subtype": "success", "is_error": False, "result": f"You chose: {answer}", "session_id": "mock-sess", "duration_ms": 100})
"#;

    let script_path =
        std::env::temp_dir().join(format!("mock_cosh_tui_{}.py", std::process::id()));
    std::fs::write(&script_path, mock_script).expect("write mock script");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod");
    }

    std::env::set_var("COSH_TUI_PATH", script_path.to_str().unwrap());

    let adapter = cosh_shell::adapter::CoshTuiAdapter::default();

    let request = cosh_shell::AgentRequest {
        id: "test-ask".to_string(),
        session_id: "sess".to_string(),
        command_block: cosh_shell::CommandBlock {
            id: "blk".to_string(),
            session_id: "sess".to_string(),
            command: "echo test".to_string(),
            cwd: "/tmp".to_string(),
            end_cwd: "/tmp".to_string(),
            started_at_ms: 0,
            ended_at_ms: 0,
            duration_ms: 0,
            exit_code: 1,
            status: cosh_shell::CommandStatus::Failed,
            output: cosh_shell::OutputRefs {
                terminal_output_ref: None,
                terminal_output_bytes: 0,
            },
        },
        context_blocks: vec![],
        context_hints: vec![],
        user_input: Some("test ask user question".to_string()),
        findings: vec![],
        mode: cosh_shell::AgentMode::RecommendOnly,
        user_confirmed: true,
        hook_finding: None,
        recommended_skill: None,
    };

    let handle = adapter.start_cancellable(request, cosh_shell::CoshApprovalMode::Ask);

    let mut events = Vec::new();
    let mut saw_question = false;
    let mut saw_completed = false;
    let mut question_request_id = None;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);

    loop {
        if std::time::Instant::now() > deadline {
            panic!(
                "timeout waiting for events. collected {} events: {:#?}",
                events.len(),
                events
            );
        }

        match handle.poll_event_timeout(Duration::from_millis(100)) {
            Ok(cosh_shell::AgentRunPoll::Event(event)) => {
                eprintln!("[test] event: {event:?}");
                match &event {
                    cosh_shell::AgentEvent::UserQuestion {
                        question,
                        options,
                        request_id,
                        ..
                    } => {
                        assert_eq!(question, "Which color?");
                        assert_eq!(options, &["Red".to_string(), "Blue".to_string()]);
                        saw_question = true;
                        question_request_id = request_id.clone();

                        if let Some(req_id) = &question_request_id {
                            let response = cosh_shell::QuestionResponse {
                                request_id: req_id.clone(),
                                answer: "Red".to_string(),
                            };
                            handle
                                .respond_question(response)
                                .expect("respond_question should succeed");
                        } else {
                            panic!("UserQuestion has no request_id");
                        }
                    }
                    cosh_shell::AgentEvent::AgentCompleted { summary, .. } => {
                        saw_completed = true;
                        eprintln!("[test] completed: {summary}");
                    }
                    cosh_shell::AgentEvent::AgentFailed { error, .. } => {
                        panic!("agent failed: {error}");
                    }
                    _ => {}
                }
                events.push(event);
                if saw_completed {
                    break;
                }
            }
            Ok(cosh_shell::AgentRunPoll::Timeout) => continue,
            Ok(cosh_shell::AgentRunPoll::Finished) => {
                eprintln!("[test] finished (channel closed)");
                break;
            }
            Err(err) => panic!("poll error: {err:?}"),
        }
    }

    assert!(saw_question, "should have received UserQuestion event");
    assert!(
        question_request_id.is_some(),
        "UserQuestion should have request_id"
    );
    assert!(
        saw_completed,
        "should have received AgentCompleted after answering"
    );

    let has_answer_text = events.iter().any(|e| {
        matches!(e, cosh_shell::AgentEvent::TextDelta { text, .. } if text.contains("You chose: Red"))
    });
    assert!(
        has_answer_text,
        "should have received text delta with the answer"
    );
}
