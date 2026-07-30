use std::time::Duration;

use super::readonly_pipeline::{
    run_readonly_pipeline, validate_readonly_pipeline, ReadonlyPipelineConfig,
};
use super::broker::can_run_approved_bash_tool;

const DEFAULT_SEGMENT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_TOTAL_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;
const DEFAULT_OUTPUT_LIMIT_LINES: usize = 1000;

/// Configuration for the CompoundReadonlyExecutor.
#[derive(Debug, Clone)]
pub struct CompoundReadonlyConfig {
    pub segment_timeout: Duration,
    pub total_timeout: Duration,
    pub output_limit_bytes: usize,
    pub output_limit_lines: usize,
}

impl Default for CompoundReadonlyConfig {
    fn default() -> Self {
        Self {
            segment_timeout: DEFAULT_SEGMENT_TIMEOUT,
            total_timeout: DEFAULT_TOTAL_TIMEOUT,
            output_limit_bytes: DEFAULT_OUTPUT_LIMIT_BYTES,
            output_limit_lines: DEFAULT_OUTPUT_LIMIT_LINES,
        }
    }
}

/// The combined output from all segments of a compound command, joined
/// by a newline separator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompoundReadonlyOutput {
    /// Exit code of the last segment that ran.
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompoundReadonlyError {
    pub reason: &'static str,
    pub detail: String,
}

/// Describes the connector that joins two consecutive segments in a compound
/// command. Used to implement `&&` / `||` short-circuit semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentConnector {
    /// `;` or newline — always run the next segment regardless of exit code.
    Sequence,
    /// `&&` — run the next segment only when the current exit code is 0.
    And,
    /// `||` — run the next segment only when the current exit code is non-zero.
    Or,
}

/// A validated plan produced by [`validate_compound_readonly`].
#[derive(Debug, Clone)]
pub struct CompoundReadonlyPlan {
    /// Segments in order, each paired with the connector that precedes it
    /// (the first segment has no predecessor, so `None`).
    pub segments: Vec<(Option<SegmentConnector>, Vec<Vec<String>>)>,
}

/// Validates a compound command for execution through the
/// CompoundReadonlyExecutor. Returns `Err` if any segment is not
/// individually eligible for auto-allow or if a state-mutating builtin
/// is present.
///
/// The validation deliberately reuses the existing per-route validators
/// (`can_run_approved_bash_tool`, `validate_readonly_pipeline`) so the
/// executor cannot be widened without simultaneously widening the
/// underlying validators.
pub fn validate_compound_readonly(
    _command: &str,
    parsed_segments: &[Vec<Vec<String>>],
    connectors: &[SegmentConnector],
) -> Result<CompoundReadonlyPlan, CompoundReadonlyError> {
    if parsed_segments.is_empty() {
        return Err(error("empty-compound", "compound command has no segments"));
    }
    if connectors.len() != parsed_segments.len().saturating_sub(1) {
        return Err(error(
            "connector-mismatch",
            "connector count does not match segment count",
        ));
    }

    let mut plan_segments = Vec::new();
    for (index, segment) in parsed_segments.iter().enumerate() {
        validate_segment_eligibility(segment)?;
        let connector = if index == 0 {
            None
        } else {
            Some(connectors[index - 1])
        };
        plan_segments.push((connector, segment.clone()));
    }

    Ok(CompoundReadonlyPlan {
        segments: plan_segments,
    })
}

/// Validates that a single compound segment is eligible for CompoundReadonlyExecutor.
/// A segment is eligible when it is individually auto-allowable through the
/// DirectReadonlyBroker or ReadonlyPipelineExecutor routes, and contains no
/// state-mutating shell builtins.
fn validate_segment_eligibility(
    segment: &[Vec<String>],
) -> Result<(), CompoundReadonlyError> {
    let Some(first_stage) = segment.first() else {
        return Err(error("empty-segment", "compound segment is empty"));
    };
    let Some(program) = first_stage.first() else {
        return Err(error("empty-stage", "segment stage is empty"));
    };

    // Reject state-mutating shell builtins (cd, export, unset, source, .).
    if matches!(program.as_str(), "cd" | "export" | "unset" | "source" | ".") {
        return Err(error(
            "state-mutating-builtin",
            program.clone(),
        ));
    }
    // Reject bare env-assignments in command position.
    if program.contains('=') {
        return Err(error("env-assignment-in-command", program.clone()));
    }

    if segment.len() > 1 {
        // Pipeline segment — validate through readonly pipeline executor.
        let segment_text = segment
            .iter()
            .map(|stage| stage.join(" "))
            .collect::<Vec<_>>()
            .join(" | ");
        validate_readonly_pipeline(&segment_text)
            .map_err(|e| error("segment-not-readonly-pipeline", e.detail))?;
    } else {
        // Simple segment — validate through direct readonly broker.
        let segment_text = first_stage.join(" ");
        can_run_approved_bash_tool(&segment_text)
            .map_err(|e| error("segment-not-direct-readonly", e))?;
    }
    Ok(())
}

/// Executes a validated compound command plan. Each segment is run through
/// its appropriate executor (`DirectReadonlyBroker` for simple commands,
/// `ReadonlyPipelineExecutor` for pipelines). `&&` / `||` short-circuit
/// semantics are implemented in-process without invoking a shell.
///
/// The final stdout is the concatenation of each segment's stdout, separated
/// by a blank line when multiple segments produce output.
pub fn run_compound_readonly(
    plan: &CompoundReadonlyPlan,
    config: &CompoundReadonlyConfig,
) -> Result<CompoundReadonlyOutput, CompoundReadonlyError> {
    let deadline = std::time::Instant::now() + config.total_timeout;
    let mut combined_stdout = String::new();
    let mut combined_stderr = String::new();
    let mut last_exit_code: Option<i32> = None;

    for (connector, segment) in &plan.segments {
        if std::time::Instant::now() >= deadline {
            return Err(error("compound-timeout", "compound command timed out"));
        }

        // Short-circuit evaluation: skip segment if connector condition is unmet.
        if let Some(conn) = connector {
            match (conn, last_exit_code) {
                (SegmentConnector::And, Some(code)) if code != 0 => continue,
                (SegmentConnector::Or, Some(code)) if code == 0 => continue,
                _ => {}
            }
        }

        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let seg_timeout = config.segment_timeout.min(remaining);

        let (stdout, stderr, exit_code) = if segment.len() > 1 {
            // Pipeline segment.
            let segment_text = segment
                .iter()
                .map(|stage| stage.join(" "))
                .collect::<Vec<_>>()
                .join(" | ");
            let pipeline_config = ReadonlyPipelineConfig {
                stage_timeout: seg_timeout,
                total_timeout: seg_timeout,
                output_limit_bytes: config.output_limit_bytes,
                output_limit_lines: config.output_limit_lines,
            };
            let output = run_readonly_pipeline(&segment_text, &pipeline_config)
                .map_err(|e| error("segment-pipeline-failed", e.detail))?;
            (output.stdout, output.stderr, output.exit_code)
        } else {
            // Simple segment — run via DirectReadonlyBroker.
            let stage = &segment[0];
            run_direct_readonly_segment(stage, seg_timeout, config.output_limit_bytes)?
        };

        if !combined_stdout.is_empty() && !stdout.is_empty() {
            combined_stdout.push('\n');
        }
        combined_stdout.push_str(&stdout);
        if !combined_stderr.is_empty() && !stderr.is_empty() {
            combined_stderr.push('\n');
        }
        combined_stderr.push_str(&stderr);
        last_exit_code = exit_code;
    }

    Ok(CompoundReadonlyOutput {
        exit_code: last_exit_code,
        stdout: combined_stdout,
        stderr: combined_stderr,
    })
}

/// Runs a simple (non-pipeline) segment through the direct readonly broker:
/// spawns the program without a shell, with a timeout, and returns
/// (stdout, stderr, exit_code).
fn run_direct_readonly_segment(
    argv: &[String],
    timeout: Duration,
    output_limit_bytes: usize,
) -> Result<(String, String, Option<i32>), CompoundReadonlyError> {
    use std::fs::File;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    let Some(program) = argv.first() else {
        return Err(error("empty-stage", "empty argv"));
    };

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let pid = std::process::id();
    let stdout_path = PathBuf::from(std::env::temp_dir()).join(format!(
        "cosh-compound-direct-{pid}-{nanos}-stdout"
    ));
    let stderr_path = PathBuf::from(std::env::temp_dir()).join(format!(
        "cosh-compound-direct-{pid}-{nanos}-stderr"
    ));

    let stdout_file = File::create(&stdout_path)
        .map_err(|e| error("executor-io", format!("create stdout: {e}")))?;
    let stderr_file = File::create(&stderr_path)
        .map_err(|e| error("executor-io", format!("create stderr: {e}")))?;

    let mut child = Command::new(program)
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .map_err(|e| error("executor-spawn", format!("{program}: {e}")))?;

    let deadline = Instant::now() + timeout;
    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&stdout_path);
                let _ = std::fs::remove_file(&stderr_path);
                return Err(error("segment-timeout", argv.join(" ")));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => {
                let _ = std::fs::remove_file(&stdout_path);
                let _ = std::fs::remove_file(&stderr_path);
                return Err(error("executor-wait", e.to_string()));
            }
        }
    };

    let read_output = |path: &PathBuf| -> Result<String, CompoundReadonlyError> {
        let bytes = std::fs::read(path).map_err(|e| error("executor-io", e.to_string()))?;
        let truncated = &bytes[..bytes.len().min(output_limit_bytes)];
        let mut text = String::from_utf8_lossy(truncated).to_string();
        if bytes.len() > output_limit_bytes {
            text.push_str("\n<truncated>");
        }
        Ok(super::strip_ansi(&text))
    };

    let stdout = read_output(&stdout_path)?;
    let stderr = read_output(&stderr_path)?;
    let _ = std::fs::remove_file(&stdout_path);
    let _ = std::fs::remove_file(&stderr_path);
    Ok((stdout, stderr, exit_code))
}

fn error(reason: &'static str, detail: impl Into<String>) -> CompoundReadonlyError {
    CompoundReadonlyError {
        reason,
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compound_readonly_validates_all_direct_readonly_segments() {
        // Both segments qualify via DirectReadonlyBroker.
        let segments = vec![
            vec![vec!["pwd".to_string()]],
            vec![vec!["git".to_string(), "status".to_string()]],
        ];
        let connectors = vec![SegmentConnector::And];
        let plan = validate_compound_readonly("pwd && git status", &segments, &connectors)
            .expect("valid compound");
        assert_eq!(plan.segments.len(), 2);
        assert_eq!(plan.segments[0].0, None);
        assert_eq!(plan.segments[1].0, Some(SegmentConnector::And));
    }

    #[test]
    fn compound_readonly_rejects_cd_segment() {
        let segments = vec![
            vec![vec!["cd".to_string(), "/tmp".to_string()]],
            vec![vec!["git".to_string(), "status".to_string()]],
        ];
        let connectors = vec![SegmentConnector::And];
        let err = validate_compound_readonly("cd /tmp && git status", &segments, &connectors)
            .expect_err("cd must be rejected");
        assert_eq!(err.reason, "state-mutating-builtin");
    }

    #[test]
    fn compound_readonly_rejects_export_segment() {
        let segments = vec![
            vec![vec!["export".to_string(), "FOO=bar".to_string()]],
            vec![vec!["git".to_string(), "status".to_string()]],
        ];
        let connectors = vec![SegmentConnector::And];
        let err = validate_compound_readonly("export FOO=bar && git status", &segments, &connectors)
            .expect_err("export must be rejected");
        assert_eq!(err.reason, "state-mutating-builtin");
    }

    #[test]
    fn compound_readonly_rejects_unknown_command_segment() {
        let segments = vec![
            vec![vec!["custom-tool".to_string()]],
            vec![vec!["git".to_string(), "status".to_string()]],
        ];
        let connectors = vec![SegmentConnector::And];
        let err =
            validate_compound_readonly("custom-tool && git status", &segments, &connectors)
                .expect_err("unknown command must be rejected");
        assert_eq!(err.reason, "segment-not-direct-readonly");
    }

    #[test]
    fn compound_readonly_executes_and_short_circuits() {
        // `pwd && git status`: both segments should run (pwd exits 0).
        let segments = vec![
            vec![vec!["pwd".to_string()]],
            vec![vec!["git".to_string(), "status".to_string(), "--short".to_string()]],
        ];
        let connectors = vec![SegmentConnector::And];
        let plan = validate_compound_readonly("pwd && git status --short", &segments, &connectors)
            .expect("valid plan");
        let output = run_compound_readonly(&plan, &CompoundReadonlyConfig::default())
            .expect("execution succeeded");
        assert!(
            output.exit_code.is_some(),
            "exit code should be set"
        );
        // pwd output should be present.
        assert!(
            !output.stdout.is_empty(),
            "stdout should contain pwd output"
        );
    }
}
