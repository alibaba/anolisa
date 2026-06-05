use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::types::{AgentEvent, QuestionSelectionMode};

use super::AdapterError;

const MAX_TOOL_OUTPUT_CHARS: usize = 4_000;

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
        }
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
        if let Some((phase, message)) = self.extract_claude_status(&value) {
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
            } else {
                if let Some(text) = self.extract_assistant_snapshot_delta(&value) {
                    self.push_text_event(&mut events, text);
                } else if !self.emitted_text {
                    if let Some(text) = extract_claude_result_text(&value) {
                        self.push_text_event(&mut events, text);
                    }
                }
            }
        }

        if value.get("type").and_then(|value| value.as_str()) == Some("result") {
            self.completed = true;
            if value.get("is_error").and_then(|value| value.as_bool()) == Some(true) {
                events.push(AgentEvent::AgentFailed {
                    run_id: self.run_id.clone(),
                    error: extract_claude_error_text(&value)
                        .or_else(|| extract_claude_result_text(&value))
                        .unwrap_or_else(|| "claude code returned an error".to_string()),
                });
            } else {
                events.push(AgentEvent::AgentCompleted {
                    run_id: self.run_id.clone(),
                    summary: "claude code analysis completed".to_string(),
                });
            }
        }

        events
    }

    fn remember_session_id(&mut self, value: &serde_json::Value) {
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
                    "preparing claude-code session".to_string(),
                ))
            }
            Some("init") => {
                let model = value
                    .get("model")
                    .and_then(|value| value.as_str())
                    .unwrap_or("model");
                Some((
                    "initialized".to_string(),
                    format!("claude-code initialized {model}"),
                ))
            }
            Some("status") => {
                let status = value
                    .get("status")
                    .and_then(|value| value.as_str())
                    .filter(|status| !status.is_empty())?;
                Some((status.to_string(), format!("claude-code status: {status}")))
            }
            _ => None,
        }
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
                summary: "claude code analysis completed".to_string(),
            })?;
        }
        Ok(())
    }
}

fn extract_claude_stream_delta(value: &serde_json::Value) -> Option<String> {
    value
        .pointer("/event/delta/text")
        .or_else(|| value.pointer("/delta/text"))
        .or_else(|| value.pointer("/message/delta/text"))
        .and_then(|value| value.as_str())
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

fn extract_claude_thinking_delta(value: &serde_json::Value) -> Option<String> {
    value
        .pointer("/event/delta/thinking")
        .or_else(|| value.pointer("/delta/thinking"))
        .or_else(|| value.pointer("/message/delta/thinking"))
        .and_then(|value| value.as_str())
        .filter(|text| !text.is_empty())
        .map(|_| "claude-code thinking".to_string())
}

fn extract_claude_assistant_text(value: &serde_json::Value) -> Option<String> {
    if value.get("type").and_then(|value| value.as_str()) != Some("assistant") {
        return None;
    }

    extract_content_text(
        value
            .pointer("/message/content")
            .or_else(|| value.get("content")),
    )
}

fn extract_claude_result_text(value: &serde_json::Value) -> Option<String> {
    if value.get("type").and_then(|value| value.as_str()) != Some("result") {
        return None;
    }

    value
        .get("result")
        .and_then(|value| value.as_str())
        .filter(|text| !text.trim().is_empty())
        .map(|text| text.trim().to_string())
}

fn extract_claude_error_text(value: &serde_json::Value) -> Option<String> {
    let errors = value.get("errors")?.as_array()?;
    let text = errors
        .iter()
        .filter_map(|value| value.as_str())
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn extract_content_text(value: Option<&serde_json::Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }

    let parts = value
        .as_array()?
        .iter()
        .filter_map(|part| part.get("text").and_then(|value| value.as_str()))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(""))
    }
}

fn tool_result_content(value: Option<&serde_json::Value>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    if let Some(text) = extract_content_text(Some(value)) {
        return text;
    }
    serde_json::to_string(value).unwrap_or_default()
}

fn tool_result_outputs(part: &serde_json::Value) -> Vec<(&'static str, String)> {
    let mut outputs = Vec::new();
    if let Some(stdout) = tool_result_text_field(part, "stdout") {
        outputs.push(("stdout", bound_tool_output(stdout)));
    }
    if let Some(stderr) = tool_result_text_field(part, "stderr") {
        outputs.push(("stderr", bound_tool_output(stderr)));
    }
    if !outputs.is_empty() {
        return outputs;
    }

    let content = bound_tool_output(tool_result_content(part.get("content")));
    if content.is_empty() {
        return outputs;
    }
    let stream = if tool_result_status(part) == "success" {
        "stdout"
    } else {
        "stderr"
    };
    outputs.push((stream, content));
    outputs
}

fn tool_result_text_field(part: &serde_json::Value, field: &str) -> Option<String> {
    part.get(field)
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn tool_result_status(part: &serde_json::Value) -> String {
    if part
        .get("interrupted")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
        || part.get("status").and_then(|value| value.as_str()) == Some("interrupted")
    {
        return "interrupted".to_string();
    }
    if part
        .get("is_error")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
        || part
            .get("stderr")
            .and_then(|value| value.as_str())
            .is_some_and(|value| !value.is_empty())
    {
        return "error".to_string();
    }
    "success".to_string()
}

fn bound_tool_output(text: String) -> String {
    let mut chars = text.chars();
    let preview = chars
        .by_ref()
        .take(MAX_TOOL_OUTPUT_CHARS)
        .collect::<String>();
    let omitted = chars.count();
    if omitted == 0 {
        return preview;
    }
    format!("{preview}\n... {omitted} chars omitted")
}

#[derive(Debug, Clone)]
struct ClaudeToolUse {
    id: String,
    name: String,
    input: String,
    input_value: serde_json::Value,
    context_text: Option<String>,
}

#[derive(Debug, Clone)]
struct StreamingClaudeToolUse {
    id: String,
    name: String,
    input_value: serde_json::Value,
    input_json: String,
}

impl StreamingClaudeToolUse {
    fn into_tool_use(self) -> ClaudeToolUse {
        let input_value = if self.input_json.trim().is_empty() {
            self.input_value
        } else {
            serde_json::from_str(&self.input_json).unwrap_or(self.input_value)
        };
        let input =
            if let Some(command) = input_value.get("command").and_then(|value| value.as_str()) {
                command.to_string()
            } else {
                serde_json::to_string(&input_value).unwrap_or_else(|_| "{}".to_string())
            };
        ClaudeToolUse {
            id: self.id,
            name: self.name,
            input,
            input_value,
            context_text: None,
        }
    }
}

fn extract_claude_tool_uses(value: &serde_json::Value) -> Vec<ClaudeToolUse> {
    if let Some(tool) = extract_claude_permission_tool_use(value) {
        return vec![tool];
    }

    let Some(parts) = message_parts(value) else {
        return Vec::new();
    };

    let context_text = extract_content_text(
        value
            .pointer("/message/content")
            .or_else(|| value.pointer("/message/parts"))
            .or_else(|| value.get("content")),
    );
    parts
        .iter()
        .filter_map(|part| tool_use_from_part(part, context_text.clone()))
        .collect()
}

fn message_parts(value: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    value
        .pointer("/message/content")
        .or_else(|| value.pointer("/message/parts"))
        .or_else(|| value.get("content"))
        .and_then(|value| value.as_array())
}

fn tool_use_from_part(
    part: &serde_json::Value,
    context_text: Option<String>,
) -> Option<ClaudeToolUse> {
    if part.get("type").and_then(|value| value.as_str()) == Some("tool_use") {
        let id = part
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or("tool-use")
            .to_string();
        let name = part
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("tool")
            .to_string();
        let input_value = part
            .get("input")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        return Some(ClaudeToolUse {
            id,
            name,
            input: tool_input_display(&input_value),
            input_value,
            context_text,
        });
    }

    let function_call = part.get("functionCall")?;
    let id = function_call
        .get("id")
        .and_then(|value| value.as_str())
        .unwrap_or("tool-use")
        .to_string();
    let name = function_call
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("tool")
        .to_string();
    let input_value = function_call
        .get("args")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Some(ClaudeToolUse {
        id,
        name,
        input: tool_input_display(&input_value),
        input_value,
        context_text,
    })
}

fn tool_input_display(input_value: &serde_json::Value) -> String {
    if let Some(command) = input_value.get("command").and_then(|value| value.as_str()) {
        command.to_string()
    } else {
        serde_json::to_string(input_value).unwrap_or_else(|_| "{}".to_string())
    }
}

struct ParsedToolResult {
    tool_id: String,
    status: String,
    outputs: Vec<(String, String)>,
}

fn tool_result_part(
    envelope: &serde_json::Value,
    part: &serde_json::Value,
) -> Option<ParsedToolResult> {
    if part.get("type").and_then(|value| value.as_str()) == Some("tool_result") {
        let tool_id = part
            .get("tool_use_id")
            .and_then(|value| value.as_str())
            .unwrap_or("tool-result")
            .to_string();
        return Some(ParsedToolResult {
            tool_id,
            status: tool_result_status(part),
            outputs: tool_result_outputs(part)
                .into_iter()
                .map(|(stream, text)| (stream.to_string(), text))
                .collect(),
        });
    }

    let function_response = part.get("functionResponse")?;
    let tool_id = function_response
        .get("id")
        .and_then(|value| value.as_str())
        .or_else(|| {
            envelope
                .pointer("/toolCallResult/callId")
                .and_then(|value| value.as_str())
        })
        .or_else(|| {
            function_response
                .get("name")
                .and_then(|value| value.as_str())
        })
        .unwrap_or("tool-result")
        .to_string();
    let status = envelope
        .pointer("/toolCallResult/status")
        .and_then(|value| value.as_str())
        .map(|status| {
            if status == "success" {
                "success".to_string()
            } else {
                "error".to_string()
            }
        })
        .unwrap_or_else(|| "success".to_string());
    let output = envelope
        .pointer("/toolCallResult/resultDisplay")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            function_response
                .pointer("/response/output")
                .and_then(|value| value.as_str())
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| {
            serde_json::to_string(
                function_response
                    .get("response")
                    .unwrap_or(function_response),
            )
            .unwrap_or_default()
        });
    let stream = if status == "success" {
        "stdout"
    } else {
        "stderr"
    };
    Some(ParsedToolResult {
        tool_id,
        status,
        outputs: vec![(stream.to_string(), bound_tool_output(output))],
    })
}

fn extract_claude_permission_tool_use(value: &serde_json::Value) -> Option<ClaudeToolUse> {
    let event = value.get("event").and_then(|value| value.as_str())?;
    if !matches!(event, "permission_request" | "tool_input") {
        return None;
    }
    let name = value.get("toolName").and_then(|value| value.as_str())?;
    if name != "AskUserQuestion" {
        return None;
    }
    let input_value = value
        .get("input")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let input = serde_json::to_string(&input_value).unwrap_or_else(|_| "{}".to_string());
    let id = format!("AskUserQuestion:{input}");
    Some(ClaudeToolUse {
        id,
        name: name.to_string(),
        input,
        input_value,
        context_text: None,
    })
}

fn is_incomplete_question_tool(tool: &ClaudeToolUse) -> bool {
    if tool.name != "AskUserQuestion" {
        return false;
    }
    if !tool
        .input_value
        .as_object()
        .is_some_and(serde_json::Map::is_empty)
    {
        return false;
    }
    tool.context_text
        .as_deref()
        .and_then(parse_question_context_text)
        .is_none()
}

fn user_question_from_tool_input(
    input: &serde_json::Value,
    context_text: Option<&str>,
) -> (String, Vec<String>, bool, QuestionSelectionMode) {
    let input = normalized_question_input(input);
    let context = context_text.and_then(parse_question_context_text);
    let question = input
        .get("question")
        .or_else(|| input.get("prompt"))
        .or_else(|| input.get("message"))
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
        .or_else(|| context.as_ref().map(|context| context.question.clone()))
        .unwrap_or_else(|| "Agent needs your input".to_string())
        .to_string();
    let mut options = input
        .get("options")
        .or_else(|| input.get("choices"))
        .or_else(|| input.get("suggestions"))
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .or_else(|| item.get("label").and_then(|value| value.as_str()))
                        .or_else(|| item.get("title").and_then(|value| value.as_str()))
                        .or_else(|| item.get("text").and_then(|value| value.as_str()))
                        .or_else(|| item.get("value").and_then(|value| value.as_str()))
                        .or_else(|| item.get("name").and_then(|value| value.as_str()))
                })
                .filter(|option| !option.trim().is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if options.is_empty() {
        if let Some(context) = &context {
            options = context.options.clone();
        }
    }
    let allow_free_text = input
        .get("allow_free_text")
        .or_else(|| input.get("allowFreeText"))
        .or_else(|| input.get("free_text"))
        .or_else(|| input.get("freeText"))
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    let selection_mode = question_selection_mode(input);
    (question, options, allow_free_text, selection_mode)
}

fn normalized_question_input(input: &serde_json::Value) -> &serde_json::Value {
    input
        .get("questions")
        .and_then(|value| value.as_array())
        .and_then(|questions| questions.first())
        .unwrap_or(input)
}

#[derive(Debug, Clone)]
struct QuestionContext {
    question: String,
    options: Vec<String>,
}

fn parse_question_context_text(text: &str) -> Option<QuestionContext> {
    let mut lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return None;
    }

    let question = lines.remove(0).to_string();
    let options = lines
        .into_iter()
        .flat_map(option_candidates_from_context_line)
        .collect::<Vec<_>>();
    Some(QuestionContext { question, options })
}

fn option_candidates_from_context_line(line: &str) -> Vec<String> {
    let trimmed = line
        .trim_start_matches(|ch: char| {
            ch == '-' || ch == '*' || ch == '•' || ch.is_ascii_digit() || ch == '.' || ch == ')'
        })
        .trim();
    let tokens = trimmed
        .split_whitespace()
        .filter(|token| !token.trim().is_empty())
        .map(|token| token.trim_matches(',').trim_matches('，').to_string())
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if tokens.len() >= 2 && tokens.iter().all(|token| token.chars().count() <= 4) {
        return tokens;
    }
    if !trimmed.is_empty() && trimmed != line {
        return vec![trimmed.to_string()];
    }
    Vec::new()
}

fn question_selection_mode(input: &serde_json::Value) -> QuestionSelectionMode {
    let explicit = input
        .get("selection_mode")
        .or_else(|| input.get("mode"))
        .and_then(|value| value.as_str())
        .map(|value| value.to_ascii_lowercase());
    if explicit
        .as_deref()
        .is_some_and(|value| matches!(value, "multiple" | "multi" | "checkbox"))
    {
        return QuestionSelectionMode::Multiple;
    }
    if input
        .get("multi_select")
        .or_else(|| input.get("multiSelect"))
        .or_else(|| input.get("multiple"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return QuestionSelectionMode::Multiple;
    }
    QuestionSelectionMode::Single
}
