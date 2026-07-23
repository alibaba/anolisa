use super::*;
use crate::provider::mock::MockProvider;
use crate::tool::{Tool, ToolResult};
use async_trait::async_trait;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::io::BufReader;

async fn empty_reader() -> tokio::io::Lines<BufReader<&'static [u8]>> {
    BufReader::new(&b""[..]).lines()
}

fn make_core(provider: MockProvider) -> CoshCore {
    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    let tools = ToolRegistry::new();
    CoshCore::new(config, Box::new(provider), tools)
}

struct CountingShellTool {
    calls: Arc<AtomicUsize>,
}

#[test]
fn allowlisted_tools_bypass_strict_approval() {
    let mut config = CoreConfig::default();
    config.agent.approval_mode = "strict".to_string();
    config.agent.allowed_tools.insert("shell".to_string());
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(CountingShellTool {
        calls: Arc::new(AtomicUsize::new(0)),
    }));
    let core = CoshCore::new(config, Box::new(MockProvider::new(Vec::new())), tools);

    assert_eq!(
        core.classify_tool("shell", &serde_json::json!({})),
        Outcome::Allow
    );
}

#[test]
fn mcp_tools_require_approval_outside_trust_mode() {
    for mode in ["auto", "balanced", "suggest", "strict"] {
        let mut config = CoreConfig::default();
        config.agent.approval_mode = mode.to_string();
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(TestMcpTool));
        let core = CoshCore::new(config, Box::new(MockProvider::new(Vec::new())), tools);

        assert_eq!(
            core.classify_tool("mcp__remote__search", &serde_json::json!({})),
            Outcome::RequireApproval,
            "MCP tool should require approval in {mode} mode"
        );
    }
}

#[test]
fn exact_mcp_allowlist_entry_bypasses_approval() {
    let mut config = CoreConfig::default();
    config.agent.approval_mode = "strict".to_string();
    config
        .agent
        .allowed_tools
        .insert("mcp__remote__search".to_string());
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(TestMcpTool));
    let core = CoshCore::new(config, Box::new(MockProvider::new(Vec::new())), tools);

    assert_eq!(
        core.classify_tool("mcp__remote__search", &serde_json::json!({})),
        Outcome::Allow
    );
}

#[async_trait]
impl Tool for CountingShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "counting shell"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" }
            },
            "required": ["command"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::ShellExec
    }

    async fn invoke(
        &self,
        _params: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::success("provider-native shell executed"))
    }
}

struct TestMcpTool;

#[async_trait]
impl Tool for TestMcpTool {
    fn name(&self) -> &str {
        "mcp__remote__search"
    }

    fn description(&self) -> &str {
        "test MCP tool"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Mcp
    }

    async fn invoke(
        &self,
        _params: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, String> {
        Ok(ToolResult::success("called"))
    }
}

struct CountingMcpTool {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for CountingMcpTool {
    fn name(&self) -> &str {
        "mcp__remote__search"
    }

    fn description(&self) -> &str {
        "counting MCP tool"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Mcp
    }

    async fn invoke(
        &self,
        _params: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::success("called"))
    }
}

fn mcp_tool_provider() -> MockProvider {
    MockProvider::new(vec![
        vec![
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-1".to_string(),
                name: "mcp__remote__search".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta: "{}".to_string(),
            },
            GenerateEvent::ToolCallEnd { index: 0 },
            GenerateEvent::MessageEnd,
        ],
        vec![
            GenerateEvent::TextDelta("Done.".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ])
}

#[tokio::test]
async fn mcp_tools_do_not_execute_before_approval() {
    for mode in ["auto", "balanced", "suggest", "strict"] {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut config = CoreConfig::default();
        config.agent.approval_mode = mode.to_string();
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(CountingMcpTool {
            calls: Arc::clone(&calls),
        }));
        let mut core = CoshCore::new(config, Box::new(mcp_tool_provider()), tools);
        let deny = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-0","response":{"behavior":"deny"}}}"#;
        let mut reader = BufReader::new(deny.as_bytes()).lines();
        let mut output = Vec::new();

        core.handle_user_message("search", &mut reader, &mut output)
            .await
            .unwrap();

        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "MCP tool ran in {mode} mode"
        );
        assert!(String::from_utf8(output).unwrap().contains("can_use_tool"));
    }
}

#[tokio::test]
async fn text_only_response() {
    let provider = MockProvider::text_only("Hello from AI!");
    let mut core = make_core(provider);
    let mut output = Vec::new();
    let mut reader = empty_reader().await;

    core.handle_user_message("hi", &mut reader, &mut output)
        .await
        .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(output_str.contains("Hello from AI!"));
    assert_eq!(core.messages.len(), 2);
}

#[tokio::test]
async fn provider_eof_without_terminal_fails_the_request_and_turn() {
    let provider = MockProvider::new(vec![vec![GenerateEvent::TextDelta("partial".to_string())]]);
    let mut core = make_core(provider);
    core.audit = CoreAuditRecorder::test_capture(&core.session_id);
    let mut output = Vec::new();
    let mut reader = empty_reader().await;

    let result = core
        .handle_user_message("hi", &mut reader, &mut output)
        .await;

    assert!(result.is_err());
    let event_types = core.audit.captured_event_types();
    assert!(event_types.contains(&"provider.request.failed"));
    assert!(event_types.contains(&"turn.failed"));
    assert!(!event_types.contains(&"provider.request.completed"));
}

#[tokio::test]
async fn unknown_tool_returns_error_result() {
    let provider = MockProvider::new(vec![
        vec![
            GenerateEvent::TextDelta("Let me try.".to_string()),
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-1".to_string(),
                name: "nonexistent".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta: r#"{"x":1}"#.to_string(),
            },
            GenerateEvent::ToolCallEnd { index: 0 },
            GenerateEvent::MessageEnd,
        ],
        vec![
            GenerateEvent::TextDelta("Sorry, that didn't work.".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ]);

    let mut core = make_core(provider);
    let mut output = Vec::new();
    let mut reader = empty_reader().await;

    core.handle_user_message("do something", &mut reader, &mut output)
        .await
        .unwrap();

    assert!(core.messages.len() >= 4);
    let tool_result_msg = &core.messages[2];
    assert_eq!(tool_result_msg.role, "tool");
}

#[tokio::test]
async fn multi_turn_with_tool() {
    let provider = MockProvider::new(vec![
        vec![
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-1".to_string(),
                name: "shell".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta: r#"{"command":"echo hello"}"#.to_string(),
            },
            GenerateEvent::ToolCallEnd { index: 0 },
            GenerateEvent::MessageEnd,
        ],
        vec![
            GenerateEvent::TextDelta("The command output was: hello".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ]);

    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    let tools = ToolRegistry::with_defaults_for_test();
    let mut core = CoshCore::new(config, Box::new(provider), tools);
    let mut output = Vec::new();
    let mut reader = empty_reader().await;

    core.handle_user_message("run echo hello", &mut reader, &mut output)
        .await
        .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(output_str.contains("hello"));
    assert!(
        output_str.find(r#""type":"user""#) < output_str.find("The command output was: hello"),
        "{output_str}"
    );
    assert!(
        output_str.contains(r#""type":"tool_result""#),
        "{output_str}"
    );
    assert!(core.messages.len() >= 4);
}

#[tokio::test]
async fn incomplete_tool_call_stops_without_consuming_turn_budget() {
    let provider = MockProvider::new(vec![vec![
        GenerateEvent::ToolCallDelta {
            index: 0,
            arguments_delta: r#"{"command":"pwd"}"#.to_string(),
        },
        GenerateEvent::MessageEnd,
    ]]);
    let mut core = make_core(provider);
    let mut output = Vec::new();
    let mut reader = empty_reader().await;

    let error = core
        .handle_user_message("inspect this project", &mut reader, &mut output)
        .await
        .expect_err("an unnamed tool call must fail immediately");

    assert_eq!(
        error,
        "Provider emitted an incomplete tool call without a function name"
    );
    assert_eq!(core.messages.len(), 1, "must not append an empty turn");
}

#[tokio::test]
async fn mixed_tool_calls_stop_when_any_call_is_incomplete() {
    let provider = MockProvider::new(vec![vec![
        GenerateEvent::ToolCallStart {
            index: 0,
            id: "call-valid".to_string(),
            name: "shell".to_string(),
        },
        GenerateEvent::ToolCallDelta {
            index: 0,
            arguments_delta: r#"{"command":"pwd"}"#.to_string(),
        },
        GenerateEvent::ToolCallDelta {
            index: 1,
            arguments_delta: r#"{"command":"id"}"#.to_string(),
        },
        GenerateEvent::MessageEnd,
    ]]);
    let mut core = make_core(provider);
    let mut output = Vec::new();
    let mut reader = empty_reader().await;

    let error = core
        .handle_user_message("inspect this project", &mut reader, &mut output)
        .await
        .expect_err("any unnamed tool call with arguments must fail the turn");

    assert_eq!(
        error,
        "Provider emitted an incomplete tool call without a function name"
    );
    assert_eq!(core.messages.len(), 1, "must not execute the named tool");
}

#[tokio::test]
async fn text_after_tool_call_is_not_visible_before_tool_result() {
    let provider = MockProvider::new(vec![
        vec![
            GenerateEvent::TextDelta("Preparing to run the command.".to_string()),
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-1".to_string(),
                name: "shell".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta: r#"{"command":"echo hello"}"#.to_string(),
            },
            GenerateEvent::ToolCallEnd { index: 0 },
            GenerateEvent::TextDelta("SHOULD NOT BE VISIBLE BEFORE TOOL RESULT".to_string()),
            GenerateEvent::MessageEnd,
        ],
        vec![
            GenerateEvent::TextDelta("The command output was: hello".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ]);

    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    let tools = ToolRegistry::with_defaults_for_test();
    let mut core = CoshCore::new(config, Box::new(provider), tools);
    let mut output = Vec::new();
    let mut reader = empty_reader().await;

    core.handle_user_message("run echo hello", &mut reader, &mut output)
        .await
        .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("Preparing to run the command."),
        "{output_str}"
    );
    assert!(
        !output_str.contains("SHOULD NOT BE VISIBLE BEFORE TOOL RESULT"),
        "{output_str}"
    );
    assert!(
        output_str.find(r#""type":"tool_result""#)
            < output_str.find("The command output was: hello"),
        "{output_str}"
    );
}

#[tokio::test]
async fn tool_call_block_is_closed_when_stream_ends_without_tool_call_end() {
    let provider = MockProvider::new(vec![
        vec![
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-1".to_string(),
                name: "shell".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta: r#"{"command":"echo hello"}"#.to_string(),
            },
            GenerateEvent::MessageEnd,
        ],
        vec![
            GenerateEvent::TextDelta("done".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ]);

    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    let tools = ToolRegistry::with_defaults_for_test();
    let mut core = CoshCore::new(config, Box::new(provider), tools);
    let mut output = Vec::new();
    let mut reader = empty_reader().await;

    core.handle_user_message("run echo hello", &mut reader, &mut output)
        .await
        .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(output_str.contains(r#""type":"content_block_stop","index":0"#));
    assert!(
        output_str.find(r#""type":"content_block_stop","index":0"#)
            < output_str.find(r#""type":"tool_result""#),
        "{output_str}"
    );
}

#[tokio::test]
async fn multiple_tool_call_blocks_are_closed_with_distinct_indexes_without_tool_call_end() {
    let provider = MockProvider::new(vec![
        vec![
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-1".to_string(),
                name: "first_unknown".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta: r#"{"value":1}"#.to_string(),
            },
            GenerateEvent::ToolCallStart {
                index: 1,
                id: "call-2".to_string(),
                name: "second_unknown".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 1,
                arguments_delta: r#"{"value":2}"#.to_string(),
            },
            GenerateEvent::MessageEnd,
        ],
        vec![
            GenerateEvent::TextDelta("done".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ]);

    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    let tools = ToolRegistry::new();
    let mut core = CoshCore::new(config, Box::new(provider), tools);
    let mut output = Vec::new();
    let mut reader = empty_reader().await;

    core.handle_user_message("run two tools", &mut reader, &mut output)
        .await
        .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    let first_message = output_str
        .split(r#"{"type":"stream_event","event":{"type":"message_stop"}}"#)
        .next()
        .expect("first stream message");
    assert_eq!(
        first_message
            .matches(r#""type":"content_block_start","index":0"#)
            .count(),
        1,
        "{output_str}"
    );
    assert_eq!(
        first_message
            .matches(r#""type":"content_block_start","index":1"#)
            .count(),
        1,
        "{output_str}"
    );
    assert_eq!(
        first_message
            .matches(r#""type":"content_block_stop","index":0"#)
            .count(),
        1,
        "{output_str}"
    );
    assert_eq!(
        first_message
            .matches(r#""type":"content_block_stop","index":1"#)
            .count(),
        1,
        "{output_str}"
    );
    assert!(
        output_str.find(r#""type":"content_block_stop","index":1"#)
            < output_str.find(r#""type":"tool_result""#),
        "{output_str}"
    );
}

#[tokio::test]
async fn approval_flow_allow() {
    let provider = MockProvider::new(vec![
        vec![
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-1".to_string(),
                name: "shell".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta: r#"{"command":"echo approved"}"#.to_string(),
            },
            GenerateEvent::ToolCallEnd { index: 0 },
            GenerateEvent::MessageEnd,
        ],
        vec![
            GenerateEvent::TextDelta("Done.".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ]);

    let mut config = CoreConfig::default();
    config.agent.approval_mode = "suggest".to_string();
    let tools = ToolRegistry::with_defaults_for_test();
    let mut core = CoshCore::new(config, Box::new(provider), tools);

    let allow_response = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-0","response":{"behavior":"allow"}}}"#;
    let input = format!("{allow_response}\n");
    let mut reader = BufReader::new(input.as_bytes()).lines();
    let mut output = Vec::new();

    core.handle_user_message("run echo approved", &mut reader, &mut output)
        .await
        .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(output_str.contains("can_use_tool"));
    assert!(core.messages.len() >= 4);
}

#[tokio::test]
async fn approval_flow_deny() {
    let provider = MockProvider::new(vec![
        vec![
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-1".to_string(),
                name: "shell".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta: r#"{"command":"rm -rf /"}"#.to_string(),
            },
            GenerateEvent::ToolCallEnd { index: 0 },
            GenerateEvent::MessageEnd,
        ],
        vec![
            GenerateEvent::TextDelta("I understand, the command was denied.".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ]);

    let mut config = CoreConfig::default();
    config.agent.approval_mode = "suggest".to_string();
    let tools = ToolRegistry::with_defaults_for_test();

    let deny_response = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-0","response":{"behavior":"deny","message":"Too dangerous"}}}"#;
    let input = format!("{deny_response}\n");
    let mut reader = BufReader::new(input.as_bytes()).lines();

    let mut core = CoshCore::new(config, Box::new(provider), tools);
    let mut output = Vec::new();

    core.handle_user_message("delete everything", &mut reader, &mut output)
        .await
        .unwrap();

    let tool_result = core.messages.iter().find(|m| m.role == "tool").unwrap();
    if let crate::provider::MessageContent::Blocks(blocks) = &tool_result.content {
        if let crate::provider::MessageContentBlock::ToolResult {
            content, is_error, ..
        } = &blocks[0]
        {
            assert!(is_error);
            assert!(content.contains("denied"));
        }
    }
}

#[tokio::test]
async fn request_id_skips_mismatched() {
    let core = make_core(MockProvider::text_only(""));
    let mismatched = r#"{"type":"control_response","response":{"subtype":"success","request_id":"wrong-id","response":{"behavior":"allow"}}}"#;
    let correct = r#"{"type":"control_response","response":{"subtype":"success","request_id":"expected-id","response":{"behavior":"deny","message":"denied"}}}"#;
    let input = format!("{mismatched}\n{correct}\n");
    let mut reader = BufReader::new(input.as_bytes()).lines();

    let result = core
        .wait_for_approval("expected-id", false, &mut reader)
        .await;
    assert!(matches!(result, ApprovalResult::Denied(_)));
}

#[tokio::test]
async fn approval_flow_host_executed_shell_uses_tool_result() {
    let shell_calls = Arc::new(AtomicUsize::new(0));
    let provider = MockProvider::new(vec![
        vec![
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-1".to_string(),
                name: "shell".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta: r#"{"command":"df -h"}"#.to_string(),
            },
            GenerateEvent::ToolCallEnd { index: 0 },
            GenerateEvent::MessageEnd,
        ],
        vec![
            GenerateEvent::TextDelta("Received shell evidence.".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ]);

    let mut config = CoreConfig::default();
    config.agent.approval_mode = "suggest".to_string();
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(CountingShellTool {
        calls: Arc::clone(&shell_calls),
    }));
    let mut core = CoshCore::new(config, Box::new(provider), tools);

    let response = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-0","response":{"behavior":"host_executed_shell","result":{"llmContent":"ShellCommandCompleted evidence\ncommand: df -h\nstatus: completed","returnDisplay":"df -h completed","metadata":{"command":"df -h","status":"completed","exit_code":0}}}}}"#;
    let input = format!("{response}\n");
    let mut reader = BufReader::new(input.as_bytes()).lines();
    let mut output = Vec::new();

    core.handle_user_message("check disk", &mut reader, &mut output)
        .await
        .unwrap();

    assert_eq!(
        shell_calls.load(Ordering::SeqCst),
        0,
        "host-executed result must not run provider-native shell executor"
    );
    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("Received shell evidence."),
        "{output_str}"
    );
    assert!(
        !output_str.contains(r#""type":"tool_result""#),
        "{output_str}"
    );
    let tool_result = core
        .messages
        .iter()
        .find(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some("call-1"))
        .expect("tool result");
    match &tool_result.content {
        crate::provider::MessageContent::Text(content) => {
            assert!(content.contains("ShellCommandCompleted evidence"));
            assert!(content.contains("command: df -h"));
        }
        _ => panic!("expected text tool result"),
    }
}

#[tokio::test]
async fn approval_flow_rejects_host_executed_for_non_shell_tool() {
    let provider = MockProvider::new(vec![
        vec![
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-write".to_string(),
                name: "write_file".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta:
                    r#"{"file_path":"/tmp/cosh-host-executed-non-shell","content":"bad"}"#
                        .to_string(),
            },
            GenerateEvent::ToolCallEnd { index: 0 },
            GenerateEvent::MessageEnd,
        ],
        vec![
            GenerateEvent::TextDelta("Rejected.".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ]);

    let mut config = CoreConfig::default();
    config.agent.approval_mode = "suggest".to_string();
    let tools = ToolRegistry::with_defaults_for_test();
    let mut core = CoshCore::new(config, Box::new(provider), tools);

    let response = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-0","response":{"behavior":"host_executed_shell","result":{"llmContent":"should not be accepted","returnDisplay":null,"metadata":{"command":"echo bad","status":"completed","exit_code":0}}}}}"#;
    let input = format!("{response}\n");
    let mut reader = BufReader::new(input.as_bytes()).lines();
    let mut output = Vec::new();

    core.handle_user_message("write file", &mut reader, &mut output)
        .await
        .unwrap();

    let tool_result = core
        .messages
        .iter()
        .find(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some("call-write"))
        .expect("tool result");
    match &tool_result.content {
        crate::provider::MessageContent::Text(content) => {
            assert!(content.contains("host_executed_shell is only valid for shell tools"));
            assert!(!content.contains("should not be accepted"));
        }
        _ => panic!("expected text tool result"),
    }
}

#[tokio::test]
async fn ask_user_question_flow() {
    let provider = MockProvider::new(vec![
        vec![
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-1".to_string(),
                name: "ask_user_question".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta: r#"{"question":"Which language?","options":[{"label":"Rust"},{"label":"Python"}]}"#.to_string(),
            },
            GenerateEvent::ToolCallEnd { index: 0 },
            GenerateEvent::MessageEnd,
        ],
        vec![
            GenerateEvent::TextDelta("Great, you chose Rust!".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ]);

    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    let tools = ToolRegistry::with_defaults_for_test();
    let mut core = CoshCore::new(config, Box::new(provider), tools);

    let answer_response = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-0","response":{"answer":"Rust"}}}"#;
    let input = format!("{answer_response}\n");
    let mut reader = BufReader::new(input.as_bytes()).lines();
    let mut output = Vec::new();

    core.handle_user_message("what language?", &mut reader, &mut output)
        .await
        .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(output_str.contains("ask_user"));

    let tool_result = core.messages.iter().find(|m| m.role == "tool").unwrap();
    if let crate::provider::MessageContent::Blocks(blocks) = &tool_result.content {
        if let crate::provider::MessageContentBlock::ToolResult { content, .. } = &blocks[0] {
            assert!(content.contains("Rust"));
        }
    }
}

#[tokio::test]
async fn cosh_shell_evidence_read_output_uses_control_protocol_result() {
    let provider = MockProvider::new(vec![
        vec![
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-evidence".to_string(),
                name: "cosh_shell_evidence".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta: r#"{"action":"read_output","output_id":"terminal-output://raw-session-a1b2/cmd-1","direction":"tail","lines":42}"#.to_string(),
            },
            GenerateEvent::ToolCallEnd { index: 0 },
            GenerateEvent::MessageEnd,
        ],
        vec![
            GenerateEvent::TextDelta("I can see the captured output.".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ]);

    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    let tools = ToolRegistry::new().with_shell_evidence();
    let mut core = CoshCore::new(config, Box::new(provider), tools);

    let response = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-0","response":{"behavior":"shell_evidence","result":{"llmContent":"ShellEvidenceExcerpt\noutput_id: terminal-output://raw-session-a1b2/cmd-1\nexcerpt_status: available\nstdout","returnDisplay":"captured output","metadata":{"action":"read_output","output_id":"terminal-output://raw-session-a1b2/cmd-1","excerpt_status":"available","is_error":false}}}}}"#;
    let input = format!("{response}\n");
    let mut reader = BufReader::new(input.as_bytes()).lines();
    let mut output = Vec::new();

    core.handle_user_message("read output", &mut reader, &mut output)
        .await
        .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains(r#""subtype":"shell_evidence""#),
        "{output_str}"
    );
    assert!(
        output_str.contains(r#""action":"read_output""#),
        "{output_str}"
    );
    assert!(
        output_str.contains(r#""tool_use_id":"call-evidence""#),
        "{output_str}"
    );
    assert!(output_str.contains(r#""lines":42"#), "{output_str}");
    assert!(
        !output_str.contains(r#""bypass_recent_filter""#),
        "{output_str}"
    );
    assert!(
        output_str.contains(r#""type":"tool_result""#),
        "{output_str}"
    );
    assert!(
        output_str.contains("I can see the captured output."),
        "{output_str}"
    );

    let tool_result = core
        .messages
        .iter()
        .find(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some("call-evidence"))
        .expect("tool result");
    match &tool_result.content {
        crate::provider::MessageContent::Text(content) => {
            assert!(content.contains("ShellEvidenceExcerpt"));
            assert!(content.contains("excerpt_status: available"));
        }
        _ => panic!("expected text tool result"),
    }
}

#[tokio::test]
async fn cosh_shell_evidence_list_commands_uses_control_protocol_result() {
    let provider = MockProvider::new(vec![
        vec![
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-evidence".to_string(),
                name: "cosh_shell_evidence".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta: r#"{"action":"list_commands","limit":2}"#.to_string(),
            },
            GenerateEvent::ToolCallEnd { index: 0 },
            GenerateEvent::MessageEnd,
        ],
        vec![
            GenerateEvent::TextDelta("I can see the command index.".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ]);

    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    let tools = ToolRegistry::new().with_shell_evidence();
    let mut core = CoshCore::new(config, Box::new(provider), tools);

    let response = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-0","response":{"behavior":"shell_evidence","result":{"llmContent":"ShellEvidenceCommandIndex\ncommand_id: cmd-1\noutput_available: true","returnDisplay":null,"metadata":{"action":"list_commands","scope":"current_ledger","limit":2,"next_cursor":null,"is_error":false}}}}}"#;
    let input = format!("{response}\n");
    let mut reader = BufReader::new(input.as_bytes()).lines();
    let mut output = Vec::new();

    core.handle_user_message("list commands", &mut reader, &mut output)
        .await
        .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains(r#""subtype":"shell_evidence""#),
        "{output_str}"
    );
    assert!(
        output_str.contains(r#""action":"list_commands""#),
        "{output_str}"
    );
    assert!(output_str.contains(r#""limit":2"#), "{output_str}");
    assert!(
        output_str.contains("I can see the command index."),
        "{output_str}"
    );

    let tool_result = core
        .messages
        .iter()
        .find(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some("call-evidence"))
        .expect("tool result");
    match &tool_result.content {
        crate::provider::MessageContent::Text(content) => {
            assert!(content.contains("ShellEvidenceCommandIndex"));
            assert!(content.contains("output_available: true"));
        }
        _ => panic!("expected text tool result"),
    }
}

#[tokio::test]
async fn cosh_shell_evidence_preserves_error_result() {
    let provider = MockProvider::new(vec![
        vec![
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-evidence".to_string(),
                name: "cosh_shell_evidence".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta:
                    r#"{"action":"read_output","output_id":"terminal-output://old-session/cmd-1"}"#
                        .to_string(),
            },
            GenerateEvent::ToolCallEnd { index: 0 },
            GenerateEvent::MessageEnd,
        ],
        vec![
            GenerateEvent::TextDelta("The output is stale.".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ]);

    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    let tools = ToolRegistry::new().with_shell_evidence();
    let mut core = CoshCore::new(config, Box::new(provider), tools);

    let response = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-0","response":{"behavior":"shell_evidence","result":{"llmContent":"ShellEvidenceExcerpt\noutput_id: terminal-output://old-session/cmd-1\nexcerpt_status: unavailable\nreason: stale_session","returnDisplay":"stale output","metadata":{"action":"read_output","output_id":"terminal-output://old-session/cmd-1","excerpt_status":"unavailable","is_error":true,"reason":"stale_session"}}}}}"#;
    let input = format!("{response}\n");
    let mut reader = BufReader::new(input.as_bytes()).lines();
    let mut output = Vec::new();

    core.handle_user_message("read output", &mut reader, &mut output)
        .await
        .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(output_str.contains(r#""is_error":true"#), "{output_str}");
    let tool_result = core
        .messages
        .iter()
        .find(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some("call-evidence"))
        .expect("tool result");
    match &tool_result.content {
        crate::provider::MessageContent::Text(content) => {
            assert!(content.contains("excerpt_status: unavailable"));
            assert!(content.contains("reason: stale_session"));
        }
        _ => panic!("expected text tool result"),
    }
}

#[tokio::test]
async fn cosh_shell_evidence_read_output_forwards_bypass_recent_filter() {
    let provider = MockProvider::new(vec![
        vec![
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-evidence".to_string(),
                name: "cosh_shell_evidence".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta: r#"{"action":"read_output","output_id":"terminal-output://raw-session-a1b2/cmd-1","bypass_recent_filter":true}"#.to_string(),
            },
            GenerateEvent::ToolCallEnd { index: 0 },
            GenerateEvent::MessageEnd,
        ],
        vec![GenerateEvent::MessageEnd],
    ]);

    let tools = ToolRegistry::new().with_shell_evidence();
    let mut core = CoshCore::new(CoreConfig::default(), Box::new(provider), tools);

    let response = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-0","response":{"behavior":"shell_evidence","result":{"llmContent":"ShellEvidenceExcerpt\noutput_id: terminal-output://raw-session-a1b2/cmd-1\nexcerpt_status: available\nstdout","returnDisplay":"captured output","metadata":{"action":"read_output","output_id":"terminal-output://raw-session-a1b2/cmd-1","excerpt_status":"available","is_error":false}}}}}"#;
    let input = format!("{response}\n");
    let mut reader = BufReader::new(input.as_bytes()).lines();
    let mut output = Vec::new();

    core.handle_user_message("read output", &mut reader, &mut output)
        .await
        .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains(r#""bypass_recent_filter":true"#),
        "{output_str}"
    );
}

#[tokio::test]
async fn cosh_shell_evidence_already_delivered_is_not_error() {
    let core = make_core(MockProvider::new(vec![]));
    let response = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-0","response":{"behavior":"shell_evidence","result":{"llmContent":"ShellEvidenceExcerpt\noutput_id: terminal-output://raw-session/cmd-1\nexcerpt_status: already_delivered\nreason: already_delivered_recent_shell_tool_output","returnDisplay":null,"metadata":{"action":"read_output","output_id":"terminal-output://raw-session/cmd-1","excerpt_status":"already_delivered","is_error":false,"reason":"already_delivered_recent_shell_tool_output"}}}}}"#;
    let input = format!("{response}\n");
    let mut reader = BufReader::new(input.as_bytes()).lines();

    let result = core.wait_for_shell_evidence("req-0", &mut reader).await;

    assert!(!result.is_error, "{}", result.output);
    assert!(result.output.contains("excerpt_status: already_delivered"));
}

#[tokio::test]
async fn cosh_shell_evidence_bypasses_normal_tool_hooks() {
    let provider = MockProvider::new(vec![
        vec![
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-list".to_string(),
                name: "cosh_shell_evidence".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta: r#"{"action":"list_commands","limit":2}"#.to_string(),
            },
            GenerateEvent::ToolCallEnd { index: 0 },
            GenerateEvent::MessageEnd,
        ],
        vec![
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-read".to_string(),
                name: "cosh_shell_evidence".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta:
                    r#"{"action":"read_output","output_id":"terminal-output://raw-session/cmd-1"}"#
                        .to_string(),
            },
            GenerateEvent::ToolCallEnd { index: 0 },
            GenerateEvent::MessageEnd,
        ],
        vec![
            GenerateEvent::TextDelta("evidence hooks bypassed".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ]);

    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    config.hooks = config::HooksConfig {
        enabled: true,
        pre_tool_use: vec![config::HookDefinition {
            command: "echo '{\"decision\":\"block\",\"reason\":\"pre hook should not run\"}'"
                .to_string(),
            name: Some("block-evidence".to_string()),
            matcher: Some("cosh_shell_evidence".to_string()),
            timeout: Some(5000),
            sequential: None,
        }],
        post_tool_use: vec![config::HookDefinition {
            command: "echo '{\"decision\":\"block\",\"reason\":\"post hook should not run\"}'"
                .to_string(),
            name: Some("deny-evidence".to_string()),
            matcher: Some("cosh_shell_evidence".to_string()),
            timeout: Some(5000),
            sequential: None,
        }],
        ..Default::default()
    };
    let tools = ToolRegistry::new().with_shell_evidence();
    let mut core = CoshCore::new(config, Box::new(provider), tools);

    let list_response = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-0","response":{"behavior":"shell_evidence","result":{"llmContent":"ShellEvidenceCommandIndex\ncommand_id: cmd-1","returnDisplay":null,"metadata":{"action":"list_commands","is_error":false}}}}}"#;
    let read_response = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-1","response":{"behavior":"shell_evidence","result":{"llmContent":"ShellEvidenceExcerpt\noutput_id: terminal-output://raw-session/cmd-1\nstdout","returnDisplay":"stdout","metadata":{"action":"read_output","is_error":false}}}}}"#;
    let input = format!("{list_response}\n{read_response}\n");
    let mut reader = BufReader::new(input.as_bytes()).lines();
    let mut output = Vec::new();

    core.handle_user_message("inspect shell evidence", &mut reader, &mut output)
        .await
        .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains(r#""action":"list_commands""#),
        "{output_str}"
    );
    assert!(
        output_str.contains(r#""action":"read_output""#),
        "{output_str}"
    );
    assert!(
        output_str.contains("evidence hooks bypassed"),
        "{output_str}"
    );
    assert!(!output_str.contains("hook_notification"), "{output_str}");
    assert!(!output_str.contains("Blocked by hook"), "{output_str}");
    assert!(
        !output_str.contains("Post-tool hook denied"),
        "{output_str}"
    );
    assert!(
        !output_str.contains("pre hook should not run"),
        "{output_str}"
    );
    assert!(
        !output_str.contains("post hook should not run"),
        "{output_str}"
    );
}

#[tokio::test]
async fn cosh_shell_evidence_rejects_read_output_without_output_id() {
    let core = make_core(MockProvider::new(vec![]));
    let mut reader = empty_reader().await;
    let mut output = Vec::new();

    let result = core
        .handle_shell_evidence(
            "call-evidence",
            &serde_json::json!({"action":"read_output"}),
            &mut reader,
            &mut output,
        )
        .await;

    assert!(result.is_error);
    assert!(result.output.contains("missing required output_id"));
    assert!(String::from_utf8(output).unwrap().is_empty());
}

#[tokio::test]
async fn cosh_shell_evidence_rejects_list_commands_read_output_fields() {
    let core = make_core(MockProvider::new(vec![]));
    let mut reader = empty_reader().await;
    let mut output = Vec::new();

    let result = core
        .handle_shell_evidence(
            "call-evidence",
            &serde_json::json!({
                "action":"list_commands",
                "output_id":"terminal-output://raw-session/cmd-1"
            }),
            &mut reader,
            &mut output,
        )
        .await;

    assert!(result.is_error);
    assert!(result.output.contains("accepts only limit and cursor"));
    assert!(String::from_utf8(output).unwrap().is_empty());
}

#[tokio::test]
async fn cosh_shell_evidence_list_commands_ignores_direction_hint() {
    let core = make_core(MockProvider::new(vec![]));
    let response = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-0","response":{"behavior":"shell_evidence","result":{"llmContent":"ShellEvidenceCommandIndex\ncommand_id: cmd-1","returnDisplay":null,"metadata":{"action":"list_commands","scope":"current_ledger","limit":10,"next_cursor":null,"is_error":false}}}}}"#;
    let input = format!("{response}\n");
    let mut reader = BufReader::new(input.as_bytes()).lines();
    let mut output = Vec::new();

    let result = core
        .handle_shell_evidence(
            "call-evidence",
            &serde_json::json!({
                "action":"list_commands",
                "direction":"tail",
                "limit":10
            }),
            &mut reader,
            &mut output,
        )
        .await;

    assert!(!result.is_error, "{}", result.output);
    assert!(result.output.contains("ShellEvidenceCommandIndex"));
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains(r#""action":"list_commands""#), "{output}");
    assert!(output.contains(r#""limit":10"#), "{output}");
    assert!(!output.contains(r#""direction""#), "{output}");
}

#[tokio::test]
async fn cosh_shell_evidence_rejects_invalid_limit_type() {
    let core = make_core(MockProvider::new(vec![]));
    let mut reader = empty_reader().await;
    let mut output = Vec::new();

    let result = core
        .handle_shell_evidence(
            "call-evidence",
            &serde_json::json!({"action":"list_commands","limit":"many"}),
            &mut reader,
            &mut output,
        )
        .await;

    assert!(result.is_error);
    assert!(result.output.contains("limit must be an integer"));
    assert!(String::from_utf8(output).unwrap().is_empty());
}

#[tokio::test]
async fn cosh_shell_evidence_rejects_invalid_bypass_recent_filter_type() {
    let core = make_core(MockProvider::new(vec![]));
    let mut reader = empty_reader().await;
    let mut output = Vec::new();

    let result = core
        .handle_shell_evidence(
            "call-evidence",
            &serde_json::json!({
                "action":"read_output",
                "output_id":"terminal-output://raw-session/cmd-1",
                "bypass_recent_filter":"true"
            }),
            &mut reader,
            &mut output,
        )
        .await;

    assert!(result.is_error);
    assert!(result
        .output
        .contains("bypass_recent_filter must be a boolean"));
    assert!(String::from_utf8(output).unwrap().is_empty());
}

#[tokio::test]
async fn thinking_delta_emits_stream_event() {
    let provider = MockProvider::new(vec![vec![
        GenerateEvent::ThinkingDelta("Step 1: analyze...".to_string()),
        GenerateEvent::ThinkingDelta("Step 2: conclude.".to_string()),
        GenerateEvent::TextDelta("The answer is 42.".to_string()),
        GenerateEvent::MessageEnd,
    ]]);
    let mut core = make_core(provider);
    let mut output = Vec::new();
    let mut reader = empty_reader().await;

    core.handle_user_message("think about this", &mut reader, &mut output)
        .await
        .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(output_str.contains("thinking_delta"));
    assert!(output_str.contains("Step 1: analyze..."));
    assert!(output_str.contains("The answer is 42."));
    let thinking_line = output_str
        .lines()
        .find(|l| l.contains("thinking_delta"))
        .expect("should have thinking_delta line");
    let v: serde_json::Value = serde_json::from_str(thinking_line).unwrap();
    assert_eq!(
        v.pointer("/event/delta/thinking").and_then(|t| t.as_str()),
        Some("Step 1: analyze...")
    );
}
