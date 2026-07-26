//! LLM call trajectory recorder — captures each request/response pair and
//! exports the full session as an ATIF v1.7 document.
//!
//! Similar to how `~/.claude` or `~/.qoder` persist conversation histories,
//! this recorder saves the optimization analysis LLM interactions to disk
//! in ATIF format for debugging, auditing, and replay.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use agentsight_atif::{
    Agent, AtifTrajectory, FinalMetrics, Metrics, Step, StepSource, ATIF_SCHEMA_VERSION,
};
use chrono::Utc;

use super::types::ChatMessage;

/// A single recorded LLM call.
#[derive(Debug, Clone)]
pub struct RecordedCall {
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

    /// Export recorded calls as an ATIF v1.7 trajectory document.
    ///
    /// Each recorded LLM call becomes a group of steps:
    /// - system message -> system step
    /// - user/assistant messages -> user steps
    /// - LLM response -> agent step (with metrics)
    pub fn to_atif(&self) -> AtifTrajectory {
        let calls = self.calls.lock().map(|c| c.clone()).unwrap_or_default();

        let mut steps: Vec<Step> = Vec::new();
        let mut step_id: usize = 0;
        let mut total_prompt: u64 = 0;
        let mut total_completion: u64 = 0;

        for call in &calls {
            // System prompt step (first system message, if any).
            if let Some(sys) = call.messages.iter().find(|m| m.role == "system") {
                step_id += 1;
                steps.push(Step {
                    step_id,
                    timestamp: Some(call.start_ts.clone()),
                    source: StepSource::System,
                    message: sys.content.clone(),
                    model_name: None,
                    reasoning_effort: None,
                    reasoning_content: None,
                    tool_calls: None,
                    observation: None,
                    metrics: None,
                    extra: None,
                    llm_call_count: None,
                    is_copied_context: None,
                });
            }

            // User messages (non-system) as user steps.
            for msg in call.messages.iter().filter(|m| m.role != "system") {
                step_id += 1;
                let source = if msg.role == "assistant" {
                    StepSource::Agent
                } else {
                    StepSource::User
                };
                steps.push(Step {
                    step_id,
                    timestamp: Some(call.start_ts.clone()),
                    source,
                    message: msg.content.clone(),
                    model_name: if msg.role == "assistant" {
                        Some(call.model.clone())
                    } else {
                        None
                    },
                    reasoning_effort: None,
                    reasoning_content: None,
                    tool_calls: None,
                    observation: None,
                    metrics: None,
                    extra: None,
                    llm_call_count: None,
                    is_copied_context: None,
                });
            }

            // Agent response step.
            step_id += 1;
            total_prompt += call.input_tokens as u64;
            total_completion += call.output_tokens as u64;
            let mut extra = HashMap::new();
            extra.insert(
                "start_timestamp".to_string(),
                serde_json::Value::String(call.start_ts.clone()),
            );
            steps.push(Step {
                step_id,
                timestamp: Some(call.end_ts.clone()),
                source: StepSource::Agent,
                message: call.response.clone(),
                model_name: Some(call.model.clone()),
                reasoning_effort: None,
                reasoning_content: None,
                tool_calls: None,
                observation: None,
                metrics: Some(Metrics {
                    prompt_tokens: Some(call.input_tokens as u64),
                    completion_tokens: Some(call.output_tokens as u64),
                    cached_tokens: None,
                    cost_usd: None,
                    logprobs: None,
                    completion_token_ids: None,
                    prompt_token_ids: None,
                    extra: None,
                }),
                extra: Some(extra),
                llm_call_count: None,
                is_copied_context: None,
            });
        }

        let mut doc_extra = HashMap::new();
        doc_extra.insert(
            "recorded_at".to_string(),
            serde_json::Value::String(Utc::now().to_rfc3339()),
        );
        doc_extra.insert("call_count".to_string(), serde_json::json!(calls.len()));

        AtifTrajectory {
            schema_version: ATIF_SCHEMA_VERSION.to_string(),
            session_id: Some(self.session_label.clone()),
            trajectory_id: None,
            notes: None,
            agent: Agent {
                name: "agentsight-opt".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                model_name: Some(self.model.clone()),
                tool_definitions: None,
                extra: None,
            },
            steps,
            final_metrics: Some(FinalMetrics {
                total_prompt_tokens: Some(total_prompt),
                total_completion_tokens: Some(total_completion),
                total_cached_tokens: None,
                total_cost_usd: None,
                total_steps: Some(step_id),
                extra: None,
            }),
            continued_trajectory_ref: None,
            extra: Some(doc_extra),
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

        let json = self.to_atif_json()?;
        std::fs::write(&path, json)?;

        tracing::info!(
            "Saved opt LLM trajectory ({} calls) to {}",
            self.len(),
            path.display()
        );
        Ok(path)
    }

    /// Serialize the ATIF v1.7 document to a pretty JSON string.
    ///
    /// # Errors
    /// Returns an I/O error if serialization fails.
    pub fn to_atif_json(&self) -> std::io::Result<String> {
        let doc = self.to_atif();
        serde_json::to_string_pretty(&doc).map_err(|e| std::io::Error::other(e.to_string()))
    }

    /// The session label identifying this analysis run.
    pub fn session_label(&self) -> &str {
        &self.session_label
    }

    /// The model used for LLM calls.
    pub fn model_name(&self) -> &str {
        &self.model
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

        // Verify ATIF v1.7 export structure.
        let doc = recorder.to_atif();
        assert_eq!(doc.schema_version, "ATIF-v1.7");
        assert_eq!(doc.session_id.as_deref(), Some("perf-issues/test-session"));
        assert!(doc.steps.len() > 2); // at least system + user + agent per call
        let metrics = doc.final_metrics.as_ref().unwrap();
        assert_eq!(metrics.total_prompt_tokens, Some(2300));
        assert_eq!(metrics.total_completion_tokens, Some(72));

        // Save to temp dir and verify file exists.
        let tmp_dir = std::env::temp_dir().join("agentsight-opt-test-trajectories");
        let path = recorder.save_to_dir(&tmp_dir).unwrap();
        assert!(path.exists());
        assert!(path.to_str().unwrap().ends_with(".atif.json"));

        // Verify the file is valid ATIF v1.7 JSON.
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["schema_version"], "ATIF-v1.7");
        assert_eq!(parsed["agent"]["name"], "agentsight-opt");
        assert!(!parsed["steps"].as_array().unwrap().is_empty());

        // Cleanup.
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&tmp_dir);

        println!(
            "ATIF v1.7 trajectory saved and verified at: {}",
            path.display()
        );
    }
}
