//! ATIF v1.7 data models — shared trajectory schema crate.
//!
//! Ported from AgentOpt's `atif/models.rs` (the authoritative ATIF v1.7
//! implementation). Schema follows:
//! <https://github.com/harbor-framework/harbor/blob/main/rfcs/0001-trajectory-format.md>
//!
//! This is a leaf crate (serde/serde_json only) and the single ATIF data model
//! for the workspace: the log collector (`agentsight-trajectory-collector`) and
//! the eBPF export path (`agentsight::atif`) both produce these types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const ATIF_SCHEMA_VERSION: &str = "ATIF-v1.7";

// ---------------------------------------------------------------------------
// ToolCall
// ---------------------------------------------------------------------------

/// A tool/function invocation made by the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub tool_call_id: String,
    pub function_name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<HashMap<String, serde_json::Value>>,
}

// ---------------------------------------------------------------------------
// SubagentTrajectoryRef
// ---------------------------------------------------------------------------

/// Reference to a delegated subagent trajectory (ATIF v1.7).
///
/// Resolution: `trajectory_id` matches an embedded entry in the parent's
/// `subagent_trajectories` array; `trajectory_path` points at an external file.
/// At least one MUST be set. `session_id` is informational only (run-scoped).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentTrajectoryRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trajectory_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trajectory_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<HashMap<String, serde_json::Value>>,
}

// ---------------------------------------------------------------------------
// ObservationResult & Observation
// ---------------------------------------------------------------------------

/// Key under which a provider's out-of-band tool-failure flag is preserved in
/// [`ObservationResult::extra`]. ATIF has no typed status field, and folding the
/// flag into `content` would force consumers to re-derive failure by matching
/// error words in free text — which misfires on successful calls whose output
/// merely mentions "error".
pub const EXTRA_IS_ERROR: &str = "is_error";

/// Whether two identifiers name the same tool call.
///
/// Comparison first accepts the original identifiers case-insensitively. It
/// collapses `_` and `-` only when exactly one side contains a separator, which
/// covers the measured `call_cb113b8e…` → `callcb113b8e…` echo without merging
/// distinct IDs that both use punctuation (`call_a-1` vs `call_a_1`).
pub fn same_call_id(a: &str, b: &str) -> bool {
    if a.eq_ignore_ascii_case(b) {
        return true;
    }

    let has_separator = |s: &str| s.contains(['_', '-']);
    if has_separator(a) == has_separator(b) {
        return false;
    }

    let normalized = |s: &str| -> String {
        s.chars()
            .filter(|c| *c != '_' && *c != '-')
            .flat_map(char::to_lowercase)
            .collect()
    };
    normalized(a) == normalized(b)
}

/// A single observation result from a tool call or action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservationResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_trajectory_ref: Option<Vec<SubagentTrajectoryRef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<HashMap<String, serde_json::Value>>,
}

impl ObservationResult {
    /// Whether the tool call behind this result failed, as reported by the
    /// provider.
    ///
    /// `None` means the provider reported nothing, which is **not** the same as
    /// success — callers must treat it as unknown rather than passing.
    pub fn is_error(&self) -> Option<bool> {
        self.extra.as_ref()?.get(EXTRA_IS_ERROR)?.as_bool()
    }
}

/// Environment feedback/results after actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    #[serde(default)]
    pub results: Vec<ObservationResult>,
}

// ---------------------------------------------------------------------------
// Metrics & FinalMetrics
// ---------------------------------------------------------------------------

/// LLM operational metrics for a single step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<Vec<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_token_ids: Option<Vec<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_token_ids: Option<Vec<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<HashMap<String, serde_json::Value>>,
}

/// Aggregate metrics for the entire trajectory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalMetrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_prompt_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_completion_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cached_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_steps: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<HashMap<String, serde_json::Value>>,
}

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

/// Agent configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_definitions: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<HashMap<String, serde_json::Value>>,
}

fn default_version() -> String {
    "unknown".into()
}

// ---------------------------------------------------------------------------
// Step
// ---------------------------------------------------------------------------

/// Allowed `source` values for a Step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepSource {
    System,
    User,
    Agent,
}

/// A single step in an ATIF trajectory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub step_id: usize,
    pub source: StepSource,
    #[serde(default)]
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation: Option<Observation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Metrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<HashMap<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_call_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_copied_context: Option<bool>,
}

// ---------------------------------------------------------------------------
// Trajectory (root)
// ---------------------------------------------------------------------------

/// Root-level ATIF trajectory object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtifTrajectory {
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
    pub agent: Agent,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trajectory_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_metrics: Option<FinalMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continued_trajectory_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_trajectories: Option<Vec<AtifTrajectory>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<HashMap<String, serde_json::Value>>,
}

fn default_schema_version() -> String {
    ATIF_SCHEMA_VERSION.into()
}

impl AtifTrajectory {
    /// Serialize to a JSON Value with `None` fields stripped out.
    ///
    /// # Errors
    /// Returns a `serde_json` error if serialization fails.
    pub fn to_json_value(&self) -> serde_json::Result<serde_json::Value> {
        let v = serde_json::to_value(self)?;
        Ok(strip_nulls(v))
    }

    /// Validate step_id sequence (1-based, contiguous).
    ///
    /// # Errors
    /// Returns a message describing the first out-of-sequence step.
    pub fn validate_step_ids(&self) -> Result<(), String> {
        for (i, step) in self.steps.iter().enumerate() {
            if step.step_id != i + 1 {
                return Err(format!(
                    "Step at index {} has step_id={}, expected {}",
                    i,
                    step.step_id,
                    i + 1
                ));
            }
        }
        Ok(())
    }
}

/// Recursively strip null values from a JSON object.
fn strip_nulls(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let filtered: serde_json::Map<String, serde_json::Value> = map
                .into_iter()
                .filter(|(_, val)| !val.is_null())
                .map(|(k, val)| (k, strip_nulls(val)))
                .collect();
            serde_json::Value::Object(filtered)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(strip_nulls).collect())
        }
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Validator
// ---------------------------------------------------------------------------

/// Validate an ATIF trajectory from a JSON string.
///
/// # Errors
/// Returns a message on parse failure, schema mismatch, or bad step ids.
pub fn validate_trajectory_str(json: &str) -> Result<AtifTrajectory, String> {
    let data: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let trajectory: AtifTrajectory =
        serde_json::from_value(data).map_err(|e| format!("Schema validation failed: {e}"))?;
    trajectory.validate_step_ids()?;
    Ok(trajectory)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_trajectory() -> AtifTrajectory {
        AtifTrajectory {
            schema_version: ATIF_SCHEMA_VERSION.into(),
            agent: Agent {
                name: "qoder".into(),
                version: "unknown".into(),
                model_name: Some("qwen-max".into()),
                tool_definitions: None,
                extra: None,
            },
            steps: vec![Step {
                step_id: 1,
                source: StepSource::User,
                message: "hello".into(),
                timestamp: Some("2026-07-25T00:00:00Z".into()),
                model_name: None,
                reasoning_effort: None,
                reasoning_content: None,
                tool_calls: None,
                observation: None,
                metrics: None,
                extra: None,
                llm_call_count: None,
                is_copied_context: None,
            }],
            session_id: Some("s-1".into()),
            trajectory_id: None,
            notes: None,
            final_metrics: None,
            continued_trajectory_ref: None,
            subagent_trajectories: None,
            extra: None,
        }
    }

    #[test]
    fn call_id_matching_only_tolerates_one_sided_separator_loss() {
        assert!(same_call_id("CALL_abc", "call_ABC"));
        assert!(same_call_id("call_47ad96", "call47ad96"));
        assert!(same_call_id("call47ad96", "call-47ad96"));

        assert!(!same_call_id("call_a-1", "call_a_1"));
        assert!(!same_call_id("call-a-b", "call_a_b"));
        assert!(!same_call_id("call_abc", "call_def"));
    }

    #[test]
    fn schema_version_constant() {
        assert_eq!(ATIF_SCHEMA_VERSION, "ATIF-v1.7");
    }

    #[test]
    fn serialize_skips_none_fields() {
        let traj = minimal_trajectory();
        let json = serde_json::to_string(&traj).unwrap();
        assert!(!json.contains("trajectory_id"));
        assert!(!json.contains("reasoning_content"));
        assert!(json.contains("\"source\":\"user\""));
    }

    #[test]
    fn roundtrip_and_validate() {
        let traj = minimal_trajectory();
        let json = serde_json::to_string(&traj).unwrap();
        let parsed = validate_trajectory_str(&json).unwrap();
        assert_eq!(parsed.schema_version, ATIF_SCHEMA_VERSION);
        assert_eq!(parsed.session_id.as_deref(), Some("s-1"));
        assert_eq!(parsed.steps.len(), 1);
    }

    #[test]
    fn validate_rejects_bad_step_ids() {
        let mut traj = minimal_trajectory();
        traj.steps[0].step_id = 5;
        assert!(traj.validate_step_ids().is_err());
    }

    #[test]
    fn strip_nulls_removes_nested_nulls() {
        let v = serde_json::json!({"a": null, "b": {"c": null, "d": 1}, "e": [null, 2]});
        let stripped = strip_nulls(v);
        assert_eq!(stripped, serde_json::json!({"b": {"d": 1}, "e": [null, 2]}));
    }
}
