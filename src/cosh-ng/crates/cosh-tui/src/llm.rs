//! LLM API client for cosh-tui chat capability.
//!
//! Supports OpenAI-compatible APIs (DashScope, DeepSeek, etc.) with
//! streaming (SSE) and OpenAI function-calling (tool_calls) semantics.
//! The agentic loop lives in `app.rs`; this module only handles
//! request/response framing.

use std::collections::BTreeMap;
use std::io::BufRead;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolSpec>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
}

/// OpenAI tool specification sent in a request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Chat message (user / assistant / system / tool).
///
/// `content` stays as `String` (empty is allowed — for tool-call assistant
/// messages whose content is conceptually null). `tool_calls`,
/// `tool_call_id`, and `name` are serialized only when present.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    /// An assistant message that issues tool calls. `content` may be empty.
    pub fn assistant_tool_calls(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
        }
    }

    /// A tool-result message fed back to the LLM.
    pub fn tool_result(tool_call_id: impl Into<String>, name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: Some(name.into()),
        }
    }
}

/// A concrete tool invocation parsed from the LLM response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_function_type")]
    pub tool_type: String,
    pub function: FunctionCall,
}

fn default_function_type() -> String {
    "function".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// Raw JSON string (OpenAI returns this as a serialized string, not object).
    pub arguments: String,
}

#[derive(Debug, Deserialize)]
pub struct ChatResponse {
    pub choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    pub message: MessageContent,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct MessageContent {
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCall>>,
}

// ---------------------------------------------------------------------------
// Streaming types
// ---------------------------------------------------------------------------

/// SSE streaming chunk (OpenAI format).
#[derive(Debug, Deserialize)]
pub struct StreamChunk {
    pub choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
pub struct StreamChoice {
    pub delta: StreamDelta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct StreamDelta {
    pub content: Option<String>,
    pub role: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCallDelta>>,
}

/// Incremental tool_call chunk inside an SSE delta. OpenAI streams these
/// in multiple pieces (id+name first, then argument fragments).
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct ToolCallDelta {
    pub index: Option<usize>,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub tool_type: Option<String>,
    pub function: Option<FunctionDelta>,
}

#[derive(Debug, Deserialize)]
pub struct FunctionDelta {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[derive(Default)]
struct ToolCallBuilder {
    id: String,
    name: String,
    arguments: String,
}

/// Streaming state for the UI event loop (unused outside this module but
/// kept for API compatibility with earlier revisions).
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum StreamingState {
    Idle,
    Streaming,
    Done,
    Error(String),
}

// ---------------------------------------------------------------------------
// LLM client
// ---------------------------------------------------------------------------

/// LLM client that uses blocking HTTP calls (ureq).
pub struct LlmClient {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout_secs: u64,
    pub max_retries: u32,
}

impl LlmClient {
    /// Build an LLM client from the loaded settings.
    pub fn from_config(settings: &crate::config::Settings) -> Option<Self> {
        let api_key = crate::config::resolve_api_key(settings)?;
        let base_url = crate::config::resolve_base_url(settings);
        let model = crate::config::resolve_model_name(settings);
        let gen_config = settings.model.generation_config.as_ref();
        let timeout_secs = gen_config.and_then(|gc| gc.timeout).unwrap_or(120);
        let max_retries = gen_config.and_then(|gc| gc.max_retries).unwrap_or(2);

        Some(Self {
            base_url,
            api_key,
            model,
            timeout_secs,
            max_retries,
        })
    }

    fn build_agent(&self) -> ureq::Agent {
        ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(10))
            .timeout_read(std::time::Duration::from_secs(self.timeout_secs))
            .build()
    }

    /// Send a chat completion request (blocking, non-streaming). Used by
    /// `/compress` for one-shot summarization; the agentic loop uses
    /// `chat_stream` instead.
    pub fn chat(&self, messages: &[Message]) -> Result<String, String> {
        let request = ChatRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            temperature: Some(0.7),
            max_tokens: Some(4096),
            stream: false,
            tools: None,
            tool_choice: None,
        };

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let auth_header = format!("Bearer {}", self.api_key);

        let request_value = serde_json::to_value(&request)
            .map_err(|e| format!("Failed to serialize request: {}", e))?;

        let agent = self.build_agent();
        let mut last_err = String::new();

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                let backoff = std::time::Duration::from_secs(1 << (attempt - 1).min(4));
                std::thread::sleep(backoff);
            }

            let response = agent
                .post(&url)
                .set("Authorization", &auth_header)
                .set("Content-Type", "application/json")
                .send_json(&request_value);

            match response {
                Ok(resp) => {
                    let body: ChatResponse = resp
                        .into_json()
                        .map_err(|e| format!("Failed to parse response: {}", e))?;
                    return body
                        .choices
                        .first()
                        .and_then(|c| c.message.content.clone())
                        .ok_or_else(|| "Empty response from LLM".to_string());
                }
                Err(ureq::Error::Status(code, resp)) if code >= 500 => {
                    last_err = format!(
                        "API error: {} — {}",
                        code,
                        resp.into_string().unwrap_or_default()
                    );
                }
                Err(ureq::Error::Status(code, resp)) => {
                    let body_text = resp.into_string().unwrap_or_default();
                    return Err(format!("API error: {} — {}", code, body_text));
                }
                Err(ureq::Error::Transport(e)) => {
                    last_err = format!("Transport error: {}", e);
                }
            }
        }

        Err(last_err)
    }

    /// Send a streaming chat request with optional tool definitions.
    /// Returns a StreamReader that yields content tokens and accumulates
    /// tool_call fragments internally.
    ///
    /// Retries on transient failures (5xx, transport errors) up to
    /// `max_retries` times with exponential backoff (1s, 2s, 4s, ...).
    /// 4xx errors are not retried.
    pub fn chat_stream(
        &self,
        messages: &[Message],
        tools: Option<Vec<ToolSpec>>,
    ) -> Result<StreamReader, String> {
        let request = ChatRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            temperature: Some(0.7),
            max_tokens: Some(4096),
            stream: true,
            tools,
            tool_choice: None,
        };

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let auth_header = format!("Bearer {}", self.api_key);

        let request_value =
            serde_json::to_value(&request).map_err(|e| format!("Serialize error: {}", e))?;

        let agent = self.build_agent();
        let mut last_err = String::new();

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                let backoff = std::time::Duration::from_secs(1 << (attempt - 1).min(4));
                std::thread::sleep(backoff);
            }

            let response = agent
                .post(&url)
                .set("Authorization", &auth_header)
                .set("Content-Type", "application/json")
                .send_json(&request_value);

            match response {
                Ok(resp) => {
                    let reader = resp.into_reader();
                    return Ok(StreamReader {
                        reader: std::io::BufReader::new(reader),
                        done: false,
                        tool_calls_buffer: BTreeMap::new(),
                    });
                }
                Err(ureq::Error::Status(code, resp)) if code >= 500 => {
                    last_err = format!(
                        "API error: {} — {}",
                        code,
                        resp.into_string().unwrap_or_default()
                    );
                }
                Err(ureq::Error::Status(code, resp)) => {
                    let body_text = resp.into_string().unwrap_or_default();
                    return Err(format!("API error: {} — {}", code, body_text));
                }
                Err(ureq::Error::Transport(e)) => {
                    last_err = format!("Transport error: {}", e);
                }
            }
        }

        Err(last_err)
    }
}

// ---------------------------------------------------------------------------
// Stream reader
// ---------------------------------------------------------------------------

/// Reads SSE events from a streaming response, yielding content tokens.
/// Tool calls are accumulated internally across deltas; retrieve them
/// via `take_tool_calls()` after `next_token()` returns `Ok(None)`.
pub struct StreamReader {
    reader: std::io::BufReader<Box<dyn std::io::Read + Send + Sync>>,
    done: bool,
    tool_calls_buffer: BTreeMap<usize, ToolCallBuilder>,
}

impl StreamReader {
    /// Read the next content token from the stream. Returns:
    /// - `Ok(Some(text))` for a content chunk
    /// - `Ok(None)` when the stream is finished (call `take_tool_calls()` to get any tool calls)
    /// - `Err(msg)` on error
    pub fn next_token(&mut self) -> Result<Option<String>, String> {
        if self.done {
            return Ok(None);
        }

        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => {
                    self.done = true;
                    return Ok(None);
                }
                Ok(_) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    if line == "data: [DONE]" {
                        self.done = true;
                        return Ok(None);
                    }
                    if let Some(data) = line.strip_prefix("data: ") {
                        match serde_json::from_str::<StreamChunk>(data) {
                            Ok(chunk) => {
                                if let Some(choice) = chunk.choices.first() {
                                    // Accumulate any tool-call fragments.
                                    if let Some(calls) = &choice.delta.tool_calls {
                                        self.absorb_tool_calls(calls);
                                    }

                                    let finish = choice.finish_reason.clone();
                                    let content = choice.delta.content.clone();

                                    if finish.is_some() {
                                        self.done = true;
                                        if let Some(text) = content {
                                            if !text.is_empty() {
                                                return Ok(Some(text));
                                            }
                                        }
                                        return Ok(None);
                                    }
                                    if let Some(text) = content {
                                        if !text.is_empty() {
                                            return Ok(Some(text));
                                        }
                                    }
                                }
                            }
                            Err(_) => continue,
                        }
                    }
                }
                Err(e) => {
                    self.done = true;
                    return Err(format!("Stream read error: {}", e));
                }
            }
        }
    }

    fn absorb_tool_calls(&mut self, deltas: &[ToolCallDelta]) {
        for (i, delta) in deltas.iter().enumerate() {
            // Some providers omit `index` on the first chunk; fall back to
            // the ordinal position within this SSE frame.
            let idx = delta.index.unwrap_or(i);
            let entry = self.tool_calls_buffer.entry(idx).or_default();
            if let Some(id) = &delta.id {
                if !id.is_empty() {
                    entry.id = id.clone();
                }
            }
            if let Some(func) = &delta.function {
                if let Some(n) = &func.name {
                    entry.name.push_str(n);
                }
                if let Some(a) = &func.arguments {
                    entry.arguments.push_str(a);
                }
            }
        }
    }

    /// Consume accumulated tool calls. Returns an empty Vec if none were seen.
    pub fn take_tool_calls(&mut self) -> Vec<ToolCall> {
        let buffer = std::mem::take(&mut self.tool_calls_buffer);
        buffer
            .into_iter()
            .filter_map(|(_, b)| {
                if b.name.is_empty() {
                    None
                } else {
                    Some(ToolCall {
                        // OpenAI rejects subsequent requests when two
                        // `tool_call_id`s collide. Falling back on
                        // `(pid, frame_index)` collided across streams in the
                        // same process (idx=0 always reused), so we use a
                        // process-global monotonic counter instead.
                        id: if b.id.is_empty() {
                            next_synthetic_tool_call_id()
                        } else {
                            b.id
                        },
                        tool_type: "function".to_string(),
                        function: FunctionCall {
                            name: b.name,
                            arguments: if b.arguments.is_empty() {
                                "{}".to_string()
                            } else {
                                b.arguments
                            },
                        },
                    })
                }
            })
            .collect()
    }
}

/// Process-global monotonic counter for synthesizing tool-call IDs when the
/// upstream provider omits them. Embeds the PID so multiple cosh-tui
/// processes don't collide if their logs ever cross.
fn next_synthetic_tool_call_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("call_{}_{}", std::process::id(), n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_chunk_parsing() {
        let data = r#"{"choices":[{"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let chunk: StreamChunk = serde_json::from_str(data).unwrap();
        assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("Hello"));
        assert!(chunk.choices[0].finish_reason.is_none());
    }

    #[test]
    fn test_stream_chunk_finish() {
        let data = r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#;
        let chunk: StreamChunk = serde_json::from_str(data).unwrap();
        assert_eq!(chunk.choices[0].finish_reason.as_deref(), Some("stop"));
        assert!(chunk.choices[0].delta.content.is_none());
    }

    #[test]
    fn test_message_serialization_user() {
        let msg = Message::user("hello");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"role\":\"user\""));
        assert!(json.contains("hello"));
        // Optional fields should be omitted.
        assert!(!json.contains("tool_calls"));
        assert!(!json.contains("tool_call_id"));
    }

    #[test]
    fn test_message_serialization_tool_result() {
        let msg = Message::tool_result("call_abc", "run_shell_command", "exit 0");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"role\":\"tool\""));
        assert!(json.contains("\"tool_call_id\":\"call_abc\""));
        assert!(json.contains("\"name\":\"run_shell_command\""));
    }

    #[test]
    fn test_message_assistant_with_tool_calls_serializes() {
        let tc = ToolCall {
            id: "c1".to_string(),
            tool_type: "function".to_string(),
            function: FunctionCall {
                name: "run_shell_command".to_string(),
                arguments: "{\"command\":\"ls\"}".to_string(),
            },
        };
        let msg = Message::assistant_tool_calls("", vec![tc]);
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"role\":\"assistant\""));
        assert!(json.contains("\"tool_calls\""));
        assert!(json.contains("\"name\":\"run_shell_command\""));
    }

    #[test]
    fn test_chat_request_stream_flag() {
        let req = ChatRequest {
            model: "test".to_string(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            stream: true,
            tools: None,
            tool_choice: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"stream\":true"));
        assert!(!json.contains("tools"));
    }

    #[test]
    fn test_chat_request_with_tools() {
        let spec = ToolSpec {
            tool_type: "function".to_string(),
            function: FunctionSpec {
                name: "my_tool".to_string(),
                description: "desc".to_string(),
                parameters: serde_json::json!({"type":"object"}),
            },
        };
        let req = ChatRequest {
            model: "x".to_string(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            stream: true,
            tools: Some(vec![spec]),
            tool_choice: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"tools\""));
        assert!(json.contains("\"my_tool\""));
    }

    #[test]
    fn test_tool_call_delta_parsing() {
        let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"run_shell_command","arguments":""}}]},"finish_reason":null}]}"#;
        let chunk: StreamChunk = serde_json::from_str(data).unwrap();
        let calls = chunk.choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(calls[0].index, Some(0));
        assert_eq!(calls[0].id.as_deref(), Some("c1"));
        assert_eq!(
            calls[0].function.as_ref().unwrap().name.as_deref(),
            Some("run_shell_command")
        );
    }

    #[test]
    fn test_tool_call_delta_arguments_chunk() {
        // Subsequent deltas carry only argument fragments.
        let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"command\":"}}]},"finish_reason":null}]}"#;
        let chunk: StreamChunk = serde_json::from_str(data).unwrap();
        let calls = chunk.choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(
            calls[0].function.as_ref().unwrap().arguments.as_deref(),
            Some("{\"command\":")
        );
    }

    #[test]
    fn test_stream_reader_absorbs_tool_calls() {
        // Simulate the reader's internal accumulation logic.
        let mut buffer: BTreeMap<usize, ToolCallBuilder> = BTreeMap::new();
        // Frame 1: id + name
        let frame1 = vec![ToolCallDelta {
            index: Some(0),
            id: Some("call_1".to_string()),
            tool_type: Some("function".to_string()),
            function: Some(FunctionDelta {
                name: Some("run_shell_command".to_string()),
                arguments: Some(String::new()),
            }),
        }];
        // Frame 2: argument fragment A
        let frame2 = vec![ToolCallDelta {
            index: Some(0),
            id: None,
            tool_type: None,
            function: Some(FunctionDelta {
                name: None,
                arguments: Some("{\"command\":\"upt".to_string()),
            }),
        }];
        // Frame 3: argument fragment B
        let frame3 = vec![ToolCallDelta {
            index: Some(0),
            id: None,
            tool_type: None,
            function: Some(FunctionDelta {
                name: None,
                arguments: Some("ime\"}".to_string()),
            }),
        }];

        for frame in [frame1, frame2, frame3] {
            for (i, delta) in frame.iter().enumerate() {
                let idx = delta.index.unwrap_or(i);
                let entry = buffer.entry(idx).or_default();
                if let Some(id) = &delta.id {
                    if !id.is_empty() {
                        entry.id = id.clone();
                    }
                }
                if let Some(func) = &delta.function {
                    if let Some(n) = &func.name {
                        entry.name.push_str(n);
                    }
                    if let Some(a) = &func.arguments {
                        entry.arguments.push_str(a);
                    }
                }
            }
        }

        let built = buffer.remove(&0).unwrap();
        assert_eq!(built.id, "call_1");
        assert_eq!(built.name, "run_shell_command");
        assert_eq!(built.arguments, "{\"command\":\"uptime\"}");
    }

    #[test]
    fn test_from_config_reads_max_retries() {
        let mut settings = crate::config::Settings::default();
        settings.security.auth.api_key = Some("sk-test".to_string());
        settings.model.generation_config = Some(crate::config::GenerationConfig {
            timeout: Some(60),
            max_retries: Some(5),
            disable_cache_control: None,
            extra: Default::default(),
        });
        let client = LlmClient::from_config(&settings).unwrap();
        assert_eq!(client.max_retries, 5);
        assert_eq!(client.timeout_secs, 60);
    }

    #[test]
    fn test_from_config_defaults_max_retries() {
        let mut settings = crate::config::Settings::default();
        settings.security.auth.api_key = Some("sk-test".to_string());
        let client = LlmClient::from_config(&settings).unwrap();
        assert_eq!(client.max_retries, 2);
        assert_eq!(client.timeout_secs, 120);
    }
}
