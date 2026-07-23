use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// =====================================================================
// Auth types (used by CoreControlRequest::AuthRequired)
// =====================================================================

#[derive(Debug, Clone, Serialize)]
pub enum AuthReason {
    #[serde(rename = "not_configured")]
    NotConfigured,
    #[serde(rename = "invalid")]
    Invalid,
    #[serde(rename = "expired")]
    Expired,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthField {
    pub name: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    pub secret: bool,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthProvider {
    pub id: String,
    pub label: String,
    pub fields: Vec<AuthField>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub builtin_base_url: Option<String>,
    pub builtin_provider_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub builtin_default_model: Option<String>,
}

// =====================================================================
// Input messages (Shell → Core, read from stdin)
// =====================================================================

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum InputMessage {
    #[serde(rename = "user")]
    User {
        message: UserMessageContent,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        parent_tool_use_id: Option<Value>,
        #[serde(default)]
        shell_context: Option<ShellContext>,
    },

    #[serde(rename = "control_request")]
    ControlRequest {
        request_id: String,
        request: ShellControlRequest,
    },

    #[serde(rename = "control_response")]
    ControlResponse { response: ControlResponsePayload },

    #[serde(rename = "registry_request")]
    RegistryRequest {
        request_id: String,
        domain: String,
        action: String,
        #[serde(default)]
        params: Value,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserMessageContent {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ShellContext {
    pub cwd: PathBuf,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub last_exit_code: i32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "subtype")]
pub enum ShellControlRequest {
    #[serde(rename = "initialize")]
    Initialize,

    #[serde(rename = "interrupt")]
    Interrupt,

    #[serde(rename = "shutdown")]
    Shutdown,

    #[serde(rename = "config_override")]
    ConfigOverride {
        #[serde(default)]
        approval_mode: Option<String>,
        #[serde(default)]
        allowed_tools: Option<Vec<String>>,
    },

    #[serde(rename = "switch_model")]
    SwitchModel { model: String },

    #[serde(rename = "reload_config")]
    ReloadConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ControlResponsePayload {
    pub subtype: String,
    pub request_id: String,
    pub response: ControlResponseBody,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ControlResponseBody {
    pub behavior: Option<String>,
    pub message: Option<String>,
    pub result: Option<HostExecutedShellResult>,
    #[serde(rename = "toolUseID")]
    pub tool_use_id: Option<String>,
    #[serde(default, rename = "updatedPermissions")]
    pub updated_permissions: Option<Value>,
    pub answer: Option<String>,
    pub selected_options: Option<Vec<usize>>,
    // Auth response fields
    pub provider_id: Option<String>,
    pub provider_type: Option<String>,
    #[serde(default)]
    pub values: Option<HashMap<String, String>>,
    pub persist: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HostExecutedShellResult {
    #[serde(rename = "llmContent")]
    pub llm_content: String,
    #[serde(rename = "returnDisplay")]
    pub return_display: Option<String>,
    #[serde(default)]
    pub metadata: Option<Value>,
}

// =====================================================================
// Output messages (Core → Shell, written to stdout)
// =====================================================================

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum OutputMessage {
    #[serde(rename = "system")]
    System {
        subtype: String,
        #[serde(flatten)]
        payload: SystemPayload,
    },

    #[serde(rename = "stream_event")]
    StreamEvent { event: StreamEventPayload },

    #[serde(rename = "assistant")]
    Assistant {
        session_id: String,
        message: AssistantMessage,
    },

    #[serde(rename = "user")]
    User {
        session_id: String,
        message: UserOutputMessage,
    },

    #[serde(rename = "control_request")]
    ControlRequest {
        request_id: String,
        request: CoreControlRequest,
    },

    #[serde(rename = "control_response")]
    ControlResponse {
        response: CoreControlResponsePayload,
    },

    #[serde(rename = "result")]
    Result {
        #[serde(skip_serializing_if = "Option::is_none")]
        subtype: Option<String>,
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        errors: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_code: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_error_code: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_error_phase: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        env_delta: Option<EnvDelta>,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },

    #[serde(rename = "registry_response")]
    RegistryResponse {
        request_id: String,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct CoreControlResponsePayload {
    pub subtype: String,
    pub request_id: String,
    pub response: CoreControlResponseBody,
}

#[derive(Debug, Clone, Serialize)]
pub struct CoreControlResponseBody {
    pub subtype: String,
    pub capabilities: CoreControlCapabilities,
}

#[derive(Debug, Clone, Serialize)]
pub struct CoreControlCapabilities {
    pub can_handle_can_use_tool: bool,
    pub can_handle_host_executed_shell_tool_result: bool,
    pub can_handle_shell_evidence_tool: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct SystemPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_resumable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hook_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    /// Hook decision (allow/ask/block/deny) for per-hook color-coding in cosh-shell.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantMessage {
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserOutputMessage {
    pub role: String,
    pub content: Vec<UserContentBlock>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum UserContentBlock {
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        is_error: bool,
        content: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "subtype")]
pub enum CoreControlRequest {
    #[serde(rename = "can_use_tool")]
    CanUseTool {
        tool_name: String,
        input: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        tool_use_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        audit_ref: Option<String>,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        hook_requires_approval: bool,
    },

    #[serde(rename = "ask_user")]
    AskUser {
        question: String,
        options: Vec<AskUserOption>,
        allow_free_text: bool,
        multi_select: bool,
    },

    #[serde(rename = "auth_required")]
    AuthRequired {
        reason: AuthReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_message: Option<String>,
        providers: Vec<AuthProvider>,
    },

    #[serde(rename = "shell_evidence")]
    ShellEvidence {
        tool_use_id: String,
        action: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        limit: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cursor: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        direction: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        lines: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        bypass_recent_filter: Option<bool>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct AskUserOption {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum StreamEventPayload {
    #[serde(rename = "message_start")]
    MessageStart,

    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: u32,
        content_block: ContentBlockInfo,
    },

    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: u32, delta: ContentDelta },

    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: u32 },

    #[serde(rename = "message_stop")]
    MessageStop,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ContentBlockInfo {
    #[serde(rename = "text")]
    Text,
    #[serde(rename = "thinking")]
    Thinking,
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
#[allow(clippy::enum_variant_names)]
pub enum ContentDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EnvDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_cwd: Option<PathBuf>,
    #[serde(default)]
    pub env_changes: HashMap<String, Option<String>>,
}

// =====================================================================
// Helper constructors
// =====================================================================

impl OutputMessage {
    pub fn system_init(
        session_id: &str,
        model: &str,
        tools: Vec<String>,
        session_resumable: bool,
    ) -> Self {
        Self::System {
            subtype: "init".to_string(),
            payload: SystemPayload {
                session_id: Some(session_id.to_string()),
                session_resumable: Some(session_resumable),
                model: Some(model.to_string()),
                tools: Some(tools),
                ..Default::default()
            },
        }
    }

    pub fn initialize_success(request_id: &str, can_handle_shell_evidence_tool: bool) -> Self {
        Self::ControlResponse {
            response: CoreControlResponsePayload {
                subtype: "success".to_string(),
                request_id: request_id.to_string(),
                response: CoreControlResponseBody {
                    subtype: "initialize".to_string(),
                    capabilities: CoreControlCapabilities {
                        can_handle_can_use_tool: true,
                        can_handle_host_executed_shell_tool_result: true,
                        can_handle_shell_evidence_tool,
                    },
                },
            },
        }
    }

    pub fn system_status(status: &str) -> Self {
        Self::System {
            subtype: "status".to_string(),
            payload: SystemPayload {
                status: Some(status.to_string()),
                ..Default::default()
            },
        }
    }

    pub fn hook_notification(
        hook_name: &str,
        message: &str,
        tool_use_id: Option<&str>,
        decision: Option<&str>,
    ) -> Self {
        Self::System {
            subtype: "hook_notification".to_string(),
            payload: SystemPayload {
                status: Some(message.to_string()),
                hook_name: Some(hook_name.to_string()),
                tool_use_id: tool_use_id.map(String::from),
                decision: decision.map(String::from),
                ..Default::default()
            },
        }
    }

    pub fn assistant_text(session_id: &str, text: &str) -> Self {
        Self::Assistant {
            session_id: session_id.to_string(),
            message: AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: text.to_string(),
                }],
            },
        }
    }

    pub fn tool_result(session_id: &str, tool_use_id: &str, content: &str, is_error: bool) -> Self {
        Self::User {
            session_id: session_id.to_string(),
            message: UserOutputMessage {
                role: "user".to_string(),
                content: vec![UserContentBlock::ToolResult {
                    tool_use_id: tool_use_id.to_string(),
                    is_error,
                    content: content.to_string(),
                }],
            },
        }
    }

    pub fn result_success(session_id: &str, result: &str) -> Self {
        Self::Result {
            subtype: Some("success".to_string()),
            is_error: false,
            result: Some(result.to_string()),
            errors: None,
            error_code: None,
            session_error_code: None,
            session_error_phase: None,
            session_id: Some(session_id.to_string()),
            env_delta: None,
            duration_ms: None,
        }
    }

    pub fn result_error(session_id: &str, error: &str) -> Self {
        Self::result_error_with_code(session_id, error, None)
    }

    /// Builds an error result with a stable code for machine consumers.
    pub fn result_error_with_code(session_id: &str, error: &str, error_code: Option<&str>) -> Self {
        Self::Result {
            subtype: Some("error".to_string()),
            is_error: true,
            result: Some(error.to_string()),
            errors: Some(vec![error.to_string()]),
            error_code: error_code.map(str::to_string),
            session_error_code: None,
            session_error_phase: None,
            session_id: Some(session_id.to_string()),
            env_delta: None,
            duration_ms: None,
        }
    }

    pub fn session_result_error(
        session_id: &str,
        error: &str,
        session_error_code: &str,
        session_error_phase: &str,
    ) -> Self {
        Self::Result {
            subtype: Some("error".to_string()),
            is_error: true,
            result: Some(error.to_string()),
            errors: Some(vec![error.to_string()]),
            error_code: None,
            session_error_code: Some(session_error_code.to_string()),
            session_error_phase: Some(session_error_phase.to_string()),
            session_id: Some(session_id.to_string()),
            env_delta: None,
            duration_ms: None,
        }
    }

    pub fn stream_message_start() -> Self {
        Self::StreamEvent {
            event: StreamEventPayload::MessageStart,
        }
    }

    pub fn stream_message_stop() -> Self {
        Self::StreamEvent {
            event: StreamEventPayload::MessageStop,
        }
    }

    pub fn stream_text_start(index: u32) -> Self {
        Self::StreamEvent {
            event: StreamEventPayload::ContentBlockStart {
                index,
                content_block: ContentBlockInfo::Text,
            },
        }
    }

    pub fn stream_text_delta(index: u32, text: &str) -> Self {
        Self::StreamEvent {
            event: StreamEventPayload::ContentBlockDelta {
                index,
                delta: ContentDelta::TextDelta {
                    text: text.to_string(),
                },
            },
        }
    }

    pub fn stream_tool_use_start(index: u32, id: &str, name: &str) -> Self {
        Self::StreamEvent {
            event: StreamEventPayload::ContentBlockStart {
                index,
                content_block: ContentBlockInfo::ToolUse {
                    id: id.to_string(),
                    name: name.to_string(),
                },
            },
        }
    }

    pub fn stream_tool_use_delta(index: u32, partial_json: &str) -> Self {
        Self::StreamEvent {
            event: StreamEventPayload::ContentBlockDelta {
                index,
                delta: ContentDelta::InputJsonDelta {
                    partial_json: partial_json.to_string(),
                },
            },
        }
    }

    pub fn stream_thinking_start(index: u32) -> Self {
        Self::StreamEvent {
            event: StreamEventPayload::ContentBlockStart {
                index,
                content_block: ContentBlockInfo::Thinking,
            },
        }
    }

    pub fn stream_thinking_delta(index: u32, thinking: &str) -> Self {
        Self::StreamEvent {
            event: StreamEventPayload::ContentBlockDelta {
                index,
                delta: ContentDelta::ThinkingDelta {
                    thinking: thinking.to_string(),
                },
            },
        }
    }

    pub fn stream_block_stop(index: u32) -> Self {
        Self::StreamEvent {
            event: StreamEventPayload::ContentBlockStop { index },
        }
    }

    pub fn can_use_tool(
        request_id: &str,
        tool_name: &str,
        input: Value,
        tool_use_id: &str,
        hook_requires_approval: bool,
    ) -> Self {
        Self::can_use_tool_with_audit_ref(
            request_id,
            tool_name,
            input,
            tool_use_id,
            hook_requires_approval,
            None,
        )
    }

    /// Builds a Tool approval request linked to a persisted audit event.
    pub fn can_use_tool_with_audit_ref(
        request_id: &str,
        tool_name: &str,
        input: Value,
        tool_use_id: &str,
        hook_requires_approval: bool,
        audit_ref: Option<String>,
    ) -> Self {
        Self::ControlRequest {
            request_id: request_id.to_string(),
            request: CoreControlRequest::CanUseTool {
                tool_name: tool_name.to_string(),
                input,
                description: None,
                tool_use_id: tool_use_id.to_string(),
                hook_requires_approval,
                audit_ref,
            },
        }
    }

    pub fn auth_required(
        request_id: &str,
        reason: AuthReason,
        error_message: Option<String>,
        providers: Vec<AuthProvider>,
    ) -> Self {
        Self::ControlRequest {
            request_id: request_id.to_string(),
            request: CoreControlRequest::AuthRequired {
                reason,
                error_message,
                providers,
            },
        }
    }

    pub fn shell_evidence_list_commands(
        request_id: &str,
        tool_use_id: &str,
        limit: u16,
        cursor: Option<&str>,
    ) -> Self {
        Self::ControlRequest {
            request_id: request_id.to_string(),
            request: CoreControlRequest::ShellEvidence {
                tool_use_id: tool_use_id.to_string(),
                action: "list_commands".to_string(),
                limit: Some(limit),
                cursor: cursor.map(str::to_string),
                output_id: None,
                direction: None,
                lines: None,
                bypass_recent_filter: None,
            },
        }
    }

    pub fn shell_evidence_read_output(
        request_id: &str,
        tool_use_id: &str,
        output_id: &str,
        direction: &str,
        lines: u16,
        bypass_recent_filter: bool,
    ) -> Self {
        Self::ControlRequest {
            request_id: request_id.to_string(),
            request: CoreControlRequest::ShellEvidence {
                tool_use_id: tool_use_id.to_string(),
                action: "read_output".to_string(),
                limit: None,
                cursor: None,
                output_id: Some(output_id.to_string()),
                direction: Some(direction.to_string()),
                lines: Some(lines),
                bypass_recent_filter: bypass_recent_filter.then_some(true),
            },
        }
    }
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_user_message() {
        let json = r#"{"type":"user","message":{"role":"user","content":"hello world"},"parent_tool_use_id":null,"session_id":"default"}"#;
        let msg: InputMessage = serde_json::from_str(json).expect("should parse user message");
        match msg {
            InputMessage::User {
                message,
                session_id,
                ..
            } => {
                assert_eq!(message.role, "user");
                assert_eq!(message.content, "hello world");
                assert_eq!(session_id.as_deref(), Some("default"));
            }
            _ => panic!("expected User variant"),
        }
    }

    #[test]
    fn parse_initialize_request() {
        let json = r#"{"request_id":"init-1","type":"control_request","request":{"subtype":"initialize"}}"#;
        let msg: InputMessage = serde_json::from_str(json).expect("should parse initialize");
        match msg {
            InputMessage::ControlRequest {
                request_id,
                request,
            } => {
                assert_eq!(request_id, "init-1");
                assert!(matches!(request, ShellControlRequest::Initialize));
            }
            _ => panic!("expected ControlRequest variant"),
        }
    }

    #[test]
    fn parse_control_response_allow() {
        let json = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-1","response":{"behavior":"allow","updatedPermissions":[],"toolUseID":"toolu_abc"}}}"#;
        let msg: InputMessage = serde_json::from_str(json).expect("should parse control_response");
        match msg {
            InputMessage::ControlResponse { response } => {
                assert_eq!(response.request_id, "req-1");
                assert_eq!(response.response.behavior.as_deref(), Some("allow"));
                assert_eq!(response.response.tool_use_id.as_deref(), Some("toolu_abc"));
            }
            _ => panic!("expected ControlResponse variant"),
        }
    }

    #[test]
    fn parse_control_response_deny() {
        let json = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-2","response":{"behavior":"deny","message":"User denied"}}}"#;
        let msg: InputMessage = serde_json::from_str(json).unwrap();
        match msg {
            InputMessage::ControlResponse { response } => {
                assert_eq!(response.response.behavior.as_deref(), Some("deny"));
                assert_eq!(response.response.message.as_deref(), Some("User denied"));
            }
            _ => panic!("expected ControlResponse"),
        }
    }

    #[test]
    fn parse_control_response_host_executed_shell() {
        let json = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-3","response":{"behavior":"host_executed_shell","result":{"llmContent":"ShellCommandCompleted evidence\ncommand: df -h","returnDisplay":"df -h completed","metadata":{"command":"df -h","status":"completed","exit_code":0}}}}}"#;
        let msg: InputMessage = serde_json::from_str(json).unwrap();
        match msg {
            InputMessage::ControlResponse { response } => {
                assert_eq!(
                    response.response.behavior.as_deref(),
                    Some("host_executed_shell")
                );
                let result = response.response.result.expect("host result");
                assert!(result.llm_content.contains("ShellCommandCompleted"));
                assert_eq!(result.return_display.as_deref(), Some("df -h completed"));
                assert_eq!(
                    result
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("exit_code"))
                        .and_then(Value::as_i64),
                    Some(0)
                );
            }
            _ => panic!("expected ControlResponse"),
        }
    }

    #[test]
    fn parse_control_response_host_executed_shell_with_null_return_display() {
        let json = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-3","response":{"behavior":"host_executed_shell","result":{"llmContent":"ShellCommandCompleted evidence\ncommand: df -h","returnDisplay":null,"metadata":{"command":"df -h","status":"completed","exit_code":0}}}}}"#;
        let msg: InputMessage = serde_json::from_str(json).unwrap();
        match msg {
            InputMessage::ControlResponse { response } => {
                let result = response.response.result.expect("host result");
                assert!(result.llm_content.contains("ShellCommandCompleted"));
                assert_eq!(result.return_display, None);
                assert_eq!(
                    result
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("exit_code"))
                        .and_then(Value::as_i64),
                    Some(0)
                );
            }
            _ => panic!("expected ControlResponse"),
        }
    }

    #[test]
    fn serialize_system_init() {
        let msg = OutputMessage::system_init(
            "sess-1",
            "mock-model",
            vec!["shell".to_string(), "read_file".to_string()],
            true,
        );
        let json = serde_json::to_string(&msg).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "system");
        assert_eq!(v["subtype"], "init");
        assert_eq!(v["session_id"], "sess-1");
        assert_eq!(v["session_resumable"], true);
        assert_eq!(v["model"], "mock-model");
    }

    #[test]
    fn serialize_initialize_success_capabilities() {
        let msg = OutputMessage::initialize_success("init-1", false);
        let json = serde_json::to_string(&msg).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "control_response");
        assert_eq!(v["response"]["subtype"], "success");
        assert_eq!(v["response"]["request_id"], "init-1");
        assert_eq!(v["response"]["response"]["subtype"], "initialize");
        assert_eq!(
            v["response"]["response"]["capabilities"]["can_handle_can_use_tool"],
            true
        );
        assert_eq!(
            v["response"]["response"]["capabilities"]["can_handle_host_executed_shell_tool_result"],
            true
        );
        assert_eq!(
            v["response"]["response"]["capabilities"]["can_handle_shell_evidence_tool"],
            false
        );
        assert!(v["response"]["response"]["capabilities"]
            .get("can_handle_shell_output_evidence_tool")
            .is_none());
    }

    #[test]
    fn serialize_initialize_success_shell_evidence_capability() {
        let msg = OutputMessage::initialize_success("init-1", true);
        let json = serde_json::to_string(&msg).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v["response"]["response"]["capabilities"]["can_handle_shell_evidence_tool"],
            true
        );
    }

    #[test]
    fn serialize_assistant_text() {
        let msg = OutputMessage::assistant_text("sess-1", "Hello!");
        let json = serde_json::to_string(&msg).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "assistant");
        assert_eq!(v["session_id"], "sess-1");
        assert_eq!(v["message"]["content"][0]["type"], "text");
        assert_eq!(v["message"]["content"][0]["text"], "Hello!");
    }

    #[test]
    fn serialize_can_use_tool() {
        let msg = OutputMessage::can_use_tool(
            "req-1",
            "Bash",
            serde_json::json!({"command": "echo hello"}),
            "toolu_001",
            false,
        );
        let json = serde_json::to_string(&msg).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "control_request");
        assert_eq!(v["request_id"], "req-1");
        assert_eq!(v["request"]["subtype"], "can_use_tool");
        assert_eq!(v["request"]["tool_name"], "Bash");
        assert_eq!(v["request"]["input"]["command"], "echo hello");
        assert_eq!(v["request"]["tool_use_id"], "toolu_001");
    }

    #[test]
    fn serialize_shell_evidence_read_output() {
        let msg = OutputMessage::shell_evidence_read_output(
            "evidence-1",
            "toolu_abc",
            "terminal-output://raw-session-a1b2/cmd-1",
            "tail",
            120,
            true,
        );
        let json = serde_json::to_string(&msg).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "control_request");
        assert_eq!(v["request_id"], "evidence-1");
        assert_eq!(v["request"]["subtype"], "shell_evidence");
        assert_eq!(v["request"]["tool_use_id"], "toolu_abc");
        assert_eq!(v["request"]["action"], "read_output");
        assert_eq!(
            v["request"]["output_id"],
            "terminal-output://raw-session-a1b2/cmd-1"
        );
        assert_eq!(v["request"]["direction"], "tail");
        assert_eq!(v["request"]["lines"], 120);
        assert_eq!(v["request"]["bypass_recent_filter"], true);
    }

    #[test]
    fn serialize_shell_evidence_list_commands() {
        let msg = OutputMessage::shell_evidence_list_commands("evidence-1", "toolu_abc", 20, None);
        let json = serde_json::to_string(&msg).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "control_request");
        assert_eq!(v["request_id"], "evidence-1");
        assert_eq!(v["request"]["subtype"], "shell_evidence");
        assert_eq!(v["request"]["tool_use_id"], "toolu_abc");
        assert_eq!(v["request"]["action"], "list_commands");
        assert_eq!(v["request"]["limit"], 20);
        assert!(v["request"].get("output_id").is_none());
        assert!(v["request"].get("direction").is_none());
        assert!(v["request"].get("lines").is_none());
    }

    #[test]
    fn can_use_tool_parseable_by_cosh_shell_format() {
        // Verify our output matches the format cosh-shell's parse_control_request() expects
        let msg = OutputMessage::can_use_tool(
            "mock-req-001",
            "Bash",
            serde_json::json!({"command": "echo hello"}),
            "toolu_mock001",
            false,
        );
        let json = serde_json::to_string(&msg).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();

        // cosh-shell checks: v["type"] == "control_request"
        assert_eq!(v.get("type").unwrap().as_str().unwrap(), "control_request");
        // cosh-shell checks: v["request"]["subtype"]
        assert_eq!(
            v.get("request")
                .unwrap()
                .get("subtype")
                .unwrap()
                .as_str()
                .unwrap(),
            "can_use_tool"
        );
        // cosh-shell checks: v["request_id"]
        assert!(v.get("request_id").is_some());
        // cosh-shell checks: v["request"]["tool_name"]
        assert!(v.get("request").unwrap().get("tool_name").is_some());
        // cosh-shell checks: v["request"]["input"]
        assert!(v.get("request").unwrap().get("input").is_some());
        // cosh-shell checks: v["request"]["tool_use_id"]
        assert!(v.get("request").unwrap().get("tool_use_id").is_some());
    }

    #[test]
    fn can_use_tool_carries_real_optional_audit_reference() {
        let msg = OutputMessage::can_use_tool_with_audit_ref(
            "req-1",
            "Bash",
            serde_json::json!({"command": "echo ok"}),
            "toolu-1",
            false,
            Some("audit-event-1".to_string()),
        );
        let value = serde_json::to_value(msg).unwrap();
        assert_eq!(value["request"]["audit_ref"], "audit-event-1");
    }

    #[test]
    fn serialize_result_success() {
        let msg = OutputMessage::result_success("sess-1", "Done");
        let json = serde_json::to_string(&msg).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "result");
        assert_eq!(v["subtype"], "success");
        assert_eq!(v["is_error"], false);
        assert_eq!(v["result"], "Done");
        assert_eq!(v["session_id"], "sess-1");
    }

    #[test]
    fn serialize_result_error() {
        let msg = OutputMessage::result_error("sess-1", "cancelled");
        let json = serde_json::to_string(&msg).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "result");
        assert_eq!(v["is_error"], true);
        assert!(v.get("session_error_code").is_none());
    }

    #[test]
    fn serialize_structured_session_result_error() {
        let msg =
            OutputMessage::session_result_error("sess-1", "session missing", "not_found", "load");
        let json = serde_json::to_string(&msg).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "result");
        assert_eq!(v["is_error"], true);
        assert_eq!(v["session_error_code"], "not_found");
        assert_eq!(v["session_error_phase"], "load");
    }

    #[test]
    fn serialize_stream_text_delta() {
        let msg = OutputMessage::stream_text_delta(0, "hello");
        let json = serde_json::to_string(&msg).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "stream_event");
        assert_eq!(v["event"]["type"], "content_block_delta");
        assert_eq!(v["event"]["index"], 0);
        assert_eq!(v["event"]["delta"]["type"], "text_delta");
        assert_eq!(v["event"]["delta"]["text"], "hello");
    }

    #[test]
    fn serialize_stream_tool_use_start() {
        let msg = OutputMessage::stream_tool_use_start(1, "call_1", "shell");
        let json = serde_json::to_string(&msg).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "stream_event");
        assert_eq!(v["event"]["type"], "content_block_start");
        assert_eq!(v["event"]["index"], 1);
        assert_eq!(v["event"]["content_block"]["type"], "tool_use");
        assert_eq!(v["event"]["content_block"]["id"], "call_1");
        assert_eq!(v["event"]["content_block"]["name"], "shell");
    }

    #[test]
    fn serialize_stream_message_start_stop() {
        let start = OutputMessage::stream_message_start();
        let json = serde_json::to_string(&start).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "stream_event");
        assert_eq!(v["event"]["type"], "message_start");

        let stop = OutputMessage::stream_message_stop();
        let json = serde_json::to_string(&stop).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["event"]["type"], "message_stop");
    }

    #[test]
    fn stream_text_delta_matches_cosh_shell_parser_path() {
        // ClaudeStreamParser extracts text from: value.pointer("/event/delta/text")
        let msg = OutputMessage::stream_text_delta(0, "check...");
        let json = serde_json::to_string(&msg).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        let text = v.pointer("/event/delta/text").and_then(|t| t.as_str());
        assert_eq!(text, Some("check..."));
    }

    #[test]
    fn parse_interrupt_request() {
        let json =
            r#"{"type":"control_request","request_id":"int-1","request":{"subtype":"interrupt"}}"#;
        let msg: InputMessage = serde_json::from_str(json).unwrap();
        match msg {
            InputMessage::ControlRequest { request, .. } => {
                assert!(matches!(request, ShellControlRequest::Interrupt));
            }
            _ => panic!("expected ControlRequest"),
        }
    }

    #[test]
    fn parse_shutdown_request() {
        let json =
            r#"{"type":"control_request","request_id":"shut-1","request":{"subtype":"shutdown"}}"#;
        let msg: InputMessage = serde_json::from_str(json).unwrap();
        match msg {
            InputMessage::ControlRequest { request, .. } => {
                assert!(matches!(request, ShellControlRequest::Shutdown));
            }
            _ => panic!("expected ControlRequest"),
        }
    }

    #[test]
    fn serialize_stream_thinking_delta() {
        let msg = OutputMessage::stream_thinking_delta(0, "Let me think...");
        let json = serde_json::to_string(&msg).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "stream_event");
        assert_eq!(v["event"]["type"], "content_block_delta");
        assert_eq!(v["event"]["index"], 0);
        assert_eq!(v["event"]["delta"]["type"], "thinking_delta");
        assert_eq!(v["event"]["delta"]["thinking"], "Let me think...");
    }

    #[test]
    fn thinking_delta_matches_cosh_shell_parser_path() {
        let msg = OutputMessage::stream_thinking_delta(0, "reasoning...");
        let json = serde_json::to_string(&msg).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        let thinking = v.pointer("/event/delta/thinking").and_then(|t| t.as_str());
        assert_eq!(thinking, Some("reasoning..."));
    }

    #[test]
    fn serialize_stream_thinking_start() {
        let msg = OutputMessage::stream_thinking_start(0);
        let json = serde_json::to_string(&msg).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["event"]["type"], "content_block_start");
        assert_eq!(v["event"]["content_block"]["type"], "thinking");
    }
}
