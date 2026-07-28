pub mod mock;
pub mod openai_compat;
pub mod profile;
pub mod sysom;

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: MessageContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallInfo>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallInfo {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<MessageContentBlock>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MessageContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

impl MessageContent {
    /// Extract the full text content, joining blocks if necessary.
    pub fn as_text(&self) -> String {
        match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .map(|b| match b {
                    MessageContentBlock::Text { text } => text.as_str(),
                    MessageContentBlock::ToolResult { content, .. } => content.as_str(),
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

impl Message {
    pub fn user(content: &str) -> Self {
        Self {
            role: "user".to_string(),
            content: MessageContent::Text(content.to_string()),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }
    }

    pub fn assistant(content: &str) -> Self {
        Self {
            role: "assistant".to_string(),
            content: MessageContent::Text(content.to_string()),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }
    }

    pub fn assistant_with_tool_calls(content: &str, tool_calls: Vec<ToolCallInfo>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: MessageContent::Text(content.to_string()),
            tool_call_id: None,
            name: None,
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
        }
    }

    pub fn system(content: &str) -> Self {
        Self {
            role: "system".to_string(),
            content: MessageContent::Text(content.to_string()),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }
    }

    pub fn tool_result(tool_call_id: &str, content: &str, _is_error: bool) -> Self {
        Self {
            role: "tool".to_string(),
            content: MessageContent::Text(content.to_string()),
            tool_call_id: Some(tool_call_id.to_string()),
            name: None,
            tool_calls: None,
        }
    }
}

#[cfg(test)]
mod message_tests {
    use super::{Message, ToolCallFunction, ToolCallInfo};

    #[test]
    fn constructors_preserve_runtime_content() {
        let secret = "api_key=sk-runtime-secret-value";
        assert_eq!(Message::user(secret).content.as_text(), secret);
        assert_eq!(Message::assistant(secret).content.as_text(), secret);
        assert_eq!(Message::system(secret).content.as_text(), secret);
        assert_eq!(
            Message::tool_result("tool-1", secret, false)
                .content
                .as_text(),
            secret
        );

        let message = Message::assistant_with_tool_calls(
            secret,
            vec![ToolCallInfo {
                id: "tool-1".to_string(),
                call_type: "function".to_string(),
                function: ToolCallFunction {
                    name: "write_file".to_string(),
                    arguments: format!(r#"{{"content":"{secret}"}}"#),
                },
            }],
        );
        assert_eq!(message.content.as_text(), secret);
        assert!(message.tool_calls.unwrap()[0]
            .function
            .arguments
            .contains(secret));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDeclaration {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone)]
pub struct GenerateConfig {
    pub model: String,
    pub max_tokens: u32,
    pub temperature: Option<f64>,
    pub include_usage: bool,
    pub extra_params: Option<serde_json::Value>,
}

impl Default for GenerateConfig {
    fn default() -> Self {
        Self {
            model: "mock".to_string(),
            max_tokens: 4096,
            temperature: None,
            include_usage: false,
            extra_params: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum GenerateEvent {
    TextDelta(String),
    ToolCallStart {
        index: u32,
        id: String,
        name: String,
    },
    ToolCallDelta {
        index: u32,
        arguments_delta: String,
    },
    ToolCallEnd {
        index: u32,
    },
    ThinkingDelta(String),
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: u32,
    },
    MessageEnd,
    Cancelled,
    Error(String),
}

/// Highest tool-call index a provider may report within one message.
///
/// Consumers size per-call state by index (`Vec` slots keyed by position), so an
/// unbounded index turns one malformed frame into a multi-billion-entry
/// allocation. No real turn issues more than a few dozen parallel calls, so
/// anything past this limit is a protocol violation rather than a large message.
pub const MAX_TOOL_CALL_INDEX: u32 = 127;

impl GenerateEvent {
    /// The tool-call index this event addresses, if it addresses one.
    ///
    /// Lets a consumer bound-check every index-bearing event in one place
    /// instead of repeating the guard in each match arm.
    pub fn tool_call_index(&self) -> Option<u32> {
        match self {
            Self::ToolCallStart { index, .. }
            | Self::ToolCallDelta { index, .. }
            | Self::ToolCallEnd { index } => Some(*index),
            _ => None,
        }
    }
}

pub type GenerateStream = Pin<Box<dyn Stream<Item = GenerateEvent> + Send>>;

#[async_trait]
pub trait ContentGenerator: Send + Sync {
    async fn generate(
        &self,
        messages: &[Message],
        tools: &[ToolDeclaration],
        config: &GenerateConfig,
    ) -> Result<GenerateStream, String>;

    fn cancel(&self);
}
