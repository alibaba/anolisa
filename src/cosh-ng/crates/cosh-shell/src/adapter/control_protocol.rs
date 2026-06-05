use serde_json::{json, Value};

use crate::tools::is_shell_tool_name;
use crate::types::{AgentEvent, QuestionSelectionMode};

const SHELL_HANDOFF_EVIDENCE_PROMPT_MARKER: &str = "ShellCommandCompleted";
const SHELL_HANDOFF_CONTINUATION_HINT: &str =
    "analysis-only continuation after foreground shell handoff";
pub const ANALYSIS_ONLY_SHELL_DENY_MESSAGE: &str = "The foreground shell command already completed and its output was injected. Summarize the existing shell evidence or ask the user to start a new request before running another shell command.";

pub enum ControlRequest {
    Initialize {
        request_id: String,
    },
    CanUseTool {
        request_id: String,
        tool_name: String,
        tool_input: Value,
        tool_use_id: String,
    },
    AskUser {
        request_id: String,
        question: String,
        options: Vec<String>,
        allow_free_text: bool,
        selection_mode: QuestionSelectionMode,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ControlProtocolCapabilities {
    pub provider_initialize_seen: bool,
    pub can_handle_can_use_tool: bool,
    pub can_handle_host_executed_shell_tool_result: bool,
}

#[derive(Debug, Default)]
pub struct PendingControlProtocolToolCall {
    pending_shell_tool_calls: Vec<AgentEvent>,
    held_events: Vec<AgentEvent>,
}

impl PendingControlProtocolToolCall {
    pub fn take_matching_control_shell(&mut self, tool_use_id: &str) -> bool {
        if let Some(index) = self.pending_shell_tool_call_index(tool_use_id) {
            self.pending_shell_tool_calls.remove(index);
            if self.pending_shell_tool_calls.is_empty() {
                self.held_events.clear();
            }
            true
        } else {
            false
        }
    }

    pub fn stage_or_emit(&mut self, event: AgentEvent) -> Vec<AgentEvent> {
        if matches!(&event, AgentEvent::ToolCall { tool_id: Some(_), name, .. } if is_shell_tool_name(name))
        {
            self.pending_shell_tool_calls.push(event);
            return Vec::new();
        }

        if let Some(tool_id) = provider_tool_result_id(&event) {
            let mut events = self.take_pending_shell_tool_call(tool_id);
            events.push(event);
            if self.pending_shell_tool_calls.is_empty() {
                events.append(&mut self.held_events);
            }
            return events;
        }

        if !self.pending_shell_tool_calls.is_empty() {
            if is_terminal_agent_event(&event) {
                self.pending_shell_tool_calls.clear();
                let mut events = std::mem::take(&mut self.held_events);
                events.push(event);
                return events;
            }
            self.held_events.push(event);
            return Vec::new();
        }

        let mut events = std::mem::take(&mut self.held_events);
        events.push(event);
        events
    }

    pub fn flush(&mut self) -> Vec<AgentEvent> {
        let mut events = std::mem::take(&mut self.pending_shell_tool_calls);
        events.append(&mut self.held_events);
        events
    }

    fn pending_shell_tool_call_index(&self, tool_use_id: &str) -> Option<usize> {
        self.pending_shell_tool_calls
            .iter()
            .position(|event| matches!(event, AgentEvent::ToolCall { tool_id: Some(tool_id), .. } if tool_id == tool_use_id))
    }

    fn take_pending_shell_tool_call(&mut self, tool_use_id: &str) -> Vec<AgentEvent> {
        let Some(index) = self.pending_shell_tool_call_index(tool_use_id) else {
            return Vec::new();
        };
        vec![self.pending_shell_tool_calls.remove(index)]
    }
}

fn provider_tool_result_id(event: &AgentEvent) -> Option<&str> {
    match event {
        AgentEvent::ToolOutputDelta { tool_id, .. } | AgentEvent::ToolCompleted { tool_id, .. } => {
            Some(tool_id)
        }
        _ => None,
    }
}

fn is_terminal_agent_event(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::AgentCompleted { .. }
            | AgentEvent::AgentFailed { .. }
            | AgentEvent::AgentCancelled { .. }
    )
}

pub fn parse_control_request(line: &str) -> Option<ControlRequest> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    if v.get("type")?.as_str()? != "control_request" {
        return None;
    }
    let request = v.get("request")?;
    let subtype = request.get("subtype")?.as_str()?;
    let request_id = v.get("request_id")?.as_str()?.to_string();

    match subtype {
        "initialize" => Some(ControlRequest::Initialize { request_id }),
        "can_use_tool" => {
            let tool_name = request.get("tool_name")?.as_str()?.to_string();
            let tool_input = request.get("input")?.clone();
            let tool_use_id = request.get("tool_use_id")?.as_str()?.to_string();
            Some(ControlRequest::CanUseTool {
                request_id,
                tool_name,
                tool_input,
                tool_use_id,
            })
        }
        "ask_user" => {
            let question = request.get("question")?.as_str()?.to_string();
            let options = request
                .get("options")
                .and_then(|value| value.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| {
                            item.get("label")
                                .and_then(|label| label.as_str())
                                .or_else(|| item.as_str())
                                .map(str::to_string)
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let allow_free_text = request
                .get("allow_free_text")
                .and_then(|value| value.as_bool())
                .unwrap_or(true);
            let selection_mode = if request
                .get("multi_select")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                QuestionSelectionMode::Multiple
            } else {
                QuestionSelectionMode::Single
            };
            Some(ControlRequest::AskUser {
                request_id,
                question,
                options,
                allow_free_text,
                selection_mode,
            })
        }
        _ => None,
    }
}

pub fn should_deny_shell_request_for_analysis_continuation(prompt: &str, tool_name: &str) -> bool {
    prompt.contains(SHELL_HANDOFF_EVIDENCE_PROMPT_MARKER)
        && prompt.contains(SHELL_HANDOFF_CONTINUATION_HINT)
        && is_shell_tool_name(tool_name)
}

pub fn parse_initialize_capabilities(line: &str) -> Option<ControlProtocolCapabilities> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    if v.get("type")?.as_str()? != "control_response" {
        return None;
    }
    let envelope = v.get("response")?;
    if envelope.get("subtype")?.as_str()? != "success" {
        return None;
    }
    let response = envelope.get("response")?;
    if response.get("subtype")?.as_str()? != "initialize" {
        return None;
    }
    let capabilities = response.get("capabilities");
    Some(ControlProtocolCapabilities {
        provider_initialize_seen: true,
        can_handle_can_use_tool: bool_capability(capabilities, "can_handle_can_use_tool"),
        can_handle_host_executed_shell_tool_result: bool_capability(
            capabilities,
            "can_handle_host_executed_shell_tool_result",
        ),
    })
}

fn bool_capability(capabilities: Option<&Value>, key: &str) -> bool {
    capabilities
        .and_then(|value| value.get(key))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

#[derive(Debug, Clone)]
pub struct ApprovalResponse {
    pub request_id: String,
    pub tool_use_id: Option<String>,
    pub tool_input: Option<Value>,
    pub decision: ApprovalDecision,
}

#[derive(Debug, Clone)]
pub enum ApprovalDecision {
    Allow,
    Deny {
        message: String,
    },
    HostExecutedShell {
        result: Box<HostExecutedShellResult>,
    },
    Answer {
        answer: String,
    },
}

pub fn analysis_continuation_shell_deny_response(
    prompt: &str,
    request_id: &str,
    tool_name: &str,
    tool_input: &Value,
    tool_use_id: &str,
) -> Option<ApprovalResponse> {
    if !should_deny_shell_request_for_analysis_continuation(prompt, tool_name) {
        return None;
    }
    Some(ApprovalResponse {
        request_id: request_id.to_string(),
        tool_use_id: Some(tool_use_id.to_string()),
        tool_input: Some(tool_input.clone()),
        decision: ApprovalDecision::Deny {
            message: ANALYSIS_ONLY_SHELL_DENY_MESSAGE.to_string(),
        },
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostExecutedShellResult {
    pub llm_content: String,
    pub return_display: Option<String>,
    pub metadata: HostExecutedShellMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostExecutedShellMetadata {
    pub command: String,
    pub status: String,
    pub exit_code: i32,
    pub signal: Option<String>,
    pub cwd: String,
    pub end_cwd: String,
    pub duration_ms: u64,
    pub output_ref: Option<String>,
    pub redaction_status: String,
    pub approval_id: Option<String>,
    pub tool_use_id: Option<String>,
}

pub fn serialize_initialize(request_id: &str) -> String {
    json!({
        "request_id": request_id,
        "type": "control_request",
        "request": { "subtype": "initialize" }
    })
    .to_string()
}

pub fn serialize_user_message(content: &str, session_id: Option<&str>) -> String {
    json!({
        "type": "user",
        "message": { "role": "user", "content": content },
        "parent_tool_use_id": null,
        "session_id": session_id.unwrap_or("default")
    })
    .to_string()
}

pub fn serialize_co_allow(request_id: &str) -> String {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": {
                "behavior": "allow"
            }
        }
    })
    .to_string()
}

pub fn serialize_claude_allow(request_id: &str, updated_input: &Value) -> String {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": {
                "behavior": "allow",
                "updatedInput": updated_input
            }
        }
    })
    .to_string()
}

pub fn serialize_deny(request_id: &str, message: &str) -> String {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": {
                "behavior": "deny",
                "message": message
            }
        }
    })
    .to_string()
}

pub fn serialize_host_executed_shell_result(
    request_id: &str,
    result: &HostExecutedShellResult,
) -> String {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": {
                "behavior": "host_executed_shell",
                "result": {
                    "llmContent": result.llm_content,
                    "returnDisplay": result.return_display,
                    "metadata": {
                        "command": result.metadata.command,
                        "status": result.metadata.status,
                        "exit_code": result.metadata.exit_code,
                        "signal": result.metadata.signal,
                        "cwd": result.metadata.cwd,
                        "end_cwd": result.metadata.end_cwd,
                        "duration_ms": result.metadata.duration_ms,
                        "output_ref": result.metadata.output_ref,
                        "redaction_status": result.metadata.redaction_status,
                        "approval_id": result.metadata.approval_id,
                        "tool_use_id": result.metadata.tool_use_id,
                    }
                }
            }
        }
    })
    .to_string()
}

pub fn serialize_answer(request_id: &str, answer: &str) -> String {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": {
                "answer": answer
            }
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_can_use_tool() {
        let line = r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"echo hello"},"tool_use_id":"toolu_xxx"}}"#;
        let req = parse_control_request(line).expect("should parse");
        match req {
            ControlRequest::CanUseTool {
                request_id,
                tool_name,
                tool_input,
                tool_use_id,
            } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(tool_name, "Bash");
                assert_eq!(tool_input["command"], "echo hello");
                assert_eq!(tool_use_id, "toolu_xxx");
            }
            _ => panic!("expected CanUseTool"),
        }
    }

    #[test]
    fn parse_initialize() {
        let line = r#"{"request_id":"init-1","type":"control_request","request":{"subtype":"initialize"}}"#;
        let req = parse_control_request(line).expect("should parse");
        match req {
            ControlRequest::Initialize { request_id } => {
                assert_eq!(request_id, "init-1");
            }
            _ => panic!("expected Initialize"),
        }
    }

    #[test]
    fn parse_ask_user() {
        let line = r#"{"type":"control_request","request_id":"ask-1","request":{"subtype":"ask_user","question":"Pick one","options":[{"label":"Blue"},{"label":"Green"}],"allow_free_text":false,"multi_select":true}}"#;
        let req = parse_control_request(line).expect("should parse");
        match req {
            ControlRequest::AskUser {
                request_id,
                question,
                options,
                allow_free_text,
                selection_mode,
            } => {
                assert_eq!(request_id, "ask-1");
                assert_eq!(question, "Pick one");
                assert_eq!(options, ["Blue", "Green"]);
                assert!(!allow_free_text);
                assert_eq!(selection_mode, QuestionSelectionMode::Multiple);
            }
            _ => panic!("expected AskUser"),
        }
    }

    #[test]
    fn parse_non_control_request_returns_none() {
        assert!(parse_control_request(r#"{"type":"assistant","message":"hi"}"#).is_none());
        assert!(parse_control_request(r#"{"type":"result","result":"done"}"#).is_none());
        assert!(parse_control_request("not json at all").is_none());
        assert!(parse_control_request("").is_none());
    }

    #[test]
    fn parse_initialize_capabilities_from_success_response() {
        let line = r#"{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}"#;
        let capabilities = parse_initialize_capabilities(line).expect("capabilities");
        assert!(capabilities.provider_initialize_seen);
        assert!(capabilities.can_handle_can_use_tool);
        assert!(capabilities.can_handle_host_executed_shell_tool_result);
    }

    #[test]
    fn parse_initialize_capabilities_defaults_missing_flags_to_false() {
        let line = r#"{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{}}}}"#;
        let capabilities = parse_initialize_capabilities(line).expect("capabilities");
        assert!(capabilities.provider_initialize_seen);
        assert!(!capabilities.can_handle_can_use_tool);
        assert!(!capabilities.can_handle_host_executed_shell_tool_result);
    }

    #[test]
    fn parse_initialize_capabilities_ignores_other_responses() {
        assert!(parse_initialize_capabilities(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-1","response":{"behavior":"allow"}}}"#
        )
        .is_none());
        assert!(parse_initialize_capabilities(r#"{"type":"assistant","message":"hi"}"#).is_none());
    }

    #[test]
    fn serialize_co_allow_format() {
        let s = serialize_co_allow("req-42");
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["type"], "control_response");
        assert_eq!(v["response"]["subtype"], "success");
        assert_eq!(v["response"]["request_id"], "req-42");
        assert_eq!(v["response"]["response"]["behavior"], "allow");
        assert!(v["response"]["response"].get("updatedInput").is_none());
        assert!(v["response"]["response"]
            .get("updatedPermissions")
            .is_none());
        assert!(v["response"]["response"].get("toolUseID").is_none());
    }

    #[test]
    fn serialize_claude_allow_format() {
        let s = serialize_claude_allow("req-42", &json!({"command":"pwd"}));
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["type"], "control_response");
        assert_eq!(v["response"]["subtype"], "success");
        assert_eq!(v["response"]["request_id"], "req-42");
        assert_eq!(v["response"]["response"]["behavior"], "allow");
        assert_eq!(v["response"]["response"]["updatedInput"]["command"], "pwd");
        assert!(v["response"]["response"]
            .get("updatedPermissions")
            .is_none());
        assert!(v["response"]["response"].get("toolUseID").is_none());
    }

    #[test]
    fn serialize_deny_format() {
        let s = serialize_deny("req-99", "User denied");
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["type"], "control_response");
        assert_eq!(v["response"]["subtype"], "success");
        assert_eq!(v["response"]["request_id"], "req-99");
        assert_eq!(v["response"]["response"]["behavior"], "deny");
        assert_eq!(v["response"]["response"]["message"], "User denied");
    }

    #[test]
    fn serialize_host_executed_shell_result_format() {
        let result = HostExecutedShellResult {
            llm_content: "command: df -h\nstatus: completed\nbounded_output:\nFilesystem ..."
                .to_string(),
            return_display: Some("df -h completed".to_string()),
            metadata: HostExecutedShellMetadata {
                command: "df -h".to_string(),
                status: "completed".to_string(),
                exit_code: 0,
                signal: None,
                cwd: "/Users/example".to_string(),
                end_cwd: "/Users/example".to_string(),
                duration_ms: 823,
                output_ref: Some("terminal-output://block-1".to_string()),
                redaction_status: "bounded".to_string(),
                approval_id: Some("req-1".to_string()),
                tool_use_id: Some("toolu-1".to_string()),
            },
        };
        let s = serialize_host_executed_shell_result("ctrl-1", &result);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["type"], "control_response");
        assert_eq!(v["response"]["subtype"], "success");
        assert_eq!(v["response"]["request_id"], "ctrl-1");
        assert_eq!(v["response"]["response"]["behavior"], "host_executed_shell");
        assert_eq!(
            v["response"]["response"]["result"]["llmContent"],
            result.llm_content
        );
        assert_eq!(
            v["response"]["response"]["result"]["returnDisplay"],
            "df -h completed"
        );
        assert_eq!(
            v["response"]["response"]["result"]["metadata"]["command"],
            "df -h"
        );
        assert_eq!(
            v["response"]["response"]["result"]["metadata"]["exit_code"],
            0
        );
        assert!(v["response"]["response"]["result"]["metadata"]["signal"].is_null());
        assert_eq!(
            v["response"]["response"]["result"]["metadata"]["tool_use_id"],
            "toolu-1"
        );
    }

    #[test]
    fn serialize_answer_format() {
        let s = serialize_answer("ask-1", "Blue");
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["type"], "control_response");
        assert_eq!(v["response"]["subtype"], "success");
        assert_eq!(v["response"]["request_id"], "ask-1");
        assert_eq!(v["response"]["response"]["answer"], "Blue");
        assert!(v["response"]["response"].get("behavior").is_none());
    }

    #[test]
    fn analysis_continuation_deny_only_matches_shell_tools() {
        let prompt = "ShellCommandCompleted evidence\nanalysis-only continuation after foreground shell handoff";

        assert!(should_deny_shell_request_for_analysis_continuation(
            prompt,
            "run_shell_command"
        ));
        assert!(!should_deny_shell_request_for_analysis_continuation(
            prompt, "Read"
        ));
        assert!(!should_deny_shell_request_for_analysis_continuation(
            "normal user prompt",
            "run_shell_command"
        ));
        assert!(!should_deny_shell_request_for_analysis_continuation(
            "normal user prompt mentioning ShellCommandCompleted evidence",
            "run_shell_command"
        ));
    }

    #[test]
    fn analysis_continuation_shell_deny_response_preserves_request_fields() {
        let response = analysis_continuation_shell_deny_response(
            "ShellCommandCompleted evidence\nanalysis-only continuation after foreground shell handoff",
            "req-1",
            "run_shell_command",
            &json!({ "command": "df -h" }),
            "toolu-1",
        )
        .expect("deny response");

        assert_eq!(response.request_id, "req-1");
        assert_eq!(response.tool_use_id.as_deref(), Some("toolu-1"));
        assert_eq!(
            response
                .tool_input
                .as_ref()
                .and_then(|input| input.get("command"))
                .and_then(|command| command.as_str()),
            Some("df -h")
        );
        assert!(matches!(
            response.decision,
            ApprovalDecision::Deny { ref message }
                if message == ANALYSIS_ONLY_SHELL_DENY_MESSAGE
        ));
    }

    #[test]
    fn pending_control_tool_call_drops_matching_shell_snapshot() {
        let mut pending = PendingControlProtocolToolCall::default();

        assert!(pending
            .stage_or_emit(AgentEvent::ToolCall {
                run_id: "run-1".to_string(),
                tool_id: Some("toolu-1".to_string()),
                name: "shell".to_string(),
                input: "memory_pressure".to_string(),
            })
            .is_empty());

        assert!(pending.take_matching_control_shell("toolu-1"));

        assert!(pending.flush().is_empty());
    }

    #[test]
    fn pending_control_tool_call_releases_shell_snapshot_with_result_before_held_text() {
        let mut pending = PendingControlProtocolToolCall::default();

        assert!(pending
            .stage_or_emit(AgentEvent::ToolCall {
                run_id: "run-1".to_string(),
                tool_id: Some("toolu-1".to_string()),
                name: "shell".to_string(),
                input: "memory_pressure".to_string(),
            })
            .is_empty());

        assert!(pending
            .stage_or_emit(AgentEvent::TextDelta {
                run_id: "run-1".to_string(),
                text: "final".to_string(),
            })
            .is_empty());

        let events = pending.stage_or_emit(AgentEvent::ToolOutputDelta {
            run_id: "run-1".to_string(),
            tool_id: "toolu-1".to_string(),
            stream: "stdout".to_string(),
            text: "output".to_string(),
        });

        assert!(matches!(
            &events[..],
            [
                AgentEvent::ToolCall {
                    tool_id: Some(tool_id),
                    ..
                },
                AgentEvent::ToolOutputDelta {
                    tool_id: output_id,
                    ..
                },
                AgentEvent::TextDelta { text, .. },
            ] if tool_id == "toolu-1" && output_id == "toolu-1" && text == "final"
        ));
    }

    #[test]
    fn serialize_initialize_format() {
        let s = serialize_initialize("init-7");
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["type"], "control_request");
        assert_eq!(v["request_id"], "init-7");
        assert_eq!(v["request"]["subtype"], "initialize");
    }

    #[test]
    fn serialize_user_message_format() {
        let s = serialize_user_message("hello world", Some("sess-1"));
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["type"], "user");
        assert_eq!(v["message"]["role"], "user");
        assert_eq!(v["message"]["content"], "hello world");
        assert!(v["parent_tool_use_id"].is_null());
        assert_eq!(v["session_id"], "sess-1");

        let s2 = serialize_user_message("hi", None);
        let v2: Value = serde_json::from_str(&s2).unwrap();
        assert_eq!(v2["session_id"], "default");
    }
}
