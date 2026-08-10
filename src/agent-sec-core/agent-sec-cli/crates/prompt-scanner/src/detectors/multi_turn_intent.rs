//! L4 multi-turn intent detector — judges an assistant reply in context.
//!
//! L4 is an **optional** layer: it needs a running Ollama instance with
//! the target model loaded.  When unavailable the scanner skips it and
//! MULTI_TURN mode returns a pass-through verdict.  It only runs when the
//! caller explicitly selects multi-turn scanning, so no separate disable
//! flag exists.

use std::time::Instant;

use crate::detectors::{DetectInput, DetectionLayer};
use crate::error::ScannerError;
use crate::models::multi_turn_intent::MultiTurnIntentClassifier;
use crate::result::{LayerResult, ThreatDetail};

/// Max characters of the prompt kept as evidence.
const MAX_EVIDENCE_CHARS: usize = 200;

/// L4 detection layer: classifies a (history, query, response) triple.
pub struct MultiTurnIntentDetector {
    classifier: MultiTurnIntentClassifier,
}

impl MultiTurnIntentDetector {
    /// Build the layer with the given `p_harmful` block threshold.
    ///
    /// # Errors
    ///
    /// Returns [`ScannerError::Config`] when the configured model service
    /// backend is unsupported.
    pub fn new(harmful_threshold: f64) -> Result<Self, ScannerError> {
        Ok(MultiTurnIntentDetector {
            classifier: MultiTurnIntentClassifier::new(harmful_threshold)?,
        })
    }

    /// Build the layer over a pre-built classifier (used by tests).
    pub fn with_classifier(classifier: MultiTurnIntentClassifier) -> Self {
        MultiTurnIntentDetector { classifier }
    }

    /// A non-detection result, used whenever L4 cannot judge the input.
    fn passthrough(reason: &str, latency_ms: f64) -> LayerResult {
        log::debug!("multi_turn_intent passthrough: {reason}");
        LayerResult {
            layer_name: "multi_turn_intent".to_string(),
            detected: false,
            score: Some(0.0),
            details: vec![],
            latency_ms,
        }
    }
}

impl DetectionLayer for MultiTurnIntentDetector {
    fn name(&self) -> &'static str {
        "multi_turn_intent"
    }

    /// Whether Ollama is reachable and the target model is loaded.
    ///
    /// Reported as `false` (so the scanner skips the layer) on any error.
    fn is_available(&self) -> bool {
        self.classifier.check_ready()
    }

    /// Never fails: the layer fails open, so callers see a pass-through
    /// result instead of an error when classification is impossible.
    fn detect(&self, input: &DetectInput<'_>) -> Result<LayerResult, ScannerError> {
        let Some(conversation) = input.conversation.as_ref() else {
            return Ok(Self::passthrough("missing_conversation_context", 0.0));
        };

        let t0 = Instant::now();
        let response = match self.classifier.classify(
            conversation.history,
            input.text,
            conversation.assistant_response,
        ) {
            Ok(response) => response,
            Err(err) => {
                log::warn!("Intent classifier call failed: {err}");
                return Ok(Self::passthrough(
                    "classifier_error",
                    t0.elapsed().as_secs_f64() * 1000.0,
                ));
            }
        };
        let latency_ms = t0.elapsed().as_secs_f64() * 1000.0;

        if !response.blocked {
            return Ok(LayerResult {
                layer_name: self.name().to_string(),
                detected: false,
                score: Some(0.0),
                details: vec![],
                latency_ms,
            });
        }

        Ok(LayerResult {
            layer_name: self.name().to_string(),
            detected: true,
            score: Some(response.p_harmful),
            details: vec![ThreatDetail {
                rule_id: "L4-MULTI-TURN".to_string(),
                description: "Multi-turn intent classifier flagged the assistant response \
                              as harmful given the conversation history."
                    .to_string(),
                matched_text: input.text.chars().take(MAX_EVIDENCE_CHARS).collect(),
                category: "jailbreak".to_string(),
            }],
            latency_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detectors::Conversation;
    use crate::models::multi_turn_intent::{Turn, DEFAULT_HARMFUL_THRESHOLD};
    use model_service::{GenerateRequest, ModelClient, ModelOptions};
    use serde_json::{json, Value};

    struct FakeClient {
        body: Value,
        ready: bool,
    }

    impl ModelClient for FakeClient {
        fn check_model(&self, _model: &str) -> bool {
            self.ready
        }

        fn generate(
            &self,
            _request: &GenerateRequest<'_>,
        ) -> Result<Value, model_service::ModelServiceError> {
            Ok(self.body.clone())
        }

        fn chat(
            &self,
            _model: &str,
            _messages: &[(&str, &str)],
            _options: &ModelOptions,
            _logprobs: bool,
            _top_logprobs: u32,
        ) -> Result<Value, model_service::ModelServiceError> {
            unreachable!("L4 uses the generate endpoint only")
        }
    }

    struct DownClient;

    impl ModelClient for DownClient {
        fn check_model(&self, _model: &str) -> bool {
            false
        }

        fn generate(
            &self,
            _request: &GenerateRequest<'_>,
        ) -> Result<Value, model_service::ModelServiceError> {
            Err(model_service::ModelServiceError::Inference(
                "connection refused".into(),
            ))
        }

        fn chat(
            &self,
            _model: &str,
            _messages: &[(&str, &str)],
            _options: &ModelOptions,
            _logprobs: bool,
            _top_logprobs: u32,
        ) -> Result<Value, model_service::ModelServiceError> {
            unreachable!()
        }
    }

    fn logprob_body(lp0: f64, lp1: f64, response: &str) -> Value {
        json!({
            "response": response,
            "logprobs": [{
                "top_logprobs": [
                    {"token": "0", "logprob": lp0},
                    {"token": "1", "logprob": lp1},
                ]
            }]
        })
    }

    fn detector_with(client: Box<dyn ModelClient>) -> MultiTurnIntentDetector {
        MultiTurnIntentDetector::with_classifier(MultiTurnIntentClassifier::with_client(
            DEFAULT_HARMFUL_THRESHOLD,
            "warden",
            client,
        ))
    }

    fn detect_with_conversation(
        detector: &MultiTurnIntentDetector,
        history: &[Turn],
        assistant_response: &str,
    ) -> LayerResult {
        let variants: Vec<String> = Vec::new();
        let mut input = DetectInput::new("how do I do it", &variants);
        input.conversation = Some(Conversation {
            history,
            assistant_response,
        });
        detector.detect(&input).expect("L4 never returns Err")
    }

    #[test]
    fn harmful_exchange_is_detected_with_p_harmful_score() {
        let detector = detector_with(Box::new(FakeClient {
            body: logprob_body(0.9f64.ln(), 0.1f64.ln(), "0"),
            ready: true,
        }));
        let lr = detect_with_conversation(&detector, &[], "step 1: ...");
        assert!(lr.detected);
        assert_eq!(lr.score, Some(0.9));
        let detail = &lr.details[0];
        assert_eq!(detail.rule_id, "L4-MULTI-TURN");
        assert_eq!(detail.category, "jailbreak");
        assert_eq!(detail.matched_text, "how do I do it");
    }

    #[test]
    fn benign_exchange_passes() {
        let detector = detector_with(Box::new(FakeClient {
            body: logprob_body(0.1f64.ln(), 0.9f64.ln(), "1"),
            ready: true,
        }));
        let lr = detect_with_conversation(&detector, &[], "sorry, I cannot help");
        assert!(!lr.detected);
        assert_eq!(lr.score, Some(0.0));
        assert!(lr.details.is_empty());
    }

    #[test]
    fn missing_conversation_context_passes_through() {
        let detector = detector_with(Box::new(FakeClient {
            body: logprob_body(0.9f64.ln(), 0.1f64.ln(), "0"),
            ready: true,
        }));
        let variants: Vec<String> = Vec::new();
        // No conversation attached: L4 cannot judge, so it must not block.
        let lr = detector
            .detect(&DetectInput::new("hello", &variants))
            .unwrap();
        assert!(!lr.detected);
        assert_eq!(lr.layer_name, "multi_turn_intent");
    }

    #[test]
    fn classifier_failure_fails_open() {
        let detector = detector_with(Box::new(DownClient));
        let lr = detect_with_conversation(&detector, &[], "whatever");
        assert!(!lr.detected, "an unreachable service must not block");
        assert_eq!(lr.score, Some(0.0));
    }

    #[test]
    fn availability_tracks_model_presence() {
        let up = detector_with(Box::new(FakeClient {
            body: logprob_body(0.1f64.ln(), 0.9f64.ln(), "1"),
            ready: true,
        }));
        assert!(up.is_available());

        let down = detector_with(Box::new(DownClient));
        assert!(!down.is_available());
    }

    #[test]
    fn history_is_forwarded_to_the_classifier() {
        let detector = detector_with(Box::new(FakeClient {
            body: logprob_body(0.9f64.ln(), 0.1f64.ln(), "0"),
            ready: true,
        }));
        let history = vec![Turn::Text("user: earlier turn".to_string())];
        let lr = detect_with_conversation(&detector, &history, "reply");
        assert!(lr.detected);
    }
}
