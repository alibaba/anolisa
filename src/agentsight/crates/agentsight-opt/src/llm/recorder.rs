//! LLM call trajectory recorder — captures each request/response pair and
//! exports the full session as an ATIF v1.6 document.
//!
//! Similar to how `~/.claude` or `~/.qoder` persist conversation histories,
//! this recorder saves the optimization analysis LLM interactions to disk
//! in ATIF format for debugging, auditing, and replay.

use std::path::Path;
use std::sync::Mutex;

use chrono::Utc;

use super::types::ChatMessage;
use crate::atif::{AtifAgent, AtifFinalMetrics, AtifStep, AtifStepMetrics, AtifTrajectory};

/// A single recorded LLM call.
#[derive(Debug, Clone)]
pub struct RecordedCall {
    /// Label identifying the call purpose (e.g. "perf:fast_tool", "accuracy:extract").
    pub label: String,
    /// Input messages sent to the LLM.
    pub messages: Vec<ChatMessage>,
    /// The LLM response text.
    pub response: String,
    /// Model used for this call.
    pub model: String,
    /// Input token count (from usage).
    pub input_tokens: u32,
    /// Output token count (from usage).
    pub output_tokens: u32,
    /// ISO 8601 timestamp when the request was sent.
    pub start_ts: String,
    /// ISO 8601 timestamp when the response was received.
    pub end_ts: String,
}

/// Parameters for recording a single LLM call.
pub struct RecordParams<'a> {
    pub label: &'a str,
    pub messages: &'a [ChatMessage],
    pub response: &'a str,
    pub model: &'a str,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub start_ts: &'a str,
    pub end_ts: &'a str,
}

/// Thread-safe recorder that accumulates LLM calls during an analysis session.
#[derive(Debug, Default)]
pub struct TrajectoryRecorder {
    calls: Mutex<Vec<RecordedCall>>,
    model: String,
    session_label: String,
}

impl TrajectoryRecorder {
    /// Create a new recorder for a given analysis session.
    ///
    /// `session_label` identifies the analysis run (e.g. "perf-issues/session-abc123").
    pub fn new(model: impl Into<String>, session_label: impl Into<String>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            model: model.into(),
            session_label: session_label.into(),
        }
    }

    /// Record a completed LLM call.
    pub fn record(&self, params: RecordParams<'_>) {
        let call = RecordedCall {
            label: params.label.to_string(),
            messages: params.messages.to_vec(),
            response: params.response.to_string(),
            model: params.model.to_string(),
            input_tokens: params.input_tokens,
            output_tokens: params.output_tokens,
            start_ts: params.start_ts.to_string(),
            end_ts: params.end_ts.to_string(),
        };
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(call);
        }
    }

    /// Number of recorded calls.
    pub fn len(&self) -> usize {
        self.calls.lock().map(|c| c.len()).unwrap_or(0)
    }

    /// Whether no calls have been recorded.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Export recorded calls as an ATIF trajectory document.
    ///
    /// Each recorded LLM call becomes a group of steps:
    /// - system message → system step
    /// - user/assistant messages → user steps
    /// - LLM response → agent step (with metrics)
    pub fn to_atif(&self) -> AtifTrajectory {
        let calls = self.calls.lock().map(|c| c.clone()).unwrap_or_default();

        let mut steps: Vec<AtifStep> = Vec::new();
        let mut step_id: u32 = 0;
        let mut total_prompt: u64 = 0;
        let mut total_completion: u64 = 0;

        for call in &calls {
            // Add a "user" step describing the analysis task (the label).
            step_id += 1;
            steps.push(AtifStep {
                step_id,
                timestamp: Some(call.start_ts.clone()),
                source: "user".to_string(),
                message: Some(format!("[{}]", call.label)),
                model_name: None,
                reasoning_content: None,
                tool_calls: None,
                observation: None,
                metrics: None,
                extra: None,
            });

            // System prompt step (first system message, if any).
            if let Some(sys) = call.messages.iter().find(|m| m.role == "system") {
                step_id += 1;
                steps.push(AtifStep {
                    step_id,
                    timestamp: Some(call.start_ts.clone()),
                    source: "system".to_string(),
                    message: Some(sys.content.clone()),
                    model_name: None,
                    reasoning_content: None,
                    tool_calls: None,
                    observation: None,
                    metrics: None,
                    extra: None,
                });
            }

            // User messages (non-system) as user steps.
            for msg in call.messages.iter().filter(|m| m.role != "system") {
                step_id += 1;
                let source = if msg.role == "assistant" {
                    "agent"
                } else {
                    "user"
                };
                steps.push(AtifStep {
                    step_id,
                    timestamp: Some(call.start_ts.clone()),
                    source: source.to_string(),
                    message: Some(msg.content.clone()),
                    model_name: if source == "agent" {
                        Some(call.model.clone())
                    } else {
                        None
                    },
                    reasoning_content: None,
                    tool_calls: None,
                    observation: None,
                    metrics: None,
                    extra: None,
                });
            }

            // Agent response step.
            step_id += 1;
            total_prompt += call.input_tokens as u64;
            total_completion += call.output_tokens as u64;
            steps.push(AtifStep {
                step_id,
                timestamp: Some(call.end_ts.clone()),
                source: "agent".to_string(),
                message: Some(call.response.clone()),
                model_name: Some(call.model.clone()),
                reasoning_content: None,
                tool_calls: None,
                observation: None,
                metrics: Some(AtifStepMetrics {
                    prompt_tokens: Some(call.input_tokens),
                    completion_tokens: Some(call.output_tokens),
                    cached_tokens: None,
                    extra: None,
                }),
                extra: Some(serde_json::json!({
                    "start_timestamp": call.start_ts,
                    "label": call.label,
                })),
            });
        }

        AtifTrajectory {
            schema_version: "ATIF-v1.6".to_string(),
            session_id: self.session_label.clone(),
            agent: Some(AtifAgent {
                name: "agentsight-opt".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                model_name: Some(self.model.clone()),
                tool_definitions: None,
                extra: None,
            }),
            steps,
            final_metrics: Some(AtifFinalMetrics {
                total_prompt_tokens: Some(total_prompt),
                total_completion_tokens: Some(total_completion),
                total_cached_tokens: None,
                total_steps: Some(step_id),
                extra: None,
            }),
            extra: Some(serde_json::json!({
                "recorded_at": Utc::now().to_rfc3339(),
                "call_count": calls.len(),
            })),
        }
    }

    /// Export and save the ATIF trajectory to a directory.
    ///
    /// File name: `{session_label}_{timestamp}.atif.json`
    /// Returns the path of the saved file.
    pub fn save_to_dir(&self, dir: &Path) -> std::io::Result<std::path::PathBuf> {
        std::fs::create_dir_all(dir)?;

        let ts = Utc::now().format("%Y%m%d_%H%M%S");
        // Sanitize session_label for file name.
        let safe_label: String = self
            .session_label
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let file_name = format!("{safe_label}_{ts}.atif.json");
        let path = dir.join(file_name);

        let doc = self.to_atif();
        let json =
            serde_json::to_string_pretty(&doc).map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::write(&path, json)?;

        tracing::info!(
            "Saved opt LLM trajectory ({} calls) to {}",
            self.len(),
            path.display()
        );
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recorder_saves_atif() {
        let recorder = TrajectoryRecorder::new("gpt-4o", "perf-issues/test-session");

        // Simulate two LLM calls.
        recorder.record(RecordParams {
            label: "perf:fast_tool",
            messages: &[
                ChatMessage::system("You are a perf analyzer."),
                ChatMessage::user("Analyze this trajectory."),
            ],
            response: r#"{"issues": []}"#,
            model: "gpt-4o",
            input_tokens: 1500,
            output_tokens: 42,
            start_ts: "2026-07-24T10:00:00+00:00",
            end_ts: "2026-07-24T10:00:05+00:00",
        });
        recorder.record(RecordParams {
            label: "perf:prefix_cache",
            messages: &[
                ChatMessage::system("You are a cache analyzer."),
                ChatMessage::user("Check cache hits."),
            ],
            response: r#"{"applicable": true}"#,
            model: "gpt-4o",
            input_tokens: 800,
            output_tokens: 30,
            start_ts: "2026-07-24T10:00:05+00:00",
            end_ts: "2026-07-24T10:00:08+00:00",
        });

        assert_eq!(recorder.len(), 2);
        assert!(!recorder.is_empty());

        // Verify ATIF export structure.
        let doc = recorder.to_atif();
        assert_eq!(doc.schema_version, "ATIF-v1.6");
        assert_eq!(doc.session_id, "perf-issues/test-session");
        assert!(doc.steps.len() > 2); // at least label + system + user + agent per call
        let metrics = doc.final_metrics.as_ref().unwrap();
        assert_eq!(metrics.total_prompt_tokens, Some(2300));
        assert_eq!(metrics.total_completion_tokens, Some(72));

        // Save to temp dir and verify file exists.
        let tmp_dir = std::env::temp_dir().join("agentsight-opt-test-trajectories");
        let path = recorder.save_to_dir(&tmp_dir).unwrap();
        assert!(path.exists());
        assert!(path.to_str().unwrap().ends_with(".atif.json"));

        // Verify the file is valid ATIF JSON.
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["schema_version"], "ATIF-v1.6");
        assert_eq!(parsed["agent"]["name"], "agentsight-opt");
        assert!(!parsed["steps"].as_array().unwrap().is_empty());

        // Cleanup.
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&tmp_dir);

        println!(
            "✓ ATIF trajectory saved and verified at: {}",
            path.display()
        );
    }
}
