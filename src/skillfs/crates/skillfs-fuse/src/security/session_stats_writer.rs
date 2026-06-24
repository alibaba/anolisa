//! Dedicated JSONL writer for mount-session summary statistics.
//!
//! Writes to the session metrics log path:
//!   `/var/log/anolisa/sls/ops/skillfs.jsonl`
//!
//! Design contract:
//!
//! * Every write opens the file, appends one JSONL line, and closes the
//!   handle. This satisfies the logrotate requirement in the log rotation contract
//!   (rename-based rotation must not strand writes in a stale fd).
//! * No background thread, no bounded channel. The summary is written
//!   synchronously at mount exit — typically once per session.
//! * Write failures are logged via `tracing::warn` but never propagate as
//!   FUSE errors or change mount exit status.
//! * This writer does **not** reuse [`super::audit::JsonlFileAuditSink`].

use std::io::Write;
use std::path::{Path, PathBuf};

use tracing::warn;

use super::session_stats::{SkillfsSessionSummary, serialize_session_summary};

/// Default session metrics log path per the default deployment convention.
pub const SKILLFS_SESSION_METRICS_LOG_PATH: &str = "/var/log/anolisa/sls/ops/skillfs.jsonl";

/// Best-effort JSONL summary writer.
///
/// Each call to [`SessionStatsWriter::write_summary`] opens the target file
/// in append mode, writes one JSON line, and closes the handle. Errors are
/// surfaced as `tracing::warn` and returned as `Err` so callers can decide
/// whether to retry — but in practice the CLI should not retry or abort.
pub struct SessionStatsWriter {
    path: PathBuf,
}

impl SessionStatsWriter {
    /// Create a writer targeting the given path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Create a writer targeting the default session metrics log.
    pub fn default_path() -> Self {
        Self::new(SKILLFS_SESSION_METRICS_LOG_PATH)
    }

    /// The target file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Write one session summary as a JSONL line (open + append + close).
    ///
    /// Returns `Ok(())` on success. On failure, logs a warning and returns
    /// the underlying IO error. Callers should treat failure as non-fatal.
    pub fn write_summary(&self, summary: &SkillfsSessionSummary) -> std::io::Result<()> {
        let mut line = serialize_session_summary(summary);
        line.push('\n');

        // Production: the file should already exist (pre-created with mode
        // 666 by the deployment setup). create(true) is a
        // dev/fallback path only — it inherits umask, not the expected 666.
        let result = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut file| file.write_all(line.as_bytes()));

        if let Err(ref e) = result {
            warn!(
                error = %e,
                path = %self.path.display(),
                "skillfs session stats: failed to write summary to session metrics log"
            );
        }

        result
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::session_stats::SkillfsSessionStats;

    #[test]
    fn writes_one_valid_json_line_to_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("skillfs.jsonl");

        let writer = SessionStatsWriter::new(&log_path);
        let stats = SkillfsSessionStats::new();
        stats.set_skill_counts(20, 6);
        stats.record_skill_hit("weather");
        let summary = stats.build_summary("test-write-1", "agent");

        writer.write_summary(&summary).expect("write must succeed");

        let content = std::fs::read_to_string(&log_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(
            lines.len(),
            1,
            "expected exactly 1 line, got {}",
            lines.len()
        );

        let parsed: serde_json::Value =
            serde_json::from_str(lines[0]).expect("line must be valid JSON");
        assert_eq!(parsed["component.name"], "skillfs");
        assert_eq!(parsed["session_id"], "test-write-1");
        assert_eq!(parsed["pruned_skill_count"], 14);
    }

    #[test]
    fn second_write_appends_another_line() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("skillfs.jsonl");

        let writer = SessionStatsWriter::new(&log_path);

        let stats1 = SkillfsSessionStats::new();
        let summary1 = stats1.build_summary("session-a", "agent");
        writer.write_summary(&summary1).unwrap();

        let stats2 = SkillfsSessionStats::new();
        stats2.record_decision(crate::security::session_stats::RuntimeDecisionOutcome::Allow);
        let summary2 = stats2.build_summary("session-b", "agent");
        writer.write_summary(&summary2).unwrap();

        let content = std::fs::read_to_string(&log_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2, "expected 2 lines, got {}", lines.len());

        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first["session_id"], "session-a");
        assert_eq!(second["session_id"], "session-b");
        assert_eq!(second["allow_times"], 1);
    }

    #[test]
    fn write_failure_on_invalid_path_returns_error() {
        let writer = SessionStatsWriter::new("/nonexistent/deep/skillfs.jsonl");
        let stats = SkillfsSessionStats::new();
        let summary = stats.build_summary("fail-test", "agent");
        let result = writer.write_summary(&summary);
        assert!(result.is_err(), "expected write to fail on invalid path");
    }

    #[test]
    fn default_path_is_session_metrics_path() {
        let writer = SessionStatsWriter::default_path();
        assert_eq!(
            writer.path().to_str().unwrap(),
            "/var/log/anolisa/sls/ops/skillfs.jsonl"
        );
    }
}
