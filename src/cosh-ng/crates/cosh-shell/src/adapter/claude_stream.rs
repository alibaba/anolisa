use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::types::{AgentEvent, AuthOutcome};

use super::claude_stream_extract::{
    extract_claude_assistant_text, extract_claude_error_text, extract_claude_result_text,
    extract_claude_stream_delta, extract_claude_thinking_delta, extract_claude_tool_uses,
    is_incomplete_question_tool, message_parts, tool_result_part, user_question_from_tool_input,
    ClaudeToolUse, StreamingClaudeToolUse,
};
use super::AdapterError;

pub(super) struct ClaudeStreamParser {
    run_id: String,
    session_state: Option<Arc<Mutex<Option<String>>>>,
    assistant_text: String,
    current_stream_text: String,
    seen_tool_uses: HashSet<String>,
    seen_tool_results: HashSet<String>,
    streaming_tool_uses: HashMap<usize, StreamingClaudeToolUse>,
    emitted_text: bool,
    emitted_startup_status: bool,
    completed: bool,
    session_capture_enabled: bool,
    session_resumable: Option<bool>,
    session_error_code: Option<String>,
    session_error_phase: Option<String>,
}

impl ClaudeStreamParser {
    pub(super) fn new(run_id: String, session_state: Option<Arc<Mutex<Option<String>>>>) -> Self {
        Self {
            run_id,
            session_state,
            assistant_text: String::new(),
            current_stream_text: String::new(),
            seen_tool_uses: HashSet::new(),
            seen_tool_results: HashSet::new(),
            streaming_tool_uses: HashMap::new(),
            emitted_text: false,
            emitted_startup_status: false,
            completed: false,
            session_capture_enabled: true,
            session_resumable: None,
            session_error_code: None,
            session_error_phase: None,
        }
    }

    pub(super) fn session_resumable(&self) -> Option<bool> {
        self.session_resumable
    }

    pub(super) fn session_error_code(&self) -> Option<&str> {
        self.session_error_code.as_deref()
    }

    pub(super) fn session_error_phase(&self) -> Option<&str> {
        self.session_error_phase.as_deref()
    }

    pub(super) fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }

        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            return Vec::new();
        };
        self.remember_session_id(&value);
        self.remember_stream_boundary(&value);

        let mut events = Vec::new();
        if let Some(hook_event) = self.extract_hook_notification(&value) {
            events.push(hook_event);
            return events;
        }
        if let Some(auth_result) = self.extract_auth_result(&value) {
            events.push(auth_result);
        } else if let Some((phase, message)) = self.extract_claude_status(&value) {
            events.push(AgentEvent::StatusChanged {
                run_id: self.run_id.clone(),
                phase,
                message,
            });
        } else if let Some(message) = extract_claude_thinking_delta(&value) {
            events.push(AgentEvent::StatusChanged {
                run_id: self.run_id.clone(),
                phase: "thinking".to_string(),
                message,
            });
        } else if let Some(text) = extract_claude_stream_delta(&value) {
            self.push_stream_text_event(&mut events, text);
        } else if let Some(tool_call) = self.extract_streaming_tool_call(&value) {
            events.push(tool_call);
        } else if self.contains_streaming_tool_snapshot(&value) {
            return events;
        } else if let Some(tool_call) = self.extract_tool_call(&value) {
            events.push(tool_call);
        } else {
            let tool_result_events = self.extract_tool_result_events(&value);
            if !tool_result_events.is_empty() {
                events.extend(tool_result_events);
            } else if let Some(text) = self.extract_assistant_snapshot_delta(&value) {
                self.push_text_event(&mut events, text);
            } else if !self.emitted_text {
                if let Some(text) = extract_claude_result_text(&value) {
                    self.push_text_event(&mut events, text);
                }
            }
        }

        if value.get("type").and_then(|value| value.as_str()) == Some("result") {
            self.completed = true;
            self.session_error_code = value
                .get("session_error_code")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            self.session_error_phase = value
                .get("session_error_phase")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            if value.get("is_error").and_then(|value| value.as_bool()) == Some(true) {
                events.push(AgentEvent::AgentFailed {
                    run_id: self.run_id.clone(),
                    error: extract_claude_error_text(&value)
                        .or_else(|| extract_claude_result_text(&value))
                        .unwrap_or_else(|| "analysis returned an error".to_string()),
                });
            } else {
                events.push(AgentEvent::AgentCompleted {
                    run_id: self.run_id.clone(),
                    summary: "analysis completed".to_string(),
                });
            }
        }

        events
    }

    fn remember_session_id(&mut self, value: &serde_json::Value) {
        if value.get("type").and_then(|value| value.as_str()) == Some("system")
            && value.get("subtype").and_then(|value| value.as_str()) == Some("init")
        {
            if let Some(resumable) = value
                .get("session_resumable")
                .and_then(|value| value.as_bool())
            {
                self.session_capture_enabled = resumable;
                self.session_resumable = Some(resumable);
                if !resumable {
                    if let Some(state) = &self.session_state {
                        if let Ok(mut current) = state.lock() {
                            *current = None;
                        }
                    }
                    return;
                }
            }
        }
        if !self.session_capture_enabled {
            return;
        }
        let Some(session_id) = value.get("session_id").and_then(|value| value.as_str()) else {
            return;
        };
        if let Some(state) = &self.session_state {
            if let Ok(mut current) = state.lock() {
                *current = Some(session_id.to_string());
            }
        }
    }

    fn remember_stream_boundary(&mut self, value: &serde_json::Value) {
        if value
            .pointer("/event/type")
            .and_then(|value| value.as_str())
            == Some("message_start")
        {
            self.current_stream_text.clear();
        }
    }

    fn extract_tool_call(&mut self, value: &serde_json::Value) -> Option<AgentEvent> {
        for tool in extract_claude_tool_uses(value) {
            if self.is_streaming_tool_id(&tool.id) {
                continue;
            }
            if let Some(event) = self.event_from_tool_use(tool) {
                return Some(event);
            }
        }
        None
    }

    fn is_streaming_tool_id(&self, id: &str) -> bool {
        self.streaming_tool_uses.values().any(|tool| tool.id == id)
    }

    fn contains_streaming_tool_snapshot(&self, value: &serde_json::Value) -> bool {
        extract_claude_tool_uses(value)
            .iter()
            .any(|tool| self.is_streaming_tool_id(&tool.id))
    }

    fn extract_streaming_tool_call(&mut self, value: &serde_json::Value) -> Option<AgentEvent> {
        let event = value.get("event")?;
        match event.get("type").and_then(|value| value.as_str()) {
            Some("content_block_start") => {
                let index = event.get("index").and_then(|value| value.as_u64())? as usize;
                let block = event.get("content_block")?;
                if block.get("type").and_then(|value| value.as_str()) != Some("tool_use") {
                    return None;
                }
                let id = block
                    .get("id")
                    .and_then(|value| value.as_str())
                    .unwrap_or("tool-use")
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(|value| value.as_str())
                    .unwrap_or("tool")
                    .to_string();
                let input_value = block
                    .get("input")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                self.streaming_tool_uses.insert(
                    index,
                    StreamingClaudeToolUse {
                        id,
                        name,
                        input_value,
                        input_json: String::new(),
                    },
                );
                None
            }
            Some("content_block_delta") => {
                let index = event.get("index").and_then(|value| value.as_u64())? as usize;
                let partial_json = event
                    .pointer("/delta/partial_json")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                if let Some(tool) = self.streaming_tool_uses.get_mut(&index) {
                    tool.input_json.push_str(partial_json);
                }
                None
            }
            Some("content_block_stop") => {
                let index = event.get("index").and_then(|value| value.as_u64())? as usize;
                let tool = self.streaming_tool_uses.remove(&index)?;
                self.event_from_tool_use(tool.into_tool_use())
            }
            _ => None,
        }
    }

    fn event_from_tool_use(&mut self, tool: ClaudeToolUse) -> Option<AgentEvent> {
        if tool.name == "AskUserQuestion" {
            if is_incomplete_question_tool(&tool) {
                return None;
            }
            if !self.seen_tool_uses.insert(tool.id.clone()) {
                return None;
            }
            let (question, options, allow_free_text, selection_mode) =
                user_question_from_tool_input(&tool.input_value, tool.context_text.as_deref());
            return Some(AgentEvent::UserQuestion {
                run_id: self.run_id.clone(),
                provider_request_id: None,
                question,
                options,
                allow_free_text,
                selection_mode,
            });
        }
        if !self.seen_tool_uses.insert(tool.id.clone()) {
            return None;
        }
        Some(AgentEvent::ToolCall {
            run_id: self.run_id.clone(),
            tool_id: Some(tool.id),
            name: tool.name,
            input: tool.input,
        })
    }

    fn extract_claude_status(&mut self, value: &serde_json::Value) -> Option<(String, String)> {
        if value.get("type").and_then(|value| value.as_str()) != Some("system") {
            return None;
        }

        match value.get("subtype").and_then(|value| value.as_str()) {
            Some("hook_started") if !self.emitted_startup_status => {
                self.emitted_startup_status = true;
                Some((
                    "initializing".to_string(),
                    "preparing model session".to_string(),
                ))
            }
            Some("init") => {
                let model = value
                    .get("model")
                    .and_then(|value| value.as_str())
                    .unwrap_or("model");
                Some((
                    "initialized".to_string(),
                    format!("model initialized {model}"),
                ))
            }
            Some("status") => {
                let status = value
                    .get("status")
                    .and_then(|value| value.as_str())
                    .filter(|status| !status.is_empty())?;
                Some((status.to_string(), format!("model status: {status}")))
            }
            _ => None,
        }
    }

    fn extract_auth_result(&self, value: &serde_json::Value) -> Option<AgentEvent> {
        if value.get("type").and_then(|value| value.as_str()) != Some("system")
            || value.get("subtype").and_then(|value| value.as_str()) != Some("status")
        {
            return None;
        }
        let outcome = match value.get("status").and_then(|value| value.as_str()) {
            Some("auth_ok") => AuthOutcome::Saved,
            Some("auth_applied") => AuthOutcome::Applied,
            Some("auth_persist_failed") => AuthOutcome::Failed,
            _ => return None,
        };
        let request_id = value.get("request_id")?.as_str()?;
        if request_id.is_empty() {
            return None;
        }
        Some(AgentEvent::AuthResult {
            run_id: self.run_id.clone(),
            request_id: request_id.to_string(),
            outcome,
        })
    }

    fn extract_hook_notification(&self, value: &serde_json::Value) -> Option<AgentEvent> {
        if value.get("type").and_then(|v| v.as_str()) != Some("system") {
            return None;
        }
        if value.get("subtype").and_then(|v| v.as_str()) != Some("hook_notification") {
            return None;
        }
        let hook_name = value
            .get("hook_name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let message = value
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let tool_use_id = value
            .get("tool_use_id")
            .and_then(|v| v.as_str())
            .map(String::from);
        let decision = value
            .get("decision")
            .and_then(|v| v.as_str())
            .map(String::from);
        Some(AgentEvent::HookNotification {
            run_id: self.run_id.clone(),
            hook_name,
            message,
            tool_use_id,
            decision,
        })
    }

    fn extract_tool_result_events(&mut self, value: &serde_json::Value) -> Vec<AgentEvent> {
        let Some(parts) = message_parts(value) else {
            return Vec::new();
        };

        let mut events = Vec::new();
        for part in parts {
            let Some(result) = tool_result_part(value, part) else {
                continue;
            };
            let tool_id = result.tool_id;
            if !self.seen_tool_results.insert(tool_id.clone()) {
                continue;
            }
            let status = result.status;
            for (stream, content) in result.outputs {
                events.push(AgentEvent::ToolOutputDelta {
                    run_id: self.run_id.clone(),
                    tool_id: tool_id.clone(),
                    stream,
                    text: content,
                });
            }
            events.push(AgentEvent::ToolCompleted {
                run_id: self.run_id.clone(),
                tool_id,
                status,
            });
        }
        events
    }

    fn push_text_event(&mut self, events: &mut Vec<AgentEvent>, text: String) {
        if text.is_empty() {
            return;
        }
        self.emitted_text = true;
        events.push(AgentEvent::TextDelta {
            run_id: self.run_id.clone(),
            text,
        });
    }

    fn push_stream_text_event(&mut self, events: &mut Vec<AgentEvent>, text: String) {
        self.current_stream_text.push_str(&text);
        self.push_text_event(events, text);
    }

    fn extract_assistant_snapshot_delta(&mut self, value: &serde_json::Value) -> Option<String> {
        let text = extract_claude_assistant_text(value)?;
        let delta = if !self.current_stream_text.is_empty()
            && text.starts_with(&self.current_stream_text)
        {
            text[self.current_stream_text.len()..].to_string()
        } else if text.starts_with(&self.assistant_text) {
            text[self.assistant_text.len()..].to_string()
        } else {
            text.clone()
        };
        if !self.current_stream_text.is_empty() && text.starts_with(&self.current_stream_text) {
            self.current_stream_text = text.clone();
        }
        self.assistant_text = text;
        if delta.is_empty() {
            None
        } else {
            Some(delta)
        }
    }

    pub(super) fn finish(
        &mut self,
        sink: &mut dyn FnMut(AgentEvent) -> Result<(), AdapterError>,
    ) -> Result<(), AdapterError> {
        if !self.completed {
            sink(AgentEvent::AgentCompleted {
                run_id: self.run_id.clone(),
                summary: "analysis completed".to_string(),
            })?;
        }
        Ok(())
    }
}
