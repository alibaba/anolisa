//! Token usage record for database storage

use serde::{Deserialize, Serialize};

/// Token usage record stored in the database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRecord {
    /// Unique record ID
    pub id: i64,
    /// Timestamp in nanoseconds since Unix epoch
    pub timestamp_ns: u64,
    /// Process ID that made the request
    pub pid: u32,
    /// Process command name
    pub comm: String,
    /// Agent name (if identifiable)
    pub agent: Option<String>,
    /// Model used
    pub model: Option<String>,
    /// LLM provider (openai, anthropic, etc.)
    pub provider: String,
    /// Input tokens count
    pub input_tokens: u64,
    /// Output tokens count
    pub output_tokens: u64,
    /// Cache creation input tokens (if applicable)
    pub cache_creation_tokens: Option<u64>,
    /// Cache read input tokens (if applicable)
    pub cache_read_tokens: Option<u64>,
    /// Request ID (for correlation)
    pub request_id: Option<String>,
    /// API endpoint
    pub endpoint: Option<String>,
    /// Tool calls extracted from SSE response (JSON strings)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<String>,
    /// Reasoning content extracted from SSE response
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

impl TokenRecord {
    /// Total tokens (input + output)
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// Create a new record with current timestamp
    pub fn new(
        pid: u32,
        comm: String,
        provider: String,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};

        let timestamp_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        TokenRecord {
            id: 0, // Will be assigned by database
            timestamp_ns,
            pid,
            comm,
            agent: None,
            model: None,
            provider,
            input_tokens,
            output_tokens,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            request_id: None,
            endpoint: None,
            tool_calls: Vec::new(),
            reasoning_content: None,
        }
    }

    /// Set agent name
    pub fn with_agent(mut self, agent: impl Into<String>) -> Self {
        self.agent = Some(agent.into());
        self
    }

    /// Set model name
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set cache tokens
    pub fn with_cache_tokens(mut self, creation: u64, read: u64) -> Self {
        self.cache_creation_tokens = Some(creation);
        self.cache_read_tokens = Some(read);
        self
    }

    /// Set request ID
    pub fn with_request_id(mut self, id: impl Into<String>) -> Self {
        self.request_id = Some(id.into());
        self
    }

    /// Set endpoint
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_record_total() {
        let record = TokenRecord::new(1234, "python".to_string(), "openai".to_string(), 100, 50);
        assert_eq!(record.total_tokens(), 150);
    }

    #[test]
    fn test_token_record_builder() {
        let record = TokenRecord::new(1234, "python".to_string(), "anthropic".to_string(), 100, 50)
            .with_agent("OpenClaw")
            .with_model("claude-3-opus")
            .with_cache_tokens(10, 20);

        assert_eq!(record.agent, Some("OpenClaw".to_string()));
        assert_eq!(record.model, Some("claude-3-opus".to_string()));
        assert_eq!(record.cache_creation_tokens, Some(10));
        assert_eq!(record.cache_read_tokens, Some(20));
    }

    #[test]
    fn test_token_record_new_defaults() {
        let record = TokenRecord::new(999, "node".to_string(), "openai".to_string(), 200, 100);
        assert_eq!(record.id, 0);
        assert!(record.timestamp_ns > 0);
        assert_eq!(record.pid, 999);
        assert_eq!(record.comm, "node");
        assert!(record.agent.is_none());
        assert!(record.model.is_none());
        assert_eq!(record.provider, "openai");
        assert_eq!(record.input_tokens, 200);
        assert_eq!(record.output_tokens, 100);
        assert!(record.cache_creation_tokens.is_none());
        assert!(record.cache_read_tokens.is_none());
        assert!(record.request_id.is_none());
        assert!(record.endpoint.is_none());
        assert!(record.tool_calls.is_empty());
        assert!(record.reasoning_content.is_none());
    }

    #[test]
    fn test_with_request_id() {
        let record = TokenRecord::new(1, "p".to_string(), "o".to_string(), 0, 0)
            .with_request_id("req-123");
        assert_eq!(record.request_id, Some("req-123".to_string()));
    }

    #[test]
    fn test_with_endpoint() {
        let record = TokenRecord::new(1, "p".to_string(), "o".to_string(), 0, 0)
            .with_endpoint("/v1/chat/completions");
        assert_eq!(record.endpoint, Some("/v1/chat/completions".to_string()));
    }

    #[test]
    fn test_serde_roundtrip() {
        let record = TokenRecord::new(42, "agent".to_string(), "anthropic".to_string(), 500, 200)
            .with_agent("Claude Code")
            .with_model("claude-3")
            .with_cache_tokens(50, 100)
            .with_request_id("req-abc")
            .with_endpoint("/v1/messages");
        let json = serde_json::to_string(&record).unwrap();
        let back: TokenRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pid, 42);
        assert_eq!(back.comm, "agent");
        assert_eq!(back.provider, "anthropic");
        assert_eq!(back.input_tokens, 500);
        assert_eq!(back.output_tokens, 200);
        assert_eq!(back.total_tokens(), 700);
        assert_eq!(back.agent, Some("Claude Code".to_string()));
        assert_eq!(back.model, Some("claude-3".to_string()));
    }

    #[test]
    fn test_serde_skip_empty_tool_calls() {
        let record = TokenRecord::new(1, "p".to_string(), "o".to_string(), 0, 0);
        let json = serde_json::to_string(&record).unwrap();
        assert!(!json.contains("tool_calls"));
        assert!(!json.contains("reasoning_content"));
    }
}
