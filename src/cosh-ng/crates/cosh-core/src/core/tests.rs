use super::*;
use crate::provider::mock::MockProvider;
use crate::tool::{Tool, ToolResult};
use async_trait::async_trait;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
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

struct ExternalTool;

#[derive(Default)]
struct RecordingProvider {
    messages: Arc<Mutex<Vec<crate::provider::Message>>>,
    tools: Arc<Mutex<Vec<crate::provider::ToolDeclaration>>>,
}

#[async_trait]
impl crate::provider::ContentGenerator for RecordingProvider {
    async fn generate(
        &self,
        messages: &[crate::provider::Message],
        tools: &[crate::provider::ToolDeclaration],
        _config: &crate::provider::GenerateConfig,
    ) -> Result<crate::provider::GenerateStream, String> {
        *self.messages.lock().unwrap() = messages.to_vec();
        *self.tools.lock().unwrap() = tools.to_vec();
        Ok(Box::pin(futures::stream::iter([
            crate::provider::GenerateEvent::TextDelta("done".to_string()),
            crate::provider::GenerateEvent::MessageEnd,
        ])))
    }

    fn cancel(&self) {}
}

#[async_trait]
impl Tool for ExternalTool {
    fn name(&self) -> &str {
        "example.ops/mcp/server/tool"
    }

    fn description(&self) -> &str {
        "external tool"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object"})
    }

    fn kind(&self) -> ToolKind {
        ToolKind::External
    }

    async fn invoke(
        &self,
        _params: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, String> {
        Ok(ToolResult::success("unused"))
    }
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

#[test]
fn external_tools_require_approval_outside_trust_mode() {
    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(ExternalTool));
    let mut core = CoshCore::new(config, Box::new(MockProvider::text_only("unused")), tools);
    for mode in ["auto", "balanced", "suggest"] {
        core.config.agent.approval_mode = mode.to_string();
        assert_eq!(
            core.classify_tool("example.ops/mcp/server/tool", &serde_json::json!({})),
            Outcome::RequireApproval
        );
    }
    core.config.agent.approval_mode = "trust".to_string();
    assert_eq!(
        core.classify_tool("example.ops/mcp/server/tool", &serde_json::json!({})),
        Outcome::Allow
    );
}

#[test]
fn safe_reload_rebinds_the_complete_snapshot_before_the_next_run() {
    let mut core = make_core(MockProvider::text_only("unused"));
    let previous = core.extension_generation.current().generation.id;
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(ExternalTool));
    let candidate = RuntimeSnapshot::bootstrap(
        RuntimeGeneration::healthy(previous + 1, "candidate"),
        Arc::new(tools),
    );

    core.extension_generation.stage(candidate);
    assert_eq!(
        core.extension_generation.reload(),
        crate::extension::generation::ReloadOutcome::Activated
    );
    assert!(core.tools.get("example.ops/mcp/server/tool").is_none());

    core.bind_current_extension_snapshot();

    assert_eq!(core.bound_extension_generation, previous + 1);
    assert!(core.tools.get("example.ops/mcp/server/tool").is_some());
    assert_eq!(core.extension_generation.take_retired().len(), 1);
}

#[test]
fn web_fetch_requires_approval_outside_trust_mode() {
    for (mode, expected) in [
        ("trust", Outcome::Allow),
        ("auto", Outcome::RequireApproval),
        ("balanced", Outcome::RequireApproval),
        ("suggest", Outcome::RequireApproval),
        ("strict", Outcome::RequireApproval),
    ] {
        let mut config = CoreConfig::default();
        config.agent.approval_mode = mode.to_string();
        let tools = ToolRegistry::with_defaults_for_test();
        let core = CoshCore::new(config, Box::new(MockProvider::new(Vec::new())), tools);

        assert_eq!(
            core.classify_tool("web_fetch", &serde_json::json!({})),
            expected,
            "unexpected web_fetch policy in {mode} mode"
        );
    }
}

#[tokio::test]
async fn project_context_reaches_the_provider_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let context_dir = dir.path().join(".copilot-shell");
    std::fs::create_dir(&context_dir).unwrap();
    std::fs::write(context_dir.join("CONTEXT.md"), "provider-visible marker").unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = RecordingProvider {
        messages: Arc::clone(&captured),
        ..RecordingProvider::default()
    };
    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    let mut core = CoshCore::new(config, Box::new(provider), ToolRegistry::new());
    core.shell_context = Some(ShellContext {
        cwd: dir.path().to_path_buf(),
        env: std::collections::HashMap::new(),
        last_exit_code: 0,
    });
    let mut reader = empty_reader().await;
    let mut output = Vec::new();

    core.handle_user_message("hello", &mut reader, &mut output)
        .await
        .unwrap();

    let messages = captured.lock().unwrap();
    let system = messages.first().expect("provider system message");
    assert_eq!(system.role, "system");
    assert!(system.content.as_text().contains("# Context"));
    assert!(system
        .content
        .as_text()
        .contains("## Project Context\nprovider-visible marker"));
}

#[tokio::test]
async fn user_provided_secret_reaches_the_provider_boundary() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = RecordingProvider {
        messages: Arc::clone(&captured),
        ..RecordingProvider::default()
    };
    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    let mut core = CoshCore::new(config, Box::new(provider), ToolRegistry::new());
    let mut reader = empty_reader().await;
    let mut output = Vec::new();
    let secret = "sk-user-provided-secret-value";

    core.handle_user_message(
        &format!("write api_key={secret} to the config"),
        &mut reader,
        &mut output,
    )
    .await
    .unwrap();

    let messages = captured.lock().unwrap();
    let user_message = messages
        .iter()
        .find(|message| message.role == "user")
        .expect("provider user message");
    assert!(user_message.content.as_text().contains(secret));
}

fn find_declaration<'a>(
    declarations: &'a [crate::provider::ToolDeclaration],
    name: &str,
) -> &'a crate::provider::ToolDeclaration {
    declarations
        .iter()
        .find(|tool| tool.name == name)
        .unwrap_or_else(|| panic!("missing '{name}' declaration"))
}

/// A BeforeModel hook that rewrites every tool `description` to `compressed`
/// and strips schema properties, mirroring tokenless schema compression.
fn compress_schema_hook(command: &str) -> crate::config::HookDefinition {
    crate::config::HookDefinition {
        command: command.to_string(),
        name: Some("compress-schema".to_string()),
        matcher: None,
        timeout: Some(10_000),
        sequential: None,
        env: Default::default(),
    }
}

#[tokio::test]
async fn before_model_hook_rewrites_tool_declarations_for_one_provider_call() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = RecordingProvider {
        tools: Arc::clone(&captured),
        ..RecordingProvider::default()
    };
    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    config.hooks.enabled = true;
    config.hooks.before_model = vec![compress_schema_hook(
        r#"python3 -c '
import json, sys
payload = json.load(sys.stdin)
tools = payload["llm_request"]["config"]["tools"]
for tool in tools:
    tool["description"] = "compressed"
    tool["parameters"] = {"type": "object", "properties": {"api_key": {"type": "string"}}}
print(json.dumps({"hookSpecificOutput": {"llm_request": {"config": {"tools": tools}}}}))
'"#,
    )];
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(CountingShellTool {
        calls: Arc::new(AtomicUsize::new(0)),
    }));
    let mut core = CoshCore::new(config, Box::new(provider), tools);
    let mut reader = empty_reader().await;
    let mut output = Vec::new();

    core.handle_user_message("hello", &mut reader, &mut output)
        .await
        .unwrap();

    let recorded = captured.lock().unwrap();
    let shell = find_declaration(&recorded, "shell");
    assert_eq!(shell.description, "compressed");
    // The schema property named `api_key` is a declaration, not a secret:
    // redaction must not have collapsed it into a "<redacted>" string.
    assert_eq!(shell.parameters["properties"]["api_key"]["type"], "string");

    // The registry is the source of truth for the next turn and stays intact.
    let declarations = core.tools.declarations();
    let original = find_declaration(&declarations, "shell");
    assert_eq!(original.description, "counting shell");
    assert!(original.parameters["properties"].get("command").is_some());
}

#[tokio::test]
async fn before_model_hook_rejecting_tool_set_changes_keeps_originals() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = RecordingProvider {
        tools: Arc::clone(&captured),
        ..RecordingProvider::default()
    };
    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    config.hooks.enabled = true;
    // Appending an undeclared tool changes tool-selection semantics, so the
    // whole array is discarded rather than partially applied.
    config.hooks.before_model = vec![compress_schema_hook(
        r#"python3 -c '
import json, sys
payload = json.load(sys.stdin)
tools = payload["llm_request"]["config"]["tools"]
tools.append({"name": "smuggled", "description": "x", "parameters": {"type": "object"}})
print(json.dumps({"hookSpecificOutput": {"llm_request": {"config": {"tools": tools}}}}))
'"#,
    )];
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(CountingShellTool {
        calls: Arc::new(AtomicUsize::new(0)),
    }));
    let mut core = CoshCore::new(config, Box::new(provider), tools);
    let mut reader = empty_reader().await;
    let mut output = Vec::new();

    core.handle_user_message("hello", &mut reader, &mut output)
        .await
        .unwrap();

    let recorded = captured.lock().unwrap();
    assert_eq!(
        find_declaration(&recorded, "shell").description,
        "counting shell"
    );
    assert!(recorded.iter().all(|tool| tool.name != "smuggled"));
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

/// Pending-call state is sized from the provider's index, so an out-of-range
/// index must fail the turn rather than allocate a slot per reported position.
#[tokio::test]
async fn out_of_range_tool_call_index_fails_the_turn() {
    for index in [MAX_TOOL_CALL_INDEX + 1, u32::MAX] {
        let provider = MockProvider::new(vec![vec![
            GenerateEvent::ToolCallStart {
                index,
                id: "call-1".to_string(),
                name: "shell".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index,
                arguments_delta: r#"{"command":"ls"}"#.to_string(),
            },
            GenerateEvent::ToolCallEnd { index },
            GenerateEvent::MessageEnd,
        ]]);
        let mut core = make_core(provider);
        core.audit = CoreAuditRecorder::test_capture(&core.session_id);
        let mut output = Vec::new();
        let mut reader = empty_reader().await;

        let error = core
            .handle_user_message("hi", &mut reader, &mut output)
            .await
            .expect_err("index {index} must fail the turn");
        assert!(error.contains(&index.to_string()), "{error}");
        assert!(
            error.contains(&MAX_TOOL_CALL_INDEX.to_string()),
            "the limit must be named: {error}"
        );
        assert!(
            core.audit.captured_event_types().contains(&"turn.failed"),
            "index {index} must be audited as a failed turn"
        );
    }
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
            env: Default::default(),
        }],
        post_tool_use: vec![config::HookDefinition {
            command: "echo '{\"decision\":\"block\",\"reason\":\"post hook should not run\"}'"
                .to_string(),
            name: Some("deny-evidence".to_string()),
            matcher: Some("cosh_shell_evidence".to_string()),
            timeout: Some(5000),
            sequential: None,
            env: Default::default(),
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

// ---------------------------------------------------------------------------
// malformed tool arguments: visibility and retry budget
// ---------------------------------------------------------------------------

/// One turn calling `shell` with arguments that are terminated but unparseable.
///
/// Each turn uses a fresh id, as a real provider would: the retry budget must
/// count the tool and the failure, not the call id.
fn unparseable_shell_turn(call_id: &str) -> Vec<GenerateEvent> {
    vec![
        GenerateEvent::ToolCallStart {
            index: 0,
            id: call_id.to_string(),
            name: "shell".to_string(),
        },
        GenerateEvent::ToolCallDelta {
            index: 0,
            arguments_delta: r#"{"command":"echo hello"#.to_string(),
        },
        GenerateEvent::ToolCallEnd { index: 0 },
        GenerateEvent::MessageEnd,
    ]
}

async fn run_shell_turns(turns: Vec<Vec<GenerateEvent>>) -> (Result<(), String>, String) {
    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    let tools = ToolRegistry::with_defaults_for_test();
    let mut core = CoshCore::new(config, Box::new(MockProvider::new(turns)), tools);
    let mut reader = empty_reader().await;
    let mut output = Vec::new();

    let result = core
        .handle_user_message("write the file", &mut reader, &mut output)
        .await;

    (result, String::from_utf8(output).unwrap())
}

#[tokio::test]
async fn rejected_tool_arguments_are_reported_to_the_shell_as_a_failed_tool() {
    let (result, output) = run_shell_turns(vec![
        unparseable_shell_turn("call-1"),
        vec![
            GenerateEvent::TextDelta("I will stop here.".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ])
    .await;

    result.expect("one rejection is recoverable");
    // Without this the Shell keeps a pending tool on screen forever: the
    // rejection only ever reached the model's context.
    assert!(
        output.contains(r#""type":"tool_result""#),
        "the rejection must close the pending tool in the UI: {output}"
    );
    assert!(output.contains("attempt 1/3"), "{output}");
    assert!(output.contains("code=invalid_json"), "{output}");
    // The rejected payload can hold session content, so the result the user sees
    // must describe the failure without quoting any of it.
    let rejection = output
        .lines()
        .find(|line| line.contains(r#""type":"tool_result""#))
        .expect("a tool result on the wire");
    assert!(!rejection.contains("echo hello"), "{rejection}");
}

#[tokio::test]
async fn three_consecutive_argument_rejections_stop_the_run() {
    let (result, output) = run_shell_turns(vec![
        unparseable_shell_turn("call-1"),
        unparseable_shell_turn("call-2"),
        unparseable_shell_turn("call-3"),
        unparseable_shell_turn("call-4"),
    ])
    .await;

    let error = result.expect_err("the run stops once the budget is spent");
    assert!(error.contains("shell"), "{error}");
    assert!(error.contains("code=invalid_json"), "{error}");
    assert!(error.contains("never executed"), "{error}");

    // The third rejection is still delivered, so the last thing on screen is a
    // failed tool rather than one that looks like it is still generating.
    assert!(output.contains("attempt 3/3"), "{output}");
    assert_eq!(
        output.matches(r#""type":"tool_result""#).count(),
        3,
        "exactly three attempts may be spent: {output}"
    );
    assert!(
        !output.contains("call-4"),
        "the fourth turn must never be requested: {output}"
    );
}

/// The assistant message declares every call in the batch up front, so ending
/// the run on the third rejection must not leave a later call unanswered:
/// headless persists the session and reuses it for the next user message, and an
/// unpaired `tool_use` id makes that request malformed.
#[tokio::test]
async fn stopping_on_exhaustion_still_answers_every_call_in_the_batch() {
    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    let tools = ToolRegistry::with_defaults_for_test();
    let mut core = CoshCore::new(
        config,
        Box::new(MockProvider::new(vec![
            unparseable_shell_turn("call-1"),
            unparseable_shell_turn("call-2"),
            // The fatal third rejection shares its message with a second call.
            vec![
                GenerateEvent::ToolCallStart {
                    index: 0,
                    id: "call-fatal".to_string(),
                    name: "shell".to_string(),
                },
                GenerateEvent::ToolCallDelta {
                    index: 0,
                    arguments_delta: r#"{"command":"echo hello"#.to_string(),
                },
                GenerateEvent::ToolCallEnd { index: 0 },
                GenerateEvent::ToolCallStart {
                    index: 1,
                    id: "call-trailing".to_string(),
                    name: "shell".to_string(),
                },
                GenerateEvent::ToolCallDelta {
                    index: 1,
                    arguments_delta: r#"{"command":"echo trailing"}"#.to_string(),
                },
                GenerateEvent::ToolCallEnd { index: 1 },
                GenerateEvent::MessageEnd,
            ],
        ])),
        tools,
    );
    core.audit = CoreAuditRecorder::test_capture(&core.session_id);
    let mut reader = empty_reader().await;
    let mut output = Vec::new();

    core.handle_user_message("write the file", &mut reader, &mut output)
        .await
        .expect_err("the run stops once the budget is spent");

    for call_id in ["call-1", "call-2", "call-fatal", "call-trailing"] {
        let results = core
            .messages
            .iter()
            .filter(|message| {
                message.role == "tool" && message.tool_call_id.as_deref() == Some(call_id)
            })
            .count();
        assert_eq!(results, 1, "{call_id} must have exactly one tool result");
    }

    // The trailing call was answered, not run: nothing may execute after the
    // budget is spent.
    let trailing = tool_result_text(&core, "call-trailing");
    assert!(trailing.contains("was not executed"), "{trailing}");

    // A skipped call is still part of the transcript, so it owes the audit trace
    // one `tool.requested` and one terminal event like any other call — and it is
    // counted, so the turn metrics do not under-report the failures.
    assert_eq!(
        core.audit.captured_tool_event_types("call-trailing"),
        vec!["tool.requested", "tool.failed"]
    );
    assert_eq!(core.metrics.tool_calls_total, 4);
    assert_eq!(core.metrics.tool_calls_fail, 4);
    assert!(!core
        .audit
        .captured_tool_event_types("call-trailing")
        .contains(&"tool.execution.started"));

    // And the Shell was told, so its pending tool closes instead of hanging.
    let output = String::from_utf8(output).unwrap();
    let emitted = output
        .lines()
        .find(|line| line.contains(r#""tool_use_id":"call-trailing""#))
        .expect("a tool result for the trailing call on the wire");
    assert!(emitted.contains(r#""is_error":true"#), "{emitted}");
    assert!(emitted.contains("was not executed"), "{emitted}");
}

#[tokio::test]
async fn a_recovered_tool_call_clears_the_rejection_budget() {
    let (result, output) = run_shell_turns(vec![
        unparseable_shell_turn("call-1"),
        unparseable_shell_turn("call-2"),
        // The model recovers, which must forget the streak entirely...
        vec![
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-good".to_string(),
                name: "shell".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta: r#"{"command":"echo recovered"}"#.to_string(),
            },
            GenerateEvent::ToolCallEnd { index: 0 },
            GenerateEvent::MessageEnd,
        ],
        // ...so this rejection is attempt 1 again, not the fatal third.
        unparseable_shell_turn("call-3"),
        vec![
            GenerateEvent::TextDelta("Done.".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ])
    .await;

    result.expect("a recovered call in between keeps the run alive");
    assert!(output.contains("attempt 1/3"), "{output}");
    assert!(!output.contains("attempt 3/3"), "{output}");
}

// ---------------------------------------------------------------------------
// ask_user_question argument validation
// ---------------------------------------------------------------------------

/// One malformed `ask_user_question` call, as it would arrive from a provider.
struct AskUserRejectionCase {
    label: &'static str,
    /// `None` models a `ToolCallStart` that never received argument deltas.
    arguments: Option<&'static str>,
    expected_code: &'static str,
}

/// Drive one turn that issues `ask_user_question` with `arguments`, followed by
/// a plain-text turn. Returns the emitted stdout and the resulting core.
async fn run_ask_user_turn(arguments: Option<&str>) -> (String, CoshCore) {
    let mut first_turn = vec![GenerateEvent::ToolCallStart {
        index: 0,
        id: "call-ask".to_string(),
        name: "ask_user_question".to_string(),
    }];
    if let Some(arguments) = arguments {
        first_turn.push(GenerateEvent::ToolCallDelta {
            index: 0,
            arguments_delta: arguments.to_string(),
        });
    }
    first_turn.push(GenerateEvent::ToolCallEnd { index: 0 });
    first_turn.push(GenerateEvent::MessageEnd);

    let provider = MockProvider::new(vec![
        first_turn,
        vec![
            GenerateEvent::TextDelta("Recovered without a question.".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ]);

    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    let tools = ToolRegistry::with_defaults_for_test();
    let mut core = CoshCore::new(config, Box::new(provider), tools);
    core.audit = CoreAuditRecorder::test_capture(&core.session_id);
    let mut reader = empty_reader().await;
    let mut output = Vec::new();

    core.handle_user_message("what now?", &mut reader, &mut output)
        .await
        .expect("turn completes");

    (String::from_utf8(output).unwrap(), core)
}

fn tool_result_text(core: &CoshCore, tool_call_id: &str) -> String {
    core.messages
        .iter()
        .find(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some(tool_call_id))
        .expect("tool result appended to the provider conversation")
        .content
        .as_text()
}

#[tokio::test]
async fn malformed_ask_user_arguments_never_reach_the_user() {
    let cases = [
        AskUserRejectionCase {
            label: "no argument delta after tool call start",
            arguments: None,
            expected_code: "empty_arguments",
        },
        AskUserRejectionCase {
            label: "empty arguments",
            arguments: Some(""),
            expected_code: "empty_arguments",
        },
        AskUserRejectionCase {
            label: "truncated json",
            arguments: Some(r#"{"question":"How should local chan"#),
            expected_code: "invalid_json",
        },
        AskUserRejectionCase {
            label: "non-object root",
            arguments: Some(r#"["How should local changes be handled?"]"#),
            expected_code: "root_not_object",
        },
        AskUserRejectionCase {
            label: "empty object",
            arguments: Some("{}"),
            expected_code: "missing_question",
        },
        AskUserRejectionCase {
            label: "null question",
            arguments: Some(r#"{"question":null}"#),
            expected_code: "question_wrong_type",
        },
        AskUserRejectionCase {
            label: "null options",
            arguments: Some(r#"{"question":"Pick one","options":null}"#),
            expected_code: "options_wrong_type",
        },
        AskUserRejectionCase {
            label: "null allow_free_text",
            arguments: Some(r#"{"question":"Pick one","allow_free_text":null}"#),
            expected_code: "allow_free_text_wrong_type",
        },
        AskUserRejectionCase {
            label: "number question",
            arguments: Some(r#"{"question":7}"#),
            expected_code: "question_wrong_type",
        },
        AskUserRejectionCase {
            label: "array question",
            arguments: Some(r#"{"question":["one","two"]}"#),
            expected_code: "question_wrong_type",
        },
        AskUserRejectionCase {
            label: "object question",
            arguments: Some(r#"{"question":{"text":"pick one"}}"#),
            expected_code: "question_wrong_type",
        },
        AskUserRejectionCase {
            label: "empty question string",
            arguments: Some(r#"{"question":""}"#),
            expected_code: "empty_question",
        },
        AskUserRejectionCase {
            label: "whitespace question string",
            arguments: Some(r#"{"question":"   "}"#),
            expected_code: "empty_question",
        },
        AskUserRejectionCase {
            label: "claude-style nested questions",
            arguments: Some(
                r#"{"questions":[{"question":"How should local changes be handled?","header":"Local changes","options":[{"label":"Stash"}],"multiSelect":false}]}"#,
            ),
            expected_code: "unsupported_nested_questions",
        },
        AskUserRejectionCase {
            label: "options wrong type",
            arguments: Some(r#"{"question":"Pick one","options":{"label":"Stash"}}"#),
            expected_code: "options_wrong_type",
        },
        AskUserRejectionCase {
            label: "option label wrong type",
            arguments: Some(r#"{"question":"Pick one","options":[{"label":42}]}"#),
            expected_code: "option_invalid",
        },
        AskUserRejectionCase {
            label: "option description wrong type",
            arguments: Some(
                r#"{"question":"Pick one","options":[{"label":"Stash","description":[]}]}"#,
            ),
            expected_code: "option_invalid",
        },
        AskUserRejectionCase {
            label: "allow_free_text wrong type",
            arguments: Some(r#"{"question":"Pick one","allow_free_text":"true"}"#),
            expected_code: "allow_free_text_wrong_type",
        },
        AskUserRejectionCase {
            label: "multi_select wrong type",
            arguments: Some(r#"{"question":"Pick one","multi_select":"no"}"#),
            expected_code: "multi_select_wrong_type",
        },
        AskUserRejectionCase {
            label: "no answer path",
            arguments: Some(r#"{"question":"Pick one","allow_free_text":false,"options":[]}"#),
            expected_code: "no_answer_path",
        },
    ];

    for case in cases {
        let (output, core) = run_ask_user_turn(case.arguments).await;

        assert!(
            !output.contains(r#""subtype":"ask_user""#),
            "case {}: no ask_user control request may be emitted, got {output}",
            case.label
        );
        assert!(
            !output.contains("control_request"),
            "case {}: rejected arguments must not open any control request, got {output}",
            case.label
        );
        assert!(
            !output.contains("Agent needs your input"),
            "case {}: generic fallback leaked into output",
            case.label
        );

        let tool_text = tool_result_text(&core, "call-ask");
        assert!(
            tool_text.contains(&format!("code={}", case.expected_code)),
            "case {}: expected code={} in {tool_text}",
            case.label,
            case.expected_code
        );

        let event_types = core.audit.captured_event_types();
        assert!(
            event_types.contains(&"tool.requested"),
            "case {}: rejection must still be audited as requested",
            case.label
        );
        assert!(
            event_types.contains(&"tool.failed"),
            "case {}: rejection must be audited as failed",
            case.label
        );
        assert!(
            !event_types.contains(&"tool.execution.started"),
            "case {}: rejected arguments must not start an execution",
            case.label
        );

        assert_eq!(
            (
                core.metrics.tool_calls_total,
                core.metrics.tool_calls_fail,
                core.metrics.tool_calls_success
            ),
            (1, 1, 0),
            "case {}: a rejected question counts once, as a failure",
            case.label
        );

        let last = core.messages.last().expect("assistant reply");
        assert_eq!(last.role, "assistant", "case {}", case.label);
        assert!(
            last.content
                .as_text()
                .contains("Recovered without a question."),
            "case {}: the provider turn after the tool error must still run",
            case.label
        );
    }
}

#[tokio::test]
async fn valid_ask_user_arguments_still_produce_a_question_and_answer() {
    let provider = MockProvider::new(vec![
        vec![
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-ask".to_string(),
                name: "ask_user_question".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta: r#"{"question":"How should local changes be handled?","options":[{"label":"Stash","description":"git stash"},{"label":"Discard"}],"allow_free_text":false,"multi_select":true}"#.to_string(),
            },
            GenerateEvent::ToolCallEnd { index: 0 },
            GenerateEvent::MessageEnd,
        ],
        vec![
            GenerateEvent::TextDelta("Stashing then.".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ]);

    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    let tools = ToolRegistry::with_defaults_for_test();
    let mut core = CoshCore::new(config, Box::new(provider), tools);
    core.audit = CoreAuditRecorder::test_capture(&core.session_id);
    let input = "{\"type\":\"control_response\",\"response\":{\"subtype\":\"success\",\"request_id\":\"req-0\",\"response\":{\"answer\":\"Stash\"}}}\n";
    let mut reader = BufReader::new(input.as_bytes()).lines();
    let mut output = Vec::new();

    core.handle_user_message("what now?", &mut reader, &mut output)
        .await
        .expect("turn completes");

    let output_str = String::from_utf8(output).unwrap();
    let request_line = output_str
        .lines()
        .find(|line| line.contains("\"subtype\":\"ask_user\""))
        .expect("ask_user control request");
    let request: serde_json::Value = serde_json::from_str(request_line).unwrap();
    assert_eq!(
        request
            .pointer("/request/question")
            .and_then(|v| v.as_str()),
        Some("How should local changes be handled?")
    );
    assert_eq!(
        request
            .pointer("/request/options/0/description")
            .and_then(|v| v.as_str()),
        Some("git stash")
    );
    assert_eq!(
        request
            .pointer("/request/allow_free_text")
            .and_then(|v| v.as_bool()),
        Some(false)
    );
    assert_eq!(
        request
            .pointer("/request/multi_select")
            .and_then(|v| v.as_bool()),
        Some(true)
    );

    assert_eq!(tool_result_text(&core, "call-ask"), "Stash");
    let event_types = core.audit.captured_event_types();
    assert!(event_types.contains(&"tool.execution.started"));
    assert!(event_types.contains(&"tool.completed"));
    // Answered questions count like any other tool call, so a single rejected
    // question cannot make the tool look like it always fails.
    assert_eq!(core.metrics.tool_calls_total, 1);
    assert_eq!(core.metrics.tool_calls_success, 1);
    assert_eq!(core.metrics.tool_calls_fail, 0);
}

#[tokio::test]
async fn malformed_tool_arguments_fail_without_executing_the_tool() {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = MockProvider::new(vec![
        vec![
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-shell".to_string(),
                name: "shell".to_string(),
            },
            GenerateEvent::ToolCallDelta {
                index: 0,
                arguments_delta: r#"{"command":"ls -l"#.to_string(),
            },
            GenerateEvent::ToolCallEnd { index: 0 },
            GenerateEvent::MessageEnd,
        ],
        vec![
            GenerateEvent::TextDelta("Retrying with valid arguments.".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ]);

    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(CountingShellTool {
        calls: Arc::clone(&calls),
    }));
    let mut core = CoshCore::new(config, Box::new(provider), tools);
    core.audit = CoreAuditRecorder::test_capture(&core.session_id);
    let mut reader = empty_reader().await;
    let mut output = Vec::new();

    core.handle_user_message("list files", &mut reader, &mut output)
        .await
        .expect("turn completes");

    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "malformed arguments must not reach the tool"
    );
    let tool_text = tool_result_text(&core, "call-shell");
    assert!(
        tool_text.contains("code=invalid_json"),
        "expected a diagnosable tool error, got {tool_text}"
    );
    let event_types = core.audit.captured_event_types();
    assert!(event_types.contains(&"tool.requested"));
    assert!(event_types.contains(&"tool.failed"));
    assert!(!event_types.contains(&"tool.execution.started"));
}

/// Tools that take no parameters legitimately arrive with empty arguments, which
/// must stay executable after the malformed-argument tightening.
#[tokio::test]
async fn empty_arguments_still_invoke_a_regular_tool() {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = MockProvider::new(vec![
        vec![
            GenerateEvent::ToolCallStart {
                index: 0,
                id: "call-shell".to_string(),
                name: "shell".to_string(),
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
    config.agent.approval_mode = "trust".to_string();
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(CountingShellTool {
        calls: Arc::clone(&calls),
    }));
    let mut core = CoshCore::new(config, Box::new(provider), tools);
    let mut reader = empty_reader().await;
    let mut output = Vec::new();

    core.handle_user_message("run it", &mut reader, &mut output)
        .await
        .expect("turn completes");

    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

/// The in-band `COSH_QUESTION:` text protocol shares the tool's validation, so a
/// schema-incompatible payload must not become a question — and because the
/// marker suppresses the assistant text, the turn must fail visibly instead of
/// ending as an ordinary reply the user never saw.
#[tokio::test]
async fn cosh_question_text_with_unsupported_schema_fails_visibly() {
    for (label, payload, expected_code) in [
        (
            "unsupported schema",
            r#"{"prompt":"How should local changes be handled?"}"#,
            "missing_question",
        ),
        (
            "explicit null question",
            r#"{"question":null}"#,
            "question_wrong_type",
        ),
        ("truncated json", r#"{"question":"How sho"#, "invalid_json"),
        ("no payload", "", "empty_arguments"),
        (
            "unanswerable question",
            r#"{"question":"Pick one","allow_free_text":false,"options":[]}"#,
            "no_answer_path",
        ),
    ] {
        let provider = MockProvider::new(vec![vec![
            GenerateEvent::TextDelta(format!("COSH_QUESTION:{payload}")),
            GenerateEvent::MessageEnd,
        ]]);

        let mut config = CoreConfig::default();
        config.agent.approval_mode = "trust".to_string();
        let tools = ToolRegistry::with_defaults_for_test();
        let mut core = CoshCore::new(config, Box::new(provider), tools);
        core.audit = CoreAuditRecorder::test_capture(&core.session_id);
        let mut reader = empty_reader().await;
        let mut output = Vec::new();

        let error = core
            .handle_user_message("what now?", &mut reader, &mut output)
            .await
            .expect_err("an invalid in-band question must fail the turn");

        assert!(
            error.contains(&format!("code={expected_code}")),
            "case {label}: expected code={expected_code} in {error}"
        );
        assert!(
            !error.contains(payload) || payload.is_empty(),
            "case {label}: the rejected payload must not be echoed: {error}"
        );

        let output_str = String::from_utf8(output).unwrap();
        assert!(
            !output_str.contains(r#""subtype":"ask_user""#),
            "case {label}: {output_str}"
        );
        assert!(
            !output_str.contains("Agent needs your input"),
            "case {label}: {output_str}"
        );
        // The marker suppressed the text, so nothing may be presented as a
        // finished assistant answer either.
        assert!(
            !output_str.contains(r#""type":"assistant""#),
            "case {label}: suppressed text must not surface as an answer: {output_str}"
        );
    }
}

/// With the question tool disabled the marker cannot become a question, so the
/// text must stay visible instead of being suppressed with nothing to replace it.
#[tokio::test]
async fn cosh_question_text_stays_visible_when_questions_are_disabled() {
    let provider = MockProvider::new(vec![vec![
        GenerateEvent::TextDelta(
            "COSH_QUESTION:{\"prompt\":\"How should local changes be handled?\"}".to_string(),
        ),
        GenerateEvent::MessageEnd,
    ]]);

    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(CountingShellTool {
        calls: Arc::new(AtomicUsize::new(0)),
    }));
    tools
        .retain_selected_tools("shell")
        .expect("selection drops the question tool");
    assert!(!tools.supports_ask_user_question());
    let mut core = CoshCore::new(config, Box::new(provider), tools);
    let mut reader = empty_reader().await;
    let mut output = Vec::new();

    core.handle_user_message("what now?", &mut reader, &mut output)
        .await
        .expect("turn completes as an ordinary reply");

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains(r#""type":"assistant""#),
        "the reply must not be swallowed: {output_str}"
    );
    assert!(
        !output_str.contains(r#""subtype":"ask_user""#),
        "{output_str}"
    );
}

#[tokio::test]
async fn cosh_question_text_with_valid_schema_still_asks() {
    let provider = MockProvider::new(vec![
        vec![
            GenerateEvent::TextDelta(
                "COSH_QUESTION:{\"question\":\"Which branch?\",\"options\":[{\"label\":\"main\"}]}"
                    .to_string(),
            ),
            GenerateEvent::MessageEnd,
        ],
        vec![
            GenerateEvent::TextDelta("Using main.".to_string()),
            GenerateEvent::MessageEnd,
        ],
    ]);

    let mut config = CoreConfig::default();
    config.agent.approval_mode = "trust".to_string();
    let tools = ToolRegistry::with_defaults_for_test();
    let mut core = CoshCore::new(config, Box::new(provider), tools);
    let input = "{\"type\":\"control_response\",\"response\":{\"subtype\":\"success\",\"request_id\":\"req-0\",\"response\":{\"answer\":\"main\"}}}\n";
    let mut reader = BufReader::new(input.as_bytes()).lines();
    let mut output = Vec::new();

    core.handle_user_message("what now?", &mut reader, &mut output)
        .await
        .expect("turn completes");

    let output_str = String::from_utf8(output).unwrap();
    let request_line = output_str
        .lines()
        .find(|line| line.contains(r#""subtype":"ask_user""#))
        .expect("ask_user control request");
    let request: serde_json::Value = serde_json::from_str(request_line).unwrap();
    assert_eq!(
        request
            .pointer("/request/question")
            .and_then(|v| v.as_str()),
        Some("Which branch?")
    );
}
