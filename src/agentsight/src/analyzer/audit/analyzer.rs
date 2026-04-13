//! AuditAnalyzer - extracts audit records from aggregated results
//!
//! Pure logic layer, no IO. Converts `AggregatedResult` into `AuditRecord`.

use crate::aggregator::HttpPair;
use crate::aggregator::AggregatedProcess;
use crate::aggregator::AggregatedResult;
use crate::analyzer::HttpRecord;
use super::record::{AuditEventType, AuditExtra, AuditRecord};

/// Analyzes aggregated results and extracts audit records
pub struct AuditAnalyzer;

impl AuditAnalyzer {
    /// Create a new AuditAnalyzer
    pub fn new() -> Self {
        AuditAnalyzer
    }

    /// Analyze an aggregated result and extract an audit record if applicable
    ///
    /// Note: For HTTP/HTTP2 requests, prefer using `analyze_http` method instead,
    /// which handles both HTTP/1.1 and HTTP/2 uniformly via HttpRecord.
    pub fn analyze(&self, result: &AggregatedResult) -> Option<AuditRecord> {
        match result {
            AggregatedResult::ProcessComplete(process) => {
                Some(self.extract_process_action(process))
            }
            // HTTP/HTTP2 results should be handled via analyze_http()
            // This legacy path is kept for backward compatibility
            AggregatedResult::SseComplete(pair) => {
                // Only create audit for SSE responses (LLM streaming calls)
                Some(self.extract_llm_call_legacy(pair, true))
            }
            _ => None,
        }
    }

    /// Extract audit record from HttpRecord
    ///
    /// Only creates llm_call for SSE responses, which are LLM streaming API calls.
    /// Non-SSE requests (like npm package queries) are filtered out.
    /// This method works for both HTTP/1.1 and HTTP/2 uniformly.
    pub fn analyze_http(&self, http_record: &HttpRecord) -> Option<AuditRecord> {
        // Only create llm_call for SSE responses
        if !http_record.is_sse {
            return None;
        }

        // Extract model from request body if available
        let model = http_record.request_body
            .as_ref()
            .and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok())
            .and_then(|json| json.get("model")?.as_str().map(|s| s.to_string()));

        Some(AuditRecord {
            id: None,
            event_type: AuditEventType::LlmCall,
            timestamp_ns: http_record.timestamp_ns,
            pid: http_record.pid,
            ppid: None,
            comm: http_record.comm.clone(),
            duration_ns: http_record.duration_ns,
            extra: AuditExtra::LlmCall {
                provider: None,
                model,
                request_method: Some(http_record.method.clone()),
                request_path: Some(http_record.path.clone()),
                response_status: Some(http_record.status_code),
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                is_sse: true,
            },
        })
    }

    /// Extract an LLM call audit record from an HTTP pair (legacy method)
    fn extract_llm_call_legacy(&self, pair: &HttpPair, is_sse: bool) -> AuditRecord {
        let request = &pair.request;
        let pid = request.source_event.pid;
        let comm = request.source_event.comm_str();

        // Extract request info
        let request_method = Some(request.method.clone());
        let request_path = Some(request.path.clone());
        let request_ts = request.source_event.timestamp_ns;

        // Extract response info
        let response_status = Some(pair.response.parsed.status_code);

        // Extract model from request body
        let model = detect_model_from_request(pair);

        // Calculate duration
        let response_end_ts = pair.response.end_timestamp_ns();
        let duration_ns = response_end_ts.saturating_sub(request_ts);

        AuditRecord {
            id: None,
            event_type: AuditEventType::LlmCall,
            timestamp_ns: request_ts,
            pid,
            ppid: None,
            comm,
            duration_ns,
            extra: AuditExtra::LlmCall {
                provider: None,
                model,
                request_method,
                request_path,
                response_status,
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                is_sse,
            },
        }
    }

    /// Extract a process action audit record from an aggregated process
    fn extract_process_action(&self, process: &AggregatedProcess) -> AuditRecord {
        AuditRecord {
            id: None,
            event_type: AuditEventType::ProcessAction,
            timestamp_ns: process.start_timestamp_ns,
            pid: process.pid,
            ppid: Some(process.ppid),
            comm: process.comm.clone(),
            duration_ns: process.duration_ns(),
            extra: AuditExtra::ProcessAction {
                filename: process.filename.clone(),
                args: process.args.clone(),
                exit_code: None, // AggregatedProcess doesn't have exit_code yet
            },
        }
    }
}

impl Default for AuditAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

/// Try to detect model from request body JSON
fn detect_model_from_request(pair: &HttpPair) -> Option<String> {
    let body = pair.request.body();
    if body.is_empty() {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(body).ok()?;
    json.get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}
