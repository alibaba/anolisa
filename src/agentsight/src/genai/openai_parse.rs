//! GenAI OpenAI body / SSE parsing & message conversion helpers
//!
//! Pure static helpers for parsing OpenAI request/response bodies and
//! converting between `OpenAIChatMessage` and the parts-based message
//! representation. Logic preserved verbatim from the original `builder.rs`; only
//! visibility was widened to `pub(super)` so siblings (`call_builder`)
//! can call these.

use super::GenAIBuilder;
use super::semantic::{InputMessage, LLMRequest, MessagePart, OutputMessage};
use crate::analyzer::message::types::OpenAIChatMessage;
use crate::analyzer::{HttpRecord, ParsedApiMessage};
use std::collections::HashMap;

impl GenAIBuilder {
    /// 从 HTTP request body 直接解析 LLMRequest（OpenAI/Anthropic 格式）
    pub(super) fn parse_request_body(body: &str) -> Option<LLMRequest> {
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

    /// Extract the LLM API response ID from parsed message or SSE body.
    ///
    /// Priority:
    /// 1. ParsedApiMessage response.id (OpenAI / Anthropic)
    /// 2. SSE response body first chunk "id" field
    /// 3. None (caller should fall back to call_id)
    pub(super) fn extract_response_id(
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

    /// Convert OpenAI ChatMessage to parts-based InputMessage
    pub(super) fn openai_msg_to_input(m: &OpenAIChatMessage) -> InputMessage {
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
    pub(super) fn openai_msg_to_output(
        m: &OpenAIChatMessage,
        finish_reason: Option<&str>,
    ) -> OutputMessage {
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
    pub(super) fn parse_openai_tool_call_value(tc: &serde_json::Value) -> Option<MessagePart> {
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
    pub(super) fn parse_sse_response_body(
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
    pub(super) fn extract_parts_from_sse_body(
        body: &str,
    ) -> Option<(Vec<MessagePart>, Option<String>)> {
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
}
