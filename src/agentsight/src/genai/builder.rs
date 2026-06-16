//! GenAI Semantic Builder
//!
//! This module builds GenAI semantic events from AnalysisResult.
//! It reuses already-extracted data to avoid redundant parsing.

use super::id_resolver::IdResolver;
use super::semantic::GenAISemanticEvent;
use crate::aggregator::{ConnectionId, ParsedRequest};
use crate::analyzer::AnalysisResult;
use crate::analyzer::token::TokenParser;
use crate::parser::sse::ParsedSseEvent;
use crate::response_map::ResponseSessionMapper;
use crate::storage::sqlite::{PendingCallInfo, SseEnrichment};
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
    pub(super) id_resolver: IdResolver,
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

    /// Generate globally unique ID (unique across restarts)
    pub(super) fn generate_id(&self) -> String {
        let seq = self.call_counter.fetch_add(1, Ordering::Relaxed);
        format!("{}_{}", self.session_prefix, seq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_id_unique() {
        let builder = GenAIBuilder::new();
        let id1 = builder.generate_id();
        let id2 = builder.generate_id();
        assert_ne!(id1, id2);
        assert!(id1.contains('_'));
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
