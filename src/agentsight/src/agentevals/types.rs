use serde::{Deserialize, Serialize};

// Re-export genai types (using renamed crate to avoid clash with internal genai module)
pub use genai_crate::chat::{ChatMessage, ChatRole, ContentPart, MessageContent, ToolCall};
pub use genai_crate::Client;

/// Evaluation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluatorResult {
    pub key: String,
    pub score: Score,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// Score type (boolean or continuous)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Score {
    Boolean(bool),
    Float(f64),
}

impl Score {
    pub fn as_bool(&self) -> bool {
        match self {
            Score::Boolean(b) => *b,
            Score::Float(f) => *f > 0.5,
        }
    }

    pub fn as_f64(&self) -> f64 {
        match self {
            Score::Boolean(b) => if *b { 1.0 } else { 0.0 },
            Score::Float(f) => *f,
        }
    }
}

/// Few-shot example
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FewShotExample {
    pub inputs: Option<String>,
    pub outputs: String,
    pub reasoning: String,
    pub score: Score,
}

/// Create a user message
pub fn user_message(content: impl Into<MessageContent>) -> ChatMessage {
    ChatMessage::user(content)
}

/// Create an assistant message
pub fn assistant_message(content: impl Into<MessageContent>) -> ChatMessage {
    ChatMessage::assistant(content)
}

/// Create an assistant message with tool calls
pub fn assistant_with_tools(tool_calls: Vec<ToolCall>) -> ChatMessage {
    ChatMessage::from(tool_calls)
}

/// Create a tool response message
pub fn tool_response(tool_call_id: impl Into<String>, content: impl Into<String>) -> ChatMessage {
    use genai_crate::chat::ToolResponse;
    ChatMessage::from(ToolResponse::new(tool_call_id, content))
}

/// Create a tool call
pub fn create_tool_call(name: impl Into<String>, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        call_id: "call_001".to_string(),
        fn_name: name.into(),
        fn_arguments: arguments,
        thought_signatures: None,
    }
}

/// Format a ChatMessage to string (for LLM prompt)
pub fn format_message(msg: &ChatMessage) -> String {
    let role_label = match msg.role {
        ChatRole::User => "User",
        ChatRole::Assistant => "Assistant",
        ChatRole::System => "System",
        ChatRole::Tool => "Tool",
    };

    let mut result = format!("{}: ", role_label);

    for part in &msg.content {
        match part {
            genai_crate::chat::ContentPart::Text(text) => {
                result.push_str(text);
            }
            genai_crate::chat::ContentPart::ToolCall(tool_call) => {
                result.push_str(&format!(
                    "\n  [Tool Call] {}({})",
                    tool_call.fn_name,
                    tool_call.fn_arguments
                ));
            }
            _ => {}
        }
    }

    result
}

/// Format a list of messages to string
pub fn format_messages_to_string(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .map(format_message)
        .collect::<Vec<_>>()
        .join("\n\n")
}

