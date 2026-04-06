//! ATIF → ChatMessage conversion
//!
//! Converts an AtifDocument into a sequence of genai ChatMessages suitable
//! for trajectory evaluation by the LLM-as-Judge.

use super::types::ChatMessage;
use crate::atif::{AtifDocument, AtifStep};
use genai_crate::chat::ToolResponse;

/// Convert an ATIF document's steps into a flat ChatMessage sequence.
///
/// Mapping rules:
/// - system step → System message
/// - user step   → User message
/// - agent step  → Assistant message (with optional tool calls), followed by
///                  Tool response messages for each observation result
pub fn atif_to_chat_messages(doc: &AtifDocument) -> Vec<ChatMessage> {
    let mut messages = Vec::new();

    for step in &doc.steps {
        convert_step(step, &mut messages);
    }

    messages
}

fn convert_step(step: &AtifStep, messages: &mut Vec<ChatMessage>) {
    match step.source.as_str() {
        "system" => {
            if let Some(ref text) = step.message {
                if !text.is_empty() {
                    messages.push(ChatMessage::system(text.as_str()));
                }
            }
        }
        "user" => {
            if let Some(ref text) = step.message {
                if !text.is_empty() {
                    messages.push(ChatMessage::user(text.as_str()));
                }
            }
        }
        "agent" => {
            convert_agent_step(step, messages);
        }
        _ => {
            // Unknown source type — treat as assistant if it has content
            if let Some(ref text) = step.message {
                if !text.is_empty() {
                    messages.push(ChatMessage::assistant(text.as_str()));
                }
            }
        }
    }
}

fn convert_agent_step(step: &AtifStep, messages: &mut Vec<ChatMessage>) {
    let has_tool_calls = step.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty());

    // Build the text content, optionally prefixing with reasoning
    let text_content = build_agent_text(step);

    if has_tool_calls {
        let tool_calls = step.tool_calls.as_ref().unwrap();

        // Build genai ToolCall objects
        let genai_tool_calls: Vec<genai_crate::chat::ToolCall> = tool_calls
            .iter()
            .map(|tc| genai_crate::chat::ToolCall {
                call_id: tc.tool_call_id.clone(),
                fn_name: tc.function_name.clone(),
                fn_arguments: tc.arguments.clone(),
                thought_signatures: None,
            })
            .collect();

        // If there's text content alongside tool calls, emit text first
        if let Some(ref text) = text_content {
            messages.push(ChatMessage::assistant(text.as_str()));
        }

        // Emit the tool calls as an assistant message
        messages.push(ChatMessage::from(genai_tool_calls));

        // Emit observation results as tool response messages
        if let Some(ref observation) = step.observation {
            for (i, result) in observation.results.iter().enumerate() {
                let call_id = result
                    .source_call_id
                    .clone()
                    .unwrap_or_else(|| format!("unknown_{}", i));
                let content = result
                    .content
                    .clone()
                    .unwrap_or_default();
                messages.push(ChatMessage::from(ToolResponse::new(call_id, content)));
            }
        }
    } else if let Some(ref text) = text_content {
        // Agent step with text only (no tool calls)
        messages.push(ChatMessage::assistant(text.as_str()));
    }
    // Skip empty agent steps (no message, no tool calls)
}

/// Build the text content for an agent step, optionally prefixing reasoning.
fn build_agent_text(step: &AtifStep) -> Option<String> {
    let has_reasoning = step.reasoning_content.as_ref().is_some_and(|r| !r.is_empty());
    let has_message = step.message.as_ref().is_some_and(|m| !m.is_empty());

    match (has_reasoning, has_message) {
        (true, true) => Some(format!(
            "<reasoning>{}</reasoning>\n{}",
            step.reasoning_content.as_ref().unwrap(),
            step.message.as_ref().unwrap()
        )),
        (true, false) => Some(format!(
            "<reasoning>{}</reasoning>",
            step.reasoning_content.as_ref().unwrap()
        )),
        (false, true) => Some(step.message.clone().unwrap()),
        (false, false) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atif::{
        AtifAgent, AtifDocument, AtifFinalMetrics, AtifObservation, AtifObservationResult,
        AtifStep, AtifToolCall,
    };

    fn make_doc(steps: Vec<AtifStep>) -> AtifDocument {
        AtifDocument {
            schema_version: "ATIF-v1.6".to_string(),
            session_id: "test-session".to_string(),
            agent: AtifAgent {
                name: "test-agent".to_string(),
                version: "1.0".to_string(),
                model_name: None,
                tool_definitions: None,
                extra: None,
            },
            steps,
            final_metrics: None,
            extra: None,
        }
    }

    #[test]
    fn test_system_step() {
        let doc = make_doc(vec![AtifStep {
            step_id: 1,
            timestamp: None,
            source: "system".to_string(),
            message: Some("You are a helpful assistant.".to_string()),
            model_name: None,
            reasoning_content: None,
            tool_calls: None,
            observation: None,
            metrics: None,
            extra: None,
        }]);

        let msgs = atif_to_chat_messages(&doc);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, genai_crate::chat::ChatRole::System);
    }

    #[test]
    fn test_user_step() {
        let doc = make_doc(vec![AtifStep {
            step_id: 1,
            timestamp: None,
            source: "user".to_string(),
            message: Some("What is the weather?".to_string()),
            model_name: None,
            reasoning_content: None,
            tool_calls: None,
            observation: None,
            metrics: None,
            extra: None,
        }]);

        let msgs = atif_to_chat_messages(&doc);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, genai_crate::chat::ChatRole::User);
    }

    #[test]
    fn test_agent_text_only() {
        let doc = make_doc(vec![AtifStep {
            step_id: 1,
            timestamp: None,
            source: "agent".to_string(),
            message: Some("The weather is sunny.".to_string()),
            model_name: None,
            reasoning_content: None,
            tool_calls: None,
            observation: None,
            metrics: None,
            extra: None,
        }]);

        let msgs = atif_to_chat_messages(&doc);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, genai_crate::chat::ChatRole::Assistant);
    }

    #[test]
    fn test_agent_with_tool_calls_and_observation() {
        let doc = make_doc(vec![AtifStep {
            step_id: 1,
            timestamp: None,
            source: "agent".to_string(),
            message: None,
            model_name: None,
            reasoning_content: None,
            tool_calls: Some(vec![AtifToolCall {
                tool_call_id: "call_1".to_string(),
                function_name: "get_weather".to_string(),
                arguments: serde_json::json!({"city": "SF"}),
            }]),
            observation: Some(AtifObservation {
                results: vec![AtifObservationResult {
                    source_call_id: Some("call_1".to_string()),
                    content: Some("80 degrees and sunny".to_string()),
                }],
            }),
            metrics: None,
            extra: None,
        }]);

        let msgs = atif_to_chat_messages(&doc);
        // Should produce: 1 assistant (tool calls) + 1 tool response
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, genai_crate::chat::ChatRole::Assistant);
        assert_eq!(msgs[1].role, genai_crate::chat::ChatRole::Tool);
    }

    #[test]
    fn test_agent_with_reasoning() {
        let doc = make_doc(vec![AtifStep {
            step_id: 1,
            timestamp: None,
            source: "agent".to_string(),
            message: Some("Here is the answer.".to_string()),
            model_name: None,
            reasoning_content: Some("I need to think about this.".to_string()),
            tool_calls: None,
            observation: None,
            metrics: None,
            extra: None,
        }]);

        let msgs = atif_to_chat_messages(&doc);
        assert_eq!(msgs.len(), 1);
        // Check reasoning is prefixed
        let text = super::super::types::format_message(&msgs[0]);
        assert!(text.contains("<reasoning>"));
        assert!(text.contains("Here is the answer."));
    }

    #[test]
    fn test_empty_steps_skipped() {
        let doc = make_doc(vec![
            AtifStep {
                step_id: 1,
                timestamp: None,
                source: "agent".to_string(),
                message: None,
                model_name: None,
                reasoning_content: None,
                tool_calls: None,
                observation: None,
                metrics: None,
                extra: None,
            },
            AtifStep {
                step_id: 2,
                timestamp: None,
                source: "system".to_string(),
                message: Some("".to_string()),
                model_name: None,
                reasoning_content: None,
                tool_calls: None,
                observation: None,
                metrics: None,
                extra: None,
            },
        ]);

        let msgs = atif_to_chat_messages(&doc);
        assert_eq!(msgs.len(), 0);
    }

    #[test]
    fn test_full_trajectory() {
        let doc = make_doc(vec![
            AtifStep {
                step_id: 1,
                timestamp: None,
                source: "system".to_string(),
                message: Some("You are a weather assistant.".to_string()),
                model_name: None,
                reasoning_content: None,
                tool_calls: None,
                observation: None,
                metrics: None,
                extra: None,
            },
            AtifStep {
                step_id: 2,
                timestamp: None,
                source: "user".to_string(),
                message: Some("What is the weather in SF?".to_string()),
                model_name: None,
                reasoning_content: None,
                tool_calls: None,
                observation: None,
                metrics: None,
                extra: None,
            },
            AtifStep {
                step_id: 3,
                timestamp: None,
                source: "agent".to_string(),
                message: None,
                model_name: Some("gpt-4o".to_string()),
                reasoning_content: None,
                tool_calls: Some(vec![AtifToolCall {
                    tool_call_id: "call_1".to_string(),
                    function_name: "get_weather".to_string(),
                    arguments: serde_json::json!({"city": "San Francisco"}),
                }]),
                observation: Some(AtifObservation {
                    results: vec![AtifObservationResult {
                        source_call_id: Some("call_1".to_string()),
                        content: Some("80 degrees, sunny".to_string()),
                    }],
                }),
                metrics: None,
                extra: None,
            },
            AtifStep {
                step_id: 4,
                timestamp: None,
                source: "agent".to_string(),
                message: Some("The weather in SF is 80 degrees and sunny.".to_string()),
                model_name: Some("gpt-4o".to_string()),
                reasoning_content: None,
                tool_calls: None,
                observation: None,
                metrics: None,
                extra: None,
            },
        ]);

        let msgs = atif_to_chat_messages(&doc);
        // system + user + assistant(tool calls) + tool response + assistant(final)
        assert_eq!(msgs.len(), 5);
        assert_eq!(msgs[0].role, genai_crate::chat::ChatRole::System);
        assert_eq!(msgs[1].role, genai_crate::chat::ChatRole::User);
        assert_eq!(msgs[2].role, genai_crate::chat::ChatRole::Assistant);
        assert_eq!(msgs[3].role, genai_crate::chat::ChatRole::Tool);
        assert_eq!(msgs[4].role, genai_crate::chat::ChatRole::Assistant);
    }
}

