//! LLM-as-Judge trajectory evaluator

use super::types::{format_messages_to_string, ChatMessage, EvaluatorResult, FewShotExample, Score};
use genai_crate::chat::{ChatMessage as GenaiChatMessage, ChatRequest};
use genai_crate::Client;

/// Default trajectory accuracy prompt (no reference needed)
pub const TRAJECTORY_ACCURACY_PROMPT: &str = r#"You are an expert data labeler.
Your task is to grade the accuracy of an AI agent's internal trajectory.

<Rubric>
  An accurate trajectory:
  - Makes logical sense between steps
  - Shows clear progression
  - Is relatively efficient, though it does not need to be perfectly efficient
</Rubric>

First, try to understand the goal of the trajectory by looking at the input
(if the input is not present try to infer it from the content of the first message),
as well as the output of the final message. Once you understand the goal, grade the trajectory
as it relates to achieving that goal.

Grade the following trajectory:

<trajectory>
{outputs}
</trajectory>

Respond with your reasoning followed by the score.
Format your final score as: "Thus, the score should be: true" or "Thus, the score should be: false"
"#;

/// Prompt with reference trajectory
pub const TRAJECTORY_ACCURACY_PROMPT_WITH_REFERENCE: &str = r#"You are an expert data labeler.
Your task is to grade the accuracy of an AI agent's internal trajectory.

<Rubric>
  An accurate trajectory:
  - Makes logical sense between steps
  - Shows clear progression
  - Is relatively efficient, though it does not need to be perfectly efficient
  - Is semantically equivalent to the provided reference trajectory
</Rubric>

Based on the following reference trajectory:

<reference_trajectory>
{reference_outputs}
</reference_trajectory>

Grade this actual trajectory:

<trajectory>
{outputs}
</trajectory>

Respond with your reasoning followed by the score.
Format your final score as: "Thus, the score should be: true" or "Thus, the score should be: false"
"#;

/// LLM-as-Judge evaluator configuration
pub struct TrajectoryLlmJudgeConfig {
    pub prompt: String,
    pub model: String,
    pub feedback_key: String,
    pub continuous: bool,
    pub choices: Option<Vec<f64>>,
    pub use_reasoning: bool,
    pub few_shot_examples: Vec<FewShotExample>,
}

impl Default for TrajectoryLlmJudgeConfig {
    fn default() -> Self {
        Self {
            prompt: TRAJECTORY_ACCURACY_PROMPT.to_string(),
            model: "openai:gpt-4o-mini".to_string(),
            feedback_key: "trajectory_accuracy".to_string(),
            continuous: false,
            choices: None,
            use_reasoning: true,
            few_shot_examples: Vec::new(),
        }
    }
}

/// LLM-as-Judge trajectory evaluator
pub struct TrajectoryLlmJudge {
    config: TrajectoryLlmJudgeConfig,
    client: Client,
}

impl TrajectoryLlmJudge {
    pub fn new() -> Self {
        Self {
            config: TrajectoryLlmJudgeConfig::default(),
            client: Client::default(),
        }
    }

    pub fn with_config(mut self, config: TrajectoryLlmJudgeConfig) -> Self {
        self.config = config;
        self
    }

    /// Set a pre-configured Client (e.g., with custom auth resolver for API key injection)
    pub fn with_client(mut self, client: Client) -> Self {
        self.client = client;
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.config.model = model.into();
        self
    }

    pub fn with_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.config.prompt = prompt.into();
        self
    }

    pub fn with_feedback_key(mut self, key: impl Into<String>) -> Self {
        self.config.feedback_key = key.into();
        self
    }

    pub fn with_continuous(mut self, continuous: bool) -> Self {
        self.config.continuous = continuous;
        self
    }

    pub fn with_reasoning(mut self, use_reasoning: bool) -> Self {
        self.config.use_reasoning = use_reasoning;
        self
    }

    pub fn with_few_shot_examples(mut self, examples: Vec<FewShotExample>) -> Self {
        self.config.few_shot_examples = examples;
        self
    }

    /// Evaluate trajectory (no reference needed)
    pub async fn evaluate(
        &self,
        outputs: &[ChatMessage],
    ) -> anyhow::Result<EvaluatorResult> {
        self.evaluate_with_reference(outputs, None).await
    }

    /// Evaluate trajectory with optional reference
    pub async fn evaluate_with_reference(
        &self,
        outputs: &[ChatMessage],
        reference_outputs: Option<&[ChatMessage]>,
    ) -> anyhow::Result<EvaluatorResult> {
        let formatted_outputs = format_messages_to_string(outputs);
        let formatted_reference = reference_outputs.map(format_messages_to_string);

        let prompt = self.build_prompt(&formatted_outputs, formatted_reference.as_deref());

        let chat_req = ChatRequest::new(vec![
            GenaiChatMessage::system("You are an expert evaluator."),
            GenaiChatMessage::user(prompt),
        ]);

        let chat_res = self.client.exec_chat(&self.config.model, chat_req, None).await?;
        let response = chat_res.first_text().unwrap_or("");

        let (score_bool, score_float, reasoning) =
            parse_llm_response(response, self.config.use_reasoning);

        let score = if self.config.continuous {
            score_float
                .map(Score::Float)
                .or_else(|| score_bool.map(|b| Score::Float(if b { 1.0 } else { 0.0 })))
                .unwrap_or(Score::Float(0.0))
        } else {
            score_bool
                .map(Score::Boolean)
                .unwrap_or(Score::Boolean(false))
        };

        Ok(EvaluatorResult {
            key: self.config.feedback_key.clone(),
            score,
            comment: reasoning,
        })
    }

    fn build_prompt(&self, outputs: &str, reference_outputs: Option<&str>) -> String {
        let mut prompt = self.config.prompt.clone();

        prompt = prompt.replace("{outputs}", outputs);

        if let Some(ref_output) = reference_outputs {
            prompt = prompt.replace("{reference_outputs}", ref_output);
        } else {
            prompt = prompt.replace("{reference_outputs}", "N/A");
        }

        if !self.config.few_shot_examples.is_empty() {
            prompt.push_str("\n\nHere are some examples:\n");
            prompt.push_str(&format_few_shot_examples(&self.config.few_shot_examples));
        }

        prompt
    }
}

impl Default for TrajectoryLlmJudge {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_llm_response(response: &str, use_reasoning: bool) -> (Option<bool>, Option<f64>, Option<String>) {
    let response_lower = response.to_lowercase();

    let score_bool = if response_lower.contains("score should be: true")
        || response_lower.contains("score: true")
    {
        Some(true)
    } else if response_lower.contains("score should be: false")
        || response_lower.contains("score: false")
    {
        Some(false)
    } else {
        None
    };

    let score_float = extract_float_score(&response_lower);

    let reasoning = if use_reasoning {
        Some(response.trim().to_string())
    } else {
        None
    };

    (score_bool, score_float, reasoning)
}

fn extract_float_score(response: &str) -> Option<f64> {
    if let Some(idx) = response.find("score:") {
        let after = &response[idx + 6..];
        if let Some(num_str) = after.split_whitespace().next() {
            if let Ok(num) = num_str.parse::<f64>() {
                return Some(num);
            }
        }
    }
    None
}

fn format_few_shot_examples(examples: &[FewShotExample]) -> String {
    examples
        .iter()
        .enumerate()
        .map(|(i, ex)| {
            let mut result = format!("Example {}:\n", i + 1);
            if let Some(inputs) = &ex.inputs {
                result.push_str(&format!("Input: {}\n", inputs));
            }
            result.push_str(&format!("Output: {}\n", ex.outputs));
            result.push_str(&format!("Reasoning: {}\n", ex.reasoning));
            result.push_str(&format!(
                "Score: {}\n",
                match &ex.score {
                    Score::Boolean(b) => b.to_string(),
                    Score::Float(f) => f.to_string(),
                }
            ));
            result
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Convenience function: create and run evaluation
pub async fn evaluate_trajectory(
    outputs: &[ChatMessage],
    model: Option<&str>,
) -> anyhow::Result<EvaluatorResult> {
    let mut judge = TrajectoryLlmJudge::new();

    if let Some(m) = model {
        judge = judge.with_model(m);
    }

    judge.evaluate(outputs).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_prompt_without_reference() {
        let judge = TrajectoryLlmJudge::new();
        let outputs = "User: Hello\nAssistant: Hi";

        let prompt = judge.build_prompt(outputs, None);

        assert!(prompt.contains("User: Hello"));
        assert!(prompt.contains("Assistant: Hi"));
    }

    #[test]
    fn test_build_prompt_with_reference() {
        let judge = TrajectoryLlmJudge::new()
            .with_prompt(TRAJECTORY_ACCURACY_PROMPT_WITH_REFERENCE);

        let outputs = "User: Hello";
        let reference = "User: Hello\nAssistant: Hi";

        let prompt = judge.build_prompt(outputs, Some(reference));

        assert!(prompt.contains("User: Hello"));
        assert!(prompt.contains("reference_trajectory"));
    }

    #[test]
    fn test_parse_llm_response() {
        let response = "The trajectory is good. Thus, the score should be: true";
        let (score_bool, _, _) = parse_llm_response(response, false);
        assert_eq!(score_bool, Some(true));

        let response2 = "The trajectory is bad. Thus, the score should be: false";
        let (score_bool2, _, _) = parse_llm_response(response2, false);
        assert_eq!(score_bool2, Some(false));
    }
}

