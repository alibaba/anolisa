//! Interruption detection rules applied to individual LLMCall events.
//!
//! # Online path (called immediately after each LLMCall is built)
//! `InterruptionDetector::detect(call)` checks a single call against all
//! single-call rules and returns any detected interruption events.

use crate::genai::semantic::LLMCall;
use super::types::{InterruptionEvent, InterruptionType};

/// Configuration for the interruption detector
pub struct DetectorConfig {
    /// Ratio of output_tokens / max_tokens that triggers token_limit (default: 0.95)
    pub token_limit_ratio: f64,
    /// Minimum call duration to consider sse_truncated (avoid fast-fail false positives)
    pub sse_min_duration_ns: u64,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        DetectorConfig {
            token_limit_ratio: 0.95,
            sse_min_duration_ns: 1_000_000_000, // 1 second
        }
    }
}

pub struct InterruptionDetector {
    pub config: DetectorConfig,
}

impl Default for InterruptionDetector {
    fn default() -> Self {
        Self::new(DetectorConfig::default())
    }
}

impl InterruptionDetector {
    pub fn new(config: DetectorConfig) -> Self {
        InterruptionDetector { config }
    }

    /// Online detection: inspect a single completed LLMCall.
    ///
    /// Detects: context_overflow, llm_error, sse_truncated, token_limit.
    pub fn detect(&self, call: &LLMCall) -> Vec<InterruptionEvent> {
        let mut events = Vec::new();

        let session_id = call.metadata.get("session_id").cloned();
        let trace_id   = call.metadata.get("response_id").cloned();
        let conversation_id = call.metadata.get("conversation_id").cloned();
        let call_id    = Some(call.call_id.clone());
        let pid        = Some(call.pid);
        let agent_name = call.agent_name.clone();

        let status_code: u16 = call.metadata.get("status_code")
            .and_then(|s| s.parse().ok())
            .unwrap_or(200);

        // Helper: scan error message / response body for context-overflow keywords
        let error_text = call.error.as_deref().unwrap_or("");
        let response_body = call.metadata.get("response_body").map(|s| s.as_str()).unwrap_or("");
        let combined_error = format!("{} {}", error_text, response_body).to_ascii_lowercase();

        let is_context_overflow =
            combined_error.contains("context_length_exceeded")
            || combined_error.contains("maximum context length")
            || combined_error.contains("context window")
            || combined_error.contains("context_length")
            || combined_error.contains("reduce the length")
            || combined_error.contains("prompt is too long")
            || combined_error.contains("input is too long")
            || combined_error.contains("tokens_limit_reached")
            || combined_error.contains("context limit")
            || combined_error.contains("exceeds the model")
            // HTTP 413 from some gateways
            || status_code == 413;

        // ── 1. Context overflow ───────────────────────────────────────────────
        // Must be checked BEFORE generic LlmError so 400 + context keywords
        // are classified correctly instead of being swallowed by LlmError.
        if is_context_overflow {
            let detail = serde_json::json!({
                "model": call.model,
                "status_code": status_code,
                "error": call.error,
                "input_tokens": call.token_usage.as_ref().map(|u| u.input_tokens),
            });
            events.push(InterruptionEvent::new(
                InterruptionType::ContextOverflow,
                session_id.clone(), trace_id.clone(), conversation_id.clone(), call_id.clone(),
                pid, agent_name.clone(),
                call.end_timestamp_ns as i64,
                Some(detail),
            ));
            return events; // context overflow supersedes all other rules
        }

        // ── 2. LLM error (non-context HTTP/API errors) ────────────────────────
        if status_code >= 400 || call.error.is_some() {
            let detail = serde_json::json!({
                "status_code": status_code,
                "error": call.error,
                "model": call.model,
            });
            events.push(InterruptionEvent::new(
                InterruptionType::LlmError,
                session_id.clone(), trace_id.clone(), conversation_id.clone(), call_id.clone(),
                pid, agent_name.clone(),
                call.end_timestamp_ns as i64,
                Some(detail),
            ));
            return events;
        }

        // ── 3. SSE truncated ──────────────────────────────────────────────────
        let is_sse = call.metadata.get("is_sse").map(|s| s == "true").unwrap_or(false);
        if is_sse
            && call.response.messages.is_empty()
            && call.duration_ns >= self.config.sse_min_duration_ns
        {
            let detail = serde_json::json!({
                "model": call.model,
                "duration_ms": call.duration_ns / 1_000_000,
                "sse_event_count": call.metadata.get("sse_event_count"),
            });
            events.push(InterruptionEvent::new(
                InterruptionType::SseTruncated,
                session_id.clone(), trace_id.clone(), conversation_id.clone(), call_id.clone(),
                pid, agent_name.clone(),
                call.end_timestamp_ns as i64,
                Some(detail),
            ));
        }

        // ── 4. Token limit (output capped by max_tokens) ──────────────────────
        let finish_reason = call.response.messages.first()
            .and_then(|m| m.finish_reason.as_deref());
        if finish_reason == Some("length") {
            if let Some(max_tokens) = call.request.max_tokens {
                if let Some(usage) = &call.token_usage {
                    let ratio = usage.output_tokens as f64 / max_tokens as f64;
                    if ratio >= self.config.token_limit_ratio {
                        let detail = serde_json::json!({
                            "model": call.model,
                            "output_tokens": usage.output_tokens,
                            "max_tokens": max_tokens,
                            "ratio": ratio,
                        });
                        events.push(InterruptionEvent::new(
                            InterruptionType::TokenLimit,
                            session_id.clone(), trace_id.clone(), conversation_id.clone(), call_id.clone(),
                            pid, agent_name.clone(),
                            call.end_timestamp_ns as i64,
                            Some(detail),
                        ));
                    }
                }
            }
        }

        // ── 5. Context overflow via finish_reason (200 response, input overflow)
        // Some models return 200 with finish_reason="length" when the *input*
        // already exceeds the context window but response still arrives.
        // Detect via input_tokens >= model context ceiling (heuristic: >90% of
        // a well-known ceiling, or when input_tokens >> max_tokens).
        if finish_reason == Some("length") {
            if let Some(usage) = &call.token_usage {
                if let Some(max_tokens) = call.request.max_tokens {
                    // If input tokens are much larger than the output cap, this
                    // is almost certainly a context-length issue, not output truncation.
                    if usage.input_tokens > max_tokens * 4 {
                        let detail = serde_json::json!({
                            "model": call.model,
                            "input_tokens": usage.input_tokens,
                            "max_tokens": max_tokens,
                            "finish_reason": "length",
                            "note": "input_tokens >> max_tokens suggests context overflow",
                        });
                        events.push(InterruptionEvent::new(
                            InterruptionType::ContextOverflow,
                            session_id.clone(), trace_id.clone(), conversation_id.clone(), call_id.clone(),
                            pid, agent_name.clone(),
                            call.end_timestamp_ns as i64,
                            Some(detail),
                        ));
                    }
                }
            }
        }

        events
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genai::semantic::*;
    use std::collections::HashMap;

    fn make_base_call() -> LLMCall {
        LLMCall {
            call_id: "call-001".to_string(),
            start_timestamp_ns: 1_000_000_000,
            end_timestamp_ns: 2_000_000_000,
            duration_ns: 1_000_000_000,
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            request: LLMRequest {
                messages: vec![],
                temperature: None,
                max_tokens: None,
                frequency_penalty: None,
                presence_penalty: None,
                top_p: None,
                top_k: None,
                seed: None,
                stop_sequences: None,
                stream: false,
                tools: None,
                raw_body: None,
            },
            response: LLMResponse {
                messages: vec![],
                streamed: false,
                raw_body: None,
            },
            token_usage: None,
            error: None,
            pid: 1234,
            process_name: "agent".to_string(),
            agent_name: Some("TestAgent".to_string()),
            metadata: HashMap::from([
                ("status_code".to_string(), "200".to_string()),
            ]),
        }
    }

    #[test]
    fn test_no_interruption_for_normal_call() {
        let detector = InterruptionDetector::default();
        let call = make_base_call();
        let events = detector.detect(&call);
        assert!(events.is_empty());
    }

    #[test]
    fn test_detect_context_overflow_keyword() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.error = Some("context_length_exceeded".to_string());
        call.metadata.insert("status_code".to_string(), "400".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].interruption_type, InterruptionType::ContextOverflow);
    }

    #[test]
    fn test_detect_context_overflow_http_413() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata.insert("status_code".to_string(), "413".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].interruption_type, InterruptionType::ContextOverflow);
    }

    #[test]
    fn test_detect_context_overflow_response_body() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata.insert("status_code".to_string(), "400".to_string());
        call.metadata.insert("response_body".to_string(), "maximum context length is 128k".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].interruption_type, InterruptionType::ContextOverflow);
    }

    #[test]
    fn test_detect_llm_error_http_500() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata.insert("status_code".to_string(), "500".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].interruption_type, InterruptionType::LlmError);
    }

    #[test]
    fn test_detect_llm_error_with_error_field() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.error = Some("rate_limit_exceeded".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].interruption_type, InterruptionType::LlmError);
    }

    #[test]
    fn test_detect_sse_truncated() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata.insert("is_sse".to_string(), "true".to_string());
        call.duration_ns = 2_000_000_000; // > 1 second min
        // response.messages is empty
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].interruption_type, InterruptionType::SseTruncated);
    }

    #[test]
    fn test_no_sse_truncated_short_duration() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata.insert("is_sse".to_string(), "true".to_string());
        call.duration_ns = 500_000_000; // < 1 second min
        let events = detector.detect(&call);
        assert!(events.is_empty());
    }

    #[test]
    fn test_detect_token_limit() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.request.max_tokens = Some(4096);
        call.token_usage = Some(TokenUsage {
            input_tokens: 1000,
            output_tokens: 3900, // 3900/4096 = 0.952 >= 0.95
            total_tokens: 4900,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        });
        call.response.messages = vec![OutputMessage {
            role: "assistant".to_string(),
            parts: vec![],
            name: None,
            finish_reason: Some("length".to_string()),
        }];
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].interruption_type, InterruptionType::TokenLimit);
    }

    #[test]
    fn test_no_token_limit_below_ratio() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.request.max_tokens = Some(4096);
        call.token_usage = Some(TokenUsage {
            input_tokens: 1000,
            output_tokens: 2000, // 2000/4096 = 0.488 < 0.95
            total_tokens: 3000,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        });
        call.response.messages = vec![OutputMessage {
            role: "assistant".to_string(),
            parts: vec![],
            name: None,
            finish_reason: Some("length".to_string()),
        }];
        let events = detector.detect(&call);
        assert!(events.is_empty());
    }

    #[test]
    fn test_detect_context_overflow_via_finish_reason() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.request.max_tokens = Some(4096);
        call.token_usage = Some(TokenUsage {
            input_tokens: 20000, // >> 4096 * 4 = 16384
            output_tokens: 100,
            total_tokens: 20100,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        });
        call.response.messages = vec![OutputMessage {
            role: "assistant".to_string(),
            parts: vec![],
            name: None,
            finish_reason: Some("length".to_string()),
        }];
        let events = detector.detect(&call);
        // Should have context_overflow (from rule 5)
        assert!(events.iter().any(|e| e.interruption_type == InterruptionType::ContextOverflow));
    }

    #[test]
    fn test_context_overflow_supersedes_llm_error() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata.insert("status_code".to_string(), "400".to_string());
        call.error = Some("context_length_exceeded: max 128000 tokens".to_string());
        let events = detector.detect(&call);
        // Should be context_overflow, NOT llm_error
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].interruption_type, InterruptionType::ContextOverflow);
    }

    #[test]
    fn test_event_metadata_fields() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata.insert("status_code".to_string(), "500".to_string());
        call.metadata.insert("session_id".to_string(), "sess-abc".to_string());
        call.metadata.insert("response_id".to_string(), "trace-xyz".to_string());
        call.metadata.insert("conversation_id".to_string(), "conv-123".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].session_id, Some("sess-abc".to_string()));
        assert_eq!(events[0].trace_id, Some("trace-xyz".to_string()));
        assert_eq!(events[0].conversation_id, Some("conv-123".to_string()));
        assert_eq!(events[0].call_id, Some("call-001".to_string()));
        assert_eq!(events[0].pid, Some(1234));
        assert_eq!(events[0].agent_name, Some("TestAgent".to_string()));
    }

    #[test]
    fn test_custom_config() {
        let config = DetectorConfig {
            token_limit_ratio: 0.8,
            sse_min_duration_ns: 500_000_000,
        };
        let detector = InterruptionDetector::new(config);
        let mut call = make_base_call();
        call.request.max_tokens = Some(100);
        call.token_usage = Some(TokenUsage {
            input_tokens: 10,
            output_tokens: 85, // 85/100 = 0.85 >= 0.8 (custom ratio)
            total_tokens: 95,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        });
        call.response.messages = vec![OutputMessage {
            role: "assistant".to_string(),
            parts: vec![],
            name: None,
            finish_reason: Some("length".to_string()),
        }];
        let events = detector.detect(&call);
        assert!(events.iter().any(|e| e.interruption_type == InterruptionType::TokenLimit));
    }
}
