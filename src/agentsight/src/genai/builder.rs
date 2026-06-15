//! GenAI Semantic Builder
//!
//! This module builds GenAI semantic events from AnalysisResult.
//! It reuses already-extracted data to avoid redundant parsing.

use super::id_resolver::IdResolver;
use super::semantic::{
    GenAISemanticEvent, InputMessage, LLMCall, LLMRequest, LLMResponse, MessagePart, OutputMessage,
    TokenUsage,
};
use crate::aggregator::{ConnectionId, ParsedRequest};
use crate::analyzer::message::types::OpenAIChatMessage;
use crate::analyzer::token::TokenParser;
use crate::analyzer::{AnalysisResult, HttpRecord, ParsedApiMessage, TokenRecord};
use crate::config::default_cmdline_rules;
use crate::discovery::matcher::{CmdlineGlobMatcher, ProcessContext};
use crate::parser::sse::ParsedSseEvent;
use crate::response_map::ResponseSessionMapper;
use crate::storage::sqlite::{PendingCallInfo, SseEnrichment};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Output from `GenAIBuilder::build()`, containing built events and deferred resolution info.
pub struct BuildOutput {
    /// Built GenAI semantic events (ready to export, may have fallback session_id)
    pub events: Vec<GenAISemanticEvent>,
    /// If set, the session_id was NOT resolved from the ResponseSessionMapper and
    /// the caller should retry the lookup later using this response ID.
    /// When the lookup succeeds, update the `session_id` metadata of all events.
    pub pending_response_id: Option<String>,
}

/// Builder that constructs GenAI semantic events from AnalysisResult
pub struct GenAIBuilder {
    /// Session ID prefix (timestamp-based, unique per agentsight run)
    session_prefix: String,
    /// Counter for generating unique IDs within a session
    call_counter: AtomicU64,
    /// Resolver for `session_id` fallback / `conversation_id` based on the
    /// earliest `response_id` observed within a session / conversation.
    id_resolver: IdResolver,
}

impl Default for GenAIBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl GenAIBuilder {
    /// Create a new GenAI builder
    pub fn new() -> Self {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let pid = std::process::id();
        GenAIBuilder {
            session_prefix: format!("{ts:x}_{pid:x}"),
            call_counter: AtomicU64::new(0),
            id_resolver: IdResolver::new(),
        }
    }

    /// Build GenAI semantic events AND a `PendingCallInfo` to be written to DB
    /// before the response arrives.
    ///
    /// Returns `(output, Some(pending_info))` where `pending_info.call_id` matches
    /// the `call_id` embedded inside the returned `LLMCall` event, so the caller can
    /// first `insert_pending(pending_info)` and later `complete_pending(event)`.
    ///
    /// The `BuildOutput` also carries `pending_response_id` when the session_id
    /// could not be resolved from the `ResponseSessionMapper` so the caller can
    /// queue the events for deferred resolution.
    ///
    /// Returns `(output, None)` when no LLM API call was detected in `results`.
    pub fn build_with_pending(
        &self,
        results: &[AnalysisResult],
        response_mapper: &ResponseSessionMapper,
        pid_agent_name_cache: &std::collections::HashMap<u32, String>,
    ) -> (BuildOutput, Option<PendingCallInfo>) {
        let mut events = Vec::new();
        let mut pending: Option<PendingCallInfo> = None;
        let mut pending_response_id = None;

        if let Some(llm_call) = self.build_llm_call(results, response_mapper, pid_agent_name_cache)
        {
            // Build PendingCallInfo from the same LLMCall before moving it
            let http_record = results.iter().find_map(|r| match r {
                AnalysisResult::Http(h) => Some(h.clone()),
                _ => None,
            });

            // Extract input messages for the pending record
            let (input_messages_json, system_instructions_json) = {
                let sys: Vec<_> = llm_call
                    .request
                    .messages
                    .iter()
                    .filter(|m| m.role == "system")
                    .collect();
                let latest =
                    crate::genai::semantic::latest_round_input_messages(&llm_call.request.messages);
                (
                    if latest.is_empty() {
                        None
                    } else {
                        serde_json::to_string(&latest).ok()
                    },
                    if sys.is_empty() {
                        None
                    } else {
                        serde_json::to_string(&sys).ok()
                    },
                )
            };

            // Determine response_id from call metadata (may come from parsed_message
            // or SSE body fallback), and check if mapper resolved it.
            let response_id = llm_call.metadata.get("response_id").cloned();
            let mapper_hit = response_id
                .as_deref()
                .and_then(|rid| response_mapper.get_session_by_response_id(rid))
                .is_some();

            // If response_id exists but mapper didn't resolve session_id, queue
            // for deferred resolution so the next FileWrite event can fix it.
            if response_id.is_some() && !mapper_hit {
                pending_response_id = response_id;
                log::debug!(
                    "GenAI response_id {} not yet in mapper, will defer session_id resolution",
                    pending_response_id.as_deref().unwrap_or_default()
                );
            }

            pending = Some(PendingCallInfo {
                call_id: llm_call.call_id.clone(),
                trace_id: llm_call.metadata.get("response_id").cloned(),
                conversation_id: llm_call.metadata.get("conversation_id").cloned(),
                session_id: llm_call.metadata.get("session_id").cloned(),
                start_timestamp_ns: llm_call.start_timestamp_ns,
                pid: llm_call.pid,
                process_name: llm_call.process_name.clone(),
                agent_name: llm_call.agent_name.clone(),
                http_method: http_record.as_ref().map(|h| h.method.clone()),
                http_path: http_record.as_ref().map(|h| h.path.clone()),
                input_messages: input_messages_json,
                system_instructions: system_instructions_json,
                user_query: llm_call.metadata.get("user_query").cloned(),
                is_sse: llm_call.request.stream,
                model: Some(llm_call.model.clone()),
                provider: Some(llm_call.provider.clone()),
            });

            events.push(GenAISemanticEvent::LLMCall(llm_call));
        }

        (
            BuildOutput {
                events,
                pending_response_id,
            },
            pending,
        )
    }

    /// Build a `PendingCallInfo` directly from a raw `ParsedRequest` and
    /// `ConnectionId`, without needing a full `AnalysisResult`.
    ///
    /// This is used when the event loop detects that a PID has died while its
    /// connection was still in `RequestPending` or `SseActive` state.  By
    /// writing a pending record to `genai_events`, the HealthChecker can later
    /// find it via `list_pending_for_pid` and create a properly correlated
    /// `InterruptionEvent`.
    ///
    /// Returns `None` if the request path is not a known LLM API endpoint or
    /// the body cannot be parsed at all.
    pub fn build_pending_from_request(
        &self,
        request: &ParsedRequest,
        conn_id: &ConnectionId,
        pid_agent_name_cache: &std::collections::HashMap<u32, String>,
    ) -> Option<PendingCallInfo> {
        // Only process known LLM API paths
        let path_match = self.is_llm_api_path(&request.path);
        let body_str = if request.body_len > 0 {
            Some(request.body_str().to_string())
        } else {
            None
        };
        let body_match = !path_match && Self::is_sysom_pop_request(&body_str);
        if !path_match && !body_match {
            return None;
        }

        let call_id = self.generate_id();
        let body = request.json_body();

        // Determine if streaming
        let is_sse = body
            .as_ref()
            .and_then(|v| v.get("stream"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Parse messages from body to extract user_query / input_messages /
        // system_instructions / first_user_text / last_user_text。session_id 与
        // conversation_id 在 request 阶段采用双层兑底：
        //   1. 优先走 IdResolver::peek_*（同 PID 之前有过正常完成的调用 →
        //      LRU 已 anchor 首个 response_id，复用后与正常路径完全对齐）。
        //   2. 未命中时 → `crash_fallback_id`以 (agent_name, pid, user_text) 作为
        //      兑底 ID 输入，保证 crash-drain 路径同 PID 同 user_query 的
        //      crash 记录归一桶，不同 user_query 分桶。
        //
        // 正常响应到达后 `complete_pending` 仍会用 `IdResolver::resolve_*`
        // 重新计算并 UPDATE 正常 ID，只有 crash 路径才会保留这里写入的
        // peek/fallback 值。
        let (user_query, input_messages, system_instructions, first_user_text, last_user_text) =
            if let Some(ref v) = body {
                if let Some(messages) = v.get("messages").and_then(|m| m.as_array()) {
                    // Helper: extract text from "content" which can be either
                    // a plain string or an array of content blocks:
                    //   "content": "text"
                    //   "content": [{"type":"text","text":"..."},...]
                    let extract_text = |m: &serde_json::Value| -> Option<String> {
                        let c = m.get("content")?;
                        if let Some(s) = c.as_str() {
                            if !s.is_empty() {
                                return Some(s.to_string());
                            }
                        }
                        if let Some(arr) = c.as_array() {
                            let text: String = arr
                                .iter()
                                .filter_map(|item| {
                                    // [{"type":"text","text":"..."}]
                                    if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                                        item.get("text").and_then(|t| t.as_str())
                                    } else {
                                        None
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            if !text.is_empty() {
                                return Some(text);
                            }
                        }
                        None
                    };

                    // First user message raw text — used as `session_key` material
                    // for IdResolver peek / crash fallback.
                    let first_user_text = messages
                        .iter()
                        .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
                        .find_map(&extract_text)
                        .unwrap_or_default();

                    // Last user message raw text — used for user_query (display text)
                    // 以及 conversation_key (peek / crash fallback)。
                    let last_user_raw = messages
                        .iter()
                        .rev()
                        .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
                        .find_map(extract_text);
                    let last_user_text = last_user_raw.clone().unwrap_or_default();

                    // user_query: last user message text, stripped of metadata prefix
                    let user_query = last_user_raw.as_deref().map(Self::strip_user_query_prefix);

                    // Serialise message subsets for the pending record
                    let sys: Vec<_> = messages
                        .iter()
                        .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("system"))
                        .collect();
                    let non_sys: Vec<_> = messages
                        .iter()
                        .filter(|m| m.get("role").and_then(|r| r.as_str()) != Some("system"))
                        .collect();

                    let input_messages = if non_sys.is_empty() {
                        None
                    } else {
                        serde_json::to_string(&non_sys).ok()
                    };
                    let system_instructions = if sys.is_empty() {
                        None
                    } else {
                        serde_json::to_string(&sys).ok()
                    };

                    (
                        user_query,
                        input_messages,
                        system_instructions,
                        first_user_text,
                        last_user_text,
                    )
                } else {
                    // messages key missing or not an array
                    (None, None, None, String::new(), String::new())
                }
            } else {
                (None, None, None, String::new(), String::new())
            };

        // Extract model from request body JSON "model" field
        let model = body
            .as_ref()
            .and_then(|v| v.get("model"))
            .and_then(|m| m.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        // Extract provider from request path
        let provider = self.extract_provider_from_path(&request.path);

        // Resolve agent_name: check pid→name cache first, then comm-based matching, then comm as fallback
        let agent_name = Self::resolve_agent_name_from_comm(
            &request.source_event.comm,
            conn_id.pid,
            pid_agent_name_cache,
        )
        .or_else(|| Some(request.source_event.comm_str()));

        // 双层兑底计算 session_id / conversation_id（详见上方注释）。
        // 这里不使用 unwrap_or_else(|| “”) 是为了让“同 PID 同 agent”上下文下
        // crash_fallback_id 输入始终相同。
        let agent_name_str = agent_name.as_deref().unwrap_or("");
        let pid_i32 = conn_id.pid as i32;

        let session_id = self
            .id_resolver
            .peek_session_id(agent_name_str, pid_i32, &first_user_text)
            .or_else(|| {
                Some(super::id_resolver::crash_fallback_id(
                    "session",
                    agent_name_str,
                    pid_i32,
                    &first_user_text,
                ))
            });
        let conversation_id = self
            .id_resolver
            .peek_conversation_id(agent_name_str, pid_i32, &last_user_text)
            .or_else(|| {
                Some(super::id_resolver::crash_fallback_id(
                    "conversation",
                    agent_name_str,
                    pid_i32,
                    &last_user_text,
                ))
            });

        Some(PendingCallInfo {
            call_id,
            trace_id: None, // LLM API response_id, not available until response
            // session_id / conversation_id 在请求阶段采用双层兑底：
            // 1) IdResolver::peek_* 复用同 PID 之前正常完成调用的 anchor，
            //    响应到达后 `complete_pending` 会用同样的值覆盖；
            // 2) LRU miss 时走 `crash_fallback_id`（`crash-` 前缀与正常 ID 隔离），
            //    进程崩溃不会走到 complete_pending 时该值会保留，供
            //    handle_agent_crash_detection 按 (sid, cid) 分组。
            conversation_id,
            session_id,
            start_timestamp_ns: request.source_event.timestamp_ns,
            pid: pid_i32,
            process_name: request.source_event.comm.clone(),
            agent_name,
            http_method: Some(request.method.clone()),
            http_path: Some(request.path.clone()),
            input_messages,
            system_instructions,
            user_query,
            is_sse,
            model,
            provider,
        })
    }

    /// Extract enrichment data from SSE events captured before the process died.
    ///
    /// Parses sse_events for:
    /// - model name (from first chunk's "model" field)
    /// - trace_id / response_id (from first chunk's "id" field)
    /// - token usage (via TokenParser, from DashScope-style usage chunks)
    /// - output content (merged content deltas)
    ///
    /// Returns `None` if sse_events is empty.
    pub fn extract_sse_enrichment(sse_events: &[ParsedSseEvent]) -> Option<SseEnrichment> {
        if sse_events.is_empty() {
            return None;
        }

        let token_parser = TokenParser::new();
        let mut model: Option<String> = None;
        let mut trace_id: Option<String> = None;
        let mut content_buf = String::new();

        // Forward scan for model, trace_id, and content deltas
        for event in sse_events {
            if let Some(json) = event.json_body() {
                // Extract model from first chunk that has it
                if model.is_none() {
                    if let Some(m) = json.get("model").and_then(|v| v.as_str()) {
                        if !m.is_empty() {
                            model = Some(m.to_string());
                        }
                    }
                }
                // Extract response id (trace_id) from first chunk that has it
                if trace_id.is_none() {
                    if let Some(id) = json.get("id").and_then(|v| v.as_str()) {
                        if !id.is_empty() {
                            trace_id = Some(id.to_string());
                        }
                    }
                }
                // Accumulate content deltas
                if let Some(choices) = json.get("choices").and_then(|v| v.as_array()) {
                    for choice in choices {
                        if let Some(delta) = choice.get("delta") {
                            if let Some(c) = delta.get("content").and_then(|v| v.as_str()) {
                                content_buf.push_str(c);
                            }
                        }
                    }
                }
            }
        }

        // Reverse scan for token usage (usage chunk is near the end)
        let usage = sse_events
            .iter()
            .rev()
            .find_map(|e| token_parser.parse_event(e));

        let (input_tokens, output_tokens) = match &usage {
            Some(u) => (Some(u.input_tokens as i64), Some(u.output_tokens as i64)),
            None => (None, None),
        };

        // Use model from usage if not found in content chunks
        if model.is_none() {
            if let Some(ref u) = usage {
                model = u.model.clone();
            }
        }

        // Build output_messages JSON from accumulated content
        let output_messages = if !content_buf.is_empty() {
            // Format as a JSON array matching OutputMessage structure
            serde_json::to_string(&serde_json::json!([{
                "role": "assistant",
                "parts": [{"Text": {"content": content_buf}}]
            }]))
            .ok()
        } else {
            None
        };

        let event_count = sse_events.len() as i64;

        Some(SseEnrichment {
            model,
            trace_id,
            provider: None, // provider already set from request path in insert_pending
            output_messages,
            sse_event_count: Some(event_count),
            input_tokens,
            output_tokens,
        })
    }

    /// Build LLMCall from analysis results
    ///
    /// Combines data from TokenRecord, HttpRecord, and ParsedApiMessage
    fn build_llm_call(
        &self,
        results: &[AnalysisResult],
        response_mapper: &ResponseSessionMapper,
        pid_agent_name_cache: &std::collections::HashMap<u32, String>,
    ) -> Option<LLMCall> {
        // Extract components from analysis results
        let token_record = results.iter().find_map(|r| match r {
            AnalysisResult::Token(t) => Some(t.clone()),
            _ => None,
        });

        let http_record = results.iter().find_map(|r| match r {
            AnalysisResult::Http(h) => Some(h.clone()),
            _ => None,
        });

        let parsed_message = results.iter().find_map(|r| match r {
            AnalysisResult::Message(m) => Some(m.clone()),
            _ => None,
        });

        // Need at least HttpRecord to build LLMCall
        let http = http_record?;

        // Check if this is an LLM API call (path-based or body-based for SysOM POP API)
        let path_match = self.is_llm_api_path(&http.path);
        let body_match = !path_match && Self::is_sysom_pop_request(&http.request_body);
        let is_llm = path_match || body_match;
        if !is_llm && !http.is_sse {
            return None;
        }

        let internal_id = self.generate_id();

        // Build request from parsed message or HTTP record
        let request = self.build_request(&parsed_message, &http);
        // Build response from parsed message or HTTP record
        let response = self.build_response(&parsed_message, &http, &token_record);

        // Build token usage from TokenRecord
        let token_usage = token_record.as_ref().map(|t| TokenUsage {
            input_tokens: t.input_tokens as u32,
            output_tokens: t.output_tokens as u32,
            total_tokens: (t.input_tokens + t.output_tokens) as u32,
            cache_creation_input_tokens: t.cache_creation_tokens.map(|v| v as u32),
            cache_read_input_tokens: t.cache_read_tokens.map(|v| v as u32),
        });

        // Determine provider and model
        // Priority: path-based (most specific) > body-based > parsed_message > token_record
        let provider = self
            .extract_provider_from_path(&http.path)
            .or_else(|| Self::extract_provider_from_body(&http.request_body))
            .or_else(|| parsed_message.as_ref().map(|m| m.provider().to_string()))
            .or_else(|| token_record.as_ref().map(|t| t.provider.clone()))
            .unwrap_or_else(|| "unknown".to_string());

        // Model priority: parsed_message (most accurate) > token_record > body extraction
        let model = self
            .extract_model_from_message(&parsed_message)
            .or_else(|| {
                token_record
                    .as_ref()
                    .and_then(|t| t.model.as_ref().filter(|m| !m.is_empty()).cloned())
            })
            .or_else(|| Self::extract_model_from_body(&http.request_body, &http.response_body))
            .unwrap_or_else(|| "unknown".to_string());

        // 在 request move 之前提取用户查询 / first&last user message 原文
        let user_query = Self::extract_last_user_query(&request);
        let first_user_raw = Self::extract_first_user_raw(&request).unwrap_or_default();
        let last_user_raw = Self::extract_last_user_raw(&request).unwrap_or_default();

        // 提取 LLM API 的 response_id（如 chatcmpl-xxx），用作 trace_id
        // 同时作为 call_id 的首选值：trace_id 有值时直接复用，避免两套 ID；
        // SysOM / 解析失败等无 response_id 的场景 fallback 到内部生成的 internal_id。
        let response_id = Self::extract_response_id(&parsed_message, &http);
        let call_id = response_id.clone().unwrap_or_else(|| internal_id.clone());
        let response_id = response_id.unwrap_or_else(|| call_id.clone());

        // 提前解析 agent_name，后面调用 IdResolver 需要它作为 LRU key 维度，
        // 避免同机不同 agent 在同一 user query 下撞同一 session_id。
        let agent_name = Self::resolve_agent_name(&http.comm, http.pid, pid_agent_name_cache)
            .unwrap_or_else(|| http.comm.clone());
        let pid_i32 = http.pid as i32;

        // session_id: 优先从 agent 自身的 session 获取（通过 response ID → .jsonl UUID 映射），
        // 未命中时 fallback 到 `SHA256("session" + 该 session 内最早 response_id)`。
        let parsed_response_id = parsed_message
            .as_ref()
            .and_then(|m| m.response_id())
            .map(|s| s.to_string());
        let mapper_session = parsed_response_id
            .as_deref()
            .and_then(|rid| response_mapper.get_session_by_response_id(rid))
            .map(|s| s.to_string());
        let session_id = match mapper_session {
            Some(uuid) => Some(uuid),
            None => self.id_resolver.resolve_session_id(
                &agent_name,
                pid_i32,
                &first_user_raw,
                &response_id,
            ),
        };

        // conversation_id: SHA256("conversation" + 该 conversation 内最早 response_id)
        let conversation_id = self.id_resolver.resolve_conversation_id(
            &agent_name,
            pid_i32,
            &last_user_raw,
            &response_id,
        );

        // Extract error message from response body when status_code >= 400
        let error = if http.status_code >= 400 {
            http.response_body.as_ref().and_then(|body| {
                /// Strip HTTP chunked transfer encoding (e.g. "b6\r\n{json}\r\n0\r\n\r\n")
                /// and return the JSON object substring.
                fn strip_chunked(body: &str) -> &str {
                    // Find the first '{' — everything before it may be chunk-size hex + CRLF
                    let start = match body.find('{') {
                        Some(idx) => idx,
                        None => return body,
                    };
                    // Find the last '}' — everything after it is chunked trailer
                    let end = match body.rfind('}') {
                        Some(idx) => idx + 1,
                        None => return &body[start..],
                    };
                    &body[start..end]
                }

                /// Try to extract `message` from a JSON value (handles nested / escaped JSON)
                fn extract_message(v: &serde_json::Value) -> Option<String> {
                    if let Some(e) = v.get("error") {
                        if e.is_object() {
                            // {"error":{"message":"..."}}
                            if let Some(msg) = e.get("message").and_then(|m| m.as_str()) {
                                return Some(msg.to_string());
                            }
                        } else if let Some(s) = e.as_str() {
                            // {"error": "{\"error\":{\"message\":\"...\"}}"}  — escaped JSON string
                            if let Ok(inner) = serde_json::from_str::<serde_json::Value>(s) {
                                if let Some(msg) = inner.get("message").and_then(|m| m.as_str()) {
                                    return Some(msg.to_string());
                                }
                                if let Some(inner_e) = inner.get("error") {
                                    if let Some(msg) =
                                        inner_e.get("message").and_then(|m| m.as_str())
                                    {
                                        return Some(msg.to_string());
                                    }
                                }
                            }
                            return Some(s.to_string());
                        }
                    }
                    // Top-level {"message":"..."}
                    v.get("message")
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                }

                let json_str = strip_chunked(body);
                serde_json::from_str::<serde_json::Value>(json_str)
                    .ok()
                    .and_then(|v| extract_message(&v))
                    .or_else(|| Some(body.clone()))
            })
        } else {
            None
        };

        Some(LLMCall {
            call_id,
            start_timestamp_ns: http.timestamp_ns,
            end_timestamp_ns: http.timestamp_ns + http.duration_ns,
            duration_ns: http.duration_ns,
            provider,
            model,
            request,
            response,
            token_usage,
            error,
            pid: pid_i32,
            process_name: http.comm.clone(),
            agent_name: Some(agent_name.clone()),
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("method".to_string(), http.method);
                meta.insert("path".to_string(), http.path.clone());
                meta.insert("status_code".to_string(), http.status_code.to_string());
                meta.insert("is_sse".to_string(), http.is_sse.to_string());
                meta.insert(
                    "sse_event_count".to_string(),
                    http.sse_event_count.to_string(),
                );
                // Extract server.address and server.port from Host header
                if let Ok(headers) =
                    serde_json::from_str::<HashMap<String, String>>(&http.request_headers)
                {
                    if let Some(host) = headers.get("host").or_else(|| headers.get("Host")) {
                        if let Some((addr, port)) = host.rsplit_once(':') {
                            meta.insert("server.address".to_string(), addr.to_string());
                            meta.insert("server.port".to_string(), port.to_string());
                        } else {
                            meta.insert("server.address".to_string(), host.clone());
                        }
                    }
                }
                // Derive gen_ai.operation.name from path
                if http.path.contains("/chat/completions") || http.path.contains("/v1/messages") {
                    meta.insert("operation_name".to_string(), "chat".to_string());
                } else if http.path.contains("/completions") {
                    meta.insert("operation_name".to_string(), "text_completion".to_string());
                } else if http.path.contains("/api/v1/copilot/generate_copilot") {
                    meta.insert("operation_name".to_string(), "chat".to_string());
                }
                // conversation_id: 对话ID，同一 user query 触发的所有调用共享
                if let Some(ref cid) = conversation_id {
                    meta.insert("conversation_id".to_string(), cid.clone());
                }
                // response_id: LLM API 返回的响应 ID，用作 trace_id
                meta.insert("response_id".to_string(), response_id);
                // user_query: 用户实际输入的原文
                if let Some(ref q) = user_query {
                    meta.insert("user_query".to_string(), q.clone());
                }
                // session_id: 同一 agent 进程的完整会话标识
                if let Some(ref sid) = session_id {
                    meta.insert("session_id".to_string(), sid.clone());
                }
                meta
            },
        })
    }

    /// Build LLMRequest from parsed message or HTTP record
    fn build_request(&self, message: &Option<ParsedApiMessage>, http: &HttpRecord) -> LLMRequest {
        match message {
            Some(ParsedApiMessage::OpenAICompletion { request, .. }) => {
                if let Some(req) = request.as_ref() {
                    let msgs = req.messages.iter().map(Self::openai_msg_to_input).collect();
                    return LLMRequest {
                        messages: msgs,
                        temperature: req.temperature,
                        max_tokens: req.max_tokens,
                        frequency_penalty: req.frequency_penalty,
                        presence_penalty: req.presence_penalty,
                        top_p: req.top_p,
                        top_k: None,
                        seed: req.seed,
                        stop_sequences: req.stop.clone(),
                        stream: req.stream.unwrap_or(false),
                        tools: req.tools.clone(),
                        raw_body: http.request_body.clone(),
                    };
                }
            }
            Some(ParsedApiMessage::AnthropicMessage { request, .. }) => {
                if let Some(req) = request.as_ref() {
                    let msgs = req
                        .messages
                        .iter()
                        .map(|m| {
                            let role = format!("{:?}", m.role).to_lowercase();
                            InputMessage {
                                role,
                                parts: vec![MessagePart::Text {
                                    content: m.content.as_text(),
                                }],
                                name: None,
                            }
                        })
                        .collect();
                    return LLMRequest {
                        messages: msgs,
                        temperature: req.temperature,
                        max_tokens: Some(req.max_tokens),
                        frequency_penalty: None,
                        presence_penalty: None,
                        top_p: req.top_p,
                        top_k: req.top_k.map(|v| v as f64),
                        seed: None,
                        stop_sequences: req.stop_sequences.clone(),
                        stream: req.stream.unwrap_or(false),
                        tools: req.tools.clone(),
                        raw_body: http.request_body.clone(),
                    };
                }
            }
            Some(ParsedApiMessage::SysomMessage { request, .. }) => {
                if let Some(req) = request.as_ref() {
                    let msgs = req
                        .params
                        .messages
                        .iter()
                        .map(|m| {
                            let role = m.role.clone();
                            let mut parts = Vec::new();
                            if role == "tool" {
                                let response_val =
                                    serde_json::from_str::<serde_json::Value>(&m.content)
                                        .unwrap_or_else(|_| {
                                            serde_json::Value::String(m.content.clone())
                                        });
                                parts.push(MessagePart::ToolCallResponse {
                                    id: m.tool_call_id.clone(),
                                    response: response_val,
                                });
                            } else if !m.content.is_empty() {
                                parts.push(MessagePart::Text {
                                    content: m.content.clone(),
                                });
                            }
                            if let Some(ref tool_calls) = m.tool_calls {
                                for tc in tool_calls {
                                    let arguments = serde_json::from_str::<serde_json::Value>(
                                        &tc.function.arguments,
                                    )
                                    .ok();
                                    parts.push(MessagePart::ToolCall {
                                        id: Some(tc.id.clone()),
                                        name: tc.function.name.clone(),
                                        arguments,
                                    });
                                }
                            }
                            InputMessage {
                                role,
                                parts,
                                name: m.name.clone(),
                            }
                        })
                        .collect();
                    return LLMRequest {
                        messages: msgs,
                        temperature: req.params.temperature,
                        max_tokens: req.params.max_tokens,
                        frequency_penalty: None,
                        presence_penalty: None,
                        top_p: req.params.top_p,
                        top_k: None,
                        seed: None,
                        stop_sequences: None,
                        stream: req.params.stream,
                        tools: req.params.tools.clone(),
                        raw_body: http.request_body.clone(),
                    };
                }
            }
            _ => {}
        }

        // Fallback: no parsed message — parse request_body directly
        if let Some(ref body) = http.request_body {
            if let Some(req) = Self::parse_request_body(body) {
                return req;
            }
        }
        LLMRequest {
            messages: vec![],
            temperature: None,
            max_tokens: None,
            frequency_penalty: None,
            presence_penalty: None,
            top_p: None,
            top_k: None,
            seed: None,
            stop_sequences: None,
            stream: false,
            tools: None,
            raw_body: http.request_body.clone(),
        }
    }

    /// 从 HTTP request body 直接解析 LLMRequest（OpenAI/Anthropic 格式）
    fn parse_request_body(body: &str) -> Option<LLMRequest> {
        let v: serde_json::Value = serde_json::from_str(body).ok()?;
        let obj = v.as_object()?;

        // 解析 messages 数组
        let messages = obj
            .get("messages")
            .and_then(|m| m.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|msg| {
                        let role = msg.get("role")?.as_str()?.to_string();
                        let mut parts = Vec::new();

                        // content 可以是字符串或数组
                        if let Some(content) = msg.get("content") {
                            if let Some(s) = content.as_str() {
                                if !s.is_empty() {
                                    parts.push(MessagePart::Text {
                                        content: s.to_string(),
                                    });
                                }
                            } else if let Some(arr) = content.as_array() {
                                for item in arr {
                                    if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                        parts.push(MessagePart::Text {
                                            content: text.to_string(),
                                        });
                                    }
                                }
                            }
                        }

                        // tool_call 结果 (role=tool)
                        if role == "tool" {
                            if let Some(content) = msg.get("content") {
                                let id = msg
                                    .get("tool_call_id")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string());
                                parts = vec![MessagePart::ToolCallResponse {
                                    id,
                                    response: content.clone(),
                                }];
                            }
                        }

                        // tool_calls (role=assistant 发起的 tool calls)
                        if let Some(tool_calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                            for tc in tool_calls {
                                let id =
                                    tc.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
                                let func = tc.get("function").unwrap_or(&serde_json::Value::Null);
                                let name = func
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let arguments = func.get("arguments").cloned();
                                parts.push(MessagePart::ToolCall {
                                    id,
                                    name,
                                    arguments,
                                });
                            }
                        }

                        Some(InputMessage {
                            role,
                            parts,
                            name: None,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if messages.is_empty() {
            return None;
        }

        let tools = obj
            .get("tools")
            .and_then(|v| v.as_array())
            .map(|arr| arr.to_vec());

        Some(LLMRequest {
            messages,
            temperature: obj.get("temperature").and_then(|v| v.as_f64()),
            max_tokens: obj
                .get("max_tokens")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32),
            frequency_penalty: obj.get("frequency_penalty").and_then(|v| v.as_f64()),
            presence_penalty: obj.get("presence_penalty").and_then(|v| v.as_f64()),
            top_p: obj.get("top_p").and_then(|v| v.as_f64()),
            top_k: obj.get("top_k").and_then(|v| v.as_f64()),
            seed: obj.get("seed").and_then(|v| v.as_i64()),
            stop_sequences: obj.get("stop").and_then(|v| {
                v.as_array().map(|arr| {
                    arr.iter()
                        .filter_map(|s| s.as_str().map(String::from))
                        .collect()
                })
            }),
            stream: obj.get("stream").and_then(|v| v.as_bool()).unwrap_or(false),
            tools,
            raw_body: Some(body.to_string()),
        })
    }

    /// Build LLMResponse from parsed message or HTTP record
    fn build_response(
        &self,
        message: &Option<ParsedApiMessage>,
        http: &HttpRecord,
        _token_record: &Option<TokenRecord>,
    ) -> LLMResponse {
        // Try to extract from parsed message first
        let (messages, finish_reason): (Vec<OutputMessage>, Option<String>) = match message {
            Some(ParsedApiMessage::OpenAICompletion { response, .. }) => response
                .as_ref()
                .map(|resp| {
                    let msgs: Vec<OutputMessage> = resp
                        .choices
                        .iter()
                        .map(|c| Self::openai_msg_to_output(&c.message, c.finish_reason.as_deref()))
                        .collect();
                    let finish = resp.choices.first().and_then(|c| c.finish_reason.clone());
                    (msgs, finish)
                })
                .unwrap_or_else(|| (vec![], None)),
            Some(ParsedApiMessage::AnthropicMessage { response, .. }) => {
                response
                    .as_ref()
                    .map(|resp| {
                        let mut parts = Vec::new();
                        for block in &resp.content {
                            match block {
                                crate::analyzer::message::AnthropicContentBlock::Text {
                                    text,
                                    ..
                                } => {
                                    if !text.is_empty() {
                                        parts.push(MessagePart::Text {
                                            content: text.clone(),
                                        });
                                    }
                                }
                                crate::analyzer::message::AnthropicContentBlock::ToolUse {
                                    id,
                                    name,
                                    input,
                                } => {
                                    // Anthropic tool_use: convert to MessagePart::ToolCall
                                    parts.push(MessagePart::ToolCall {
                                        id: Some(id.clone()),
                                        name: name.clone(),
                                        arguments: Some(input.clone()),
                                    });
                                }
                                crate::analyzer::message::AnthropicContentBlock::ToolResult {
                                    tool_use_id,
                                    content,
                                    ..
                                } => {
                                    // Anthropic tool_result: convert to MessagePart::ToolCallResponse
                                    let response_val =
                                        content.clone().unwrap_or(serde_json::Value::Null);
                                    parts.push(MessagePart::ToolCallResponse {
                                        id: Some(tool_use_id.clone()),
                                        response: response_val,
                                    });
                                }
                                crate::analyzer::message::AnthropicContentBlock::Thinking {
                                    thinking,
                                    ..
                                } => {
                                    // Anthropic thinking: convert to MessagePart::Reasoning
                                    if !thinking.is_empty() {
                                        parts.push(MessagePart::Reasoning {
                                            content: thinking.clone(),
                                        });
                                    }
                                }
                                _ => {}
                            }
                        }
                        let msgs = vec![OutputMessage {
                            role: "assistant".to_string(),
                            parts,
                            name: None,
                            finish_reason: resp.stop_reason.clone(),
                        }];
                        let finish = resp.stop_reason.clone();
                        (msgs, finish)
                    })
                    .unwrap_or_else(|| (vec![], None))
            }
            _ => (vec![], None),
        };

        // SysOM response handling
        let (messages, finish_reason) = if messages.is_empty() {
            match message {
                Some(ParsedApiMessage::SysomMessage { response, .. }) => response
                    .as_ref()
                    .map(|resp| {
                        let choice = resp.choices.first();
                        let mut parts = Vec::new();
                        if let Some(choice) = choice {
                            if !choice.message.content.is_empty() {
                                parts.push(MessagePart::Text {
                                    content: choice.message.content.clone(),
                                });
                            }
                            if let Some(ref tool_use) = choice.message.tool_use {
                                for item in tool_use {
                                    let arguments = serde_json::from_str::<serde_json::Value>(
                                        &item.function.arguments,
                                    )
                                    .ok();
                                    parts.push(MessagePart::ToolCall {
                                        id: Some(item.id.clone()),
                                        name: item.function.name.clone(),
                                        arguments,
                                    });
                                }
                            }
                        }
                        let msgs = if parts.is_empty() {
                            vec![]
                        } else {
                            vec![OutputMessage {
                                role: "assistant".to_string(),
                                parts,
                                name: None,
                                finish_reason: Some("stop".to_string()),
                            }]
                        };
                        (msgs, Some("stop".to_string()))
                    })
                    .unwrap_or_else(|| (vec![], None)),
                _ => (messages, finish_reason),
            }
        } else {
            (messages, finish_reason)
        };

        // For SSE responses, extract from response_body when no parsed message
        let messages = if messages.is_empty() && http.is_sse {
            // No parsed response — reconstruct from SSE response body directly
            if let Some(ref body) = http.response_body {
                Self::parse_sse_response_body(body, finish_reason.as_deref()).unwrap_or(messages)
            } else {
                messages
            }
        } else if http.is_sse {
            // Has parsed response but may be missing reasoning/tool_calls — enrich from SSE body
            let mut msgs = messages;
            if let Some(ref body) = http.response_body {
                if let Some(msg) = msgs.first_mut() {
                    if msg.role == "assistant" {
                        let has_reasoning = msg
                            .parts
                            .iter()
                            .any(|p| matches!(p, MessagePart::Reasoning { .. }));
                        // Check if any tool_call is missing id
                        let has_tool_calls_without_id = msg
                            .parts
                            .iter()
                            .any(|p| matches!(p, MessagePart::ToolCall { id, .. } if id.is_none()));
                        let has_tool_calls = msg
                            .parts
                            .iter()
                            .any(|p| matches!(p, MessagePart::ToolCall { .. }));

                        if let Some((extra, sse_finish)) = Self::extract_parts_from_sse_body(body) {
                            if !has_reasoning {
                                if let Some(r) = extra
                                    .iter()
                                    .find(|p| matches!(p, MessagePart::Reasoning { .. }))
                                {
                                    msg.parts.insert(0, r.clone());
                                }
                            }
                            // Always try to enrich tool_calls if missing id or no tool_calls
                            if !has_tool_calls || has_tool_calls_without_id {
                                // Remove existing tool_calls without id, replace with SSE ones
                                if has_tool_calls_without_id {
                                    msg.parts.retain(|p| !matches!(p, MessagePart::ToolCall { id, .. } if id.is_none()));
                                }
                                for p in extra
                                    .into_iter()
                                    .filter(|p| matches!(p, MessagePart::ToolCall { .. }))
                                {
                                    msg.parts.push(p);
                                }
                            }
                            // Enrich finish_reason if missing
                            if msg.finish_reason.is_none() {
                                msg.finish_reason = sse_finish;
                            }
                        }
                    }
                }
            }
            msgs
        } else {
            messages
        };

        LLMResponse {
            messages,
            streamed: http.is_sse,
            raw_body: http.response_body.clone(),
        }
    }

    /// Check if the path indicates an LLM API call
    fn is_llm_api_path(&self, path: &str) -> bool {
        path.contains("/v1/chat/completions")
            || path.contains("/v1/completions")
            || path.contains("/v1/messages")
            || path.contains("/v1/responses")
            || path.contains("/chat/completions")
            || path.contains("/completions")
            || path.contains("/api/v1/copilot/generate_copilot")
    }

    /// Check if request body contains SysOM POP API markers
    /// SysOM uses path "/" with action in body (llmParamString field)
    fn is_sysom_pop_request(request_body: &Option<String>) -> bool {
        request_body
            .as_ref()
            .map(|b| b.contains("llmParamString"))
            .unwrap_or(false)
    }

    /// Extract provider from path
    fn extract_provider_from_path(&self, path: &str) -> Option<String> {
        if path.contains("anthropic") || path.contains("/v1/messages") {
            Some("anthropic".to_string())
        } else if path.contains("/v1/chat/completions")
            || path.contains("/v1/completions")
            || path.contains("/v1/responses")
        {
            Some("openai".to_string())
        } else if path.contains("/api/v1/copilot/generate_copilot") {
            Some("sysom".to_string())
        } else {
            None
        }
    }

    /// Extract provider from request body (for POP API style requests)
    fn extract_provider_from_body(request_body: &Option<String>) -> Option<String> {
        if Self::is_sysom_pop_request(request_body) {
            Some("sysom".to_string())
        } else {
            None
        }
    }

    /// Extract model from parsed message
    fn extract_model_from_message(&self, message: &Option<ParsedApiMessage>) -> Option<String> {
        match message {
            Some(ParsedApiMessage::OpenAICompletion { request, .. }) => {
                request.as_ref().map(|r| r.model.clone())
            }
            Some(ParsedApiMessage::AnthropicMessage { request, .. }) => {
                request.as_ref().map(|r| r.model.clone())
            }
            Some(ParsedApiMessage::SysomMessage { request, .. }) => {
                request.as_ref().map(|r| r.params.model.clone())
            }
            _ => None,
        }
    }

    /// 从 HTTP request/response body 中直接提取 model 字段
    ///
    /// 优先从 request body 取（用户请求的 model），
    /// 如果没有则从 response body 取（SSE 响应中的 model）
    /// 对于 SysOM 请求，需要从 llmParamString 内嵌 JSON 中提取 model
    fn extract_model_from_body(
        request_body: &Option<String>,
        response_body: &Option<String>,
    ) -> Option<String> {
        // 尝试从 request body 获取
        if let Some(body) = request_body {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
                // 标准 OpenAI/Anthropic 格式
                if let Some(model) = v.get("model").and_then(|m| m.as_str()) {
                    if !model.is_empty() {
                        return Some(model.to_string());
                    }
                }
                // SysOM 格式：model 嵌套在 llmParamString 中
                if let Some(lps) = v.get("llmParamString").and_then(|v| v.as_str()) {
                    if let Ok(inner) = serde_json::from_str::<serde_json::Value>(lps) {
                        if let Some(model) = inner.get("model").and_then(|m| m.as_str()) {
                            if !model.is_empty() {
                                return Some(model.to_string());
                            }
                        }
                    }
                }
            }
        }
        // 尝试从 response body 获取（SSE 响应是 JSON 数组，取第一个 chunk）
        if let Some(body) = response_body {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
                // 非 SSE: 直接是 JSON 对象
                if let Some(model) = v.get("model").and_then(|m| m.as_str()) {
                    if !model.is_empty() {
                        return Some(model.to_string());
                    }
                }
                // SSE: JSON 数组，取第一个 chunk 的 model
                if let Some(arr) = v.as_array() {
                    for chunk in arr {
                        if let Some(model) = chunk.get("model").and_then(|m| m.as_str()) {
                            if !model.is_empty() {
                                return Some(model.to_string());
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Extract the LLM API response ID from parsed message or SSE body.
    ///
    /// Priority:
    /// 1. ParsedApiMessage response.id (OpenAI / Anthropic)
    /// 2. SSE response body first chunk "id" field
    /// 3. None (caller should fall back to call_id)
    fn extract_response_id(
        parsed_message: &Option<ParsedApiMessage>,
        http: &HttpRecord,
    ) -> Option<String> {
        // 1. Try parsed message response.id
        if let Some(msg) = parsed_message {
            match msg {
                ParsedApiMessage::OpenAICompletion {
                    response: Some(resp),
                    ..
                } => {
                    if !resp.id.is_empty() {
                        return Some(resp.id.clone());
                    }
                }
                ParsedApiMessage::AnthropicMessage {
                    response: Some(resp),
                    ..
                } => {
                    if !resp.id.is_empty() {
                        return Some(resp.id.clone());
                    }
                }
                _ => {}
            }
        }

        // 2. SSE fallback: extract "id" field from response body
        if http.is_sse {
            if let Some(ref body) = http.response_body {
                // Try JSON array format first (from HTTP/2 stream aggregation)
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
                    if let Some(arr) = v.as_array() {
                        for chunk in arr {
                            if let Some(id) = chunk.get("id").and_then(|v| v.as_str()) {
                                if !id.is_empty() {
                                    return Some(id.to_string());
                                }
                            }
                        }
                    }
                }
                // Try SSE line format (from HTTP/1.1: "data: {...}" per line)
                for line in body.lines() {
                    let json_str = line.strip_prefix("data: ").unwrap_or(line).trim();
                    if json_str.is_empty() || json_str == "[DONE]" {
                        continue;
                    }
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) {
                        if let Some(id) = v.get("id").and_then(|v| v.as_str()) {
                            if !id.is_empty() {
                                return Some(id.to_string());
                            }
                        }
                    }
                }
            }
        }

        None
    }

    /// Generate globally unique ID (unique across restarts)
    fn generate_id(&self) -> String {
        let seq = self.call_counter.fetch_add(1, Ordering::Relaxed);
        format!("{}_{}", self.session_prefix, seq)
    }

    /// 提取第一条有实际文本内容的 user message 的原始文本
    ///
    /// 仅返回含非空 `Text` 片段的首条 user message，供 `IdResolver`
    /// 生成 session_key 使用。跳过只含 tool_result 等的 user message。
    fn extract_first_user_raw(request: &LLMRequest) -> Option<String> {
        request
            .messages
            .iter()
            .filter(|m| m.role == "user")
            .find_map(|m| {
                let text: String = m
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        MessagePart::Text { content } if !content.is_empty() => {
                            Some(content.as_str())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if text.is_empty() { None } else { Some(text) }
            })
    }

    /// 提取最后一条有实际文本内容的 user message 的原始文本
    ///
    /// 跳过 Anthropic 格式中只包含 tool_result 的 user message
    fn extract_last_user_raw(request: &LLMRequest) -> Option<String> {
        request
            .messages
            .iter()
            .rev()
            .filter(|m| m.role == "user")
            .find_map(|m| {
                let text: String = m
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        MessagePart::Text { content } if !content.is_empty() => {
                            Some(content.as_str())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if text.is_empty() { None } else { Some(text) }
            })
    }

    /// 提取清理后的 user query（去除 metadata 前缀，用于展示）
    fn extract_last_user_query(request: &LLMRequest) -> Option<String> {
        Self::extract_last_user_raw(request).map(|raw| Self::strip_user_query_prefix(&raw))
    }

    /// 去除 user message 中的 metadata 前缀，只保留用户实际输入的文本
    ///
    /// OpenClaw 等 Agent 会在 user message 前面加上元数据，格式如：
    /// ```text
    /// Sender (untrusted metadata):
    /// ```json
    /// {"label":"...", ...}
    /// ```
    ///
    /// [Tue 2026-03-31 17:19 GMT+8] 用户实际输入
    /// ```
    fn strip_user_query_prefix(text: &str) -> String {
        // 查找最后一个 [timestamp] 模式，取其后的内容
        // 格式: [Day YYYY-MM-DD HH:MM TZ] 或 [Day, DD Mon YYYY HH:MM:SS TZ]
        if let Some(pos) = text.rfind(']') {
            // 确认 ] 前面有对应的 [
            if let Some(bracket_start) = text[..pos].rfind('[') {
                let bracket_content = &text[bracket_start + 1..pos];
                // 简单验证：方括号内包含数字（日期）和冒号（时间）
                if bracket_content.contains(':')
                    && bracket_content.chars().any(|c| c.is_ascii_digit())
                {
                    let after = text[pos + 1..].trim_start();
                    if !after.is_empty() {
                        return after.to_string();
                    }
                }
            }
        }
        text.to_string()
    }

    /// Resolve agent name from comm string only (no /proc access).
    /// Used for dead-PID drain where the process is already gone.
    fn resolve_agent_name_from_comm(
        comm: &str,
        pid: u32,
        cache: &std::collections::HashMap<u32, String>,
    ) -> Option<String> {
        // First check the pid→agent_name cache (works even for dead processes)
        if let Some(name) = cache.get(&pid) {
            return Some(name.clone());
        }
        let ctx = ProcessContext {
            comm: comm.to_string(),
            cmdline_args: vec![],
            exe_path: String::new(),
        };
        default_cmdline_rules()
            .iter()
            .filter_map(CmdlineGlobMatcher::from_config)
            .find(|m| m.matches(&ctx))
            .map(|m| m.info().name.clone())
    }

    /// 通过进程名匹配 agent registry，返回已知 agent 名称
    fn resolve_agent_name(
        comm: &str,
        pid: u32,
        cache: &std::collections::HashMap<u32, String>,
    ) -> Option<String> {
        // First check the pid→agent_name cache (works even for dead processes)
        if let Some(name) = cache.get(&pid) {
            return Some(name.clone());
        }
        // Read cmdline from /proc/{pid}/cmdline for accurate agent matching
        let cmdline_args = std::fs::read(format!("/proc/{pid}/cmdline"))
            .ok()
            .map(|data| {
                data.split(|&b| b == 0)
                    .filter(|s| !s.is_empty())
                    .map(|s| String::from_utf8_lossy(s).to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let exe_path = std::fs::read_link(format!("/proc/{pid}/exe"))
            .ok()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        let ctx = ProcessContext {
            comm: comm.to_string(),
            cmdline_args,
            exe_path,
        };
        default_cmdline_rules()
            .iter()
            .filter_map(CmdlineGlobMatcher::from_config)
            .find(|m| m.matches(&ctx))
            .map(|m| m.info().name.clone())
    }

    /// Convert OpenAI ChatMessage to parts-based InputMessage
    fn openai_msg_to_input(m: &OpenAIChatMessage) -> InputMessage {
        let role = format!("{:?}", m.role).to_lowercase();
        let mut parts = Vec::new();

        // Reasoning content first
        if let Some(ref rc) = m.reasoning_content {
            if !rc.is_empty() {
                parts.push(MessagePart::Reasoning {
                    content: rc.clone(),
                });
            }
        }

        // For tool role: content is tool_call_response
        if role == "tool" {
            let response_val = m
                .content
                .as_ref()
                .map(|c| {
                    let text = c.as_text();
                    // Try to parse as JSON, fall back to string
                    serde_json::from_str::<serde_json::Value>(&text)
                        .unwrap_or_else(|_| serde_json::Value::String(text))
                })
                .unwrap_or(serde_json::Value::Null);
            parts.push(MessagePart::ToolCallResponse {
                id: m.tool_call_id.clone(),
                response: response_val,
            });
        } else {
            // Text content
            if let Some(ref c) = m.content {
                let text = c.as_text();
                if !text.is_empty() {
                    parts.push(MessagePart::Text { content: text });
                }
            }
        }

        // Tool calls
        if let Some(ref tcs) = m.tool_calls {
            for tc in tcs {
                if let Some(part) = Self::parse_openai_tool_call_value(tc) {
                    parts.push(part);
                }
            }
        }

        InputMessage {
            role,
            parts,
            name: m.name.clone(),
        }
    }

    /// Convert OpenAI ChatMessage to parts-based OutputMessage
    fn openai_msg_to_output(m: &OpenAIChatMessage, finish_reason: Option<&str>) -> OutputMessage {
        let role = format!("{:?}", m.role).to_lowercase();
        let mut parts = Vec::new();

        // Reasoning content first
        if let Some(ref rc) = m.reasoning_content {
            if !rc.is_empty() {
                parts.push(MessagePart::Reasoning {
                    content: rc.clone(),
                });
            }
        }

        // Text content
        if let Some(ref c) = m.content {
            let text = c.as_text();
            if !text.is_empty() {
                parts.push(MessagePart::Text { content: text });
            }
        }

        // Tool calls
        if let Some(ref tcs) = m.tool_calls {
            for tc in tcs {
                if let Some(part) = Self::parse_openai_tool_call_value(tc) {
                    parts.push(part);
                }
            }
        }

        OutputMessage {
            role,
            parts,
            name: m.name.clone(),
            finish_reason: finish_reason.map(|s| s.to_string()),
        }
    }

    /// Parse a serde_json::Value tool_call into MessagePart::ToolCall
    fn parse_openai_tool_call_value(tc: &serde_json::Value) -> Option<MessagePart> {
        let func = tc.get("function")?;
        let name = func.get("name")?.as_str()?.to_string();
        let id = tc.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
        // Parse arguments as JSON object (not string)
        let arguments = func.get("arguments").and_then(|v| match v {
            serde_json::Value::String(s) => serde_json::from_str(s).ok(),
            other => Some(other.clone()),
        });
        Some(MessagePart::ToolCall {
            id,
            name,
            arguments,
        })
    }

    // NOTE: token_record_to_parts and parse_tool_call_strings removed.
    // Tool calls and reasoning are now extracted directly from SSE response body
    // via extract_parts_from_sse_body / parse_sse_response_body.

    /// Parse SSE response body (JSON array of chunks) into a complete OutputMessage.
    ///
    /// Merges content/reasoning deltas and tool_call argument fragments by index.
    /// Extracts finish_reason from the last SSE chunk that has one.
    fn parse_sse_response_body(
        body: &str,
        fallback_finish_reason: Option<&str>,
    ) -> Option<Vec<OutputMessage>> {
        let (parts, sse_finish_reason) = Self::extract_parts_from_sse_body(body)?;
        if parts.is_empty() {
            return None;
        }
        // Prefer finish_reason from SSE, fall back to caller-supplied value
        let finish_reason =
            sse_finish_reason.or_else(|| fallback_finish_reason.map(|s| s.to_string()));
        Some(vec![OutputMessage {
            role: "assistant".to_string(),
            parts,
            name: None,
            finish_reason,
        }])
    }

    /// Extract MessageParts + finish_reason by aggregating all SSE chunks in response_body.
    ///
    /// Handles OpenAI SSE delta format:
    /// - content deltas → single Text part
    /// - reasoning_content deltas → single Reasoning part
    /// - tool_calls deltas (fragmented by index) → merged ToolCall parts
    /// - finish_reason from the last non-null value in choices
    ///
    /// Returns (parts, finish_reason) or None if no content found.
    fn extract_parts_from_sse_body(body: &str) -> Option<(Vec<MessagePart>, Option<String>)> {
        let chunks: Vec<serde_json::Value> = serde_json::from_str(body).ok()?;

        let mut content_buf = String::new();
        let mut reasoning_buf = String::new();
        let mut finish_reason: Option<String> = None;
        // tool_call delta merging: index -> (id, name, arguments_accumulated)
        let mut tc_map: HashMap<u32, (String, String, String)> = HashMap::new();

        log::debug!("[GenAI] Parsing SSE body with {} chunks", chunks.len());

        for chunk in chunks.iter() {
            let choices = chunk.get("choices").and_then(|c| c.as_array());
            let choices = match choices {
                Some(c) => c,
                None => continue,
            };
            for choice in choices {
                let delta = match choice.get("delta") {
                    Some(d) => d,
                    None => continue,
                };
                // Content
                if let Some(c) = delta.get("content").and_then(|v| v.as_str()) {
                    content_buf.push_str(c);
                }
                // Reasoning
                if let Some(r) = delta.get("reasoning_content").and_then(|v| v.as_str()) {
                    reasoning_buf.push_str(r);
                }
                // Tool call deltas — merge by index
                if let Some(calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
                    for tc in calls {
                        let idx = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        let entry = tc_map
                            .entry(idx)
                            .or_insert_with(|| (String::new(), String::new(), String::new()));
                        if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                            if !id.is_empty() {
                                entry.0 = id.to_string();
                            }
                            // 空字符串不覆盖已有的 id
                        }
                        if let Some(func) = tc.get("function") {
                            if let Some(name) = func.get("name").and_then(|v| v.as_str()) {
                                entry.1 = name.to_string();
                            }
                            if let Some(args) = func.get("arguments").and_then(|v| v.as_str()) {
                                entry.2.push_str(args);
                            }
                        }
                    }
                }
                // Finish reason — take the last non-null value
                if let Some(fr) = choice.get("finish_reason").and_then(|v| v.as_str()) {
                    finish_reason = Some(fr.to_string());
                }
            }
        }

        let mut parts = Vec::new();

        // Reasoning first
        if !reasoning_buf.is_empty() {
            parts.push(MessagePart::Reasoning {
                content: reasoning_buf,
            });
        }
        // Text content
        if !content_buf.is_empty() {
            parts.push(MessagePart::Text {
                content: content_buf,
            });
        }
        // Merged tool calls
        if !tc_map.is_empty() {
            let mut indices: Vec<u32> = tc_map.keys().cloned().collect();
            indices.sort();
            for idx in indices {
                if let Some((id, name, arguments)) = tc_map.remove(&idx) {
                    let parsed_args: Option<serde_json::Value> = if arguments.is_empty() {
                        None
                    } else {
                        serde_json::from_str(&arguments).ok()
                    };
                    parts.push(MessagePart::ToolCall {
                        id: if id.is_empty() { None } else { Some(id) },
                        name,
                        arguments: parsed_args,
                    });
                }
            }
        }

        if parts.is_empty() {
            None
        } else {
            Some((parts, finish_reason))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_generate_id_unique() {
        let builder = GenAIBuilder::new();
        let id1 = builder.generate_id();
        let id2 = builder.generate_id();
        assert_ne!(id1, id2);
        assert!(id1.contains('_'));
    }

    #[test]
    fn test_is_llm_api_path() {
        let builder = GenAIBuilder::new();
        assert!(builder.is_llm_api_path("/v1/chat/completions"));
        assert!(builder.is_llm_api_path("/v1/completions"));
        assert!(builder.is_llm_api_path("/v1/messages"));
        assert!(builder.is_llm_api_path("/api/v1/copilot/generate_copilot"));
        assert!(builder.is_llm_api_path("/proxy/v1/chat/completions"));
        assert!(!builder.is_llm_api_path("/api/health"));
        assert!(!builder.is_llm_api_path("/v1/models"));
    }

    #[test]
    fn test_is_sysom_pop_request() {
        assert!(GenAIBuilder::is_sysom_pop_request(&Some(
            r#"{"llmParamString":"{}"}"#.to_string()
        )));
        assert!(!GenAIBuilder::is_sysom_pop_request(&Some("{}".to_string())));
        assert!(!GenAIBuilder::is_sysom_pop_request(&None));
    }

    #[test]
    fn test_extract_provider_from_path() {
        let builder = GenAIBuilder::new();
        assert_eq!(
            builder.extract_provider_from_path("/v1/chat/completions"),
            Some("openai".to_string())
        );
        assert_eq!(
            builder.extract_provider_from_path("/v1/messages"),
            Some("anthropic".to_string())
        );
        assert_eq!(
            builder.extract_provider_from_path("/api/v1/copilot/generate_copilot"),
            Some("sysom".to_string())
        );
        assert_eq!(builder.extract_provider_from_path("/unknown"), None);
    }

    #[test]
    fn test_extract_provider_from_body() {
        assert_eq!(
            GenAIBuilder::extract_provider_from_body(&Some(
                r#"{"llmParamString":"{}"} "#.to_string()
            )),
            Some("sysom".to_string())
        );
        assert_eq!(
            GenAIBuilder::extract_provider_from_body(&Some("{}".to_string())),
            None
        );
    }

    #[test]
    fn test_extract_model_from_body_request() {
        let body = Some(r#"{"model": "gpt-4", "messages": []}"#.to_string());
        assert_eq!(
            GenAIBuilder::extract_model_from_body(&body, &None),
            Some("gpt-4".to_string())
        );
    }

    #[test]
    fn test_extract_model_from_body_sysom() {
        let body = Some(r#"{"llmParamString": "{\"model\":\"qwen-max\"}"} "#.to_string());
        assert_eq!(
            GenAIBuilder::extract_model_from_body(&body, &None),
            Some("qwen-max".to_string())
        );
    }

    #[test]
    fn test_extract_model_from_body_response() {
        let resp = Some(r#"{"model": "claude-3"}"#.to_string());
        assert_eq!(
            GenAIBuilder::extract_model_from_body(&None, &resp),
            Some("claude-3".to_string())
        );
    }

    #[test]
    fn test_extract_model_from_body_sse_array() {
        let resp = Some(r#"[{"model": "gpt-4o"}, {"model": "gpt-4o"}]"#.to_string());
        assert_eq!(
            GenAIBuilder::extract_model_from_body(&None, &resp),
            Some("gpt-4o".to_string())
        );
    }

    #[test]
    fn test_extract_model_from_body_none() {
        assert_eq!(GenAIBuilder::extract_model_from_body(&None, &None), None);
    }

    #[test]
    fn test_strip_user_query_prefix_with_timestamp() {
        let text = "Sender (untrusted metadata):\n```json\n{}\n```\n\n[Tue 2026-03-31 17:19 GMT+8] hello world";
        assert_eq!(GenAIBuilder::strip_user_query_prefix(text), "hello world");
    }

    #[test]
    fn test_strip_user_query_prefix_no_timestamp() {
        let text = "plain user input";
        assert_eq!(
            GenAIBuilder::strip_user_query_prefix(text),
            "plain user input"
        );
    }

    #[test]
    fn test_strip_user_query_prefix_bracket_no_datetime() {
        let text = "[not a timestamp] content";
        // No ':' and digit in bracket content -> returns original
        assert_eq!(
            GenAIBuilder::strip_user_query_prefix(text),
            "[not a timestamp] content"
        );
    }

    #[test]
    fn test_extract_last_user_query() {
        let req = LLMRequest {
            messages: vec![
                InputMessage {
                    role: "system".to_string(),
                    parts: vec![MessagePart::Text {
                        content: "sys".to_string(),
                    }],
                    name: None,
                },
                InputMessage {
                    role: "user".to_string(),
                    parts: vec![MessagePart::Text {
                        content: "[Mon 2026-01-01 10:00 GMT+8] hi".to_string(),
                    }],
                    name: None,
                },
            ],
            temperature: None,
            max_tokens: None,
            frequency_penalty: None,
            presence_penalty: None,
            top_p: None,
            top_k: None,
            seed: None,
            stop_sequences: None,
            stream: false,
            tools: None,
            raw_body: None,
        };
        assert_eq!(
            GenAIBuilder::extract_last_user_query(&req),
            Some("hi".to_string())
        );
    }

    #[test]
    fn test_parse_request_body_openai() {
        let body = r#"{
            "model": "gpt-4",
            "messages": [
                {"role": "system", "content": "You are helpful."},
                {"role": "user", "content": "Hello"}
            ],
            "temperature": 0.7,
            "max_tokens": 1024,
            "stream": true
        }"#;
        let req = GenAIBuilder::parse_request_body(body).unwrap();
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].role, "system");
        assert_eq!(req.messages[1].role, "user");
        assert_eq!(req.temperature, Some(0.7));
        assert_eq!(req.max_tokens, Some(1024));
        assert!(req.stream);
    }

    #[test]
    fn test_parse_request_body_with_tool_calls() {
        let body = r#"{
            "model": "gpt-4",
            "messages": [
                {"role": "assistant", "content": "", "tool_calls": [{"id": "tc_1", "function": {"name": "search", "arguments": "{\"q\":\"rust\"}"}}]},
                {"role": "tool", "tool_call_id": "tc_1", "content": "found 10 results"}
            ]
        }"#;
        let req = GenAIBuilder::parse_request_body(body).unwrap();
        assert_eq!(req.messages.len(), 2);
        // assistant message has ToolCall part
        let parts = &req.messages[0].parts;
        assert!(
            parts
                .iter()
                .any(|p| matches!(p, MessagePart::ToolCall { name, .. } if name == "search"))
        );
        // tool message has ToolCallResponse part
        let tool_parts = &req.messages[1].parts;
        assert!(
            tool_parts
                .iter()
                .any(|p| matches!(p, MessagePart::ToolCallResponse { .. }))
        );
    }

    #[test]
    fn test_parse_request_body_empty_messages() {
        let body = r#"{"model": "gpt-4", "messages": []}"#;
        assert!(GenAIBuilder::parse_request_body(body).is_none());
    }

    #[test]
    fn test_parse_request_body_invalid_json() {
        assert!(GenAIBuilder::parse_request_body("not json").is_none());
    }

    #[test]
    fn test_parse_openai_tool_call_value() {
        let tc = json!({
            "id": "call_abc",
            "function": {
                "name": "get_weather",
                "arguments": "{\"city\":\"Beijing\"}"
            }
        });
        let part = GenAIBuilder::parse_openai_tool_call_value(&tc).unwrap();
        match part {
            MessagePart::ToolCall {
                id,
                name,
                arguments,
            } => {
                assert_eq!(id.unwrap(), "call_abc");
                assert_eq!(name, "get_weather");
                assert_eq!(arguments.unwrap()["city"], "Beijing");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_parse_openai_tool_call_value_no_function() {
        let tc = json!({"id": "call_abc"});
        assert!(GenAIBuilder::parse_openai_tool_call_value(&tc).is_none());
    }

    #[test]
    fn test_extract_parts_from_sse_body_content() {
        let body = r#"[{"choices":[{"delta":{"content":"Hello "}}]},{"choices":[{"delta":{"content":"world"},"finish_reason":"stop"}]}]"#;
        let (parts, finish) = GenAIBuilder::extract_parts_from_sse_body(body).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            MessagePart::Text { content } => assert_eq!(content, "Hello world"),
            _ => panic!("expected Text"),
        }
        assert_eq!(finish, Some("stop".to_string()));
    }

    #[test]
    fn test_extract_parts_from_sse_body_reasoning() {
        let body = r#"[{"choices":[{"delta":{"reasoning_content":"thinking..."}}]},{"choices":[{"delta":{"content":"answer"}}]}]"#;
        let (parts, _) = GenAIBuilder::extract_parts_from_sse_body(body).unwrap();
        assert_eq!(parts.len(), 2);
        assert!(
            matches!(&parts[0], MessagePart::Reasoning { content } if content == "thinking...")
        );
        assert!(matches!(&parts[1], MessagePart::Text { content } if content == "answer"));
    }

    #[test]
    fn test_extract_parts_from_sse_body_tool_calls() {
        let body = r#"[
            {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"tc_1","function":{"name":"search","arguments":""}}]}}]},
            {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"q\""}}]}}]},
            {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":": \"rust\"}"}}]},"finish_reason":"tool_calls"}]}
        ]"#;
        let (parts, finish) = GenAIBuilder::extract_parts_from_sse_body(body).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            MessagePart::ToolCall {
                id,
                name,
                arguments,
            } => {
                assert_eq!(id.as_deref(), Some("tc_1"));
                assert_eq!(name, "search");
                assert_eq!(arguments.as_ref().unwrap()["q"], "rust");
            }
            _ => panic!("expected ToolCall"),
        }
        assert_eq!(finish, Some("tool_calls".to_string()));
    }

    #[test]
    fn test_extract_parts_from_sse_body_empty() {
        let body = r#"[{"choices":[{"delta":{}}]}]"#;
        assert!(GenAIBuilder::extract_parts_from_sse_body(body).is_none());
    }

    #[test]
    fn test_extract_parts_from_sse_body_invalid() {
        assert!(GenAIBuilder::extract_parts_from_sse_body("not json").is_none());
    }

    #[test]
    fn test_parse_sse_response_body() {
        let body = r#"[{"choices":[{"delta":{"content":"Hi"}}]}]"#;
        let msgs = GenAIBuilder::parse_sse_response_body(body, Some("stop")).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn test_openai_msg_to_input_basic() {
        let msg = OpenAIChatMessage {
            role: crate::analyzer::message::types::MessageRole::User,
            content: Some(crate::analyzer::message::types::OpenAIContent::Text(
                "Hello".to_string(),
            )),
            reasoning_content: None,
            refusal: None,
            function_call: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            annotations: None,
            audio: None,
        };
        let input = GenAIBuilder::openai_msg_to_input(&msg);
        assert_eq!(input.role, "user");
        assert_eq!(input.parts.len(), 1);
        match &input.parts[0] {
            MessagePart::Text { content } => assert_eq!(content, "Hello"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn test_openai_msg_to_input_tool_role() {
        let msg = OpenAIChatMessage {
            role: crate::analyzer::message::types::MessageRole::Tool,
            content: Some(crate::analyzer::message::types::OpenAIContent::Text(
                "result data".to_string(),
            )),
            reasoning_content: None,
            refusal: None,
            function_call: None,
            tool_calls: None,
            tool_call_id: Some("tc_1".to_string()),
            name: None,
            annotations: None,
            audio: None,
        };
        let input = GenAIBuilder::openai_msg_to_input(&msg);
        assert_eq!(input.role, "tool");
        assert!(
            matches!(&input.parts[0], MessagePart::ToolCallResponse { id, .. } if id.as_deref() == Some("tc_1"))
        );
    }

    #[test]
    fn test_openai_msg_to_output() {
        let msg = OpenAIChatMessage {
            role: crate::analyzer::message::types::MessageRole::Assistant,
            content: Some(crate::analyzer::message::types::OpenAIContent::Text(
                "Response".to_string(),
            )),
            reasoning_content: Some("thinking".to_string()),
            refusal: None,
            function_call: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            annotations: None,
            audio: None,
        };
        let output = GenAIBuilder::openai_msg_to_output(&msg, Some("stop"));
        assert_eq!(output.role, "assistant");
        assert_eq!(output.finish_reason.as_deref(), Some("stop"));
        // Should have reasoning + text = 2 parts
        assert_eq!(output.parts.len(), 2);
        assert!(
            matches!(&output.parts[0], MessagePart::Reasoning { content } if content == "thinking")
        );
        assert!(matches!(&output.parts[1], MessagePart::Text { content } if content == "Response"));
    }

    #[test]
    fn test_resolve_agent_name_from_comm_with_cache() {
        let mut cache = HashMap::new();
        cache.insert(42u32, "CachedAgent".to_string());
        let result = GenAIBuilder::resolve_agent_name_from_comm("unknown", 42, &cache);
        assert_eq!(result, Some("CachedAgent".to_string()));
    }

    #[test]
    fn test_resolve_agent_name_from_comm_no_match() {
        let cache = HashMap::new();
        let result = GenAIBuilder::resolve_agent_name_from_comm("random_process", 99, &cache);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_model_from_message() {
        let builder = GenAIBuilder::new();
        let msg = Some(ParsedApiMessage::OpenAICompletion {
            request: Some(crate::analyzer::message::types::OpenAIRequest {
                model: "gpt-4-turbo".to_string(),
                messages: vec![],
                temperature: None,
                max_tokens: None,
                stream: None,
                top_p: None,
                n: None,
                stop: None,
                presence_penalty: None,
                frequency_penalty: None,
                user: None,
                tools: None,
                tool_choice: None,
                response_format: None,
                seed: None,
                logprobs: None,
                top_logprobs: None,
                parallel_tool_calls: None,
            }),
            response: None,
        });
        assert_eq!(
            builder.extract_model_from_message(&msg),
            Some("gpt-4-turbo".to_string())
        );
        assert_eq!(builder.extract_model_from_message(&None), None);
    }

    #[test]
    fn test_default_builder() {
        let b1 = GenAIBuilder::default();
        let b2 = GenAIBuilder::new();
        // Both should have different session prefixes (different timestamps)
        // But both should generate valid IDs
        let id1 = b1.generate_id();
        let id2 = b2.generate_id();
        assert!(id1.contains('_'));
        assert!(id2.contains('_'));
    }
}
