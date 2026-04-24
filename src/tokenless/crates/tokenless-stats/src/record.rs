//! Statistics record definitions for tokenless.
//!
//! Each record represents a single compression or rewriting operation
//! with before/after metrics and optional text content for diff export.

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Type of operation performed (three compression types)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum OperationType {
    /// Schema compression (BeforeModel hook)
    CompressSchema,
    /// Response compression (PostToolUse hook)
    CompressResponse,
    /// Command rewriting (RTK, PreToolUse hook)
    RewriteCommand,
}

impl OperationType {
    pub fn as_str(&self) -> &str {
        match self {
            OperationType::CompressSchema => "compress-schema",
            OperationType::CompressResponse => "compress-response",
            OperationType::RewriteCommand => "rewrite-command",
        }
    }
}

impl FromStr for OperationType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "compress-schema" => OperationType::CompressSchema,
            "compress-response" => OperationType::CompressResponse,
            "rewrite-command" => OperationType::RewriteCommand,
            _ => OperationType::CompressSchema,
        })
    }
}

/// A single statistics record stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsRecord {
    /// Database record ID (auto-increment primary key)
    pub id: i64,
    /// Timestamp when the record was created
    pub timestamp: DateTime<Local>,
    /// Type of operation (compress-schema, compress-response, rewrite-command)
    pub operation: OperationType,
    /// Agent identifier (e.g., "copilot-shell")
    pub agent_id: String,
    /// Source process ID (optional)
    pub source_pid: Option<i64>,
    /// Session ID for grouping related operations
    pub session_id: Option<String>,
    /// Tool use ID for correlation with specific tool calls
    pub tool_use_id: Option<String>,
    /// Characters before compression
    pub before_chars: usize,
    /// Tokens before compression (estimated)
    pub before_tokens: usize,
    /// Characters after compression
    pub after_chars: usize,
    /// Tokens after compression (estimated)
    pub after_tokens: usize,
    /// Text content before compression
    pub before_text: Option<String>,
    /// Text content after compression
    pub after_text: Option<String>,
}

impl StatsRecord {
    /// Create a new stats record
    pub fn new(
        operation: OperationType,
        agent_id: String,
        before_chars: usize,
        before_tokens: usize,
        after_chars: usize,
        after_tokens: usize,
    ) -> Self {
        Self {
            id: -1,
            timestamp: Local::now(),
            operation,
            agent_id,
            source_pid: None,
            session_id: None,
            tool_use_id: None,
            before_chars,
            before_tokens,
            after_chars,
            after_tokens,
            before_text: None,
            after_text: None,
        }
    }

    /// Set the session ID
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Set the tool use ID
    pub fn with_tool_use_id(mut self, tool_use_id: impl Into<String>) -> Self {
        self.tool_use_id = Some(tool_use_id.into());
        self
    }

    /// Set the source PID
    pub fn with_source_pid(mut self, pid: i64) -> Self {
        self.source_pid = Some(pid);
        self
    }

    /// Set text content before compression
    pub fn with_before_text(mut self, text: String) -> Self {
        self.before_text = Some(text);
        self
    }

    /// Set text content after compression
    pub fn with_after_text(mut self, text: String) -> Self {
        self.after_text = Some(text);
        self
    }

    /// Characters saved by compression
    pub fn chars_saved(&self) -> usize {
        self.before_chars.saturating_sub(self.after_chars)
    }

    /// Tokens saved by compression
    pub fn tokens_saved(&self) -> usize {
        self.before_tokens.saturating_sub(self.after_tokens)
    }

    /// Characters saved percentage
    pub fn chars_percent(&self) -> f64 {
        if self.before_chars > 0 {
            (self.chars_saved() as f64 / self.before_chars as f64) * 100.0
        } else {
            0.0
        }
    }

    /// Tokens saved percentage
    pub fn tokens_percent(&self) -> f64 {
        if self.before_tokens > 0 {
            (self.tokens_saved() as f64 / self.before_tokens as f64) * 100.0
        } else {
            0.0
        }
    }

    /// Get a formatted summary line for list output
    pub fn format_summary_line(&self) -> String {
        let session = self.session_id.as_deref().unwrap_or("-");
        let tool = self.tool_use_id.as_deref().unwrap_or("-");

        format!(
            "[ID:{}] {} | {} | Session:{} | Tool:{} | Chars:{}→{}(-{}) | Tokens:{}→{}(-{:.0}%)",
            self.id,
            self.timestamp.format("%Y-%m-%d %H:%M:%S"),
            self.agent_id,
            session,
            tool,
            self.before_chars,
            self.after_chars,
            self.chars_saved(),
            self.before_tokens,
            self.after_tokens,
            self.tokens_percent(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operation_type_from_str() {
        assert_eq!(
            "compress-schema".parse::<OperationType>().unwrap(),
            OperationType::CompressSchema
        );
        assert_eq!(
            "compress-response".parse::<OperationType>().unwrap(),
            OperationType::CompressResponse
        );
        assert_eq!(
            "rewrite-command".parse::<OperationType>().unwrap(),
            OperationType::RewriteCommand
        );
    }

    #[test]
    fn test_savings_calculation() {
        let record = StatsRecord::new(
            OperationType::CompressSchema,
            "copilot-shell".to_string(),
            1000,
            400,
            500,
            200,
        );

        assert_eq!(record.chars_saved(), 500);
        assert_eq!(record.tokens_saved(), 200);
        assert!((record.chars_percent() - 50.0).abs() < 0.1);
        assert!((record.tokens_percent() - 50.0).abs() < 0.1);
    }

    #[test]
    fn test_record_with_diff_text() {
        let record = StatsRecord::new(
            OperationType::CompressSchema,
            "copilot-shell".to_string(),
            16,
            4,
            10,
            3,
        )
        .with_before_text("original text here".to_string())
        .with_after_text("compressed".to_string());

        assert!(record.before_text.is_some());
        assert!(record.after_text.is_some());
    }

    #[test]
    fn test_format_summary_line() {
        let record = StatsRecord::new(
            OperationType::CompressSchema,
            "copilot-shell".to_string(),
            1000,
            400,
            500,
            200,
        )
        .with_session_id("session-123")
        .with_tool_use_id("call_abc");

        let line = record.format_summary_line();
        assert!(line.contains("[ID:-1]"));
        assert!(line.contains("copilot-shell"));
        assert!(line.contains("Session:session-123"));
        assert!(line.contains("Tool:call_abc"));
    }

    #[test]
    fn test_format_summary_line_with_pid() {
        let record = StatsRecord::new(
            OperationType::CompressSchema,
            "copilot-shell(12345)".to_string(),
            1000,
            400,
            500,
            200,
        )
        .with_session_id("session-123");

        let line = record.format_summary_line();
        assert!(line.contains("copilot-shell(12345)"));
        assert!(
            !line.contains("pid:"),
            "PID should be inline in agent_id, not separate"
        );
    }
}
