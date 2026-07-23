use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use futures::StreamExt;
use tokio::io::AsyncBufReadExt;

use cosh_platform::audit::LoadedPolicy;
use cosh_types::audit::{AuditOutcomeStatus, AuditProviderData, AuditToolData, Outcome};

use crate::audit::{CoreAuditRecorder, CoreAuditScope};
use crate::auth::{
    apply_auth_credentials, builtin_auth_providers, is_auth_error, wait_for_auth_response,
};
use crate::compaction::CompactionRuntime;
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
    /// Compaction runtime state: the active projection over the transcript
    /// prefix and the provider usage accounting that prices it.
    ///
    /// `messages` always stays the complete transcript; the provider only
    /// sees the projected effective context.
    pub compaction: CompactionRuntime,
    pub model: String,
    pub shell_context: Option<ShellContext>,
    pub extra_params: Option<serde_json::Value>,
    pub hook_system: HookSystem,
    pub metrics: TurnMetrics,
    pub(crate) audit: CoreAuditRecorder,
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
        Self::new_with_session_id(config, provider, tools, uuid::Uuid::new_v4().to_string())
    }

    pub(crate) fn new_with_session_id(
        config: CoreConfig,
        provider: Box<dyn ContentGenerator>,
        tools: ToolRegistry,
        session_id: String,
    ) -> Self {
        let model = config.resolve_provider().model;
        let (loaded_policy, warning) = LoadedPolicy::load();
        if let Some(w) = warning {
            tracing::warn!("{w}");
        }

        let hook_system = HookSystem::from_config(&config.hooks);

        let workspace = std::env::current_dir().ok();
        let audit = CoreAuditRecorder::initialize(&session_id, workspace.as_deref());
        Self {
            config,
            provider,
            tools,
            session_id,
            messages: Vec::new(),
            compaction: CompactionRuntime::default(),
            model,
            shell_context: None,
            extra_params: None,
            hook_system,
            metrics: TurnMetrics::default(),
            audit,
            loaded_policy,
            request_counter: AtomicU32::new(0),
            truncator: OutputTruncator::default(),
            loop_detector: LoopDetector::new(),
        }
    }

    pub fn tool_names(&self) -> Vec<String> {
        self.tools.names()
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

    /// Conservative runtime-prefix (`P`) estimate for budget computations.
    ///
    /// Skill summaries need async loading, so they are covered by the fixed
    /// reserve inside [`crate::compaction::estimate_prefix_tokens`] instead
    /// of being rendered here.
    pub(crate) fn estimate_prefix_tokens(&self) -> u64 {
        let system_prompt = ContextBuilder::build_system_prompt(
            &self.cwd(),
            &self.tool_names(),
            &[],
            &self.config.agent.approval_mode,
            self.config.ai.output_language.as_deref(),
        );
        let declarations = serde_json::to_string(&self.tools.declarations()).unwrap_or_default();
        crate::compaction::estimate_prefix_tokens(&system_prompt, &declarations)
    }

    /// Current effective-context size in tokens under the active projection.
    pub(crate) fn effective_history_tokens(&self, prefix_tokens: u64) -> u64 {
        self.compaction
            .effective_history_tokens(&self.messages, prefix_tokens)
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

        if self.config.agent.allowed_tools.contains(tool_name) {
            return Outcome::Allow;
        }

        let kind = tool.kind();

        if kind == ToolKind::ReadOnly {
            return Outcome::Allow;
        }

        // MCP servers are external programs. Do not infer their side effects
        // from a server-provided description or schema.
        if kind == ToolKind::Mcp {
            return Outcome::RequireApproval;
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
        let content = crate::redaction::redact_text(content);

        // Generate a unique run_id for this agent run.
        let run_id = uuid::Uuid::new_v4().to_string();
        self.hook_system.set_run_id(run_id.clone());

        // ─── Hook: UserPromptSubmit ───
        let cwd_str = self.cwd().to_string_lossy().to_string();
        let prompt_result = self
            .hook_system
            .fire_user_prompt_submit(&self.session_id, &cwd_str, &content)
            .await;
        self.audit.record_hook_decision(
            CoreAuditScope::run(&run_id),
            "user_prompt_submit",
            hook_outcome(&prompt_result.decision),
            hook_decision_name(&prompt_result.decision),
        );

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

            let approval_scope = CoreAuditScope::request(&run_id, None, &request_id, None);
            let audit_ref =
                self.audit
                    .record_approval_requested(approval_scope, "hook", "hook_ask", None);

            // Emit approval request with HOOK: prefix and empty input.
            self.emit(
                writer,
                &OutputMessage::can_use_tool_with_audit_ref(
                    &request_id,
                    &format!("HOOK:{hook_name}"),
                    serde_json::json!({}),
                    &synthetic_id,
                    true, // hook_requires_approval
                    audit_ref,
                ),
            );

            let approval = self.wait_for_approval(&request_id, false, reader).await;
            let (approval_status, approval_decision) = approval_audit_outcome(&approval);
            self.audit.record_approval_resolved(
                approval_scope,
                "hook",
                approval_status,
                None,
                approval_decision,
                None,
            )?;
            match approval {
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

        self.messages.push(Message::user(&content));

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
            // Usage reporting feeds compaction thresholds; the stream adapter
            // guarantees Usage is delivered before MessageEnd.
            include_usage: true,
            extra_params: self.extra_params.clone(),
        };

        let system_prompt = ContextBuilder::build_system_prompt(
            &self.cwd(),
            &self.tool_names(),
            &skill_summaries,
            &self.config.agent.approval_mode,
            self.config.ai.output_language.as_deref(),
        );
        // Runtime prefix estimate (P): system prompt + serialized tool
        // declarations + the compaction module's reserve for hook context
        // injected mid-run.
        let prefix_tokens = crate::compaction::estimate_prefix_tokens(
            &system_prompt,
            &serde_json::to_string(&tool_decls).unwrap_or_default(),
        );

        let max_turns = self.config.agent.max_turns;

        for _turn in 0..max_turns {
            // ─── Context preflight (every provider call, incl. tool loop) ───
            // The loop top is always a complete model/tool exchange boundary
            // with no pending approval or user question, so an emergency
            // compaction here can never split an unfinished interaction.
            crate::compaction::run_context_preflight(
                &mut self.compaction,
                &self.messages,
                self.provider.as_ref(),
                &self.model,
                &self.config,
                prefix_tokens,
                writer,
            )
            .await?;

            let turn_id = uuid::Uuid::new_v4().to_string();
            let turn_scope = CoreAuditScope::turn(&run_id, &turn_id);
            self.audit.record_turn_started(turn_scope);
            let mut provider_messages = self.compaction.effective_messages(&self.messages);
            crate::redaction::redact_messages(&mut provider_messages);

            // ─── Hook: BeforeModel ───
            let before_model_result = self
                .hook_system
                .fire_before_model(&self.session_id, &cwd_str, &self.model, &provider_messages)
                .await;
            self.emit_hook_notifications(writer, &before_model_result.notifications, None);
            self.audit.record_hook_decision(
                turn_scope,
                "before_model",
                AuditOutcomeStatus::Success,
                "observed",
            );

            let mut msgs_with_system = vec![Message::system(&system_prompt)];
            msgs_with_system.extend(provider_messages);

            let provider_request_id = uuid::Uuid::new_v4().to_string();
            let resolved_provider = self.config.resolve_provider();
            let provider_data = AuditProviderData {
                provider: resolved_provider.provider_type.clone(),
                model: Some(self.model.clone()),
                ..AuditProviderData::default()
            };
            let provider_scope =
                CoreAuditScope::request(&run_id, Some(&turn_id), &provider_request_id, None);
            self.audit.record_provider_started(
                provider_scope,
                &resolved_provider.provider_type,
                &provider_data,
            )?;

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
                    self.audit.record_provider_terminal(
                        provider_scope,
                        &resolved_provider.provider_type,
                        &provider_data,
                        AuditOutcomeStatus::Failed,
                        "auth_error",
                        api_start.elapsed().as_millis() as u64,
                    );
                    self.audit.record_turn_terminal(
                        turn_scope,
                        AuditOutcomeStatus::Failed,
                        Some("provider_auth_error"),
                    );
                    // Attempt re-auth
                    if self.try_reauth(reader, writer).await {
                        continue; // Retry the turn with new credentials
                    }
                    return Err(e);
                }
                Err(e) => {
                    self.metrics.api_errors += 1;
                    self.metrics.api_latency_ms += api_start.elapsed().as_millis() as u64;
                    self.audit.record_provider_terminal(
                        provider_scope,
                        &resolved_provider.provider_type,
                        &provider_data,
                        AuditOutcomeStatus::Failed,
                        "request_error",
                        api_start.elapsed().as_millis() as u64,
                    );
                    self.audit.record_turn_terminal(
                        turn_scope,
                        AuditOutcomeStatus::Failed,
                        Some("provider_request_error"),
                    );
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
            let mut message_end_seen = false;

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
                        // Explicit hand-off: provider usage feeds compaction
                        // thresholds through the runtime's accounting API.
                        self.compaction.note_provider_usage(prompt_tokens as u64);
                        // ─── SLS: token usage ───
                        self.metrics.tokens_input += prompt_tokens as u64;
                        self.metrics.tokens_output += completion_tokens as u64;
                        self.metrics.tokens_total += total_tokens as u64;
                    }
                    GenerateEvent::MessageEnd => {
                        self.metrics.api_latency_ms += api_start.elapsed().as_millis() as u64;
                        message_end_seen = true;
                        break;
                    }
                    GenerateEvent::Cancelled => {
                        self.audit.record_provider_terminal(
                            provider_scope,
                            &resolved_provider.provider_type,
                            &provider_data,
                            AuditOutcomeStatus::Cancelled,
                            "cancelled",
                            api_start.elapsed().as_millis() as u64,
                        );
                        self.audit.record_turn_terminal(
                            turn_scope,
                            AuditOutcomeStatus::Cancelled,
                            Some("provider_cancelled"),
                        );
                        return Err("provider request cancelled".to_string());
                    }
                    GenerateEvent::Error(e) => {
                        self.metrics.api_errors += 1;
                        self.metrics.api_latency_ms += api_start.elapsed().as_millis() as u64;
                        self.audit.record_provider_terminal(
                            provider_scope,
                            &resolved_provider.provider_type,
                            &provider_data,
                            AuditOutcomeStatus::Failed,
                            "stream_error",
                            api_start.elapsed().as_millis() as u64,
                        );
                        self.audit.record_turn_terminal(
                            turn_scope,
                            AuditOutcomeStatus::Failed,
                            Some("provider_stream_error"),
                        );
                        return Err(e);
                    }
                }
            }
            drop(stream);
            if !message_end_seen {
                self.audit.record_provider_terminal(
                    provider_scope,
                    &resolved_provider.provider_type,
                    &provider_data,
                    AuditOutcomeStatus::Failed,
                    "unexpected_eof",
                    api_start.elapsed().as_millis() as u64,
                );
                self.audit.record_turn_terminal(
                    turn_scope,
                    AuditOutcomeStatus::Failed,
                    Some("provider_unexpected_eof"),
                );
                return Err("provider stream ended without a terminal event".to_string());
            }
            let (input_tokens, output_tokens) = usage_info
                .map(|(input, output, _)| (Some(u64::from(input)), Some(u64::from(output))))
                .unwrap_or((None, None));
            let completed_provider_data = AuditProviderData {
                input_tokens,
                output_tokens,
                ..provider_data.clone()
            };
            self.audit.record_provider_terminal(
                provider_scope,
                &resolved_provider.provider_type,
                &completed_provider_data,
                AuditOutcomeStatus::Success,
                "completed",
                api_start.elapsed().as_millis() as u64,
            );

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
            self.audit.record_hook_decision(
                turn_scope,
                "after_model",
                AuditOutcomeStatus::Success,
                "observed",
            );

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
                if self.tools.supports_ask_user_question() {
                    if let Some(synthetic) = parse_cosh_question_text(&text_buf) {
                        let result = self
                            .handle_ask_user("synthetic-ask", &synthetic, reader, writer)
                            .await;
                        if result.is_error {
                            self.messages.push(Message::assistant(&text_buf));
                            self.audit.record_turn_terminal(
                                turn_scope,
                                AuditOutcomeStatus::Failed,
                                Some("question_failed"),
                            );
                            return Ok(());
                        }
                        self.messages.push(Message::assistant(&text_buf));
                        self.messages.push(Message::user(&format!(
                            "User answered the question: {}",
                            result.output
                        )));
                        self.audit.record_turn_terminal(
                            turn_scope,
                            AuditOutcomeStatus::Success,
                            Some("question_answered"),
                        );
                        continue;
                    }
                }

                // ─── Hook: Stop ───
                let stop_result = self
                    .hook_system
                    .fire_stop(&self.session_id, &cwd_str, &text_buf)
                    .await;
                self.emit_hook_notifications(writer, &stop_result.notifications, None);
                self.audit.record_hook_decision(
                    turn_scope,
                    "stop",
                    hook_outcome(&stop_result.decision),
                    hook_decision_name(&stop_result.decision),
                );
                if let HookDecision::Block(reason) = &stop_result.decision {
                    self.messages.push(Message::assistant(&text_buf));
                    self.messages.push(Message::user(&format!(
                        "[Hook rejected response] {reason}. Please revise your answer."
                    )));
                    self.audit.record_turn_terminal(
                        turn_scope,
                        AuditOutcomeStatus::Success,
                        Some("stop_hook_retry"),
                    );
                    continue;
                }

                self.messages.push(Message::assistant(&text_buf));
                self.audit
                    .record_turn_terminal(turn_scope, AuditOutcomeStatus::Success, None);
                return Ok(());
            }

            if tool_calls
                .iter()
                .any(|tc| tc.name.is_empty() && !tc.arguments.is_empty())
            {
                return Err(
                    "Provider emitted an incomplete tool call without a function name".to_string(),
                );
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

            // An arguments-only streamed tool-call fragment cannot be executed or
            // represented in the next provider request. Continuing would append an
            // empty assistant message and ask the model again, eventually consuming
            // the entire turn budget without making progress.
            if tc_infos.is_empty() {
                return Err(
                    "Provider emitted an incomplete tool call without a function name".to_string(),
                );
            }
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
                let tool_data = AuditToolData {
                    tool_kind: self
                        .tools
                        .get(&tc.name)
                        .map(|tool| format!("{:?}", tool.kind()).to_ascii_lowercase())
                        .unwrap_or_else(|| "virtual".to_string()),
                    input_shape: Some(json_shape(&params).to_string()),
                    input_hash: Some(hash_json(&params)),
                    ..AuditToolData::default()
                };
                let tool_scope = CoreAuditScope::tool(&run_id, &turn_id, &tc.id);
                self.audit
                    .record_tool_requested(tool_scope, &tc.name, &tool_data);

                if tc.name == "ask_user_question" && self.tools.supports_ask_user_question() {
                    self.audit
                        .record_tool_execution_started(tool_scope, &tc.name, &tool_data)?;
                    let tool_start = Instant::now();
                    let result = self.handle_ask_user(&tc.id, &params, reader, writer).await;
                    self.audit.record_tool_terminal(
                        tool_scope,
                        &tc.name,
                        &tool_data,
                        result.is_error,
                        tool_start.elapsed().as_millis() as u64,
                        result.output.len() as u64,
                    );
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
                    self.audit
                        .record_tool_execution_started(tool_scope, &tc.name, &tool_data)?;
                    let tool_start = Instant::now();
                    let result = self
                        .handle_shell_evidence(&tc.id, &params, reader, writer)
                        .await;
                    self.audit.record_tool_terminal(
                        tool_scope,
                        &tc.name,
                        &tool_data,
                        result.is_error,
                        tool_start.elapsed().as_millis() as u64,
                        result.output.len() as u64,
                    );
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
                self.audit.record_hook_decision(
                    tool_scope,
                    "pre_tool_use",
                    hook_outcome(&hook_result.decision),
                    hook_decision_name(&hook_result.decision),
                );

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
                        self.audit.record_tool_terminal(
                            tool_scope,
                            &tc.name,
                            &tool_data,
                            result.is_error,
                            0,
                            result.output.len() as u64,
                        );
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
                        self.audit
                            .record_tool_execution_started(tool_scope, &tc.name, &tool_data)?;
                        let result = self.execute_tool(&tc.name, params, &ctx).await;
                        self.emit_provider_native_tool_result(writer, &tc.id, &result);
                        tool_result_already_emitted = true;
                        result
                    }
                    Outcome::RequireApproval => {
                        let hook_requires_approval =
                            matches!(hook_result.decision, HookDecision::Ask);
                        let request_id = self.next_request_id();
                        let approval_scope = CoreAuditScope::request(
                            &run_id,
                            Some(&turn_id),
                            &request_id,
                            Some(&tc.id),
                        );
                        let audit_ref = self.audit.record_approval_requested(
                            approval_scope,
                            &tc.name,
                            if hook_requires_approval {
                                "hook_ask"
                            } else {
                                "policy_approval"
                            },
                            Some(hash_json(&params)),
                        );
                        self.emit(
                            writer,
                            &OutputMessage::can_use_tool_with_audit_ref(
                                &request_id,
                                &tc.name,
                                params.clone(),
                                &tc.id,
                                hook_requires_approval,
                                audit_ref,
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
                        let approval_wait_ms = approval_start.elapsed().as_millis() as u64;
                        let (approval_status, approval_decision) =
                            approval_audit_outcome(&approval_result);
                        if !matches!(&approval_result, ApprovalResult::HostExecutedShell { .. }) {
                            self.audit.record_approval_resolved(
                                approval_scope,
                                &tc.name,
                                approval_status,
                                None,
                                approval_decision,
                                Some(approval_wait_ms),
                            )?;
                        }
                        self.metrics.approval_wait_ms += approval_wait_ms;
                        self.metrics.approval_count += 1;
                        match approval_result {
                            ApprovalResult::Allowed => {
                                self.metrics.approval_allow += 1;
                                self.audit.record_tool_execution_started(
                                    tool_scope, &tc.name, &tool_data,
                                )?;
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
                self.audit.record_hook_decision(
                    tool_scope,
                    "post_tool_use",
                    hook_outcome(&post_hook.decision),
                    hook_decision_name(&post_hook.decision),
                );

                // Precedence: block/deny > updated response > original,
                // then append additional context.
                let mut result = if let HookDecision::Block(reason) = &post_hook.decision {
                    ToolResult::error(format!("Post-tool hook denied: {reason}"))
                } else if post_hook.updated_tool_response.is_some()
                    || post_hook.additional_context.is_some()
                {
                    let base = post_hook
                        .updated_tool_response
                        .as_deref()
                        .unwrap_or(&result.output);
                    let output = if let Some(ref extra) = post_hook.additional_context {
                        format!("{base}\n[Hook context] {extra}")
                    } else {
                        base.to_string()
                    };
                    ToolResult {
                        output,
                        // Preserve the original is_error flag on normal replacement.
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
                    let bypass_requested = failure_hook.sandbox_bypass_request.is_some();
                    self.audit.record_hook_decision(
                        tool_scope,
                        "post_tool_use_failure",
                        AuditOutcomeStatus::Success,
                        if bypass_requested {
                            "sandbox_bypass_requested"
                        } else {
                            "observed"
                        },
                    );

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
                        let approval_scope = CoreAuditScope::request(
                            &run_id,
                            Some(&turn_id),
                            &request_id,
                            Some(&tc.id),
                        );
                        let audit_ref = self.audit.record_approval_requested(
                            approval_scope,
                            &tc.name,
                            "sandbox_bypass",
                            Some(hash_json(&serde_json::json!({
                                "command": &bypass.original_command
                            }))),
                        );
                        self.emit(
                            writer,
                            &OutputMessage::can_use_tool_with_audit_ref(
                                &request_id,
                                &tc.name,
                                serde_json::json!({"command": &bypass.original_command}),
                                &tc.id,
                                true,
                                audit_ref,
                            ),
                        );

                        let approval_start = Instant::now();
                        let approval_result =
                            self.wait_for_approval(&request_id, true, reader).await;
                        let (approval_status, approval_decision) =
                            approval_audit_outcome(&approval_result);
                        if !matches!(&approval_result, ApprovalResult::HostExecutedShell { .. }) {
                            self.audit.record_approval_resolved(
                                approval_scope,
                                &tc.name,
                                approval_status,
                                Some("sandbox_bypass"),
                                approval_decision,
                                Some(approval_start.elapsed().as_millis() as u64),
                            )?;
                        }

                        match approval_result {
                            ApprovalResult::Allowed => {
                                self.audit.record_tool_execution_started(
                                    tool_scope, &tc.name, &tool_data,
                                )?;
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

                self.audit.record_tool_terminal(
                    tool_scope,
                    &tc.name,
                    &tool_data,
                    result.is_error,
                    tool_start.elapsed().as_millis() as u64,
                    result.output.len() as u64,
                );
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
                    self.audit.record_turn_terminal(
                        turn_scope,
                        AuditOutcomeStatus::Cancelled,
                        Some("interrupted"),
                    );
                    return Ok(());
                }
            }
            self.audit
                .record_turn_terminal(turn_scope, AuditOutcomeStatus::Success, None);
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

fn hook_decision_name(decision: &HookDecision) -> &'static str {
    match decision {
        HookDecision::Allow => "allow",
        HookDecision::Block(_) => "block",
        HookDecision::Ask => "ask",
        HookDecision::Passthrough => "passthrough",
    }
}

fn hook_outcome(decision: &HookDecision) -> AuditOutcomeStatus {
    match decision {
        HookDecision::Allow | HookDecision::Passthrough => AuditOutcomeStatus::Allowed,
        HookDecision::Block(_) => AuditOutcomeStatus::Denied,
        HookDecision::Ask => AuditOutcomeStatus::Started,
    }
}

fn approval_audit_outcome(approval: &ApprovalResult) -> (AuditOutcomeStatus, &'static str) {
    match approval {
        ApprovalResult::Allowed | ApprovalResult::HostExecutedShell { .. } => {
            (AuditOutcomeStatus::Allowed, "allow")
        }
        ApprovalResult::Denied(_) => (AuditOutcomeStatus::Denied, "deny"),
        ApprovalResult::Interrupted => (AuditOutcomeStatus::Cancelled, "interrupted"),
    }
}

fn json_shape(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn hash_json(value: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};

    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
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
#[path = "core/tests.rs"]
mod tests;
