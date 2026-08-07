use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;

use super::profile::{self, ProviderProfile};
use super::{
    ContentGenerator, GenerateConfig, GenerateEvent, GenerateStream, Message, ToolDeclaration,
};

pub struct OpenAICompatProvider {
    pub base_url: String,
    pub api_key: String,
    cancelled: Arc<AtomicBool>,
    profile: Box<dyn ProviderProfile>,
    explicit_cache: bool,
}

impl OpenAICompatProvider {
    pub fn new(
        base_url: &str,
        api_key: &str,
        profile: Box<dyn ProviderProfile>,
        explicit_cache: bool,
    ) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            cancelled: Arc::new(AtomicBool::new(false)),
            profile,
            explicit_cache,
        }
    }

    pub fn new_generic(base_url: &str, api_key: &str) -> Self {
        // explicit_cache is false here because GenericProfile already returns
        // false from supports_cache_control(); the config flag is meaningless
        // for generic endpoints but kept for API symmetry.
        Self::new(base_url, api_key, Box::new(profile::GenericProfile), false)
    }

    fn cache_control_enabled(&self) -> bool {
        self.profile.supports_cache_control() && self.explicit_cache
    }

    /// Inject DashScope prompt-cache markers into the request body.
    ///
    /// Mirrors copilot-shell's `addDashScopeCacheControl` with the `all`
    /// strategy when streaming (system + last message) and `system_only`
    /// when not streaming. Although cosh-core currently hardcodes `stream: true`,
    /// the strategy is selected dynamically from the body so future non-stream
    /// support needs no cache-logic change.
    fn add_cache_control(&self, body: &mut Value) {
        if !self.cache_control_enabled() {
            return;
        }

        let is_stream = body.get("stream").and_then(|v| v.as_bool()).unwrap_or(true);
        let cache_all = is_stream;

        if let Some(messages) = body.get_mut("messages").and_then(|m| m.as_array_mut()) {
            // Track the system message index so we can skip re-marking it
            // when it is also the last message (single-message edge case).
            let mut system_index: Option<usize> = None;

            // System message: always cache.
            for (i, msg) in messages.iter_mut().enumerate() {
                if msg.get("role").and_then(|r| r.as_str()) == Some("system") {
                    add_cache_control_to_message_content(msg);
                    system_index = Some(i);
                    break;
                }
            }

            // Last message: cache only in `all` strategy.
            // Skip if the last message is the same as the system message
            // (already marked above).
            if cache_all {
                if let Some(last_idx) = messages.len().checked_sub(1) {
                    if system_index != Some(last_idx) {
                        if let Some(last_msg) = messages.get_mut(last_idx) {
                            add_cache_control_to_message_content(last_msg);
                        }
                    }
                }
            }
        }

        // Note: DashScope docs explicitly state that cache_control markers
        // can only be placed in messages[].content. Tool definitions
        // already participate in the system prefix cache calculation and
        // do not support independent cache markers.
        // Ref: https://help.aliyun.com/zh/model-studio/context-cache
    }

    fn build_request_body(
        &self,
        messages: &[Message],
        tools: &[ToolDeclaration],
        config: &GenerateConfig,
    ) -> Value {
        let max_tokens_field = self.profile.max_tokens_field();
        let max_tokens = config.max_tokens;
        let mut body = serde_json::json!({
            "model": config.model,
            "messages": messages,
            max_tokens_field: max_tokens,
            "stream": true,
        });

        if let Some(temp) = config.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        // Only request usage telemetry from backends that accept the field.
        // A generic OpenAI-compatible endpoint may reject unknown
        // `stream_options` and fail the entire turn, so usage stays an
        // enhancement layered on top of the always-present local estimate.
        if config.include_usage && self.profile.supports_stream_usage() {
            body["stream_options"] = serde_json::json!({"include_usage": true});
        }

        if !tools.is_empty() {
            let tool_defs: Vec<Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect();
            body["tools"] = serde_json::json!(tool_defs);
        }

        // extra_params is merged before cache markers are injected so that
        // user-supplied fields (e.g. custom `messages` or `tools`) are in
        // place when markers are applied. If extra_params contains a
        // `messages` key it would replace the constructed messages entirely
        // — that is existing behavior and not changed by cache support.
        if let Some(extra) = &config.extra_params {
            if let (Some(body_obj), Some(extra_obj)) = (body.as_object_mut(), extra.as_object()) {
                for (k, v) in extra_obj {
                    body_obj.insert(k.clone(), v.clone());
                }
            }
        }

        self.profile.adjust_request(&mut body);

        self.add_cache_control(&mut body);

        // Last word on the output cap: `extra_params` above may have replaced
        // the resolved `max_tokens` (or introduced the `max_completion_tokens`
        // alias), which would let the wire request outspend the output reserve
        // the compaction budget priced this request against (#2240).
        super::clamp_output_cap_fields(&mut body, config.max_tokens);

        body
    }
}

#[async_trait]
impl ContentGenerator for OpenAICompatProvider {
    async fn generate(
        &self,
        messages: &[Message],
        tools: &[ToolDeclaration],
        config: &GenerateConfig,
    ) -> Result<GenerateStream, String> {
        self.cancelled.store(false, Ordering::SeqCst);
        let body = self.build_request_body(messages, tools, config);
        let url = format!("{}/chat/completions", self.base_url);

        let client = reqwest::Client::new();
        let auth_value = self.profile.auth_header_value(&self.api_key);
        let mut request = client
            .post(&url)
            .header("Authorization", auth_value)
            .header("Content-Type", "application/json");

        if self.cache_control_enabled() {
            request = request.header("X-DashScope-CacheControl", "enable");
        }

        let response = request
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown".to_string());
            return Err(format!("API error {status}: {text}"));
        }

        let cancelled = Arc::clone(&self.cancelled);
        let thinking_field: Option<String> = self.profile.thinking_field().map(|s| s.to_string());
        // With usage reporting enabled, OpenAI-compatible backends deliver the
        // usage payload in a chunk after finish_reason. Defer MessageEnd to
        // [DONE]/stream end so consumers never break before seeing Usage. Only
        // defer when usage was actually requested (a supporting profile);
        // otherwise no usage chunk is coming and MessageEnd must not wait.
        let defer_message_end = config.include_usage && self.profile.supports_stream_usage();
        let byte_stream = response.bytes_stream();
        let buffer = String::new();
        let event_queue: Vec<GenerateEvent> = Vec::new();
        let stream_state = OpenAICompatStreamState::default();

        let event_stream = futures::stream::unfold(
            (
                byte_stream,
                buffer,
                cancelled,
                event_queue,
                thinking_field,
                stream_state,
            ),
            move |(
                mut stream,
                mut buf,
                cancelled,
                mut pending,
                thinking_field,
                mut stream_state,
            )| async move {
                let tf = thinking_field.as_deref();
                loop {
                    if let Some(event) = pending.pop() {
                        return Some((
                            event,
                            (
                                stream,
                                buf,
                                cancelled,
                                pending,
                                thinking_field,
                                stream_state,
                            ),
                        ));
                    }

                    if cancelled.load(Ordering::SeqCst) {
                        return Some((
                            GenerateEvent::Cancelled,
                            (
                                stream,
                                buf,
                                cancelled,
                                pending,
                                thinking_field,
                                stream_state,
                            ),
                        ));
                    }

                    if let Some(line_end) = buf.find('\n') {
                        let line = buf[..line_end].to_string();
                        buf = buf[line_end + 1..].to_string();

                        let line = line.trim();
                        if line.is_empty() || line.starts_with(':') {
                            continue;
                        }
                        if let Some(data) = line.strip_prefix("data: ") {
                            if data.trim() == "[DONE]" {
                                return Some((
                                    GenerateEvent::MessageEnd,
                                    (
                                        stream,
                                        buf,
                                        cancelled,
                                        pending,
                                        thinking_field,
                                        stream_state,
                                    ),
                                ));
                            }
                            if let Ok(chunk) = serde_json::from_str::<Value>(data) {
                                if let Some(mut events) = parse_sse_chunk_with_state(
                                    &chunk,
                                    tf,
                                    defer_message_end,
                                    &mut stream_state,
                                ) {
                                    if !events.is_empty() {
                                        let first = events.remove(0);
                                        events.reverse();
                                        pending = events;
                                        return Some((
                                            first,
                                            (
                                                stream,
                                                buf,
                                                cancelled,
                                                pending,
                                                thinking_field,
                                                stream_state,
                                            ),
                                        ));
                                    }
                                }
                            }
                        }
                        continue;
                    }

                    match stream.next().await {
                        Some(Ok(bytes)) => {
                            buf.push_str(&String::from_utf8_lossy(&bytes));
                        }
                        Some(Err(e)) => {
                            return Some((
                                GenerateEvent::Error(format!("stream error: {e}")),
                                (
                                    stream,
                                    buf,
                                    cancelled,
                                    pending,
                                    thinking_field,
                                    stream_state,
                                ),
                            ));
                        }
                        None => {
                            if !buf.trim().is_empty() {
                                let line = buf.trim().to_string();
                                buf.clear();
                                if let Some(data) = line.strip_prefix("data: ") {
                                    if data.trim() != "[DONE]" {
                                        if let Ok(chunk) = serde_json::from_str::<Value>(data) {
                                            if let Some(mut events) = parse_sse_chunk_with_state(
                                                &chunk,
                                                tf,
                                                defer_message_end,
                                                &mut stream_state,
                                            ) {
                                                if !events.is_empty() {
                                                    let first = events.remove(0);
                                                    events.reverse();
                                                    pending = events;
                                                    return Some((
                                                        first,
                                                        (
                                                            stream,
                                                            buf,
                                                            cancelled,
                                                            pending,
                                                            thinking_field,
                                                            stream_state,
                                                        ),
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            return Some((
                                GenerateEvent::MessageEnd,
                                (
                                    stream,
                                    buf,
                                    cancelled,
                                    pending,
                                    thinking_field,
                                    stream_state,
                                ),
                            ));
                        }
                    }
                }
            },
        );

        Ok(Box::pin(event_stream))
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

/// Convert a message's string content to an array format and append a
/// `cache_control: { type: "ephemeral" }` marker to the last content part.
///
/// DashScope requires `cache_control` on individual content parts within a
/// message, not on the message itself. When the content is a plain string it
/// must first be wrapped in `[{ "type": "text", "text": "..." }]` so the
/// marker has somewhere to live.
fn add_cache_control_to_message_content(msg: &mut Value) {
    let Some(content) = msg.get_mut("content") else {
        return;
    };

    // Null content (e.g. assistant tool-call messages) has nothing to cache.
    if content.is_null() {
        return;
    }

    // If content is a string, convert to array format.
    if content.is_string() {
        let text = content.as_str().unwrap_or("").to_string();
        *content = serde_json::json!([{
            "type": "text",
            "text": text
        }]);
    }

    // Append cache_control to the last content part.
    if let Some(parts) = content.as_array_mut() {
        if let Some(last_part) = parts.last_mut() {
            // Wrap raw strings in a text object so the marker has a home.
            // Numbers, booleans and null are not cacheable content — skip.
            if last_part.is_string() {
                let text = last_part.as_str().unwrap_or("").to_string();
                *last_part = serde_json::json!({
                    "type": "text",
                    "text": text
                });
            }
            if let Some(obj) = last_part.as_object_mut() {
                obj.insert(
                    "cache_control".to_string(),
                    serde_json::json!({"type": "ephemeral"}),
                );
            }
        }
    }
}

#[derive(Default)]
struct OpenAICompatStreamState {
    argument_deltas_seen: HashSet<u32>,
    started_tool_calls: HashSet<u32>,
    emitted_text: HashMap<u32, String>,
}

#[cfg(test)]
fn parse_sse_chunk(
    chunk: &Value,
    thinking_field: Option<&str>,
    defer_message_end: bool,
) -> Option<Vec<GenerateEvent>> {
    parse_sse_chunk_with_state(
        chunk,
        thinking_field,
        defer_message_end,
        &mut OpenAICompatStreamState::default(),
    )
}

fn parse_sse_chunk_with_state(
    chunk: &Value,
    thinking_field: Option<&str>,
    defer_message_end: bool,
    stream_state: &mut OpenAICompatStreamState,
) -> Option<Vec<GenerateEvent>> {
    let mut events = Vec::new();

    if let Some(choices) = chunk.get("choices").and_then(|c| c.as_array()) {
        for choice in choices {
            let choice_index = choice.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
            let empty_delta = Value::Null;
            let delta = choice.get("delta").unwrap_or(&empty_delta);
            if let Some(field) = thinking_field {
                if let Some(text) = delta.get(field).and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        events.push(GenerateEvent::ThinkingDelta(text.to_string()));
                    }
                }
            }

            if let Some(content) = delta
                .get("content")
                .and_then(|c| c.as_str())
                .filter(|content| !content.is_empty())
            {
                stream_state
                    .emitted_text
                    .entry(choice_index)
                    .or_default()
                    .push_str(content);
                events.push(GenerateEvent::TextDelta(content.to_string()));
            } else if let Some(snapshot) = choice
                .get("message")
                .and_then(|message| message.get("content"))
                .and_then(|content| content.as_str())
                .filter(|content| !content.is_empty())
            {
                let emitted = stream_state.emitted_text.entry(choice_index).or_default();
                let suffix = snapshot
                    .strip_prefix(emitted.as_str())
                    .or_else(|| emitted.is_empty().then_some(snapshot));
                if let Some(suffix) = suffix.filter(|suffix| !suffix.is_empty()) {
                    emitted.push_str(suffix);
                    events.push(GenerateEvent::TextDelta(suffix.to_string()));
                }
            }

            if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                let final_tool_calls = choice
                    .get("message")
                    .and_then(|message| message.get("tool_calls"))
                    .and_then(|calls| calls.as_array());
                for tc in tool_calls {
                    let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
                    let final_call = final_tool_calls.and_then(|calls| {
                        calls.iter().find(|call| {
                            call.get("index").and_then(|i| i.as_u64()).unwrap_or(0)
                                == u64::from(index)
                        })
                    });

                    let delta_function = tc.get("function");
                    let final_function = final_call.and_then(|call| call.get("function"));
                    if let Some(name) = delta_function
                        .and_then(|function| function.get("name"))
                        .and_then(|name| name.as_str())
                        .filter(|name| !name.is_empty())
                        .or_else(|| {
                            final_function
                                .and_then(|function| function.get("name"))
                                .and_then(|name| name.as_str())
                                .filter(|name| !name.is_empty())
                        })
                    {
                        let id = tc
                            .get("id")
                            .and_then(|i| i.as_str())
                            .or_else(|| {
                                final_call
                                    .and_then(|call| call.get("id"))
                                    .and_then(|id| id.as_str())
                            })
                            .unwrap_or("")
                            .to_string();
                        if stream_state.started_tool_calls.insert(index) {
                            events.push(GenerateEvent::ToolCallStart {
                                index,
                                id,
                                name: name.to_string(),
                            });
                        }
                    }

                    let delta_arguments = delta_function
                        .and_then(|function| function.get("arguments"))
                        .and_then(|arguments| arguments.as_str());
                    if let Some(args) = delta_arguments.filter(|args| !args.is_empty()) {
                        stream_state.argument_deltas_seen.insert(index);
                        events.push(GenerateEvent::ToolCallDelta {
                            index,
                            arguments_delta: args.to_string(),
                        });
                    } else if !stream_state.argument_deltas_seen.contains(&index) {
                        if let Some(args) = final_function
                            .and_then(|function| function.get("arguments"))
                            .and_then(|arguments| arguments.as_str())
                            .filter(|args| !args.is_empty())
                        {
                            stream_state.argument_deltas_seen.insert(index);
                            events.push(GenerateEvent::ToolCallDelta {
                                index,
                                arguments_delta: args.to_string(),
                            });
                        }
                    }
                }
            } else if let Some(tool_calls) = choice
                .get("message")
                .and_then(|message| message.get("tool_calls"))
                .and_then(|calls| calls.as_array())
            {
                for tc in tool_calls {
                    let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
                    let Some(function) = tc.get("function") else {
                        continue;
                    };
                    let name = function
                        .get("name")
                        .and_then(|name| name.as_str())
                        .filter(|name| !name.is_empty());
                    if let Some(name) = name {
                        let id = tc
                            .get("id")
                            .and_then(|id| id.as_str())
                            .unwrap_or("")
                            .to_string();
                        if stream_state.started_tool_calls.insert(index) {
                            events.push(GenerateEvent::ToolCallStart {
                                index,
                                id,
                                name: name.to_string(),
                            });
                        }
                    }
                    if !stream_state.argument_deltas_seen.contains(&index) {
                        if let Some(arguments) = function
                            .get("arguments")
                            .and_then(|args| args.as_str())
                            .filter(|arguments| !arguments.is_empty())
                        {
                            stream_state.argument_deltas_seen.insert(index);
                            events.push(GenerateEvent::ToolCallDelta {
                                index,
                                arguments_delta: arguments.to_string(),
                            });
                        }
                    }
                }
            }

            if let Some(finish) = choice.get("finish_reason").and_then(|f| f.as_str()) {
                if (finish == "stop" || finish == "tool_calls") && !defer_message_end {
                    events.push(GenerateEvent::MessageEnd);
                }
            }
        }
    }

    if let Some(usage) = chunk.get("usage").and_then(|u| u.as_object()) {
        let prompt = usage
            .get("prompt_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let completion = usage
            .get("completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let total = usage
            .get("total_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let cached = super::extract_cached_tokens(usage);
        let usage_event = GenerateEvent::Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: total,
            cached_tokens: cached,
        };
        // Usage must precede MessageEnd; consumers stop reading at the end
        // marker and would otherwise drop the usage payload.
        match events
            .iter()
            .position(|event| matches!(event, GenerateEvent::MessageEnd))
        {
            Some(index) => events.insert(index, usage_event),
            None => events.push(usage_event),
        }
    }

    if events.is_empty() {
        None
    } else {
        Some(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_text_delta_chunk() {
        let chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {"content": "Hello"},
                "finish_reason": null
            }]
        });
        let events = parse_sse_chunk(&chunk, None, false).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], GenerateEvent::TextDelta(t) if t == "Hello"));
    }

    #[test]
    fn parse_final_message_text_without_delta() {
        let chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {},
                "message": {"content": "Repository analysis complete."},
                "finish_reason": "stop"
            }]
        });

        let events = parse_sse_chunk(&chunk, None, false).unwrap();
        assert!(matches!(
            &events[..],
            [GenerateEvent::TextDelta(text), GenerateEvent::MessageEnd]
                if text == "Repository analysis complete."
        ));
    }

    #[test]
    fn parse_tool_call_chunk() {
        let chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {
                            "name": "shell",
                            "arguments": ""
                        }
                    }]
                },
                "finish_reason": null
            }]
        });
        let events = parse_sse_chunk(&chunk, None, false).unwrap();
        assert!(!events.is_empty());
        assert!(
            matches!(&events[0], GenerateEvent::ToolCallStart { name, id, .. } if name == "shell" && id == "call_1")
        );
    }

    #[test]
    fn parse_tool_call_arguments_delta() {
        let chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": {
                            "arguments": "{\"command\":"
                        }
                    }]
                },
                "finish_reason": null
            }]
        });
        let events = parse_sse_chunk(&chunk, None, false).unwrap();
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], GenerateEvent::ToolCallDelta { arguments_delta, .. } if arguments_delta == "{\"command\":")
        );
    }

    #[test]
    fn parse_final_message_tool_call_when_delta_has_only_arguments() {
        let chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": {"arguments": "{\"command\":\"pwd\"}"}
                    }]
                },
                "message": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {
                            "name": "shell",
                            "arguments": "{\"command\":\"pwd\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });

        let events = parse_sse_chunk(&chunk, None, false).expect("tool call events");
        assert!(matches!(
            &events[0],
            GenerateEvent::ToolCallStart { id, name, .. } if id == "call_1" && name == "shell"
        ));
        assert!(matches!(
            &events[1],
            GenerateEvent::ToolCallDelta { arguments_delta, .. } if arguments_delta == "{\"command\":\"pwd\"}"
        ));
        assert!(matches!(&events[2], GenerateEvent::MessageEnd));
    }

    #[test]
    fn final_tool_call_snapshot_does_not_repeat_streamed_arguments() {
        let first_chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {
                            "name": "shell",
                            "arguments": "{\"command\":\"pwd\"}"
                        }
                    }]
                },
                "finish_reason": null
            }]
        });
        let final_chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {"tool_calls": [{"index": 0, "function": {}}]},
                "message": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {
                            "name": "shell",
                            "arguments": "{\"command\":\"pwd\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let mut state = OpenAICompatStreamState::default();

        let first = parse_sse_chunk_with_state(&first_chunk, None, false, &mut state)
            .expect("first streamed tool-call chunk");
        assert!(matches!(
            &first[..],
            [GenerateEvent::ToolCallStart { .. }, GenerateEvent::ToolCallDelta { arguments_delta, .. }]
                if arguments_delta == "{\"command\":\"pwd\"}"
        ));

        let final_events = parse_sse_chunk_with_state(&final_chunk, None, false, &mut state)
            .expect("final tool-call snapshot");
        assert!(
            !final_events
                .iter()
                .any(|event| matches!(event, GenerateEvent::ToolCallDelta { .. })),
            "final tool-call snapshots must not repeat streamed arguments"
        );
        assert!(
            !final_events
                .iter()
                .any(|event| matches!(event, GenerateEvent::ToolCallStart { .. })),
            "final tool-call snapshots must not reopen an existing tool block"
        );
        assert!(matches!(
            final_events.last(),
            Some(GenerateEvent::MessageEnd)
        ));
    }

    #[test]
    fn final_message_only_tool_call_does_not_repeat_streamed_arguments() {
        let first_chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {
                            "name": "shell",
                            "arguments": "{\"command\":\"pwd\"}"
                        }
                    }]
                },
                "finish_reason": null
            }]
        });
        let final_chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "message": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {
                            "name": "shell",
                            "arguments": "{\"command\":\"pwd\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let mut state = OpenAICompatStreamState::default();

        let _ = parse_sse_chunk_with_state(&first_chunk, None, false, &mut state)
            .expect("first streamed tool-call chunk");
        let final_events = parse_sse_chunk_with_state(&final_chunk, None, false, &mut state)
            .expect("final message-only tool-call snapshot");

        assert!(
            !final_events
                .iter()
                .any(|event| matches!(event, GenerateEvent::ToolCallDelta { .. })),
            "message-only snapshots must not repeat streamed arguments"
        );
        assert!(
            !final_events
                .iter()
                .any(|event| matches!(event, GenerateEvent::ToolCallStart { .. })),
            "message-only snapshots must not reopen an existing tool block"
        );
        assert!(matches!(
            final_events.last(),
            Some(GenerateEvent::MessageEnd)
        ));
    }

    #[test]
    fn parse_tool_call_from_final_message_without_delta() {
        let chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "message": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {
                            "name": "shell",
                            "arguments": "{\"command\":\"pwd\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });

        let events = parse_sse_chunk(&chunk, None, false).expect("tool call events");
        assert!(matches!(
            &events[0],
            GenerateEvent::ToolCallStart { id, name, .. } if id == "call_1" && name == "shell"
        ));
        assert!(matches!(
            &events[1],
            GenerateEvent::ToolCallDelta { arguments_delta, .. } if arguments_delta == "{\"command\":\"pwd\"}"
        ));
        assert!(matches!(&events[2], GenerateEvent::MessageEnd));
    }

    #[test]
    fn final_message_text_snapshot_only_emits_the_unseen_suffix() {
        let first_chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {"content": "Do"},
                "finish_reason": null
            }]
        });
        let final_chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "message": {"content": "Done"},
                "finish_reason": "stop"
            }]
        });
        let mut state = OpenAICompatStreamState::default();

        let first = parse_sse_chunk_with_state(&first_chunk, None, false, &mut state)
            .expect("first text chunk");
        assert!(matches!(
            &first[..],
            [GenerateEvent::TextDelta(text)] if text == "Do"
        ));

        let final_events = parse_sse_chunk_with_state(&final_chunk, None, false, &mut state)
            .expect("final text snapshot");
        assert!(matches!(
            &final_events[..],
            [GenerateEvent::TextDelta(text), GenerateEvent::MessageEnd] if text == "ne"
        ));
    }

    #[test]
    fn message_only_unnamed_tool_call_forwards_arguments_for_rejection() {
        let chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "message": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {"arguments": "{\"command\":\"pwd\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });

        let events = parse_sse_chunk(&chunk, None, false).expect("tool call events");
        assert!(matches!(
            &events[..],
            [GenerateEvent::ToolCallDelta { arguments_delta, .. }, GenerateEvent::MessageEnd]
                if arguments_delta == "{\"command\":\"pwd\"}"
        ));
    }

    #[test]
    fn parse_finish_reason_stop() {
        let chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "stop"
            }]
        });
        let events = parse_sse_chunk(&chunk, None, false).unwrap();
        assert!(matches!(&events[0], GenerateEvent::MessageEnd));
    }

    #[test]
    fn parse_reasoning_content_chunk() {
        let chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "reasoning_content": "Let me think step by step...",
                    "content": ""
                },
                "finish_reason": null
            }]
        });
        let events = parse_sse_chunk(&chunk, Some("reasoning_content"), false).unwrap();
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], GenerateEvent::ThinkingDelta(t) if t == "Let me think step by step...")
        );
    }

    #[test]
    fn parse_reasoning_content_without_field_configured() {
        let chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "reasoning_content": "thinking...",
                    "content": "visible"
                },
                "finish_reason": null
            }]
        });
        let events = parse_sse_chunk(&chunk, None, false).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], GenerateEvent::TextDelta(t) if t == "visible"));
    }

    #[test]
    fn parse_usage_chunk() {
        let chunk = serde_json::json!({
            "choices": [],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 50,
                "total_tokens": 150
            }
        });
        let events = parse_sse_chunk(&chunk, None, false).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            GenerateEvent::Usage {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
                cached_tokens: 0
            }
        ));
    }

    #[test]
    fn usage_precedes_message_end_in_combined_chunk() {
        let chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        });
        let events = parse_sse_chunk(&chunk, None, false).unwrap();
        assert!(matches!(&events[0], GenerateEvent::Usage { .. }));
        assert!(matches!(&events[1], GenerateEvent::MessageEnd));
    }

    #[test]
    fn finish_reason_defers_message_end_when_usage_enabled() {
        let chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "stop"
            }]
        });
        // With include_usage the usage chunk arrives after finish_reason;
        // MessageEnd must wait for [DONE] so usage is never dropped.
        assert!(parse_sse_chunk(&chunk, None, true).is_none());
        let usage_chunk = serde_json::json!({
            "choices": [],
            "usage": {"prompt_tokens": 7, "completion_tokens": 3, "total_tokens": 10}
        });
        let events = parse_sse_chunk(&usage_chunk, None, true).unwrap();
        assert!(matches!(&events[0], GenerateEvent::Usage { .. }));
    }

    #[test]
    fn build_request_with_max_completion_tokens() {
        let provider = OpenAICompatProvider::new(
            "https://api.openai.com/v1",
            "sk-test",
            Box::new(super::super::profile::OpenAIProfile),
            false,
        );
        let config = GenerateConfig {
            model: "o3".to_string(),
            max_tokens: 8192,
            ..Default::default()
        };
        let body = provider.build_request_body(&[], &[], &config);
        assert!(body.get("max_completion_tokens").is_some());
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn build_request_respects_caller_max_tokens_for_recognized_model() {
        // Regression: provider must not override the caller's max_tokens
        // even when the model is recognized.  The compaction summarizer sets
        // 2048 for qwen3.7-max; the provider must send 2048, not 65536.
        let provider = OpenAICompatProvider::new_generic("https://example.com/v1", "sk-test");
        let config = GenerateConfig {
            model: "qwen3.7-max".to_string(),
            max_tokens: 2_048,
            ..Default::default()
        };
        let body = provider.build_request_body(&[], &[], &config);
        assert_eq!(body["max_tokens"], 2_048);
    }

    #[test]
    fn build_request_uses_config_max_tokens_for_unknown_model() {
        let provider = OpenAICompatProvider::new_generic("https://example.com/v1", "sk-test");
        let config = GenerateConfig {
            model: "unknown-model".to_string(),
            max_tokens: 4096,
            ..Default::default()
        };
        let body = provider.build_request_body(&[], &[], &config);
        assert_eq!(body["max_tokens"], 4096);
    }

    #[test]
    fn build_request_with_extra_params() {
        let provider = OpenAICompatProvider::new_generic("https://example.com/v1", "sk-test");
        let config = GenerateConfig {
            model: "test".to_string(),
            max_tokens: 4096,
            extra_params: Some(serde_json::json!({
                "enable_thinking": true,
                "thinking_budget": 4096
            })),
            ..Default::default()
        };
        let body = provider.build_request_body(&[], &[], &config);
        assert_eq!(body["enable_thinking"], true);
        assert_eq!(body["thinking_budget"], 4096);
    }

    #[test]
    fn extra_params_cannot_raise_the_wire_output_cap() {
        // Regression (#2240): the compaction budget reserves `O` from the same
        // resolver that produced `max_tokens`, so the serialized request must
        // never be able to spend more than that reserve. Both cap aliases are
        // clamped because a backend may honor either one.
        let provider = OpenAICompatProvider::new_generic("https://example.com/v1", "sk-test");
        let config = GenerateConfig {
            model: "test".to_string(),
            max_tokens: 16_384,
            extra_params: Some(serde_json::json!({
                "max_tokens": 65_536u32,
                "max_completion_tokens": 65_536u32,
            })),
            ..Default::default()
        };

        let body = provider.build_request_body(&[], &[], &config);

        assert_eq!(body["max_tokens"], 16_384);
        assert_eq!(body["max_completion_tokens"], 16_384);
    }

    #[test]
    fn extra_params_cannot_raise_the_wire_cap_on_the_alias_field_profile() {
        // The OpenAI profile serializes `max_completion_tokens`; extra_params
        // must not raise it, nor sneak an unbounded `max_tokens` past the cap.
        let provider = OpenAICompatProvider::new(
            "https://api.openai.com/v1",
            "sk-test",
            Box::new(super::super::profile::OpenAIProfile),
            false,
        );
        let config = GenerateConfig {
            model: "o3".to_string(),
            max_tokens: 8_192,
            extra_params: Some(serde_json::json!({
                "max_completion_tokens": 65_536u32,
                "max_tokens": 65_536u32,
            })),
            ..Default::default()
        };

        let body = provider.build_request_body(&[], &[], &config);

        assert_eq!(body["max_completion_tokens"], 8_192);
        assert_eq!(body["max_tokens"], 8_192);
    }

    #[test]
    fn extra_params_may_still_lower_the_wire_output_cap() {
        // Asking for less than the reserve is always safe and is preserved.
        let provider = OpenAICompatProvider::new_generic("https://example.com/v1", "sk-test");
        let config = GenerateConfig {
            model: "test".to_string(),
            max_tokens: 16_384,
            extra_params: Some(serde_json::json!({"max_tokens": 512u32})),
            ..Default::default()
        };

        let body = provider.build_request_body(&[], &[], &config);

        assert_eq!(body["max_tokens"], 512);
        // The alias is not invented for a backend that never received it.
        assert!(body.get("max_completion_tokens").is_none(), "{body}");
    }

    #[test]
    fn non_numeric_extra_params_cap_is_replaced_by_the_reserve() {
        // A string or null cap could be coerced by a backend; fail closed to
        // the resolved reserve instead of forwarding it.
        let provider = OpenAICompatProvider::new_generic("https://example.com/v1", "sk-test");
        let config = GenerateConfig {
            model: "test".to_string(),
            max_tokens: 4_096,
            extra_params: Some(serde_json::json!({"max_tokens": "65536"})),
            ..Default::default()
        };

        let body = provider.build_request_body(&[], &[], &config);

        assert_eq!(body["max_tokens"], 4_096);
    }

    #[test]
    fn build_request_with_include_usage() {
        // A profile that supports usage telemetry receives stream_options.
        let provider = OpenAICompatProvider::new(
            "https://api.openai.com/v1",
            "sk-test",
            Box::new(super::super::profile::OpenAIProfile),
            false,
        );
        let config = GenerateConfig {
            model: "test".to_string(),
            max_tokens: 4096,
            include_usage: true,
            ..Default::default()
        };
        let body = provider.build_request_body(&[], &[], &config);
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn generic_profile_omits_stream_options_even_with_include_usage() {
        // Regression: the compaction feature enables include_usage on every
        // agent turn. A generic OpenAI-compatible endpoint must not be forced
        // to accept stream_options it may reject; usage degrades to the local
        // estimate instead of breaking the model conversation.
        let provider = OpenAICompatProvider::new_generic("https://example.com/v1", "sk-test");
        let config = GenerateConfig {
            model: "test".to_string(),
            max_tokens: 4096,
            include_usage: true,
            ..Default::default()
        };
        let body = provider.build_request_body(&[], &[], &config);
        assert!(
            body.get("stream_options").is_none(),
            "generic endpoints must not receive stream_options: {body}"
        );
    }

    #[test]
    fn build_request_preserves_user_provided_secrets() {
        let provider = OpenAICompatProvider::new_generic("https://example.com/v1", "sk-test");
        let secret = "short-provider-secret";
        let messages = vec![Message::user(&format!("api_key={secret}"))];

        let body = provider.build_request_body(&messages, &[], &GenerateConfig::default());
        let payload = body.to_string();

        assert!(payload.contains(secret), "{payload}");
        assert!(!payload.contains("<redacted>"), "{payload}");
    }

    #[test]
    fn parse_sse_extracts_cached_tokens_from_prompt_tokens_details() {
        let chunk = serde_json::json!({
            "choices": [],
            "usage": {
                "prompt_tokens": 1000,
                "completion_tokens": 50,
                "total_tokens": 1050,
                "prompt_tokens_details": {
                    "cached_tokens": 800
                }
            }
        });
        let events = parse_sse_chunk(&chunk, None, false).unwrap();
        assert!(matches!(
            &events[0],
            GenerateEvent::Usage {
                prompt_tokens: 1000,
                completion_tokens: 50,
                total_tokens: 1050,
                cached_tokens: 800
            }
        ));
    }

    #[test]
    fn parse_sse_extracts_cached_tokens_from_top_level() {
        let chunk = serde_json::json!({
            "choices": [],
            "usage": {
                "prompt_tokens": 1000,
                "completion_tokens": 50,
                "total_tokens": 1050,
                "cached_tokens": 600
            }
        });
        let events = parse_sse_chunk(&chunk, None, false).unwrap();
        assert!(matches!(
            &events[0],
            GenerateEvent::Usage {
                prompt_tokens: 1000,
                completion_tokens: 50,
                total_tokens: 1050,
                cached_tokens: 600
            }
        ));
    }

    #[test]
    fn parse_sse_prefers_prompt_tokens_details_over_top_level_cached_tokens() {
        let chunk = serde_json::json!({
            "choices": [],
            "usage": {
                "prompt_tokens": 1000,
                "completion_tokens": 50,
                "total_tokens": 1050,
                "prompt_tokens_details": {
                    "cached_tokens": 800
                },
                "cached_tokens": 600
            }
        });
        let events = parse_sse_chunk(&chunk, None, false).unwrap();
        assert!(matches!(
            &events[0],
            GenerateEvent::Usage {
                prompt_tokens: 1000,
                completion_tokens: 50,
                total_tokens: 1050,
                cached_tokens: 800
            }
        ));
    }

    #[test]
    fn parse_sse_defaults_cached_tokens_to_zero_when_absent() {
        let chunk = serde_json::json!({
            "choices": [],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 50,
                "total_tokens": 150
            }
        });
        let events = parse_sse_chunk(&chunk, None, false).unwrap();
        assert!(matches!(
            &events[0],
            GenerateEvent::Usage {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
                cached_tokens: 0
            }
        ));
    }

    // ── Cache control tests ──

    fn dashscope_provider(explicit: bool) -> OpenAICompatProvider {
        OpenAICompatProvider::new(
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "sk-test",
            Box::new(super::super::profile::DashScopeProfile),
            explicit,
        )
    }

    #[test]
    fn cache_control_adds_markers_to_system_and_last_message() {
        let provider = dashscope_provider(true);
        let messages = vec![
            Message::system("You are a helpful assistant."),
            Message::user("Hello"),
            Message::assistant("Hi there"),
            Message::user("How are you?"),
        ];
        let body = provider.build_request_body(&messages, &[], &GenerateConfig::default());

        let msgs = body["messages"].as_array().unwrap();
        // System message content should be array with cache_control on last part.
        let system_content = &msgs[0]["content"];
        assert!(system_content.is_array(), "system content should be array");
        let system_parts = system_content.as_array().unwrap();
        assert!(system_parts.last().unwrap().get("cache_control").is_some());
        // Last message content should also have cache_control.
        let last_content = msgs.last().unwrap()["content"].as_array().unwrap();
        assert!(last_content.last().unwrap().get("cache_control").is_some());
    }

    #[test]
    fn cache_control_disabled_skips_markers() {
        let provider = dashscope_provider(false);
        let messages = vec![
            Message::system("You are a helpful assistant."),
            Message::user("Hello"),
        ];
        let body = provider.build_request_body(&messages, &[], &GenerateConfig::default());
        assert!(
            !body.to_string().contains("cache_control"),
            "body should not contain cache_control when disabled"
        );
    }

    #[test]
    fn cache_control_string_content_converted_to_array() {
        let provider = dashscope_provider(true);
        let messages = vec![Message::system("System prompt text")];
        let body = provider.build_request_body(&messages, &[], &GenerateConfig::default());
        let system_content = &body["messages"][0]["content"];
        assert!(
            system_content.is_array(),
            "string content should be converted to array"
        );
        let parts = system_content.as_array().unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[0]["text"], "System prompt text");
        assert_eq!(parts[0]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn non_dashscope_profile_no_cache_control() {
        let provider = OpenAICompatProvider::new(
            "https://api.openai.com/v1",
            "sk-test",
            Box::new(super::super::profile::GenericProfile),
            false,
        );
        let messages = vec![Message::system("System"), Message::user("Hello")];
        let body = provider.build_request_body(&messages, &[], &GenerateConfig::default());
        assert!(
            !body.to_string().contains("cache_control"),
            "generic profile should not inject cache_control"
        );
    }

    #[test]
    fn non_dashscope_profile_no_cache_control_even_when_opted_in() {
        // User sets explicit_cache = true, but the provider profile is not
        // DashScope — the profile gate must still block marker injection.
        let provider = OpenAICompatProvider::new(
            "https://api.openai.com/v1",
            "sk-test",
            Box::new(super::super::profile::GenericProfile),
            true,
        );
        let messages = vec![Message::system("System"), Message::user("Hello")];
        let body = provider.build_request_body(&messages, &[], &GenerateConfig::default());
        assert!(
            !body.to_string().contains("cache_control"),
            "generic profile should not inject cache_control even with explicit_cache = true"
        );
    }

    #[test]
    fn cache_control_no_system_message_caches_last_only() {
        let provider = dashscope_provider(true);
        let messages = vec![
            Message::user("Hello"),
            Message::assistant("Hi there"),
            Message::user("How are you?"),
        ];
        let body = provider.build_request_body(&messages, &[], &GenerateConfig::default());

        let msgs = body["messages"].as_array().unwrap();
        // No system message, so only the last message should have cache_control.
        for (i, msg) in msgs.iter().enumerate() {
            let has_cache = msg["content"]
                .as_array()
                .and_then(|parts| parts.last())
                .and_then(|p| p.get("cache_control"))
                .is_some();
            if i == msgs.len() - 1 {
                assert!(has_cache, "last message should have cache_control");
            } else {
                assert!(!has_cache, "non-last message should not have cache_control");
            }
        }
    }

    #[test]
    fn cache_control_wraps_raw_string_content_part() {
        // Regression: a raw string as the last content part used to panic
        // because serde_json cannot index into Value::String with a string key.
        let mut msg = serde_json::json!({
            "role": "system",
            "content": ["valid text", "raw string"]
        });
        add_cache_control_to_message_content(&mut msg);

        let parts = msg["content"].as_array().unwrap();
        let last = parts.last().unwrap();
        assert_eq!(last["type"], "text");
        assert_eq!(last["text"], "raw string");
        assert_eq!(last["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn cache_control_skips_non_object_non_string_content_part() {
        // Numbers and booleans are not cacheable content; the function must
        // not panic and must simply skip the marker.
        let mut msg = serde_json::json!({
            "role": "system",
            "content": [{"type": "text", "text": "ok"}, 42]
        });
        add_cache_control_to_message_content(&mut msg);

        let parts = msg["content"].as_array().unwrap();
        let last = parts.last().unwrap();
        assert!(last.is_number(), "number part should remain untouched");
        assert!(last.get("cache_control").is_none());
    }
}
