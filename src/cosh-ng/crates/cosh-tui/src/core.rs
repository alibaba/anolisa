use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use futures::StreamExt;
use tokio::io::AsyncBufReadExt;

use cosh_platform::audit::{self, LoadedPolicy};
use cosh_types::audit::Outcome;

use crate::config::CoreConfig;
use crate::context::ContextBuilder;
use crate::loop_detect::LoopDetector;
use crate::protocol::{InputMessage, OutputMessage, ShellContext, ShellControlRequest};
use crate::provider::{ContentGenerator, GenerateConfig, GenerateEvent, Message};
use crate::tool::{ToolContext, ToolKind, ToolRegistry, ToolResult};
use crate::truncator::OutputTruncator;

pub struct CoshCore {
    pub config: CoreConfig,
    pub provider: Box<dyn ContentGenerator>,
    pub tools: ToolRegistry,
    pub session_id: String,
    pub messages: Vec<Message>,
    pub model: String,
    pub shell_context: Option<ShellContext>,
    pub extra_params: Option<serde_json::Value>,
    loaded_policy: LoadedPolicy,
    request_counter: AtomicU32,
    truncator: OutputTruncator,
    loop_detector: LoopDetector,
}

impl CoshCore {
    pub fn new(
        config: CoreConfig,
        provider: Box<dyn ContentGenerator>,
        tools: ToolRegistry,
    ) -> Self {
        let model = config.resolve_provider().model;
        let (loaded_policy, warning) = LoadedPolicy::load();
        if let Some(w) = warning {
            eprintln!("[cosh-core] {w}");
        }

        Self {
            config,
            provider,
            tools,
            session_id: uuid::Uuid::new_v4().to_string(),
            messages: Vec::new(),
            model,
            shell_context: None,
            extra_params: None,
            loaded_policy,
            request_counter: AtomicU32::new(0),
            truncator: OutputTruncator::default(),
            loop_detector: LoopDetector::new(),
        }
    }

    pub fn tool_names(&self) -> Vec<String> {
        let mut names = self.tools.names();
        names.push("ask_user_question".to_string());
        names.sort();
        names
    }

    pub fn emit<W: Write>(&self, writer: &mut W, msg: &OutputMessage) {
        if let Ok(json) = serde_json::to_string(msg) {
            let _ = writeln!(writer, "{json}");
            let _ = writer.flush();
        }
    }

    fn next_request_id(&self) -> String {
        let n = self.request_counter.fetch_add(1, Ordering::SeqCst);
        format!("req-{n}")
    }

    pub fn cwd(&self) -> PathBuf {
        self.shell_context
            .as_ref()
            .map(|ctx| ctx.cwd.clone())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    fn classify_tool(&self, tool_name: &str, params: &serde_json::Value) -> Outcome {
        let mode = self.config.agent.approval_mode.as_str();

        if mode == "trust" {
            return Outcome::Allow;
        }

        let tool = match self.tools.get(tool_name) {
            Some(t) => t,
            None => return Outcome::Deny,
        };

        let kind = tool.kind();

        if kind == ToolKind::ReadOnly {
            return Outcome::Allow;
        }

        if mode == "suggest" {
            return Outcome::RequireApproval;
        }

        if kind == ToolKind::ShellExec {
            if let Some(command) = params.get("command").and_then(|v| v.as_str()) {
                match audit::parse_action_string(command) {
                    Ok(action) => return audit::classify(&action, &self.loaded_policy).outcome,
                    Err(_) => return Outcome::RequireApproval,
                }
            }
        }

        if kind == ToolKind::FileEdit && mode == "auto" {
            return Outcome::Allow;
        }

        if mode == "auto" {
            Outcome::Allow
        } else {
            Outcome::RequireApproval
        }
    }

    pub async fn handle_user_message<W, R>(
        &mut self,
        content: &str,
        reader: &mut tokio::io::Lines<R>,
        writer: &mut W,
    ) -> Result<(), String>
    where
        W: Write,
        R: AsyncBufReadExt + Unpin,
    {
        self.messages.push(Message::user(content));

        let tool_decls = self.tools.declarations();
        let generate_config = GenerateConfig {
            model: self.model.clone(),
            max_tokens: 4096,
            temperature: None,
            include_usage: false,
            extra_params: self.extra_params.clone(),
        };

        let system_prompt = ContextBuilder::build_system_prompt(
            &self.cwd(),
            &self.tool_names(),
            &self.config.agent.approval_mode,
            self.config.ai.output_language.as_deref(),
        );

        let max_turns = self.config.agent.max_turns;

        for _turn in 0..max_turns {
            let mut msgs_with_system = vec![Message::system(&system_prompt)];
            msgs_with_system.extend(self.messages.clone());

            let mut stream = self
                .provider
                .generate(&msgs_with_system, &tool_decls, &generate_config)
                .await?;

            let mut text_buf = String::new();
            let mut tool_calls: Vec<PendingToolCall> = Vec::new();
            let mut block_index: u32 = 0;
            let mut text_block_started = false;
            let mut thinking_block_started = false;
            let mut suppress_stream_text = false;
            let mut tool_call_seen = false;

            self.emit(writer, &OutputMessage::stream_message_start());

            while let Some(event) = stream.next().await {
                match event {
                    GenerateEvent::ThinkingDelta(delta) => {
                        if !thinking_block_started {
                            self.emit(writer, &OutputMessage::stream_thinking_start(block_index));
                            thinking_block_started = true;
                        }
                        self.emit(
                            writer,
                            &OutputMessage::stream_thinking_delta(block_index, &delta),
                        );
                    }
                    GenerateEvent::TextDelta(delta) => {
                        if thinking_block_started {
                            self.emit(writer, &OutputMessage::stream_block_stop(block_index));
                            block_index += 1;
                            thinking_block_started = false;
                        }
                        if !tool_call_seen && !text_block_started {
                            self.emit(writer, &OutputMessage::stream_text_start(block_index));
                            text_block_started = true;
                        }
                        text_buf.push_str(&delta);
                        if !suppress_stream_text && !tool_call_seen {
                            if text_buf.contains("COSH_QUESTION:") {
                                suppress_stream_text = true;
                            } else {
                                self.emit(
                                    writer,
                                    &OutputMessage::stream_text_delta(block_index, &delta),
                                );
                            }
                        }
                    }
                    GenerateEvent::ToolCallStart { index, id, name } => {
                        tool_call_seen = true;
                        if thinking_block_started {
                            self.emit(writer, &OutputMessage::stream_block_stop(block_index));
                            block_index += 1;
                            thinking_block_started = false;
                        }
                        if text_block_started {
                            self.emit(writer, &OutputMessage::stream_block_stop(block_index));
                            block_index += 1;
                            text_block_started = false;
                        }
                        let idx = index as usize;
                        if tool_calls.len() <= idx {
                            tool_calls.resize_with(idx + 1, PendingToolCall::default);
                        }
                        tool_calls[idx].id = id.clone();
                        tool_calls[idx].name = name.clone();
                        tool_calls[idx].block_index = block_index;
                        tool_calls[idx].block_closed = false;
                        self.emit(
                            writer,
                            &OutputMessage::stream_tool_use_start(block_index, &id, &name),
                        );
                        block_index += 1;
                    }
                    GenerateEvent::ToolCallDelta {
                        index,
                        arguments_delta,
                    } => {
                        let idx = index as usize;
                        if tool_calls.len() <= idx {
                            tool_calls.resize_with(idx + 1, PendingToolCall::default);
                        }
                        let bi = tool_calls[idx].block_index;
                        self.emit(
                            writer,
                            &OutputMessage::stream_tool_use_delta(bi, &arguments_delta),
                        );
                        tool_calls[idx].arguments.push_str(&arguments_delta);
                    }
                    GenerateEvent::ToolCallEnd { index } => {
                        let idx = index as usize;
                        if idx < tool_calls.len() {
                            let bi = tool_calls[idx].block_index;
                            self.emit(writer, &OutputMessage::stream_block_stop(bi));
                            tool_calls[idx].block_closed = true;
                            block_index = block_index.max(bi + 1);
                        }
                    }
                    GenerateEvent::Usage { .. } => {}
                    GenerateEvent::MessageEnd => break,
                    GenerateEvent::Error(e) => return Err(e),
                }
            }
            drop(stream);

            if thinking_block_started {
                self.emit(writer, &OutputMessage::stream_block_stop(block_index));
                block_index += 1;
            }
            if text_block_started {
                self.emit(writer, &OutputMessage::stream_block_stop(block_index));
                block_index += 1;
            }
            for tc in &mut tool_calls {
                if !tc.id.is_empty() && !tc.block_closed {
                    self.emit(writer, &OutputMessage::stream_block_stop(tc.block_index));
                    tc.block_closed = true;
                    block_index = block_index.max(tc.block_index + 1);
                }
            }
            let emit_visible_text = tool_calls.is_empty()
                && !text_buf.is_empty()
                && !text_buf.contains("COSH_QUESTION:");
            let _ = block_index;
            self.emit(writer, &OutputMessage::stream_message_stop());

            if emit_visible_text {
                self.emit(
                    writer,
                    &OutputMessage::assistant_text(&self.session_id, &text_buf),
                );
            }

            if tool_calls.is_empty() {
                if let Some(synthetic) = parse_cosh_question_text(&text_buf) {
                    let result = self
                        .handle_ask_user("synthetic-ask", &synthetic, reader, writer)
                        .await;
                    if result.is_error {
                        self.messages.push(Message::assistant(&text_buf));
                        return Ok(());
                    }
                    self.messages.push(Message::assistant(&text_buf));
                    self.messages.push(Message::user(&format!(
                        "User answered the question: {}",
                        result.output
                    )));
                    continue;
                }
                self.messages.push(Message::assistant(&text_buf));
                return Ok(());
            }

            let tc_infos: Vec<crate::provider::ToolCallInfo> = tool_calls
                .iter()
                .filter(|tc| !tc.name.is_empty())
                .map(|tc| crate::provider::ToolCallInfo {
                    id: tc.id.clone(),
                    call_type: "function".to_string(),
                    function: crate::provider::ToolCallFunction {
                        name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                    },
                })
                .collect();
            self.messages
                .push(Message::assistant_with_tool_calls(&text_buf, tc_infos));

            let ctx = ToolContext {
                cwd: self.cwd(),
                session_id: self.session_id.clone(),
            };

            let mut interrupted = false;

            for tc in &tool_calls {
                if tc.name.is_empty() {
                    continue;
                }

                let params: serde_json::Value =
                    serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null);

                if tc.name == "ask_user_question" {
                    let result = self.handle_ask_user(&tc.id, &params, reader, writer).await;
                    self.messages.push(Message::tool_result(
                        &tc.id,
                        &result.output,
                        result.is_error,
                    ));
                    if interrupted {
                        return Ok(());
                    }
                    continue;
                }

                let outcome = self.classify_tool(&tc.name, &params);

                let result = match outcome {
                    Outcome::Allow => {
                        let result = self.execute_tool(&tc.name, params, &ctx).await;
                        self.emit_provider_native_tool_result(writer, &tc.id, &result);
                        result
                    }
                    Outcome::RequireApproval => {
                        let request_id = self.next_request_id();
                        self.emit(
                            writer,
                            &OutputMessage::can_use_tool(
                                &request_id,
                                &tc.name,
                                params.clone(),
                                &tc.id,
                            ),
                        );

                        let accepts_host_executed_shell = self
                            .tools
                            .get(&tc.name)
                            .map(|tool| tool.kind() == ToolKind::ShellExec)
                            .unwrap_or(false);
                        match self
                            .wait_for_approval(&request_id, accepts_host_executed_shell, reader)
                            .await
                        {
                            ApprovalResult::Allowed => {
                                let result = self.execute_tool(&tc.name, params, &ctx).await;
                                self.emit_provider_native_tool_result(writer, &tc.id, &result);
                                result
                            }
                            ApprovalResult::HostExecutedShell { llm_content } => {
                                ToolResult::success(llm_content)
                            }
                            ApprovalResult::Denied(reason) => ToolResult::error(format!(
                                "Tool call denied: {}",
                                reason.unwrap_or_else(|| "no reason given".to_string())
                            )),
                            ApprovalResult::Interrupted => {
                                interrupted = true;
                                ToolResult::error("Interrupted by user")
                            }
                        }
                    }
                    Outcome::Deny => {
                        ToolResult::error(format!("Tool '{}' denied by security policy", tc.name))
                    }
                };

                self.messages.push(Message::tool_result(
                    &tc.id,
                    &result.output,
                    result.is_error,
                ));

                if self.loop_detector.record_action(&tc.name, &tc.arguments) {
                    self.messages
                        .push(Message::system(LoopDetector::loop_warning()));
                }

                if interrupted {
                    return Ok(());
                }
            }
        }

        Err(format!("Agent exceeded max turns ({max_turns})"))
    }

    fn emit_provider_native_tool_result<W: Write>(
        &self,
        writer: &mut W,
        tool_use_id: &str,
        result: &ToolResult,
    ) {
        self.emit(
            writer,
            &OutputMessage::tool_result(
                &self.session_id,
                tool_use_id,
                &result.output,
                result.is_error,
            ),
        );
    }

    async fn execute_tool(
        &self,
        name: &str,
        params: serde_json::Value,
        ctx: &ToolContext,
    ) -> ToolResult {
        let result = match self.tools.get(name) {
            Some(tool) => match tool.invoke(params, ctx).await {
                Ok(r) => r,
                Err(e) => return ToolResult::error(e),
            },
            None => return ToolResult::error(format!("Unknown tool: {name}")),
        };

        let (output, _truncated) = self.truncator.truncate(&result.output);
        ToolResult {
            output,
            is_error: result.is_error,
        }
    }

    async fn handle_ask_user<W, R>(
        &self,
        _tool_use_id: &str,
        params: &serde_json::Value,
        reader: &mut tokio::io::Lines<R>,
        writer: &mut W,
    ) -> ToolResult
    where
        W: Write,
        R: AsyncBufReadExt + Unpin,
    {
        let question = params
            .get("question")
            .and_then(|v| v.as_str())
            .unwrap_or("Agent needs your input")
            .to_string();
        let options: Vec<crate::protocol::AskUserOption> = params
            .get("options")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let label = item
                            .get("label")
                            .and_then(|l| l.as_str())
                            .or_else(|| item.as_str())?;
                        Some(crate::protocol::AskUserOption {
                            label: label.to_string(),
                            description: item
                                .get("description")
                                .and_then(|d| d.as_str())
                                .map(|s| s.to_string()),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let allow_free_text = params
            .get("allow_free_text")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let multi_select = params
            .get("multi_select")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let request_id = self.next_request_id();
        self.emit(
            writer,
            &OutputMessage::ControlRequest {
                request_id: request_id.clone(),
                request: crate::protocol::CoreControlRequest::AskUser {
                    question,
                    options,
                    allow_free_text,
                    multi_select,
                },
            },
        );

        match self.wait_for_answer(&request_id, reader).await {
            Some(answer) => ToolResult::success(answer),
            None => ToolResult::error("User did not answer (interrupted or disconnected)"),
        }
    }

    async fn wait_for_answer<R: AsyncBufReadExt + Unpin>(
        &self,
        expected_request_id: &str,
        reader: &mut tokio::io::Lines<R>,
    ) -> Option<String> {
        while let Ok(Some(line)) = reader.next_line().await {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            let msg: InputMessage = match serde_json::from_str(&line) {
                Ok(m) => m,
                Err(_) => continue,
            };
            match msg {
                InputMessage::ControlResponse { response } => {
                    if response.request_id != expected_request_id {
                        continue;
                    }
                    return response.response.answer;
                }
                InputMessage::ControlRequest { request, .. } => {
                    if matches!(request, ShellControlRequest::Interrupt) {
                        self.provider.cancel();
                        return None;
                    }
                }
                _ => {}
            }
        }
        None
    }

    async fn wait_for_approval<R: AsyncBufReadExt + Unpin>(
        &self,
        expected_request_id: &str,
        accepts_host_executed_shell: bool,
        reader: &mut tokio::io::Lines<R>,
    ) -> ApprovalResult {
        while let Ok(Some(line)) = reader.next_line().await {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }

            let msg: InputMessage = match serde_json::from_str(&line) {
                Ok(m) => m,
                Err(_) => continue,
            };

            match msg {
                InputMessage::ControlResponse { response } => {
                    if response.request_id != expected_request_id {
                        continue;
                    }
                    match response.response.behavior.as_deref() {
                        Some("allow") => return ApprovalResult::Allowed,
                        Some("deny") => return ApprovalResult::Denied(response.response.message),
                        Some("host_executed_shell") => {
                            if !accepts_host_executed_shell {
                                return ApprovalResult::Denied(Some(
                                    "host_executed_shell is only valid for shell tools".to_string(),
                                ));
                            }
                            let Some(result) = response.response.result else {
                                return ApprovalResult::Denied(Some(
                                    "host_executed_shell response missing result".to_string(),
                                ));
                            };
                            return ApprovalResult::HostExecutedShell {
                                llm_content: result.llm_content,
                            };
                        }
                        _ => return ApprovalResult::Denied(Some("unknown response".to_string())),
                    }
                }
                InputMessage::ControlRequest { request, .. } => {
                    if matches!(request, ShellControlRequest::Interrupt) {
                        self.provider.cancel();
                        return ApprovalResult::Interrupted;
                    }
                }
                _ => {}
            }
        }
        ApprovalResult::Interrupted
    }
}

enum ApprovalResult {
    Allowed,
    Denied(Option<String>),
    HostExecutedShell { llm_content: String },
    Interrupted,
}

#[derive(Default, Clone)]
struct PendingToolCall {
    id: String,
    name: String,
    arguments: String,
    block_index: u32,
    block_closed: bool,
}

fn parse_cosh_question_text(text: &str) -> Option<serde_json::Value> {
    let marker = "COSH_QUESTION:";
    let json_text = text.split_once(marker)?.1.trim().lines().next()?.trim();
    serde_json::from_str(json_text).ok()
}

#[cfg(test)]
mod tests {
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
        let tools = ToolRegistry::with_defaults();
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
        let tools = ToolRegistry::with_defaults();
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
        let tools = ToolRegistry::with_defaults();
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
        let tools = ToolRegistry::with_defaults();
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
        let tools = ToolRegistry::with_defaults();

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
        let tools = ToolRegistry::with_defaults();
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
        let tools = ToolRegistry::with_defaults();
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
}
