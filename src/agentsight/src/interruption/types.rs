//! Interruption event types, severity levels, and core data structures.

use serde::{Deserialize, Serialize};

/// The type of conversation interruption detected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptionType {
    /// Agent process disappeared mid-session (detected by HealthChecker)
    AgentCrash,
    /// HTTP 429 or error containing "rate_limit"
    RateLimit,
    /// HTTP 401/403 or error containing "invalid_api_key" / "unauthorized"
    AuthError,
    /// HTTP 408/504 or error containing "timeout" (gateway-level only)
    NetworkTimeout,
    /// HTTP 502/503 or error containing "overloaded" / "service_unavailable"
    ServiceUnavailable,
    /// finish_reason == "content_filter" from LLM safety policy
    SafetyFilter,
    /// SSE stream ended without finish_reason=stop/tool_calls ([DONE])
    SseTruncated,
    /// context_length_exceeded or similar context-bound errors
    ContextOverflow,
    /// finish_reason == "length" and output tokens exceed threshold
    TokenLimit,
    /// HTTP status_code >= 400 的通用兜底（优先级最低，在所有特定类型之后）
    LlmError,
    /// Same error type repeated > threshold times in one conversation (agent stuck retrying)
    RetryStorm,
    /// Agent stuck in a logical loop: repeated tool sequences or similar LLM outputs
    /// without meaningful progress (no errors, just looping behavior)
    DeadLoop,
}

impl InterruptionType {
    /// String identifier stored in the database
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AgentCrash => "agent_crash",
            Self::RateLimit => "rate_limit",
            Self::AuthError => "auth_error",
            Self::NetworkTimeout => "network_timeout",
            Self::ServiceUnavailable => "service_unavailable",
            Self::SafetyFilter => "safety_filter",
            Self::SseTruncated => "sse_truncated",
            Self::ContextOverflow => "context_overflow",
            Self::TokenLimit => "token_limit",
            Self::LlmError => "llm_error",
            Self::RetryStorm => "retry_storm",
            Self::DeadLoop => "dead_loop",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "agent_crash" => Some(Self::AgentCrash),
            "rate_limit" => Some(Self::RateLimit),
            "auth_error" => Some(Self::AuthError),
            "network_timeout" => Some(Self::NetworkTimeout),
            "service_unavailable" => Some(Self::ServiceUnavailable),
            "safety_filter" => Some(Self::SafetyFilter),
            "sse_truncated" => Some(Self::SseTruncated),
            "context_overflow" => Some(Self::ContextOverflow),
            "token_limit" => Some(Self::TokenLimit),
            "llm_error" => Some(Self::LlmError),
            "retry_storm" => Some(Self::RetryStorm),
            "dead_loop" => Some(Self::DeadLoop),
            _ => None,
        }
    }

    /// Default severity for this interruption type
    pub fn default_severity(&self) -> Severity {
        match self {
            Self::AgentCrash => Severity::Critical,
            Self::RateLimit => Severity::Medium,
            Self::AuthError => Severity::High,
            Self::NetworkTimeout => Severity::High,
            Self::ServiceUnavailable => Severity::High,
            Self::SafetyFilter => Severity::Medium,
            Self::SseTruncated => Severity::High,
            Self::ContextOverflow => Severity::High,
            Self::TokenLimit => Severity::Medium,
            Self::LlmError => Severity::High,
            Self::RetryStorm => Severity::Critical,
            Self::DeadLoop => Severity::Critical,
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
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }

    /// Numeric weight for comparison (higher = worse)
    pub fn weight(&self) -> u8 {
        match self {
            Self::Critical => 4,
            Self::High => 3,
            Self::Medium => 2,
            Self::Low => 1,
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
    pub conversation_id: Option<String>,
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
        conversation_id: Option<String>,
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
            conversation_id,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interruption_type_as_str() {
        assert_eq!(InterruptionType::AgentCrash.as_str(), "agent_crash");
        assert_eq!(InterruptionType::RateLimit.as_str(), "rate_limit");
        assert_eq!(InterruptionType::AuthError.as_str(), "auth_error");
        assert_eq!(InterruptionType::NetworkTimeout.as_str(), "network_timeout");
        assert_eq!(
            InterruptionType::ServiceUnavailable.as_str(),
            "service_unavailable"
        );
        assert_eq!(InterruptionType::SafetyFilter.as_str(), "safety_filter");
        assert_eq!(InterruptionType::SseTruncated.as_str(), "sse_truncated");
        assert_eq!(
            InterruptionType::ContextOverflow.as_str(),
            "context_overflow"
        );
        assert_eq!(InterruptionType::TokenLimit.as_str(), "token_limit");
        assert_eq!(InterruptionType::LlmError.as_str(), "llm_error");
        assert_eq!(InterruptionType::RetryStorm.as_str(), "retry_storm");
        assert_eq!(InterruptionType::DeadLoop.as_str(), "dead_loop");
    }

    #[test]
    fn test_interruption_type_from_str() {
        assert_eq!(
            InterruptionType::from_str("agent_crash"),
            Some(InterruptionType::AgentCrash)
        );
        assert_eq!(
            InterruptionType::from_str("rate_limit"),
            Some(InterruptionType::RateLimit)
        );
        assert_eq!(
            InterruptionType::from_str("auth_error"),
            Some(InterruptionType::AuthError)
        );
        assert_eq!(
            InterruptionType::from_str("network_timeout"),
            Some(InterruptionType::NetworkTimeout)
        );
        assert_eq!(
            InterruptionType::from_str("service_unavailable"),
            Some(InterruptionType::ServiceUnavailable)
        );
        assert_eq!(
            InterruptionType::from_str("safety_filter"),
            Some(InterruptionType::SafetyFilter)
        );
        assert_eq!(
            InterruptionType::from_str("sse_truncated"),
            Some(InterruptionType::SseTruncated)
        );
        assert_eq!(
            InterruptionType::from_str("context_overflow"),
            Some(InterruptionType::ContextOverflow)
        );
        assert_eq!(
            InterruptionType::from_str("token_limit"),
            Some(InterruptionType::TokenLimit)
        );
        assert_eq!(
            InterruptionType::from_str("llm_error"),
            Some(InterruptionType::LlmError)
        );
        assert_eq!(
            InterruptionType::from_str("retry_storm"),
            Some(InterruptionType::RetryStorm)
        );
        assert_eq!(
            InterruptionType::from_str("dead_loop"),
            Some(InterruptionType::DeadLoop)
        );
        assert_eq!(InterruptionType::from_str("unknown"), None);
        assert_eq!(InterruptionType::from_str(""), None);
    }

    #[test]
    fn test_interruption_type_default_severity() {
        assert_eq!(
            InterruptionType::AgentCrash.default_severity(),
            Severity::Critical
        );
        assert_eq!(
            InterruptionType::RateLimit.default_severity(),
            Severity::Medium
        );
        assert_eq!(
            InterruptionType::AuthError.default_severity(),
            Severity::High
        );
        assert_eq!(
            InterruptionType::NetworkTimeout.default_severity(),
            Severity::High
        );
        assert_eq!(
            InterruptionType::ServiceUnavailable.default_severity(),
            Severity::High
        );
        assert_eq!(
            InterruptionType::SafetyFilter.default_severity(),
            Severity::Medium
        );
        assert_eq!(
            InterruptionType::SseTruncated.default_severity(),
            Severity::High
        );
        assert_eq!(
            InterruptionType::ContextOverflow.default_severity(),
            Severity::High
        );
        assert_eq!(
            InterruptionType::TokenLimit.default_severity(),
            Severity::Medium
        );
        assert_eq!(
            InterruptionType::LlmError.default_severity(),
            Severity::High
        );
        assert_eq!(
            InterruptionType::RetryStorm.default_severity(),
            Severity::Critical
        );
        assert_eq!(
            InterruptionType::DeadLoop.default_severity(),
            Severity::Critical
        );
    }

    #[test]
    fn test_severity_as_str() {
        assert_eq!(Severity::Critical.as_str(), "critical");
        assert_eq!(Severity::High.as_str(), "high");
        assert_eq!(Severity::Medium.as_str(), "medium");
        assert_eq!(Severity::Low.as_str(), "low");
    }

    #[test]
    fn test_severity_weight_ordering() {
        assert!(Severity::Critical.weight() > Severity::High.weight());
        assert!(Severity::High.weight() > Severity::Medium.weight());
        assert!(Severity::Medium.weight() > Severity::Low.weight());
    }

    #[test]
    fn test_severity_weight_values() {
        assert_eq!(Severity::Critical.weight(), 4);
        assert_eq!(Severity::High.weight(), 3);
        assert_eq!(Severity::Medium.weight(), 2);
        assert_eq!(Severity::Low.weight(), 1);
    }

    #[test]
    fn test_interruption_event_new() {
        let event = InterruptionEvent::new(
            InterruptionType::LlmError,
            Some("session-1".to_string()),
            Some("trace-1".to_string()),
            Some("conv-1".to_string()),
            Some("call-1".to_string()),
            Some(1234),
            Some("my-agent".to_string()),
            1_000_000_000,
            Some(serde_json::json!({"status_code": 500})),
        );
        assert_eq!(event.interruption_type, InterruptionType::LlmError);
        assert_eq!(event.severity, Severity::High);
        assert_eq!(event.session_id, Some("session-1".to_string()));
        assert_eq!(event.trace_id, Some("trace-1".to_string()));
        assert_eq!(event.conversation_id, Some("conv-1".to_string()));
        assert_eq!(event.call_id, Some("call-1".to_string()));
        assert_eq!(event.pid, Some(1234));
        assert_eq!(event.agent_name, Some("my-agent".to_string()));
        assert_eq!(event.occurred_at_ns, 1_000_000_000);
        assert!(!event.resolved);
        assert!(event.detail.is_some());
        assert_eq!(event.interruption_id.len(), 32);
    }

    #[test]
    fn test_interruption_event_new_no_detail() {
        let event = InterruptionEvent::new(
            InterruptionType::AgentCrash,
            None,
            None,
            None,
            None,
            None,
            None,
            500_000,
            None,
        );
        assert_eq!(event.interruption_type, InterruptionType::AgentCrash);
        assert_eq!(event.severity, Severity::Critical);
        assert!(event.session_id.is_none());
        assert!(event.detail.is_none());
        assert!(!event.resolved);
    }

    #[test]
    fn test_new_id_uniqueness() {
        let id1 = new_id();
        let id2 = new_id();
        // IDs should be 32 chars hex
        assert_eq!(id1.len(), 32);
        assert_eq!(id2.len(), 32);
        // All hex chars
        assert!(id1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_interruption_type_serde_roundtrip() {
        let types = vec![
            InterruptionType::AgentCrash,
            InterruptionType::RateLimit,
            InterruptionType::AuthError,
            InterruptionType::NetworkTimeout,
            InterruptionType::ServiceUnavailable,
            InterruptionType::SafetyFilter,
            InterruptionType::SseTruncated,
            InterruptionType::ContextOverflow,
            InterruptionType::TokenLimit,
            InterruptionType::LlmError,
            InterruptionType::RetryStorm,
            InterruptionType::DeadLoop,
        ];
        for t in types {
            let json = serde_json::to_string(&t).unwrap();
            let back: InterruptionType = serde_json::from_str(&json).unwrap();
            assert_eq!(t, back);
        }
    }

    #[test]
    fn test_severity_serde_roundtrip() {
        let severities = vec![
            Severity::Critical,
            Severity::High,
            Severity::Medium,
            Severity::Low,
        ];
        for s in severities {
            let json = serde_json::to_string(&s).unwrap();
            let back: Severity = serde_json::from_str(&json).unwrap();
            assert_eq!(s, back);
        }
    }
}
