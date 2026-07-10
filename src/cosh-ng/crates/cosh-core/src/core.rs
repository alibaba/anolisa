use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use futures::StreamExt;
use tokio::io::AsyncBufReadExt;

use cosh_platform::audit::LoadedPolicy;
use cosh_types::audit::Outcome;

use crate::auth::{
    apply_auth_credentials, builtin_auth_providers, is_auth_error, wait_for_auth_response,
};
use crate::config::{self, CoreConfig};
use crate::context::ContextBuilder;
use crate::hook::{HookDecision, HookNotification, HookSystem};
use crate::loop_detect::LoopDetector;
use crate::metrics::TurnMetrics;
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
    pub hook_system: HookSystem,
    pub metrics: TurnMetrics,
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
            tracing::warn!("{w}");
        }

        let hook_system = HookSystem::from_config(&config.hooks);

        Self {
            config,
            provider,
            tools,
            session_id: uuid::Uuid::new_v4().to_string(),
            messages: Vec::new(),
            model,
            shell_context: None,
            extra_params: None,
            hook_system,
            metrics: TurnMetrics::default(),
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

    fn emit_hook_notifications<W: Write>(
        &self,
        writer: &mut W,
        notifications: &[HookNotification],
        tool_use_id: Option<&str>,
    ) {
        for n in notifications {
            self.emit(
                writer,
                &OutputMessage::hook_notification(
                    &n.hook_name,
                    &n.message,
                    tool_use_id,
                    n.decision.as_deref(),
                ),
            );
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

    fn classify_tool(&self, tool_name: &str, _params: &serde_json::Value) -> Outcome {
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
            return Outcome::RequireApproval;
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
        // Generate a unique run_id for this agent run.
        let run_id = uuid::Uuid::new_v4().to_string();
        self.hook_system.set_run_id(run_id);

        // ─── Hook: UserPromptSubmit ───
        let cwd_str = self.cwd().to_string_lossy().to_string();
        let prompt_result = self
            .hook_system
            .fire_user_prompt_submit(&self.session_id, &cwd_str, content)
            .await;

        if let HookDecision::Block(reason) = &prompt_result.decision {
            // Block: no approval panel, notifications go to Governance fallback
            self.emit_hook_notifications(writer, &prompt_result.notifications, None);
            self.emit(
                writer,
                &OutputMessage::assistant_text(
                    &self.session_id,
                    &format!("Prompt blocked by hook: {reason}"),
                ),
            );
            return Ok(());
        }

        if matches!(prompt_result.decision, HookDecision::Ask) {
            let request_id = self.next_request_id();
            let synthetic_id = format!("prompt:{request_id}");

            // Extract the first hook name for the virtual HOOK: tool_name.
            let hook_name = prompt_result
                .notifications
                .first()
                .map(|n| n.hook_name.as_str())
                .unwrap_or("unknown");

            // Emit notifications (or fallback) with synthetic tool_use_id so
            // cosh-shell stores them in pending_hook_notifications.
            if prompt_result.notifications.is_empty() {
                // Hook returned ask but provided no reason/systemMessage — emit fallback.
                self.emit(
                    writer,
                    &OutputMessage::hook_notification(
                        hook_name,
                        "A hook requires your approval before this action can proceed.",
                        Some(&synthetic_id),
                        Some("ask"),
                    ),
                );
            } else {
                self.emit_hook_notifications(
                    writer,
                    &prompt_result.notifications,
                    Some(&synthetic_id),
                );
            }

            // Emit approval request with HOOK: prefix and empty input.
            self.emit(
                writer,
                &OutputMessage::can_use_tool(
                    &request_id,
                    &format!("HOOK:{hook_name}"),
                    serde_json::json!({}),
                    &synthetic_id,
                    true, // hook_requires_approval
                ),
            );

            match self.wait_for_approval(&request_id, false, reader).await {
                ApprovalResult::Allowed => { /* user confirmed, continue */ }
                ApprovalResult::Denied(reason) => {
                    self.emit(
                        writer,
                        &OutputMessage::assistant_text(
                            &self.session_id,
                            &format!(
                                "Prompt rejected: {}",
                                reason.unwrap_or_else(|| "user cancelled".to_string())
                            ),
                        ),
                    );
                    return Ok(());
                }
                ApprovalResult::Interrupted | ApprovalResult::HostExecutedShell { .. } => {
                    return Ok(());
                }
            }
        } else {
            // allow / passthrough: notifications without tool_use_id go to
            // deferred_events → Governance panel at end of agent run.
            self.emit_hook_notifications(writer, &prompt_result.notifications, None);
        }

        self.messages.push(Message::user(content));

        // Inject additional context from hooks
        if let Some(ref ctx) = prompt_result.additional_context {
            self.messages
                .push(Message::system(&format!("[Hook context] {ctx}")));
        }

        let tool_decls = self.tools.declarations();
        let skill_summaries = self.tools.skill_summaries().await;
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
            &skill_summaries,
            &self.config.agent.approval_mode,
            self.config.ai.output_language.as_deref(),
        );

        let max_turns = self.config.agent.max_turns;

        for _turn in 0..max_turns {
            // ─── Hook: BeforeModel ───
            let before_model_result = self
                .hook_system
                .fire_before_model(&self.session_id, &cwd_str, &self.model, &self.messages)
                .await;
            self.emit_hook_notifications(writer, &before_model_result.notifications, None);

            let mut msgs_with_system = vec![Message::system(&system_prompt)];
            msgs_with_system.extend(self.messages.clone());

            // ─── SLS: API request timing ───
            self.metrics.api_requests += 1;
            let api_start = Instant::now();

            let stream_result = self
                .provider
                .generate(&msgs_with_system, &tool_decls, &generate_config)
                .await;

            let mut stream = match stream_result {
                Ok(s) => s,
                Err(e) if is_auth_error(&e) => {
                    self.metrics.api_errors += 1;
                    self.metrics.api_latency_ms += api_start.elapsed().as_millis() as u64;
                    // Attempt re-auth
                    if self.try_reauth(reader, writer).await {
                        continue; // Retry the turn with new credentials
                    }
                    return Err(e);
                }
                Err(e) => {
                    self.metrics.api_errors += 1;
                    self.metrics.api_latency_ms += api_start.elapsed().as_millis() as u64;
                    return Err(e);
                }
            };

            let mut text_buf = String::new();
            let mut tool_calls: Vec<PendingToolCall> = Vec::new();
            let mut usage_info: Option<(u32, u32, u32)> = None;
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
                    GenerateEvent::Usage {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens,
                    } => {
                        usage_info = Some((prompt_tokens, completion_tokens, total_tokens));
                        // ─── SLS: token usage ───
                        self.metrics.tokens_input += prompt_tokens as u64;
                        self.metrics.tokens_output += completion_tokens as u64;
                        self.metrics.tokens_total += total_tokens as u64;
                    }
                    GenerateEvent::MessageEnd => {
                        self.metrics.api_latency_ms += api_start.elapsed().as_millis() as u64;
                        break;
                    }
                    GenerateEvent::Error(e) => {
                        self.metrics.api_errors += 1;
                        self.metrics.api_latency_ms += api_start.elapsed().as_millis() as u64;
                        return Err(e);
                    }
                }
            }
            drop(stream);

            // ─── Hook: AfterModel ───
            let after_model_result = self
                .hook_system
                .fire_after_model(
                    &self.session_id,
                    &cwd_str,
                    !tool_calls.is_empty(),
                    &text_buf,
                    &self.model,
                    &self.messages,
                    usage_info,
                )
                .await;
            self.emit_hook_notifications(writer, &after_model_result.notifications, None);

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

                // ─── Hook: Stop ───
                let stop_result = self
                    .hook_system
                    .fire_stop(&self.session_id, &cwd_str, &text_buf)
                    .await;
                self.emit_hook_notifications(writer, &stop_result.notifications, None);
                if let HookDecision::Block(reason) = &stop_result.decision {
                    self.messages.push(Message::assistant(&text_buf));
                    self.messages.push(Message::user(&format!(
                        "[Hook rejected response] {reason}. Please revise your answer."
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
                project_root: self.cwd(),
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

                if self
                    .tools
                    .get(&tc.name)
                    .map(|tool| tool.kind() == ToolKind::ShellEvidence)
                    .unwrap_or(false)
                {
                    let result = self
                        .handle_shell_evidence(&tc.id, &params, reader, writer)
                        .await;
                    self.emit_provider_native_tool_result(writer, &tc.id, &result);
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

                // 当工具是 skill 且 action=invoke 时，预查 skill_context 透传给
                // hook（供 agent-sec-core skill-ledger 等扩展使用）。
                let skill_context = if tc.name == "skill"
                    && params
                        .get("action")
                        .and_then(|v| v.as_str())
                        .unwrap_or("invoke")
                        == "invoke"
                {
                    let skill_name = params.get("name").and_then(|v| v.as_str());
                    if let Some(name) = skill_name {
                        self.tools.lookup_skill(name).await.map(|s| {
                            serde_json::json!({
                                "skill_name": s.name,
                                "file_path": s.file_path.to_string_lossy(),
                            })
                        })
                    } else {
                        None
                    }
                } else {
                    None
                };

                // ─── Hook: PreToolUse ───
                let hook_result = self
                    .hook_system
                    .fire_pre_tool_use(
                        &self.session_id,
                        &cwd_str,
                        &tc.id,
                        &tc.name,
                        &params,
                        skill_context.as_ref(),
                    )
                    .await;
                self.emit_hook_notifications(writer, &hook_result.notifications, Some(&tc.id));

                let (outcome, params) = match hook_result.decision {
                    HookDecision::Block(reason) => {
                        // ─── SLS: hook-blocked tool call counts as total + fail ───
                        self.metrics.tool_calls_total += 1;
                        self.metrics.tool_calls_fail += 1;
                        let result = ToolResult::error(format!("Blocked by hook: {reason}"));
                        self.messages.push(Message::tool_result(
                            &tc.id,
                            &result.output,
                            result.is_error,
                        ));
                        continue;
                    }
                    HookDecision::Ask => {
                        // Apply tool_input_patch even when decision is Ask so that
                        // sandbox-guard wrapping is preserved through the approval flow.
                        let params = if let Some(patch) = hook_result.tool_input_patch.clone() {
                            crate::hook::merge_json_pub(params, patch)
                        } else {
                            params
                        };
                        (Outcome::RequireApproval, params)
                    }
                    _ => {
                        let params = if let Some(patch) = hook_result.tool_input_patch {
                            crate::hook::merge_json_pub(params, patch)
                        } else {
                            params
                        };
                        (outcome, params)
                    }
                };

                let params_for_post_hook = params.clone();

                let mut tool_result_already_emitted = false;
                let tool_start = Instant::now();
                let result = match outcome {
                    Outcome::Allow => {
                        let result = self.execute_tool(&tc.name, params, &ctx).await;
                        self.emit_provider_native_tool_result(writer, &tc.id, &result);
                        tool_result_already_emitted = true;
                        result
                    }
                    Outcome::RequireApproval => {
                        let hook_requires_approval =
                            matches!(hook_result.decision, HookDecision::Ask);
                        let request_id = self.next_request_id();
                        self.emit(
                            writer,
                            &OutputMessage::can_use_tool(
                                &request_id,
                                &tc.name,
                                params.clone(),
                                &tc.id,
                                hook_requires_approval,
                            ),
                        );

                        let accepts_host_executed_shell = self
                            .tools
                            .get(&tc.name)
                            .map(|tool| tool.kind() == ToolKind::ShellExec)
                            .unwrap_or(false);
                        // ─── SLS: approval wait timing ───
                        let approval_start = Instant::now();
                        let approval_result = self
                            .wait_for_approval(&request_id, accepts_host_executed_shell, reader)
                            .await;
                        self.metrics.approval_wait_ms +=
                            approval_start.elapsed().as_millis() as u64;
                        self.metrics.approval_count += 1;
                        match approval_result {
                            ApprovalResult::Allowed => {
                                self.metrics.approval_allow += 1;
                                let result = self.execute_tool(&tc.name, params, &ctx).await;
                                self.emit_provider_native_tool_result(writer, &tc.id, &result);
                                tool_result_already_emitted = true;
                                result
                            }
                            ApprovalResult::HostExecutedShell {
                                llm_content,
                                exit_code,
                            } => {
                                self.metrics.approval_allow += 1;
                                let is_error = exit_code.is_some_and(|c| c != 0);
                                ToolResult {
                                    output: llm_content,
                                    is_error,
                                }
                            }
                            ApprovalResult::Denied(reason) => {
                                self.metrics.approval_deny += 1;
                                ToolResult::error(format!(
                                    "Tool call denied: {}",
                                    reason.unwrap_or_else(|| "no reason given".to_string())
                                ))
                            }
                            ApprovalResult::Interrupted => {
                                self.metrics.approval_deny += 1;
                                interrupted = true;
                                ToolResult::error("Interrupted by user")
                            }
                        }
                    }
                    Outcome::Deny => {
                        self.metrics.approval_deny += 1;
                        ToolResult::error(format!("Tool '{}' denied by security policy", tc.name))
                    }
                };
                // ─── SLS: tool call total/duration/success/fail ───
                self.metrics.tool_calls_total += 1;
                self.metrics.tool_calls_duration_ms += tool_start.elapsed().as_millis() as u64;
                if result.is_error {
                    self.metrics.tool_calls_fail += 1;
                } else {
                    self.metrics.tool_calls_success += 1;
                }

                // ─── Hook: PostToolUse ───
                let post_hook = self
                    .hook_system
                    .fire_post_tool_use(
                        &self.session_id,
                        &cwd_str,
                        &tc.id,
                        &tc.name,
                        &params_for_post_hook,
                        &result.output,
                        skill_context.as_ref(),
                    )
                    .await;
                self.emit_hook_notifications(writer, &post_hook.notifications, Some(&tc.id));

                let mut result = if let HookDecision::Block(reason) = &post_hook.decision {
                    ToolResult::error(format!("Post-tool hook denied: {reason}"))
                } else if let Some(ref extra) = post_hook.additional_context {
                    ToolResult {
                        output: format!("{}\n[Hook context] {extra}", result.output),
                        is_error: result.is_error,
                    }
                } else {
                    result
                };

                // ─── Hook: PostToolUseFailure ───
                if result.is_error {
                    // Emit tool_result BEFORE running PostToolUseFailure hooks, but only
                    // if it hasn't been emitted yet. The Allowed path already emits
                    // in-line; HostExecutedShell needs this early emit to prevent
                    // cosh-shell stall timeout from racing against hook execution.
                    if !tool_result_already_emitted {
                        self.emit_provider_native_tool_result(writer, &tc.id, &result);
                    }
                    let failure_hook = self
                        .hook_system
                        .fire_post_tool_use_failure(
                            &self.session_id,
                            &cwd_str,
                            &tc.id,
                            &tc.name,
                            &params_for_post_hook,
                            &result.output,
                            skill_context.as_ref(),
                        )
                        .await;
                    self.emit_hook_notifications(writer, &failure_hook.notifications, Some(&tc.id));

                    // ─── Sandbox Bypass ───
                    // If a hook requests sandbox bypass, present an approval
                    // panel with the original (un-sandboxed) command.
                    // ─── SLS: sandbox blocked ───
                    if let Some(bypass) = failure_hook.sandbox_bypass_request {
                        self.metrics.sandbox_blocked += 1;
                        self.emit(
                            writer,
                            &OutputMessage::hook_notification(
                                "sandbox-failure-handler",
                                &bypass.reason,
                                Some(&tc.id),
                                Some("ask"),
                            ),
                        );
                        let request_id = self.next_request_id();
                        let bypass_tool_use_id = format!("{}-bypass", tc.id);
                        self.emit(
                            writer,
                            &OutputMessage::can_use_tool(
                                &request_id,
                                &tc.name,
                                serde_json::json!({"command": &bypass.original_command}),
                                &bypass_tool_use_id,
                                true,
                            ),
                        );

                        match self.wait_for_approval(&request_id, true, reader).await {
                            ApprovalResult::Allowed => {
                                self.hook_system.set_hook_disabled("sandbox-guard", true);
                                let retry_params =
                                    serde_json::json!({"command": &bypass.original_command});
                                let retry = self.execute_tool(&tc.name, retry_params, &ctx).await;
                                // Re-enable immediately after execute, before any other
                                // operation. execute_tool returns ToolResult (infallible),
                                // so this line is always reached.
                                self.hook_system.set_hook_disabled("sandbox-guard", false);
                                self.emit_provider_native_tool_result(writer, &tc.id, &retry);
                                result = retry;
                            }
                            ApprovalResult::HostExecutedShell {
                                llm_content,
                                exit_code,
                            } => {
                                let is_error = exit_code.is_some_and(|c| c != 0);
                                result = ToolResult {
                                    output: llm_content,
                                    is_error,
                                };
                            }
                            _ => { /* denied / interrupted: keep original error */ }
                        }
                    }
                }

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

    async fn handle_shell_evidence<W, R>(
        &self,
        tool_use_id: &str,
        params: &serde_json::Value,
        reader: &mut tokio::io::Lines<R>,
        writer: &mut W,
    ) -> ToolResult
    where
        W: Write,
        R: AsyncBufReadExt + Unpin,
    {
        let Some(action) = params.get("action").and_then(|v| v.as_str()) else {
            return ToolResult::error("cosh_shell_evidence missing required action");
        };

        let request_id = self.next_request_id();
        match action {
            "list_commands" => {
                if params.get("output_id").is_some()
                    || params.get("lines").is_some()
                    || params.get("bypass_recent_filter").is_some()
                {
                    return ToolResult::error(
                        "cosh_shell_evidence action=list_commands accepts only limit and cursor",
                    );
                }
                let limit = params
                    .get("limit")
                    .map(|v| {
                        v.as_u64().ok_or_else(|| {
                            ToolResult::error("cosh_shell_evidence limit must be an integer")
                        })
                    })
                    .transpose();
                let limit = match limit {
                    Ok(limit) => limit.unwrap_or(20).clamp(1, 100) as u16,
                    Err(result) => return result,
                };
                let cursor = match params.get("cursor") {
                    Some(serde_json::Value::Null) | None => None,
                    Some(v) => match v.as_str() {
                        Some(s) => Some(s),
                        None => {
                            return ToolResult::error(
                                "cosh_shell_evidence cursor must be a string or null",
                            );
                        }
                    },
                };
                self.emit(
                    writer,
                    &OutputMessage::shell_evidence_list_commands(
                        &request_id,
                        tool_use_id,
                        limit,
                        cursor,
                    ),
                );
            }
            "read_output" => {
                let Some(output_id) = params.get("output_id").and_then(|v| v.as_str()) else {
                    return ToolResult::error(
                        "cosh_shell_evidence action=read_output missing required output_id",
                    );
                };
                let direction = params
                    .get("direction")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tail");
                if direction != "head" && direction != "tail" {
                    return ToolResult::error(
                        "cosh_shell_evidence action=read_output direction must be head or tail",
                    );
                }
                let lines = params
                    .get("lines")
                    .map(|v| {
                        v.as_u64().ok_or_else(|| {
                            ToolResult::error(
                                "cosh_shell_evidence action=read_output lines must be an integer",
                            )
                        })
                    })
                    .transpose();
                let lines = match lines {
                    Ok(lines) => lines.unwrap_or(120).clamp(1, 300) as u16,
                    Err(result) => return result,
                };
                let bypass_recent_filter = match params.get("bypass_recent_filter") {
                    Some(value) => match value.as_bool() {
                        Some(value) => value,
                        None => {
                            return ToolResult::error(
                                "cosh_shell_evidence action=read_output bypass_recent_filter must be a boolean",
                            );
                        }
                    },
                    None => false,
                };

                self.emit(
                    writer,
                    &OutputMessage::shell_evidence_read_output(
                        &request_id,
                        tool_use_id,
                        output_id,
                        direction,
                        lines,
                        bypass_recent_filter,
                    ),
                );
            }
            _ => {
                return ToolResult::error(
                    "cosh_shell_evidence action must be list_commands or read_output",
                );
            }
        }

        self.wait_for_shell_evidence(&request_id, reader).await
    }

    async fn wait_for_shell_evidence<R: AsyncBufReadExt + Unpin>(
        &self,
        expected_request_id: &str,
        reader: &mut tokio::io::Lines<R>,
    ) -> ToolResult {
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
                    if response.response.behavior.as_deref() != Some("shell_evidence") {
                        return ToolResult::error("cosh_shell_evidence received unknown response");
                    }
                    let Some(result) = response.response.result else {
                        return ToolResult::error("cosh_shell_evidence response missing result");
                    };
                    let is_error = result
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("is_error"))
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false)
                        || result
                            .metadata
                            .as_ref()
                            .and_then(|m| m.get("excerpt_status"))
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|status| {
                                !matches!(status, "available" | "already_delivered")
                            });
                    let is_error = is_error
                        || result
                            .metadata
                            .as_ref()
                            .and_then(|m| m.get("status"))
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|status| {
                                matches!(
                                    status,
                                    "unavailable" | "failed" | "redacted_confirmation_required"
                                )
                            })
                        || result
                            .metadata
                            .as_ref()
                            .and_then(|m| m.get("reason"))
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|reason| reason == "redacted_confirmation_required");
                    return ToolResult {
                        output: result.llm_content,
                        is_error,
                    };
                }
                InputMessage::ControlRequest { request, .. } => {
                    if matches!(request, ShellControlRequest::Interrupt) {
                        self.provider.cancel();
                        return ToolResult::error("Interrupted by user");
                    }
                }
                _ => {}
            }
        }
        ToolResult::error("cosh_shell_evidence response was not received")
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
                            let exit_code = result
                                .metadata
                                .as_ref()
                                .and_then(|m| m.get("exit_code"))
                                .and_then(|v| v.as_i64())
                                .map(|v| v as i32);
                            return ApprovalResult::HostExecutedShell {
                                llm_content: result.llm_content,
                                exit_code,
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

    /// Attempt to re-authenticate by sending auth_required to Shell.
    /// Returns true if re-auth succeeded and provider was rebuilt.
    async fn try_reauth<W, R>(&mut self, reader: &mut tokio::io::Lines<R>, writer: &mut W) -> bool
    where
        W: Write,
        R: AsyncBufReadExt + Unpin,
    {
        use crate::protocol::AuthReason;

        let request_id = self.next_request_id();
        let providers = builtin_auth_providers();

        let auth_msg = OutputMessage::auth_required(
            &request_id,
            AuthReason::Invalid,
            Some("API authentication failed (401/403)".to_string()),
            providers,
        );
        self.emit(writer, &auth_msg);

        let auth_result = wait_for_auth_response(&request_id, reader).await;
        // Note: buffered_lines during mid-session re-auth are discarded since
        // the retry loop will re-send if needed.
        let response = match auth_result.response {
            Some(r) => r,
            None => return false,
        };

        apply_auth_credentials(&mut self.config, &response);

        if response.persist {
            if let Err(e) = config::persist_config(&self.config) {
                tracing::warn!("failed to persist config: {e}");
            }
        }

        // Rebuild provider
        let resolved = self.config.resolve_provider();
        if resolved.provider_type == "aliyun" {
            if resolved.auth_source.as_deref() == Some("ecs_ram_role") {
                self.provider =
                    Box::new(crate::provider::sysom::SysomProvider::from_ecs_ram_role());
            } else if !resolved.access_key_id.is_empty() && !resolved.access_key_secret.is_empty() {
                self.provider = Box::new(crate::provider::sysom::SysomProvider::new(
                    &resolved.access_key_id,
                    &resolved.access_key_secret,
                    resolved.security_token.as_deref(),
                ));
            } else {
                tracing::warn!("Aliyun auth response missing AK/SK");
                return false;
            }
        } else {
            let profile = crate::provider::profile::profile_from_name(&resolved.provider_type);
            self.provider = Box::new(crate::provider::openai_compat::OpenAICompatProvider::new(
                &resolved.base_url,
                &resolved.api_key,
                profile,
            ));
        }

        self.emit(writer, &OutputMessage::system_status("auth_ok"));
        true
    }
}

enum ApprovalResult {
    Allowed,
    Denied(Option<String>),
    HostExecutedShell {
        llm_content: String,
        exit_code: Option<i32>,
    },
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
                    arguments_delta: r#"{"action":"read_output","output_id":"terminal-output://old-session/cmd-1"}"#
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
                    arguments_delta: r#"{"action":"read_output","output_id":"terminal-output://raw-session/cmd-1"}"#.to_string(),
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
}
