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

#[derive(Debug, Clone, Serialize)]
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
    Error(String),
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
