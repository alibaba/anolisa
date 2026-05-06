//! ATIF v1.6 (Agent Trajectory Interchange Format) data structures
//!
//! Defines Rust types that serialize to/from the ATIF v1.6 JSON schema.
//! See: <https://github.com/laude-institute/harbor/blob/main/docs/rfcs/0001-trajectory-format.md>

use serde::{Serialize, Deserialize};

/// Current ATIF schema version
pub const SCHEMA_VERSION: &str = "ATIF-v1.6";

// ─── Root Document ───────────────────────────────────────────────────────────

/// Root-level ATIF trajectory document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtifDocument {
    /// String defining ATIF compatibility (e.g., "ATIF-v1.6")
    pub schema_version: String,
    /// Unique identifier for the entire agent run
    pub session_id: String,
    /// Agent configuration
    pub agent: AtifAgent,
    /// Array of step objects representing the complete interaction history
    pub steps: Vec<AtifStep>,
    /// Summary metrics for the entire trajectory
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_metrics: Option<AtifFinalMetrics>,
    /// Custom root-level metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

// ─── Agent ───────────────────────────────────────────────────────────────────

/// Agent system identification and configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtifAgent {
    /// The name of the agent system
    pub name: String,
    /// The version identifier of the agent system
    pub version: String,
    /// Default LLM model used for this trajectory
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    /// Array of tool/function definitions available to the agent
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_definitions: Option<Vec<serde_json::Value>>,
    /// Custom agent configuration details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

// ─── Step ────────────────────────────────────────────────────────────────────

/// A single interaction step: system prompt, user message, or agent turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtifStep {
    /// Ordinal index of the turn (starting from 1)
    pub step_id: u32,
    /// ISO 8601 timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// The originator: "system", "user", or "agent"
    pub source: String,
    /// The dialogue message (text content)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The specific LLM model used for this turn (agent only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    /// Agent's explicit internal reasoning (agent only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// Structured tool/function invocations (agent only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<AtifToolCall>>,
    /// Environment feedback after actions (agent only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation: Option<AtifObservation>,
    /// LLM operational metrics for this step (agent only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<AtifStepMetrics>,
    /// Custom step-level metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

// ─── Tool Call ───────────────────────────────────────────────────────────────

/// A structured tool/function invocation by the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtifToolCall {
    /// Unique identifier for this tool call
    pub tool_call_id: String,
    /// The name of the function or tool being invoked
    pub function_name: String,
    /// Arguments passed to the function
    pub arguments: serde_json::Value,
}

// ─── Observation ─────────────────────────────────────────────────────────────

/// Environment feedback / results after actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtifObservation {
    /// Array of result objects from tool calls or actions
    pub results: Vec<AtifObservationResult>,
}

/// A single observation result from a tool call or action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtifObservationResult {
    /// The tool_call_id this result corresponds to
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_call_id: Option<String>,
    /// The output/result content
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

// ─── Metrics ─────────────────────────────────────────────────────────────────

/// Per-step LLM operational metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtifStepMetrics {
    /// Total input tokens sent to the model
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u32>,
    /// Total tokens generated by the LLM response
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u32>,
    /// Subset of prompt_tokens that were cache hits
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u32>,
    /// Provider-specific or experimental metrics
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

/// Aggregate statistics for the entire trajectory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtifFinalMetrics {
    /// Sum of all prompt tokens across all steps
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_prompt_tokens: Option<u64>,
    /// Sum of all completion tokens across all steps
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_completion_tokens: Option<u64>,
    /// Sum of all cached tokens across all steps
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cached_tokens: Option<u64>,
    /// Total number of steps
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_steps: Option<u32>,
    /// Custom aggregate metrics
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_version_constant() {
        assert_eq!(SCHEMA_VERSION, "ATIF-v1.6");
    }

    #[test]
    fn test_atif_document_roundtrip() {
        let doc = AtifDocument {
            schema_version: SCHEMA_VERSION.to_string(),
            session_id: "session-001".to_string(),
            agent: AtifAgent {
                name: "TestAgent".to_string(),
                version: "1.0.0".to_string(),
                model_name: Some("gpt-4".to_string()),
                tool_definitions: None,
                extra: None,
            },
            steps: vec![
                AtifStep {
                    step_id: 1,
                    timestamp: Some("2024-01-01T00:00:00Z".to_string()),
                    source: "user".to_string(),
                    message: Some("Hello".to_string()),
                    model_name: None,
                    reasoning_content: None,
                    tool_calls: None,
                    observation: None,
                    metrics: None,
                    extra: None,
                },
                AtifStep {
                    step_id: 2,
                    timestamp: Some("2024-01-01T00:00:01Z".to_string()),
                    source: "agent".to_string(),
                    message: Some("Hi there!".to_string()),
                    model_name: Some("gpt-4".to_string()),
                    reasoning_content: Some("User greeted me".to_string()),
                    tool_calls: None,
                    observation: None,
                    metrics: Some(AtifStepMetrics {
                        prompt_tokens: Some(100),
                        completion_tokens: Some(10),
                        cached_tokens: None,
                        extra: None,
                    }),
                    extra: None,
                },
            ],
            final_metrics: Some(AtifFinalMetrics {
                total_prompt_tokens: Some(100),
                total_completion_tokens: Some(10),
                total_cached_tokens: None,
                total_steps: Some(2),
                extra: None,
            }),
            extra: None,
        };

        let json = serde_json::to_string(&doc).unwrap();
        let back: AtifDocument = serde_json::from_str(&json).unwrap();
        assert_eq!(back.schema_version, "ATIF-v1.6");
        assert_eq!(back.session_id, "session-001");
        assert_eq!(back.agent.name, "TestAgent");
        assert_eq!(back.steps.len(), 2);
        assert_eq!(back.steps[0].source, "user");
        assert_eq!(back.steps[1].source, "agent");
        assert_eq!(back.final_metrics.unwrap().total_steps, Some(2));
    }

    #[test]
    fn test_atif_tool_call_roundtrip() {
        let tc = AtifToolCall {
            tool_call_id: "call-001".to_string(),
            function_name: "get_weather".to_string(),
            arguments: serde_json::json!({"location": "Tokyo"}),
        };
        let json = serde_json::to_string(&tc).unwrap();
        let back: AtifToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tool_call_id, "call-001");
        assert_eq!(back.function_name, "get_weather");
    }

    #[test]
    fn test_atif_observation_roundtrip() {
        let obs = AtifObservation {
            results: vec![AtifObservationResult {
                source_call_id: Some("call-001".to_string()),
                content: Some("Sunny, 25C".to_string()),
            }],
        };
        let json = serde_json::to_string(&obs).unwrap();
        let back: AtifObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(back.results.len(), 1);
        assert_eq!(back.results[0].content, Some("Sunny, 25C".to_string()));
    }

    #[test]
    fn test_step_with_tool_calls() {
        let step = AtifStep {
            step_id: 3,
            timestamp: None,
            source: "agent".to_string(),
            message: None,
            model_name: Some("claude-3".to_string()),
            reasoning_content: None,
            tool_calls: Some(vec![
                AtifToolCall {
                    tool_call_id: "tc1".to_string(),
                    function_name: "search".to_string(),
                    arguments: serde_json::json!({"query": "rust"}),
                },
            ]),
            observation: Some(AtifObservation {
                results: vec![AtifObservationResult {
                    source_call_id: Some("tc1".to_string()),
                    content: Some("Found 10 results".to_string()),
                }],
            }),
            metrics: Some(AtifStepMetrics {
                prompt_tokens: Some(500),
                completion_tokens: Some(50),
                cached_tokens: Some(200),
                extra: None,
            }),
            extra: None,
        };
        let json = serde_json::to_string(&step).unwrap();
        let back: AtifStep = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tool_calls.unwrap().len(), 1);
        assert_eq!(back.observation.unwrap().results.len(), 1);
        assert_eq!(back.metrics.unwrap().cached_tokens, Some(200));
    }

    #[test]
    fn test_skip_serializing_none_fields() {
        let step = AtifStep {
            step_id: 1,
            timestamp: None,
            source: "user".to_string(),
            message: Some("Hello".to_string()),
            model_name: None,
            reasoning_content: None,
            tool_calls: None,
            observation: None,
            metrics: None,
            extra: None,
        };
        let json = serde_json::to_string(&step).unwrap();
        // None fields should not be present in JSON
        assert!(!json.contains("model_name"));
        assert!(!json.contains("reasoning_content"));
        assert!(!json.contains("tool_calls"));
        assert!(!json.contains("observation"));
        assert!(!json.contains("metrics"));
    }
}
