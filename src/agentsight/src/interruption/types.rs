//! Interruption event types, severity levels, and core data structures.

use serde::{Deserialize, Serialize};

/// The type of conversation interruption detected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptionType {
    /// HTTP status_code >= 400, or SSE body contains {"error": ...}
    LlmError,
    /// SSE stream ended without receiving finish_reason=stop/tool_calls ([DONE])
    SseTruncated,
    /// Agent process disappeared mid-session (detected by HealthChecker)
    AgentCrash,
    /// finish_reason == "length" and output_tokens >= max_tokens * ratio
    TokenLimit,
    /// finish_reason == "content_filter" or error contains context_length_exceeded
    ContextOverflow,
}

impl InterruptionType {
    /// String identifier stored in the database
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LlmError        => "llm_error",
            Self::SseTruncated    => "sse_truncated",
            Self::AgentCrash      => "agent_crash",
            Self::TokenLimit      => "token_limit",
            Self::ContextOverflow => "context_overflow",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "llm_error"        => Some(Self::LlmError),
            "sse_truncated"    => Some(Self::SseTruncated),
            "agent_crash"      => Some(Self::AgentCrash),
            "token_limit"      => Some(Self::TokenLimit),
            "context_overflow" => Some(Self::ContextOverflow),
            _ => None,
        }
    }

    /// Default severity for this interruption type
    pub fn default_severity(&self) -> Severity {
        match self {
            Self::AgentCrash      => Severity::Critical,
            Self::LlmError        => Severity::High,
            Self::SseTruncated    => Severity::High,
            Self::ContextOverflow => Severity::High,
            Self::TokenLimit      => Severity::Medium,
        }
    }
}

/// Severity of the interruption
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High     => "high",
            Self::Medium   => "medium",
            Self::Low      => "low",
        }
    }

    /// Numeric weight for comparison (higher = worse)
    pub fn weight(&self) -> u8 {
        match self {
            Self::Critical => 4,
            Self::High     => 3,
            Self::Medium   => 2,
            Self::Low      => 1,
        }
    }
}

/// A single detected interruption event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptionEvent {
    /// Unique identifier (UUID v4 hex, 32 chars)
    pub interruption_id: String,
    pub session_id: Option<String>,
    pub trace_id: Option<String>,
    pub call_id: Option<String>,
    pub pid: Option<i32>,
    pub agent_name: Option<String>,
    pub interruption_type: InterruptionType,
    pub severity: Severity,
    /// Occurrence timestamp (nanoseconds since Unix epoch)
    pub occurred_at_ns: i64,
    /// JSON-encoded detail (model, error message, finish_reason, etc.)
    pub detail: Option<String>,
    /// Whether the event has been acknowledged / resolved
    pub resolved: bool,
}

impl InterruptionEvent {
    /// Create a new unresolved interruption event with auto-generated ID
    pub fn new(
        itype: InterruptionType,
        session_id: Option<String>,
        trace_id: Option<String>,
        call_id: Option<String>,
        pid: Option<i32>,
        agent_name: Option<String>,
        occurred_at_ns: i64,
        detail: Option<serde_json::Value>,
    ) -> Self {
        let severity = itype.default_severity();
        InterruptionEvent {
            interruption_id: new_id(),
            session_id,
            trace_id,
            call_id,
            pid,
            agent_name,
            interruption_type: itype,
            severity,
            occurred_at_ns,
            detail: detail.map(|v| v.to_string()),
            resolved: false,
        }
    }
}

/// Generate a 32-char hex ID (uses current timestamp + random bytes)
fn new_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Mix with a pseudo-random value derived from address of a stack var
    let stack_var: u64 = 0;
    let addr = &stack_var as *const u64 as u64;
    format!("{:016x}{:016x}", ns as u64 ^ addr, ns as u64)
}
