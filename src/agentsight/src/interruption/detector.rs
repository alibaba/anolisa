//! Interruption detection rules applied to individual LLMCall events.
//!
//! # Online path (called immediately after each LLMCall is built)
//! `InterruptionDetector::detect(call)` checks a single call against all
//! single-call rules and returns any detected interruption events.

use super::types::{InterruptionEvent, InterruptionType};
use crate::genai::semantic::LLMCall;

/// Whether the finish reason indicates a normal (non-truncated) end of generation.
fn is_normal_finish(reason: Option<&str>) -> bool {
    matches!(
        reason,
        Some("stop" | "tool_calls" | "end_turn" | "tool_use" | "stop_sequence")
    )
}

/// Whether the finish reason indicates a token-limit stop (handled by rules 9/10).
fn is_token_limit_finish(reason: Option<&str>) -> bool {
    matches!(reason, Some("length" | "max_tokens"))
}

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
    /// Detection priority (higher = checked first):
    ///   1. AuthError       — 401/403
    ///   2. RateLimit       — 429
    ///   3. NetworkTimeout  — 408/504
    ///   4. ServiceUnavailable — 502/503
    ///   5. ContextOverflow — keywords in error body
    ///   6. SafetyFilter    — finish_reason == "content_filter"
    ///   7. LlmError        — generic HTTP >= 400 fallback
    ///   8. SseTruncated    — SSE stream ended prematurely
    ///   9. TokenLimit      — finish_reason == "length" + ratio
    ///  10. ContextOverflow via finish_reason heuristic
    pub fn detect(&self, call: &LLMCall) -> Vec<InterruptionEvent> {
        let mut events = Vec::new();

        let session_id = call.metadata.get("session_id").cloned();
        let trace_id = call.metadata.get("response_id").cloned();
        let conversation_id = call.metadata.get("conversation_id").cloned();
        let call_id = Some(call.call_id.clone());
        let pid = Some(call.pid);
        let agent_name = call.agent_name.clone();

        let status_code: u16 = call
            .metadata
            .get("status_code")
            .and_then(|s| s.parse().ok())
            .unwrap_or(200);

        // 修复：从 call.response.raw_body 读取响应体，而非 call.metadata（builder 不会写入 metadata）
        let error_text = call.error.as_deref().unwrap_or("");
        let response_body = call.response.raw_body.as_deref().unwrap_or("");
        let combined_error = format!("{error_text} {response_body}").to_ascii_lowercase();

        let is_context_overflow = combined_error.contains("context_length_exceeded")
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

        // ── 1. AuthError (401/403 / invalid_api_key) ──────────────────────────
        if status_code == 401
            || status_code == 403
            || combined_error.contains("invalid_api_key")
            || combined_error.contains("authentication")
            || combined_error.contains("unauthorized")
            || combined_error.contains("invalid x-api-key")
        {
            let detail = serde_json::json!({
                "model": call.model,
                "status_code": status_code,
                "error": call.error,
            });
            events.push(InterruptionEvent::new(
                InterruptionType::AuthError,
                session_id.clone(),
                trace_id.clone(),
                conversation_id.clone(),
                call_id.clone(),
                pid,
                agent_name.clone(),
                call.end_timestamp_ns as i64,
                Some(detail),
            ));
            return events;
        }

        // ── 2. RateLimit (429 / rate_limit) ────────────────────────────────────
        if status_code == 429
            || combined_error.contains("rate_limit")
            || combined_error.contains("rate limit")
            || combined_error.contains("too many requests")
        {
            let detail = serde_json::json!({
                "model": call.model,
                "status_code": status_code,
                "error": call.error,
            });
            events.push(InterruptionEvent::new(
                InterruptionType::RateLimit,
                session_id.clone(),
                trace_id.clone(),
                conversation_id.clone(),
                call_id.clone(),
                pid,
                agent_name.clone(),
                call.end_timestamp_ns as i64,
                Some(detail),
            ));
            return events;
        }

        // ── 3. NetworkTimeout (408/504 / timeout) ─────────────────────────────
        if status_code == 408
            || status_code == 504
            || combined_error.contains("timeout")
            || combined_error.contains("timed out")
            || combined_error.contains("deadline exceeded")
        {
            let detail = serde_json::json!({
                "model": call.model,
                "status_code": status_code,
                "error": call.error,
            });
            events.push(InterruptionEvent::new(
                InterruptionType::NetworkTimeout,
                session_id.clone(),
                trace_id.clone(),
                conversation_id.clone(),
                call_id.clone(),
                pid,
                agent_name.clone(),
                call.end_timestamp_ns as i64,
                Some(detail),
            ));
            return events;
        }

        // ── 4. ServiceUnavailable (502/503 / overloaded) ──────────────────────
        if status_code == 502
            || status_code == 503
            || combined_error.contains("overloaded")
            || combined_error.contains("service_unavailable")
            || combined_error.contains("server is overloaded")
            || combined_error.contains("model is overloaded")
        {
            let detail = serde_json::json!({
                "model": call.model,
                "status_code": status_code,
                "error": call.error,
            });
            events.push(InterruptionEvent::new(
                InterruptionType::ServiceUnavailable,
                session_id.clone(),
                trace_id.clone(),
                conversation_id.clone(),
                call_id.clone(),
                pid,
                agent_name.clone(),
                call.end_timestamp_ns as i64,
                Some(detail),
            ));
            return events;
        }

        // ── 5. Context overflow ───────────────────────────────────────────────
        // 必须在 LlmError 之前检查，避免 400 + context 关键字被通用规则吞掉
        if is_context_overflow {
            let detail = serde_json::json!({
                "model": call.model,
                "status_code": status_code,
                "error": call.error,
                "input_tokens": call.token_usage.as_ref().map(|u| u.input_tokens),
            });
            events.push(InterruptionEvent::new(
                InterruptionType::ContextOverflow,
                session_id.clone(),
                trace_id.clone(),
                conversation_id.clone(),
                call_id.clone(),
                pid,
                agent_name.clone(),
                call.end_timestamp_ns as i64,
                Some(detail),
            ));
            return events; // context overflow supersedes all other rules
        }

        // ── 6. SafetyFilter (finish_reason == "content_filter") ───────────────
        // 必须在 LlmError 之前检查：部分厂商对 content_filter 返回 200 + finish_reason
        let finish_reason = call
            .response
            .messages
            .first()
            .and_then(|m| m.finish_reason.as_deref());
        if finish_reason == Some("content_filter") {
            let detail = serde_json::json!({
                "model": call.model,
                "finish_reason": "content_filter",
                "error": call.error,
            });
            events.push(InterruptionEvent::new(
                InterruptionType::SafetyFilter,
                session_id.clone(),
                trace_id.clone(),
                conversation_id.clone(),
                call_id.clone(),
                pid,
                agent_name.clone(),
                call.end_timestamp_ns as i64,
                Some(detail),
            ));
            return events;
        }

        // ── 7. LLM error (non-context HTTP/API errors) ────────────────────────
        // 通用兜底：所有 HTTP >= 400 且未被上述规则匹配的错误
        if status_code >= 400 || call.error.is_some() {
            let detail = serde_json::json!({
                "status_code": status_code,
                "error": call.error,
                "model": call.model,
            });
            events.push(InterruptionEvent::new(
                InterruptionType::LlmError,
                session_id.clone(),
                trace_id.clone(),
                conversation_id.clone(),
                call_id.clone(),
                pid,
                agent_name.clone(),
                call.end_timestamp_ns as i64,
                Some(detail),
            ));
            return events;
        }

        // ── 8. SSE truncated ──────────────────────────────────────────────────
        // 严格条件：SSE 流 + 持续时间 >= 阈值 + 无正常终止标志 + 非 token-limit
        // 正常终止标志：finish_reason 为 stop/tool_calls/end_turn/tool_use/stop_sequence
        // token-limit (length/max_tokens) 由 rule 9/10 单独处理
        let is_sse = call
            .metadata
            .get("is_sse")
            .map(|s| s == "true")
            .unwrap_or(false);
        if is_sse
            && !is_normal_finish(finish_reason)
            && !is_token_limit_finish(finish_reason)
            && call.duration_ns >= self.config.sse_min_duration_ns
        {
            let detail = serde_json::json!({
                "model": call.model,
                "duration_ms": call.duration_ns / 1_000_000,
                "sse_event_count": call.metadata.get("sse_event_count"),
            });
            events.push(InterruptionEvent::new(
                InterruptionType::SseTruncated,
                session_id.clone(),
                trace_id.clone(),
                conversation_id.clone(),
                call_id.clone(),
                pid,
                agent_name.clone(),
                call.end_timestamp_ns as i64,
                Some(detail),
            ));
        }

        // ── 9. Token limit (output capped by max_tokens) ──────────────────────
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
                            session_id.clone(),
                            trace_id.clone(),
                            conversation_id.clone(),
                            call_id.clone(),
                            pid,
                            agent_name.clone(),
                            call.end_timestamp_ns as i64,
                            Some(detail),
                        ));
                    }
                }
            }
        }

        // ── 10. Context overflow via finish_reason (200 response, input overflow)
        // 有些模型在输入超出上下文窗口时仍返回 200 + finish_reason="length"。
        // 通过 input_tokens >> max_tokens 启发式判定（input > max_tokens * 4）
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
                            session_id.clone(),
                            trace_id.clone(),
                            conversation_id.clone(),
                            call_id.clone(),
                            pid,
                            agent_name.clone(),
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
            metadata: HashMap::from([("status_code".to_string(), "200".to_string())]),
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
        call.metadata
            .insert("status_code".to_string(), "400".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].interruption_type,
            InterruptionType::ContextOverflow
        );
    }

    #[test]
    fn test_detect_context_overflow_http_413() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata
            .insert("status_code".to_string(), "413".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].interruption_type,
            InterruptionType::ContextOverflow
        );
    }

    #[test]
    fn test_detect_context_overflow_response_body() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata
            .insert("status_code".to_string(), "400".to_string());
        // 修复后从 call.response.raw_body 读取响应体
        call.response.raw_body = Some("maximum context length is 128k".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].interruption_type,
            InterruptionType::ContextOverflow
        );
    }

    #[test]
    fn test_detect_llm_error_http_500() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata
            .insert("status_code".to_string(), "500".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].interruption_type, InterruptionType::LlmError);
    }

    #[test]
    fn test_detect_llm_error_with_error_field() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.error = Some("internal_server_error".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].interruption_type, InterruptionType::LlmError);
    }

    #[test]
    fn test_detect_sse_truncated() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata
            .insert("is_sse".to_string(), "true".to_string());
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
        call.metadata
            .insert("is_sse".to_string(), "true".to_string());
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
        assert!(
            events
                .iter()
                .any(|e| e.interruption_type == InterruptionType::ContextOverflow)
        );
    }

    #[test]
    fn test_context_overflow_supersedes_llm_error() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata
            .insert("status_code".to_string(), "400".to_string());
        call.error = Some("context_length_exceeded: max 128000 tokens".to_string());
        let events = detector.detect(&call);
        // Should be context_overflow, NOT llm_error
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].interruption_type,
            InterruptionType::ContextOverflow
        );
    }

    #[test]
    fn test_event_metadata_fields() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata
            .insert("status_code".to_string(), "500".to_string());
        call.metadata
            .insert("session_id".to_string(), "sess-abc".to_string());
        call.metadata
            .insert("response_id".to_string(), "trace-xyz".to_string());
        call.metadata
            .insert("conversation_id".to_string(), "conv-123".to_string());
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
        assert!(
            events
                .iter()
                .any(|e| e.interruption_type == InterruptionType::TokenLimit)
        );
    }

    // ── 新增类型的测试 ──────────────────────────────────────────────────────

    #[test]
    fn test_detect_auth_error_401() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata
            .insert("status_code".to_string(), "401".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].interruption_type, InterruptionType::AuthError);
    }

    #[test]
    fn test_detect_auth_error_403() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata
            .insert("status_code".to_string(), "403".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].interruption_type, InterruptionType::AuthError);
    }

    #[test]
    fn test_detect_auth_error_invalid_api_key() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.error = Some("invalid_api_key".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].interruption_type, InterruptionType::AuthError);
    }

    #[test]
    fn test_detect_rate_limit_429() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata
            .insert("status_code".to_string(), "429".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].interruption_type, InterruptionType::RateLimit);
    }

    #[test]
    fn test_detect_rate_limit_error_keyword() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.error = Some("rate_limit_exceeded".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].interruption_type, InterruptionType::RateLimit);
    }

    #[test]
    fn test_detect_network_timeout_504() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata
            .insert("status_code".to_string(), "504".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].interruption_type,
            InterruptionType::NetworkTimeout
        );
    }

    #[test]
    fn test_detect_network_timeout_error_keyword() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.error = Some("request timeout".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].interruption_type,
            InterruptionType::NetworkTimeout
        );
    }

    #[test]
    fn test_detect_service_unavailable_503() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata
            .insert("status_code".to_string(), "503".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].interruption_type,
            InterruptionType::ServiceUnavailable
        );
    }

    #[test]
    fn test_detect_service_unavailable_error_keyword() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.error = Some("model is overloaded".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].interruption_type,
            InterruptionType::ServiceUnavailable
        );
    }

    #[test]
    fn test_detect_safety_filter() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.response.messages = vec![OutputMessage {
            role: "assistant".to_string(),
            parts: vec![],
            name: None,
            finish_reason: Some("content_filter".to_string()),
        }];
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].interruption_type, InterruptionType::SafetyFilter);
    }

    #[test]
    fn test_safety_filter_not_fired_on_normal_stop() {
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.response.messages = vec![OutputMessage {
            role: "assistant".to_string(),
            parts: vec![],
            name: None,
            finish_reason: Some("stop".to_string()),
        }];
        let events = detector.detect(&call);
        assert!(events.is_empty());
    }

    #[test]
    fn test_sse_truncated_with_normal_finish_not_fired() {
        // SSE 流有正常终止标志（finish_reason=stop）不应被判为截断
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata
            .insert("is_sse".to_string(), "true".to_string());
        call.duration_ns = 2_000_000_000;
        call.response.messages = vec![OutputMessage {
            role: "assistant".to_string(),
            parts: vec![],
            name: None,
            finish_reason: Some("stop".to_string()),
        }];
        let events = detector.detect(&call);
        assert!(
            events
                .iter()
                .all(|e| e.interruption_type != InterruptionType::SseTruncated)
        );
    }

    #[test]
    fn test_sse_truncated_with_tool_calls_finish_not_fired() {
        // SSE 流 finish_reason=tool_calls 是正常终止
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata
            .insert("is_sse".to_string(), "true".to_string());
        call.duration_ns = 2_000_000_000;
        call.response.messages = vec![OutputMessage {
            role: "assistant".to_string(),
            parts: vec![],
            name: None,
            finish_reason: Some("tool_calls".to_string()),
        }];
        let events = detector.detect(&call);
        assert!(
            events
                .iter()
                .all(|e| e.interruption_type != InterruptionType::SseTruncated)
        );
    }

    #[test]
    fn test_sse_tool_use_not_truncated() {
        // SSE + finish_reason="tool_use" → 不产生 SseTruncated
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata
            .insert("is_sse".to_string(), "true".to_string());
        call.duration_ns = 2_000_000_000;
        call.response.messages = vec![OutputMessage {
            role: "assistant".to_string(),
            parts: vec![],
            name: None,
            finish_reason: Some("tool_use".to_string()),
        }];
        let events = detector.detect(&call);
        assert!(
            events
                .iter()
                .all(|e| e.interruption_type != InterruptionType::SseTruncated),
            "tool_use should not trigger SseTruncated"
        );
    }

    #[test]
    fn test_sse_stop_sequence_not_truncated() {
        // SSE + finish_reason="stop_sequence" → 不产生 SseTruncated
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata
            .insert("is_sse".to_string(), "true".to_string());
        call.duration_ns = 2_000_000_000;
        call.response.messages = vec![OutputMessage {
            role: "assistant".to_string(),
            parts: vec![],
            name: None,
            finish_reason: Some("stop_sequence".to_string()),
        }];
        let events = detector.detect(&call);
        assert!(
            events
                .iter()
                .all(|e| e.interruption_type != InterruptionType::SseTruncated),
            "stop_sequence should not trigger SseTruncated"
        );
    }

    #[test]
    fn test_sse_length_not_truncated_but_token_limit_fires() {
        // SSE + finish_reason="length" → 不产生 SseTruncated
        // 但 rule 9 的 TokenLimit 逻辑仍正常触发
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata
            .insert("is_sse".to_string(), "true".to_string());
        call.duration_ns = 2_000_000_000;
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
        assert!(
            events
                .iter()
                .all(|e| e.interruption_type != InterruptionType::SseTruncated),
            "length should not trigger SseTruncated (handled by rule 9/10)"
        );
        assert!(
            events
                .iter()
                .any(|e| e.interruption_type == InterruptionType::TokenLimit),
            "length should still trigger TokenLimit via rule 9"
        );
    }

    #[test]
    fn test_sse_none_finish_still_truncated() {
        // SSE + finish_reason=None + duration > sse_min_duration → 仍产生 SseTruncated
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata
            .insert("is_sse".to_string(), "true".to_string());
        call.duration_ns = 2_000_000_000;
        // response.messages is empty → finish_reason = None
        let events = detector.detect(&call);
        assert!(
            events
                .iter()
                .any(|e| e.interruption_type == InterruptionType::SseTruncated),
            "None finish_reason with SSE should still trigger SseTruncated"
        );
    }

    #[test]
    fn test_auth_error_takes_priority_over_llm_error() {
        // 401 应被归类为 AuthError 而非 LlmError
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata
            .insert("status_code".to_string(), "401".to_string());
        call.error = Some("unauthorized".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].interruption_type, InterruptionType::AuthError);
    }

    #[test]
    fn test_rate_limit_takes_priority_over_llm_error() {
        // 429 应被归类为 RateLimit 而非 LlmError
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata
            .insert("status_code".to_string(), "429".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].interruption_type, InterruptionType::RateLimit);
    }

    #[test]
    fn test_response_body_bug_fix() {
        // 验证从 call.response.raw_body 读取响应体（非 metadata）
        let detector = InterruptionDetector::default();
        let mut call = make_base_call();
        call.metadata
            .insert("status_code".to_string(), "400".to_string());
        call.response.raw_body = Some("context_length_exceeded".to_string());
        let events = detector.detect(&call);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].interruption_type,
            InterruptionType::ContextOverflow
        );
    }
}
