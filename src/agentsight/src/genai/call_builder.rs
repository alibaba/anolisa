//! GenAI LLM-call construction
//!
//! Builds `LLMCall` / `LLMRequest` / `LLMResponse` from `AnalysisResult`,
//! including OpenAI-specific SSE aggregation and tool-call delta merging.
//! Logic preserved verbatim from the original `builder.rs`; only visibility
//! widened to `pub(super)` so `builder.rs` can call `build_llm_call`.

use super::GenAIBuilder;
use super::semantic::{
    InputMessage, LLMCall, LLMRequest, LLMResponse, MessagePart, OutputMessage, TokenUsage,
};
use crate::analyzer::{AnalysisResult, HttpRecord, ParsedApiMessage, TokenRecord};
use crate::response_map::ResponseSessionMapper;
use std::collections::HashMap;

impl GenAIBuilder {
    /// Build LLMCall from analysis results
    ///
    /// Combines data from TokenRecord, HttpRecord, and ParsedApiMessage
    pub(super) fn build_llm_call(
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

        // Classify call_kind from request content
        let call_kind = super::helpers::classify_call_kind(&request);

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

        // session_id: 优先从 request metadata 获取（Claude Code），
        // 次优先 response ID → .jsonl UUID 映射，
        // 兜底 hash。
        let metadata_session = parsed_message
            .as_ref()
            .and_then(|m| m.request_metadata_session_id());
        let parsed_response_id = parsed_message
            .as_ref()
            .and_then(|m| m.response_id())
            .map(|s| s.to_string());
        let mapper_session = parsed_response_id
            .as_deref()
            .and_then(|rid| response_mapper.get_session_by_response_id(rid))
            .map(|s| s.to_string());
        let session_id = metadata_session.or(mapper_session).or_else(|| {
            self.id_resolver
                .resolve_session_id(&agent_name, pid_i32, &first_user_raw, &response_id)
        });

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
                if let Some(addr) = meta.get("server.address").cloned() {
                    meta.insert("http.domain".to_string(), addr);
                }
                // Derive gen_ai.operation.name from path
                if http.path.contains("/chat/completions") || http.path.contains("/v1/messages") {
                    meta.insert("operation_name".to_string(), "chat".to_string());
                } else if http.path.contains("/completions") {
                    meta.insert("operation_name".to_string(), "text_completion".to_string());
                } else if http.path.contains("/api/v1/copilot/generate_copilot") {
                    meta.insert("operation_name".to_string(), "chat".to_string());
                }
                meta.insert("call_kind".to_string(), call_kind.as_str().to_string());
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

    // parse_request_body / extract_response_id / openai_msg_to_input /
    // openai_msg_to_output / parse_openai_tool_call_value / parse_sse_response_body /
    // extract_parts_from_sse_body live in `openai_parse.rs` (same impl block).

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::message::sysom::{
        SysomFunction, SysomLlmParams, SysomMessage as SysMsg, SysomRequest, SysomResponse,
        SysomResponseChoice, SysomResponseMessage, SysomToolCall, SysomToolUseItem,
    };
    use crate::analyzer::message::types::{
        AnthropicContentBlock, AnthropicMessage as AnthMsg, AnthropicMessageContent,
        AnthropicRequest, AnthropicResponse, AnthropicUsage, MessageRole, OpenAIChatMessage,
        OpenAIChoice, OpenAIContent, OpenAIRequest, OpenAIResponse,
    };
    use crate::analyzer::{AnalysisResult, HttpRecord, ParsedApiMessage, TokenRecord};
    use crate::response_map::ResponseSessionMapper;
    use std::collections::HashMap;

    fn make_http(
        path: &str,
        request_body: Option<String>,
        response_body: Option<String>,
    ) -> HttpRecord {
        HttpRecord {
            timestamp_ns: 1_000_000_000,
            pid: 100,
            comm: "test_agent".to_string(),
            method: "POST".to_string(),
            path: path.to_string(),
            status_code: 200,
            request_headers: "{}".to_string(),
            request_body,
            response_headers: "{}".to_string(),
            response_body,
            duration_ns: 1_000_000,
            is_sse: false,
            sse_event_count: 0,
        }
    }

    fn build_call(builder: &GenAIBuilder, results: &[AnalysisResult]) -> Option<LLMCall> {
        let mapper = ResponseSessionMapper::new();
        let cache = HashMap::new();
        builder.build_llm_call(results, &mapper, &cache)
    }

    fn empty_chat_msg(role: MessageRole, content: Option<&str>) -> OpenAIChatMessage {
        OpenAIChatMessage {
            role,
            content: content.map(|s| OpenAIContent::Text(s.to_string())),
            reasoning_content: None,
            refusal: None,
            function_call: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            annotations: None,
            audio: None,
        }
    }

    #[test]
    fn test_build_llm_call_returns_none_for_non_llm() {
        let builder = GenAIBuilder::new();
        let http = make_http("/api/health", None, None);
        assert!(build_call(&builder, &[AnalysisResult::Http(http)]).is_none());
    }

    #[test]
    fn test_build_llm_call_returns_none_when_no_http() {
        let builder = GenAIBuilder::new();
        let token = TokenRecord::new(1, "x".to_string(), "openai".to_string(), 1, 2);
        assert!(build_call(&builder, &[AnalysisResult::Token(token)]).is_none());
    }

    #[test]
    fn test_build_llm_call_chat_completions_with_host() {
        let builder = GenAIBuilder::new();
        let body = r#"{"model":"gpt-4","messages":[{"role":"user","content":"hi"}]}"#.to_string();
        let mut http = make_http("/v1/chat/completions", Some(body), None);
        http.request_headers = r#"{"host":"api.openai.com:443"}"#.to_string();
        let call = build_call(&builder, &[AnalysisResult::Http(http)]).unwrap();
        assert_eq!(call.provider, "openai");
        assert_eq!(call.model, "gpt-4");
        assert_eq!(
            call.metadata.get("server.address").unwrap(),
            "api.openai.com"
        );
        assert_eq!(call.metadata.get("server.port").unwrap(), "443");
        assert_eq!(call.metadata.get("http.domain").unwrap(), "api.openai.com");
        assert_eq!(call.metadata.get("operation_name").unwrap(), "chat");
        assert!(call.metadata.contains_key("user_query"));
    }

    #[test]
    fn test_build_llm_call_host_without_port() {
        let builder = GenAIBuilder::new();
        let body = r#"{"model":"gpt-4","messages":[{"role":"user","content":"hi"}]}"#.to_string();
        let mut http = make_http("/v1/chat/completions", Some(body), None);
        http.request_headers = r#"{"Host":"example.com"}"#.to_string();
        let call = build_call(&builder, &[AnalysisResult::Http(http)]).unwrap();
        assert_eq!(call.metadata.get("server.address").unwrap(), "example.com");
        assert!(!call.metadata.contains_key("server.port"));
        assert_eq!(call.metadata.get("http.domain").unwrap(), "example.com");
    }

    #[test]
    fn test_build_llm_call_completions_path() {
        let builder = GenAIBuilder::new();
        let body = r#"{"model":"x","messages":[{"role":"user","content":"hi"}]}"#.to_string();
        let http = make_http("/v1/completions", Some(body), None);
        let call = build_call(&builder, &[AnalysisResult::Http(http)]).unwrap();
        assert_eq!(
            call.metadata.get("operation_name").unwrap(),
            "text_completion"
        );
    }

    #[test]
    fn test_build_llm_call_copilot_path_operation_name() {
        let builder = GenAIBuilder::new();
        let body = r#"{"llmParamString":"{\"model\":\"qwen\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}"}"#.to_string();
        let http = make_http(
            "/api/v1/copilot/generate_copilot_response",
            Some(body),
            None,
        );
        let call = build_call(&builder, &[AnalysisResult::Http(http)]).unwrap();
        assert_eq!(call.metadata.get("operation_name").unwrap(), "chat");
        assert_eq!(call.provider, "sysom");
    }

    #[test]
    fn test_build_llm_call_sysom_body_match_only() {
        let builder = GenAIBuilder::new();
        let body = r#"{"llmParamString":"{\"model\":\"qwen\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}"}"#.to_string();
        let http = make_http("/some/proxy/path", Some(body), None);
        let call = build_call(&builder, &[AnalysisResult::Http(http)]).unwrap();
        assert_eq!(call.provider, "sysom");
    }

    #[test]
    fn test_build_llm_call_error_object_message() {
        let builder = GenAIBuilder::new();
        let mut http = make_http(
            "/v1/chat/completions",
            None,
            Some(r#"{"error":{"message":"rate limit"}}"#.to_string()),
        );
        http.status_code = 429;
        let call = build_call(&builder, &[AnalysisResult::Http(http)]).unwrap();
        assert_eq!(call.error.as_deref(), Some("rate limit"));
    }

    #[test]
    fn test_build_llm_call_error_chunked_encoding() {
        let builder = GenAIBuilder::new();
        let body = "b6\r\n{\"error\":{\"message\":\"oops\"}}\r\n0\r\n\r\n";
        let mut http = make_http("/v1/messages", None, Some(body.to_string()));
        http.status_code = 500;
        let call = build_call(&builder, &[AnalysisResult::Http(http)]).unwrap();
        assert_eq!(call.error.as_deref(), Some("oops"));
    }

    #[test]
    fn test_build_llm_call_error_escaped_json_string() {
        let builder = GenAIBuilder::new();
        let body = r#"{"error":"{\"error\":{\"message\":\"inner_msg\"}}"}"#;
        let mut http = make_http("/v1/chat/completions", None, Some(body.to_string()));
        http.status_code = 400;
        let call = build_call(&builder, &[AnalysisResult::Http(http)]).unwrap();
        assert_eq!(call.error.as_deref(), Some("inner_msg"));
    }

    #[test]
    fn test_build_llm_call_error_top_level_message() {
        let builder = GenAIBuilder::new();
        let body = r#"{"message":"top msg"}"#;
        let mut http = make_http("/v1/chat/completions", None, Some(body.to_string()));
        http.status_code = 503;
        let call = build_call(&builder, &[AnalysisResult::Http(http)]).unwrap();
        assert_eq!(call.error.as_deref(), Some("top msg"));
    }

    #[test]
    fn test_build_llm_call_error_plain_text_fallback() {
        let builder = GenAIBuilder::new();
        let body = "Internal Server Error";
        let mut http = make_http("/v1/messages", None, Some(body.to_string()));
        http.status_code = 502;
        let call = build_call(&builder, &[AnalysisResult::Http(http)]).unwrap();
        assert_eq!(call.error.as_deref(), Some("Internal Server Error"));
    }

    #[test]
    fn test_build_llm_call_error_status_below_400_no_error() {
        let builder = GenAIBuilder::new();
        let body = r#"{"model":"x","messages":[{"role":"user","content":"hi"}]}"#.to_string();
        let http = make_http("/v1/chat/completions", Some(body), None);
        let call = build_call(&builder, &[AnalysisResult::Http(http)]).unwrap();
        assert!(call.error.is_none());
    }

    #[test]
    fn test_build_llm_call_with_token_record() {
        let builder = GenAIBuilder::new();
        let http = make_http("/v1/chat/completions", None, None);
        let token = TokenRecord {
            id: 0,
            timestamp_ns: 0,
            pid: 100,
            comm: "x".to_string(),
            agent: None,
            model: Some("token-model".to_string()),
            provider: "token-provider".to_string(),
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_tokens: Some(5),
            cache_read_tokens: Some(3),
            request_id: None,
            endpoint: None,
            tool_calls: vec![],
            reasoning_content: None,
        };
        let call = build_call(
            &builder,
            &[AnalysisResult::Http(http), AnalysisResult::Token(token)],
        )
        .unwrap();
        assert_eq!(call.model, "token-model");
        let tu = call.token_usage.unwrap();
        assert_eq!(tu.input_tokens, 10);
        assert_eq!(tu.output_tokens, 20);
        assert_eq!(tu.total_tokens, 30);
        assert_eq!(tu.cache_creation_input_tokens, Some(5));
        assert_eq!(tu.cache_read_input_tokens, Some(3));
    }

    #[test]
    fn test_build_request_openai_with_data() {
        let builder = GenAIBuilder::new();
        let openai_req = OpenAIRequest {
            model: "gpt-4".to_string(),
            messages: vec![empty_chat_msg(MessageRole::User, Some("hi"))],
            temperature: Some(0.7),
            max_tokens: Some(100),
            stream: Some(true),
            top_p: Some(0.9),
            n: None,
            stop: Some(vec!["END".to_string()]),
            presence_penalty: Some(0.1),
            frequency_penalty: Some(0.2),
            user: None,
            tools: Some(vec![serde_json::json!({"name": "t"})]),
            tool_choice: None,
            response_format: None,
            seed: Some(42),
            logprobs: None,
            top_logprobs: None,
            parallel_tool_calls: None,
        };
        let parsed = ParsedApiMessage::OpenAICompletion {
            request: Some(openai_req),
            response: None,
        };
        let http = make_http("/v1/chat/completions", None, None);
        let call = build_call(
            &builder,
            &[AnalysisResult::Http(http), AnalysisResult::Message(parsed)],
        )
        .unwrap();
        assert_eq!(call.request.temperature, Some(0.7));
        assert_eq!(call.request.max_tokens, Some(100));
        assert!(call.request.stream);
        assert_eq!(call.request.seed, Some(42));
        assert_eq!(call.request.frequency_penalty, Some(0.2));
        assert_eq!(call.request.presence_penalty, Some(0.1));
        assert!(call.request.tools.is_some());
        assert_eq!(
            call.request.stop_sequences.as_deref().unwrap(),
            &["END".to_string()]
        );
    }

    #[test]
    fn test_build_request_anthropic_with_data() {
        let builder = GenAIBuilder::new();
        let anth_req = AnthropicRequest {
            model: "claude-3".to_string(),
            messages: vec![AnthMsg {
                role: MessageRole::User,
                content: AnthropicMessageContent::Text("Hi".to_string()),
            }],
            max_tokens: 200,
            system: None,
            stream: Some(false),
            temperature: Some(0.5),
            top_p: Some(0.8),
            top_k: Some(50),
            stop_sequences: Some(vec!["STOP".to_string()]),
            metadata: None,
            tools: Some(vec![serde_json::json!({"name": "t"})]),
            tool_choice: None,
        };
        let parsed = ParsedApiMessage::AnthropicMessage {
            request: Some(anth_req),
            response: None,
        };
        let http = make_http("/v1/messages", None, None);
        let call = build_call(
            &builder,
            &[AnalysisResult::Http(http), AnalysisResult::Message(parsed)],
        )
        .unwrap();
        assert_eq!(call.provider, "anthropic");
        assert_eq!(call.request.temperature, Some(0.5));
        assert_eq!(call.request.max_tokens, Some(200));
        assert_eq!(call.request.top_k, Some(50.0));
        assert_eq!(
            call.request.stop_sequences.as_deref().unwrap(),
            &["STOP".to_string()]
        );
        assert!(!call.request.stream);
        assert!(call.request.tools.is_some());
    }

    #[test]
    fn test_build_request_sysom_full() {
        let builder = GenAIBuilder::new();
        let sysom_req = SysomRequest {
            params: SysomLlmParams {
                model: "qwen".to_string(),
                messages: vec![
                    SysMsg {
                        role: "system".to_string(),
                        content: "instr".to_string(),
                        tool_call_id: None,
                        name: None,
                        tool_calls: None,
                    },
                    SysMsg {
                        role: "user".to_string(),
                        content: "hello".to_string(),
                        tool_call_id: None,
                        name: None,
                        tool_calls: None,
                    },
                    SysMsg {
                        role: "assistant".to_string(),
                        content: String::new(),
                        tool_call_id: None,
                        name: None,
                        tool_calls: Some(vec![SysomToolCall {
                            id: "call_1".to_string(),
                            call_type: "function".to_string(),
                            function: SysomFunction {
                                name: "search".to_string(),
                                arguments: r#"{"q":"rust"}"#.to_string(),
                            },
                        }]),
                    },
                    SysMsg {
                        role: "tool".to_string(),
                        content: r#"{"result":"ok"}"#.to_string(),
                        tool_call_id: Some("call_1".to_string()),
                        name: Some("search".to_string()),
                        tool_calls: None,
                    },
                    SysMsg {
                        role: "tool".to_string(),
                        content: "plaintext result".to_string(),
                        tool_call_id: Some("call_2".to_string()),
                        name: None,
                        tool_calls: None,
                    },
                ],
                stream: true,
                temperature: Some(0.3),
                max_tokens: Some(500),
                top_p: Some(0.9),
                tools: Some(vec![serde_json::json!({"name": "t"})]),
                use_dashscope: None,
            },
        };
        let parsed = ParsedApiMessage::SysomMessage {
            request: Some(sysom_req),
            response: None,
        };
        let http = make_http("/api/v1/copilot/generate_copilot", None, None);
        let call = build_call(
            &builder,
            &[AnalysisResult::Http(http), AnalysisResult::Message(parsed)],
        )
        .unwrap();
        assert_eq!(call.provider, "sysom");
        assert_eq!(call.model, "qwen");
        assert_eq!(call.request.messages.len(), 5);
        assert_eq!(call.request.temperature, Some(0.3));
        assert_eq!(call.request.max_tokens, Some(500));
        assert!(call.request.stream);
        let assistant = call
            .request
            .messages
            .iter()
            .find(|m| m.role == "assistant")
            .unwrap();
        assert!(
            assistant
                .parts
                .iter()
                .any(|p| matches!(p, MessagePart::ToolCall { name, .. } if name == "search"))
        );
        let tool_msgs: Vec<_> = call
            .request
            .messages
            .iter()
            .filter(|m| m.role == "tool")
            .collect();
        assert_eq!(tool_msgs.len(), 2);
        assert!(
            tool_msgs[0]
                .parts
                .iter()
                .any(|p| matches!(p, MessagePart::ToolCallResponse { .. }))
        );
    }

    #[test]
    fn test_build_response_anthropic_all_blocks() {
        let builder = GenAIBuilder::new();
        let anth_resp = AnthropicResponse {
            id: "msg_123".to_string(),
            type_: "message".to_string(),
            role: MessageRole::Assistant,
            content: vec![
                AnthropicContentBlock::Thinking {
                    thinking: "let me think".to_string(),
                    signature: None,
                },
                AnthropicContentBlock::Text {
                    text: "answer".to_string(),
                    cache_control: None,
                },
                AnthropicContentBlock::ToolUse {
                    id: "tu_1".to_string(),
                    name: "calc".to_string(),
                    input: serde_json::json!({"a": 1}),
                },
                AnthropicContentBlock::ToolResult {
                    tool_use_id: "tu_0".to_string(),
                    content: Some(serde_json::json!("ok")),
                    is_error: None,
                },
                AnthropicContentBlock::Text {
                    text: String::new(),
                    cache_control: None,
                },
                AnthropicContentBlock::Thinking {
                    thinking: String::new(),
                    signature: None,
                },
            ],
            model: "claude-3".to_string(),
            stop_reason: Some("end_turn".to_string()),
            stop_sequence: None,
            usage: AnthropicUsage {
                input_tokens: 10,
                output_tokens: 20,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            },
        };
        let parsed = ParsedApiMessage::AnthropicMessage {
            request: None,
            response: Some(anth_resp),
        };
        let http = make_http("/v1/messages", None, None);
        let call = build_call(
            &builder,
            &[AnalysisResult::Http(http), AnalysisResult::Message(parsed)],
        )
        .unwrap();
        assert_eq!(call.response.messages.len(), 1);
        let msg = &call.response.messages[0];
        assert_eq!(msg.role, "assistant");
        assert!(
            msg.parts.iter().any(
                |p| matches!(p, MessagePart::Reasoning { content } if content == "let me think")
            )
        );
        assert!(
            msg.parts
                .iter()
                .any(|p| matches!(p, MessagePart::Text { content } if content == "answer"))
        );
        assert!(
            msg.parts
                .iter()
                .any(|p| matches!(p, MessagePart::ToolCall { name, .. } if name == "calc"))
        );
        assert!(
            msg.parts
                .iter()
                .any(|p| matches!(p, MessagePart::ToolCallResponse { .. }))
        );
        assert_eq!(msg.finish_reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn test_build_response_openai_with_choice() {
        let builder = GenAIBuilder::new();
        let openai_resp = OpenAIResponse {
            id: "chatcmpl-1".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "gpt-4".to_string(),
            choices: vec![OpenAIChoice {
                index: 0,
                message: empty_chat_msg(MessageRole::Assistant, Some("answer")),
                finish_reason: Some("stop".to_string()),
                logprobs: None,
            }],
            usage: None,
            system_fingerprint: None,
        };
        let parsed = ParsedApiMessage::OpenAICompletion {
            request: None,
            response: Some(openai_resp),
        };
        let http = make_http("/v1/chat/completions", None, None);
        let call = build_call(
            &builder,
            &[AnalysisResult::Http(http), AnalysisResult::Message(parsed)],
        )
        .unwrap();
        assert_eq!(call.response.messages.len(), 1);
        let msg = &call.response.messages[0];
        assert!(
            msg.parts
                .iter()
                .any(|p| matches!(p, MessagePart::Text { content } if content == "answer"))
        );
        assert_eq!(msg.finish_reason.as_deref(), Some("stop"));
        assert_eq!(call.metadata.get("response_id").unwrap(), "chatcmpl-1");
    }

    #[test]
    fn test_build_response_sysom_with_content_and_tool_use() {
        let builder = GenAIBuilder::new();
        let sysom_resp = SysomResponse {
            id: Some("chatcmpl-xx".to_string()),
            choices: vec![SysomResponseChoice {
                message: SysomResponseMessage {
                    content: "answer".to_string(),
                    tool_use: Some(vec![SysomToolUseItem {
                        index: 0,
                        id: "tu_1".to_string(),
                        item_type: "function".to_string(),
                        function: SysomFunction {
                            name: "calc".to_string(),
                            arguments: r#"{"x":1}"#.to_string(),
                        },
                    }]),
                },
            }],
        };
        let parsed = ParsedApiMessage::SysomMessage {
            request: None,
            response: Some(sysom_resp),
        };
        let http = make_http("/api/v1/copilot/generate_copilot", None, None);
        let call = build_call(
            &builder,
            &[AnalysisResult::Http(http), AnalysisResult::Message(parsed)],
        )
        .unwrap();
        let msg = &call.response.messages[0];
        assert!(
            msg.parts
                .iter()
                .any(|p| matches!(p, MessagePart::Text { content } if content == "answer"))
        );
        assert!(
            msg.parts
                .iter()
                .any(|p| matches!(p, MessagePart::ToolCall { name, .. } if name == "calc"))
        );
        assert_eq!(msg.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn test_build_response_sysom_empty_message() {
        let builder = GenAIBuilder::new();
        let sysom_resp = SysomResponse {
            id: None,
            choices: vec![SysomResponseChoice {
                message: SysomResponseMessage {
                    content: String::new(),
                    tool_use: None,
                },
            }],
        };
        let parsed = ParsedApiMessage::SysomMessage {
            request: None,
            response: Some(sysom_resp),
        };
        let http = make_http("/api/v1/copilot/generate_copilot", None, None);
        let call = build_call(
            &builder,
            &[AnalysisResult::Http(http), AnalysisResult::Message(parsed)],
        )
        .unwrap();
        assert!(call.response.messages.is_empty());
    }

    #[test]
    fn test_build_response_sse_no_parsed_message() {
        let builder = GenAIBuilder::new();
        let body = r#"{"model":"gpt-4","messages":[{"role":"user","content":"q"}]}"#;
        let sse_body = r#"[{"choices":[{"delta":{"content":"hi"},"finish_reason":"stop"}]}]"#;
        let mut http = make_http(
            "/v1/chat/completions",
            Some(body.to_string()),
            Some(sse_body.to_string()),
        );
        http.is_sse = true;
        http.sse_event_count = 1;
        let call = build_call(&builder, &[AnalysisResult::Http(http)]).unwrap();
        assert!(call.response.streamed);
        assert!(!call.response.messages.is_empty());
        let msg = &call.response.messages[0];
        assert!(
            msg.parts
                .iter()
                .any(|p| matches!(p, MessagePart::Text { content } if content == "hi"))
        );
    }

    #[test]
    fn test_build_response_sse_enrichment_with_reasoning() {
        let builder = GenAIBuilder::new();
        let openai_resp = OpenAIResponse {
            id: "chatcmpl-2".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "gpt-4".to_string(),
            choices: vec![OpenAIChoice {
                index: 0,
                message: empty_chat_msg(MessageRole::Assistant, Some("answer")),
                finish_reason: None,
                logprobs: None,
            }],
            usage: None,
            system_fingerprint: None,
        };
        let parsed = ParsedApiMessage::OpenAICompletion {
            request: None,
            response: Some(openai_resp),
        };
        let sse_body = r#"[{"choices":[{"delta":{"reasoning_content":"thinking"}}]},{"choices":[{"delta":{"content":"answer"},"finish_reason":"stop"}]}]"#;
        let mut http = make_http("/v1/chat/completions", None, Some(sse_body.to_string()));
        http.is_sse = true;
        let call = build_call(
            &builder,
            &[AnalysisResult::Http(http), AnalysisResult::Message(parsed)],
        )
        .unwrap();
        let msg = &call.response.messages[0];
        assert!(
            msg.parts
                .iter()
                .any(|p| matches!(p, MessagePart::Reasoning { content } if content == "thinking"))
        );
        assert_eq!(msg.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn test_build_response_sse_enrichment_with_tool_calls() {
        let builder = GenAIBuilder::new();
        // Parsed response has a tool_call without id; SSE body has tool_calls with id
        let mut assistant_msg = empty_chat_msg(MessageRole::Assistant, Some(""));
        assistant_msg.tool_calls = Some(vec![serde_json::json!({
            "function": {"name": "x", "arguments": "{}"}
        })]);
        let openai_resp = OpenAIResponse {
            id: "chatcmpl-3".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "gpt-4".to_string(),
            choices: vec![OpenAIChoice {
                index: 0,
                message: assistant_msg,
                finish_reason: None,
                logprobs: None,
            }],
            usage: None,
            system_fingerprint: None,
        };
        let parsed = ParsedApiMessage::OpenAICompletion {
            request: None,
            response: Some(openai_resp),
        };
        let sse_body = r#"[{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"tc_1","function":{"name":"search","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}]"#;
        let mut http = make_http("/v1/chat/completions", None, Some(sse_body.to_string()));
        http.is_sse = true;
        let call = build_call(
            &builder,
            &[AnalysisResult::Http(http), AnalysisResult::Message(parsed)],
        )
        .unwrap();
        let msg = &call.response.messages[0];
        assert!(msg.parts.iter().any(
            |p| matches!(p, MessagePart::ToolCall { id, .. } if id.as_deref() == Some("tc_1"))
        ));
    }

    #[test]
    fn test_build_request_fallback_empty_when_no_body() {
        let builder = GenAIBuilder::new();
        let mut http = make_http("/v1/chat/completions", None, None);
        http.is_sse = true;
        let call = build_call(&builder, &[AnalysisResult::Http(http)]).unwrap();
        assert!(call.request.messages.is_empty());
        assert!(!call.request.stream);
    }
}
