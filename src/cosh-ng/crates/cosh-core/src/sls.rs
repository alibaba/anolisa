//! SLS (Simple Log Service) local JSONL writer.
//!
//! Appends one JSON record per line to `/var/log/anolisa/sls/ops/cosh.jsonl`.
//! The file is opened with `O_WRONLY | O_APPEND` (no `O_CREAT`) so the call
//! naturally fails when the file does not exist — this writer never creates
//! the SLS file; the anolisa platform pre-provisions it.
//!
//! Each call opens, writes, and closes the file to support logrotate
//! rename-by-path without holding stale file descriptors.

use std::fs::OpenOptions;
use std::io::Write;
use std::time::Duration;

use crate::core::CoshCore;

const DEFAULT_SLS_LOG_PATH: &str = "/var/log/anolisa/sls/ops/cosh.jsonl";

/// Resolve the SLS log path. Honors `COSH_SLS_LOG_PATH` for testing.
fn sls_log_path() -> String {
    std::env::var("COSH_SLS_LOG_PATH").unwrap_or_else(|_| DEFAULT_SLS_LOG_PATH.to_string())
}

/// Append a single JSON record as one line to the SLS log file.
/// Uses O_WRONLY | O_APPEND (no O_CREAT) so the call naturally fails
/// when the file does not exist.
/// Silently fails — SLS logging must never break the main process.
pub fn append_sls_log(record: &serde_json::Value) {
    append_sls_log_to(&sls_log_path(), record);
}

fn append_sls_log_to(path: &str, record: &serde_json::Value) {
    let Ok(line) = serde_json::to_string(record) else {
        return;
    };
    let result = OpenOptions::new()
        .write(true)
        .append(true)
        .open(path);
    let Ok(mut file) = result else { return };
    let _ = writeln!(file, "{line}");
}

impl CoshCore {
    /// Build the SLS JSONL record from the accumulated turn metrics.
    /// Field names are kept identical to the copilot-shell SLS schema
    /// (`session.*` prefix) so the SLS platform can parse both sources
    /// with the same schema.  Fields not yet available output zero/empty
    /// placeholder values to keep the schema stable.
    pub fn build_sls_record(&self, _duration: Duration) -> serde_json::Value {
        let avg_await = if self.metrics.approval_count > 0 {
            self.metrics.approval_wait_ms as f64 / self.metrics.approval_count as f64 / 1000.0
        } else {
            0.0
        };
        serde_json::json!({
            // Component identification
            "component.name": "cosh",
            "component.version": env!("CARGO_PKG_VERSION"),
            "component.agent_name": "cosh-ng",
            "session.id": self.session_id,
            "installation_id": "",  // Phase 2

            // Session configuration
            "session.model": self.model,
            "session.auth_type": self.config.resolve_provider().provider_type,
            "session.approval_mode": self.config.agent.approval_mode,

            // Audit decision counts
            "session.audit_decision_counts.approve": self.metrics.approval_allow,
            "session.audit_decision_counts.deny": self.metrics.approval_deny,
            "session.audit_decision_counts.modify": 0,  // Phase 2

            // Tool call counts
            "session.tool_call_counts.total": self.metrics.tool_calls_total,
            "session.tool_call_counts.success": self.metrics.tool_calls_success,
            "session.tool_call_counts.fail": self.metrics.tool_calls_fail,
            "session.tool_call_total_duration_seconds":
                (self.metrics.tool_calls_duration_ms as f64 / 1000.0 * 100.0).round() / 100.0,

            // Tool error counts
            "session.tool_error_counts.model_error": 0,      // Phase 2
            "session.tool_error_counts.execution_error": 0,  // Phase 2
            "session.tool_error_counts.denied": self.metrics.approval_deny,

            // Approval wait time
            "session.avg_await_duration_seconds": (avg_await * 100.0).round() / 100.0,

            // File operation stats
            "session.files.lines_added": 0,    // Phase 2
            "session.files.lines_removed": 0,  // Phase 2

            // Sandbox stats
            "session.sandbox.total_runs": self.metrics.sandbox_runs,    // Phase 2: always 0
            "session.sandbox.total_blocked": self.metrics.sandbox_blocked,

            // Token usage
            "session.tokens.input": self.metrics.tokens_input,
            "session.tokens.output": self.metrics.tokens_output,
            "session.tokens.cached": 0,  // Phase 2
            "session.tokens.total": self.metrics.tokens_total,

            // API stats
            "session.api.total_requests": self.metrics.api_requests,
            "session.api.total_errors": self.metrics.api_errors,
            "session.api.total_latency_seconds":
                (self.metrics.api_latency_ms as f64 / 1000.0 * 100.0).round() / 100.0,

            // Environment info
            "os.type": std::env::consts::OS,
            "os.arch": std::env::consts::ARCH,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn test_engine() -> CoshCore {
        let config = crate::config::CoreConfig::default();
        let provider =
            Box::new(crate::provider::mock::MockProvider::text_only("test"));
        let tools = crate::tool::ToolRegistry::new();
        CoshCore::new(config, provider, tools)
    }

    /// All 28 SLS fields must be present with correct types.
    #[test]
    fn build_sls_record_has_all_fields() {
        let engine = test_engine();
        let record = engine.build_sls_record(Duration::from_millis(1234));

        // Component identification
        assert_eq!(record["component.name"], "cosh");
        assert!(record["component.version"].is_string());
        assert_eq!(record["component.agent_name"], "cosh-ng");
        assert!(record["session.id"].is_string());
        assert!(record["installation_id"].is_string());

        // Session configuration
        assert!(record["session.model"].is_string());
        assert!(record["session.auth_type"].is_string());
        assert!(record["session.approval_mode"].is_string());

        // Audit decision counts
        assert!(record["session.audit_decision_counts.approve"].is_number());
        assert!(record["session.audit_decision_counts.deny"].is_number());
        assert!(record["session.audit_decision_counts.modify"].is_number());

        // Tool call counts
        assert!(record["session.tool_call_counts.total"].is_number());
        assert!(record["session.tool_call_counts.success"].is_number());
        assert!(record["session.tool_call_counts.fail"].is_number());
        assert!(record["session.tool_call_total_duration_seconds"].is_number());

        // Tool error counts
        assert!(record["session.tool_error_counts.model_error"].is_number());
        assert!(record["session.tool_error_counts.execution_error"].is_number());
        assert!(record["session.tool_error_counts.denied"].is_number());

        // Approval wait time
        assert!(record["session.avg_await_duration_seconds"].is_number());

        // File stats
        assert!(record["session.files.lines_added"].is_number());
        assert!(record["session.files.lines_removed"].is_number());

        // Sandbox stats
        assert!(record["session.sandbox.total_runs"].is_number());
        assert!(record["session.sandbox.total_blocked"].is_number());

        // Token usage
        assert!(record["session.tokens.input"].is_number());
        assert!(record["session.tokens.output"].is_number());
        assert!(record["session.tokens.cached"].is_number());
        assert!(record["session.tokens.total"].is_number());

        // API stats
        assert!(record["session.api.total_requests"].is_number());
        assert!(record["session.api.total_errors"].is_number());
        assert!(record["session.api.total_latency_seconds"].is_number());

        // Environment
        assert!(record["os.type"].is_string());
        assert!(record["os.arch"].is_string());
    }

    /// Metrics accumulation is reflected in the SLS record.
    #[test]
    fn build_sls_record_reflects_metrics() {
        let mut engine = test_engine();
        engine.metrics.tokens_input = 100;
        engine.metrics.tokens_output = 50;
        engine.metrics.tokens_total = 150;
        engine.metrics.api_requests = 3;
        engine.metrics.api_errors = 1;
        engine.metrics.api_latency_ms = 5000;
        engine.metrics.tool_calls_total = 4;
        engine.metrics.tool_calls_success = 3;
        engine.metrics.tool_calls_fail = 1;
        engine.metrics.approval_allow = 2;
        engine.metrics.approval_deny = 1;
        engine.metrics.approval_wait_ms = 6000;
        engine.metrics.approval_count = 3;

        let record = engine.build_sls_record(Duration::from_secs(10));

        assert_eq!(record["session.tokens.input"], 100);
        assert_eq!(record["session.tokens.output"], 50);
        assert_eq!(record["session.tokens.total"], 150);
        assert_eq!(record["session.api.total_requests"], 3);
        assert_eq!(record["session.api.total_errors"], 1);
        assert_eq!(record["session.api.total_latency_seconds"], 5.0);
        assert_eq!(record["session.tool_call_counts.total"], 4);
        assert_eq!(record["session.tool_call_counts.success"], 3);
        assert_eq!(record["session.tool_call_counts.fail"], 1);
        assert_eq!(record["session.audit_decision_counts.approve"], 2);
        assert_eq!(record["session.audit_decision_counts.deny"], 1);
        assert_eq!(record["session.avg_await_duration_seconds"], 2.0);
    }

    /// append_sls_log_to writes valid JSONL when file exists.
    #[test]
    fn append_sls_log_writes_jsonl() {
        let dir = tempfile::tempdir().expect("temp dir");
        let log_path = dir.path().join("cosh.jsonl");
        // Pre-create the file (simulates platform provisioning)
        std::fs::write(&log_path, "").unwrap();

        let path_str = log_path.to_str().unwrap();
        let record = serde_json::json!({"test": true, "count": 42});
        append_sls_log_to(path_str, &record);
        append_sls_log_to(path_str, &record);

        let mut content = String::new();
        std::fs::File::open(&log_path)
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();

        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2, "expected 2 JSONL lines");
        for line in &lines {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(v["test"], true);
            assert_eq!(v["count"], 42);
        }
    }

    /// append_sls_log_to silently skips when file does not exist.
    #[test]
    fn append_sls_log_skips_missing_file() {
        let record = serde_json::json!({"test": true});
        // Should not panic
        append_sls_log_to("/nonexistent/path/cosh.jsonl", &record);
    }
}
