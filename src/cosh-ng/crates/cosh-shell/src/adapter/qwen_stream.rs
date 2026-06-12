use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::types::{AgentEvent, QuestionSelectionMode};

use super::claude_stream::ClaudeStreamParser;
use super::AdapterError;

pub(super) struct QwenStreamParser {
    inner: ClaudeStreamParser,
}

impl QwenStreamParser {
    pub(super) fn new(run_id: String, session_state: Option<Arc<Mutex<Option<String>>>>) -> Self {
        Self {
            inner: ClaudeStreamParser::new(run_id, session_state),
        }
    }

    pub(super) fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let events = self.inner.parse_line(line);
        events.into_iter().map(|e| self.localize_event(e)).collect()
    }

    pub(super) fn finish(
        &mut self,
        sink: &mut dyn FnMut(AgentEvent) -> Result<(), AdapterError>,
    ) -> Result<(), AdapterError> {
        let mut collected = Vec::new();
        self.inner.finish(&mut |event| {
            collected.push(event);
            Ok(())
        })?;
        for event in collected {
            sink(self.localize_event(event))?;
        }
        Ok(())
    }

    fn localize_event(&self, event: AgentEvent) -> AgentEvent {
        match event {
            AgentEvent::StatusChanged {
                run_id,
                phase,
                message,
            } => AgentEvent::StatusChanged {
                run_id,
                phase,
                message: self.localize_status_message(&message),
            },
            AgentEvent::AgentCompleted { run_id, .. } => AgentEvent::AgentCompleted {
                run_id,
                summary: "co analysis completed".to_string(),
            },
            AgentEvent::AgentFailed { run_id, error } => AgentEvent::AgentFailed {
                run_id,
                error: error
                    .replace("Claude Code", "co")
                    .replace("Claude", "co")
                    .replace("claude-code", "co")
                    .replace("claude code", "co"),
            },
            AgentEvent::TextDelta { run_id, text } => synthetic_question_from_text(&run_id, &text)
                .unwrap_or(AgentEvent::TextDelta { run_id, text }),
            other => other,
        }
    }

    fn localize_status_message(&self, message: &str) -> String {
        message
            .replace("Claude Code", "co")
            .replace("Claude", "co")
            .replace("claude-code", "co")
            .replace("claude code", "co")
    }
}

fn synthetic_question_from_text(run_id: &str, text: &str) -> Option<AgentEvent> {
    let marker = "COSH_QUESTION:";
    let json_text = text.split_once(marker)?.1.trim().lines().next()?.trim();
    let value: Value = serde_json::from_str(json_text).ok()?;
    let question = value.get("question")?.as_str()?.to_string();
    let options = value
        .get("options")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .map(ToString::to_string)
                        .or_else(|| option_label(item))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let allow_free_text = bool_field(&value, "allow_free_text")
        .or_else(|| bool_field(&value, "allowFreeText"))
        .unwrap_or(options.is_empty());
    let selection_mode = if bool_field(&value, "multi_select")
        .or_else(|| bool_field(&value, "multiSelect"))
        .unwrap_or(false)
    {
        QuestionSelectionMode::Multiple
    } else {
        QuestionSelectionMode::Single
    };

    Some(AgentEvent::UserQuestion {
        run_id: run_id.to_string(),
        question,
        options,
        allow_free_text,
        selection_mode,
        request_id: None,
    })
}

fn option_label(value: &Value) -> Option<String> {
    value
        .get("label")
        .or_else(|| value.get("title"))
        .or_else(|| value.get("value"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn bool_field(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(Value::as_bool)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parser() -> QwenStreamParser {
        QwenStreamParser::new("test-run".to_string(), None)
    }

    #[test]
    fn text_delta_from_stream_event() {
        let mut p = parser();
        let events = p.parse_line(r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], AgentEvent::TextDelta { text, .. } if text == "hello"));
    }

    #[test]
    fn system_init_extracts_model() {
        let mut p = parser();
        let events = p.parse_line(r#"{"type":"system","subtype":"init","session_id":"sess-1","model":"qwen3.7-max","tools":[],"copilot_version":"2.4.1"}"#);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], AgentEvent::StatusChanged { phase, message, .. }
            if phase == "initialized" && message.contains("qwen3.7-max") && !message.contains("claude"))
        );
    }

    #[test]
    fn result_success() {
        let mut p = parser();
        let events = p.parse_line(r#"{"type":"result","subtype":"success","session_id":"s","is_error":false,"result":"done","duration_ms":100}"#);
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::TextDelta { text, .. } if text == "done")));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::AgentCompleted { summary, .. } if summary == "co analysis completed")));
    }

    #[test]
    fn result_error() {
        let mut p = parser();
        let events = p.parse_line(r#"{"type":"result","is_error":true,"errors":["rate limit"]}"#);
        assert!(events.iter().any(
            |e| matches!(e, AgentEvent::AgentFailed { error, .. } if error.contains("rate limit"))
        ));
    }

    #[test]
    fn thinking_delta() {
        let mut p = parser();
        let events = p.parse_line(r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}}"#);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], AgentEvent::StatusChanged { phase, message, .. }
            if phase == "thinking" && message == "co thinking")
        );
    }

    #[test]
    fn localizes_capitalized_claude_wording() {
        let p = parser();
        let event = p.localize_event(AgentEvent::StatusChanged {
            run_id: "r".to_string(),
            phase: "starting".to_string(),
            message: "Starting Claude Code backend".to_string(),
        });
        assert!(matches!(event, AgentEvent::StatusChanged { message, .. }
            if message == "Starting co backend"));
    }

    #[test]
    fn session_id_remembered() {
        let state = Arc::new(Mutex::new(None::<String>));
        let mut p = QwenStreamParser::new("r".to_string(), Some(Arc::clone(&state)));
        p.parse_line(
            r#"{"type":"system","subtype":"init","session_id":"qwen-sess-42","model":"m"}"#,
        );
        assert_eq!(*state.lock().unwrap(), Some("qwen-sess-42".to_string()));
    }

    #[test]
    fn finish_emits_completed_when_no_result() {
        let mut p = parser();
        p.parse_line(r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}}"#);
        let mut events = Vec::new();
        p.finish(&mut |e| {
            events.push(e);
            Ok(())
        })
        .unwrap();
        assert!(events.iter().any(|e| matches!(e, AgentEvent::AgentCompleted { summary, .. } if summary == "co analysis completed")));
    }

    #[test]
    fn tool_call_from_assistant_snapshot() {
        let mut p = parser();
        let events = p.parse_line(r#"{"type":"assistant","session_id":"s","message":{"id":"m1","type":"message","role":"assistant","model":"qwen","content":[{"type":"tool_use","id":"call_abc","name":"run_shell_command","input":{"command":"ls -la"}}]}}"#);
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolCall { name, input, .. }
            if name == "run_shell_command" && input == "ls -la")));
    }

    #[test]
    fn question_from_assistant_snapshot() {
        let mut p = parser();
        let events = p.parse_line(r#"{"type":"assistant","session_id":"s","message":{"id":"m1","type":"message","role":"assistant","model":"qwen","content":[{"type":"tool_use","id":"call_question","name":"AskUserQuestion","input":{"questions":[{"question":"Pick a shell","header":"Shell","options":[{"label":"bash"},{"label":"zsh"}],"multiSelect":false,"allowFreeText":false}]}}]}}"#);
        assert!(
            events.iter().any(|e| matches!(
                e,
                AgentEvent::UserQuestion {
                    question,
                    options,
                    allow_free_text,
                    ..
                } if question == "Pick a shell" && options == &vec!["bash".to_string(), "zsh".to_string()] && !allow_free_text
            )),
            "expected UserQuestion, got: {events:?}"
        );
    }

    #[test]
    fn multi_select_question_from_permission_request() {
        let mut p = parser();
        let events = p.parse_line(r#"{"event":"permission_request","toolName":"AskUserQuestion","input":{"questions":[{"question":"Choose checks","header":"Checks","options":[{"label":"Lint"},{"label":"Unit tests"}],"multiSelect":true,"allowFreeText":true}]},"permissionLevel":null}"#);
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::UserQuestion {
                question,
                options,
                allow_free_text,
                ..
            } if question == "Choose checks" && options == &vec!["Lint".to_string(), "Unit tests".to_string()] && *allow_free_text
        )));
    }

    #[test]
    fn free_text_only_question_from_assistant_snapshot() {
        let mut p = parser();
        let events = p.parse_line(r#"{"type":"assistant","session_id":"s","message":{"id":"m1","type":"message","role":"assistant","model":"qwen","content":[{"type":"tool_use","id":"call_question","name":"AskUserQuestion","input":{"question":"Describe the project goal","allowFreeText":true}}]}}"#);
        assert!(
            events.iter().any(|e| matches!(
                e,
                AgentEvent::UserQuestion {
                    question,
                    options,
                    allow_free_text,
                    ..
                } if question == "Describe the project goal" && options.is_empty() && *allow_free_text
            )),
            "expected free-text UserQuestion, got: {events:?}"
        );
    }

    #[test]
    fn synthetic_question_from_text_fallback() {
        let mut p = parser();
        let events = p.parse_line(r#"{"type":"assistant","session_id":"s","message":{"id":"m1","type":"message","role":"assistant","model":"qwen","content":[{"type":"text","text":"COSH_QUESTION: {\"question\":\"你喜欢什么颜色？\",\"options\":[],\"allow_free_text\":true,\"multi_select\":false}"}]}}"#);
        assert!(
            events.iter().any(|e| matches!(
                e,
                AgentEvent::UserQuestion {
                    question,
                    options,
                    allow_free_text,
                    selection_mode,
                    ..
                } if question == "你喜欢什么颜色？"
                    && options.is_empty()
                    && *allow_free_text
                    && *selection_mode == crate::types::QuestionSelectionMode::Single
            )),
            "expected synthetic UserQuestion, got: {events:?}"
        );
    }

    #[test]
    fn extra_co_fields_ignored() {
        let mut p = parser();
        let events = p.parse_line(r#"{"type":"stream_event","uuid":"xxx","parent_tool_use_id":"yyy","message_id":"zzz","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], AgentEvent::TextDelta { text, .. } if text == "ok"));
    }
}
