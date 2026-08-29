//! Runtime-owned PostTool content dispatch and arbitration.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokenless_ccr::StashStore;
use tokenless_compressors::{
    JsonCompressionConfig, JsonCompressionContext, JsonCompressor, JsonOperation, Recoverability,
};
use tokenless_protocol::{
    BYTE_ESTIMATOR_ID, CompressionRequest, CompressionResponse, ContentOrigin,
    DIAGNOSTIC_MAX_BYTES, Disposition, PROTOCOL_VERSION, Reversibility, TOKENIZER_ID,
    estimate_tokens, estimate_tokens_from_bytes,
};

use super::arbitration::{ArbitrationInput, Verdict, decide};
use super::content::{ContentType, detect};
use super::stash_ledger::StashLedger;

/// Policy resolved by Runtime for one PostTool call.
#[derive(Debug, Clone)]
pub(crate) struct PostToolPipelineConfig {
    pub(crate) timeout: Duration,
    pub(crate) max_input_bytes: usize,
    pub(crate) min_input_chars: usize,
    pub(crate) compression_enabled: bool,
    pub(crate) stash_enabled: bool,
    pub(crate) require_reversibility: bool,
    pub(crate) force_json: bool,
    pub(crate) preserve_top_level_shape: bool,
    pub(crate) allow_toon: bool,
    pub(crate) min_toon_chars: usize,
    pub(crate) json: JsonCompressionConfig,
}

/// Protocol response plus Runtime-only measurement and artifact facts.
pub(crate) struct PostToolRun {
    pub(crate) response: CompressionResponse,
    pub(crate) candidate: Option<String>,
    pub(crate) operations: Vec<JsonOperation>,
    pub(crate) stash_writes: Option<usize>,
    pub(crate) stash_errors: Option<usize>,
    pub(crate) stash_size: Option<usize>,
    pub(crate) unrecoverable_truncations: Option<usize>,
}

/// The first Runtime-owned PostTool pipeline, dispatching only JSON.
pub(crate) struct PostToolPipeline;

impl PostToolPipeline {
    pub(crate) fn run(
        request: &CompressionRequest,
        config: &PostToolPipelineConfig,
        stash_store: Option<&Arc<dyn StashStore>>,
    ) -> PostToolRun {
        let started = Instant::now();

        if request.content.len() > config.max_input_bytes {
            let mut run = passthrough(
                request,
                estimate_tokens_from_bytes(request.content.len()) as u64,
                ContentType::Unknown,
                Some(format!(
                    "input exceeds {} MiB limit",
                    config.max_input_bytes / (1024 * 1024)
                )),
            );
            run.response.tokenizer_id = BYTE_ESTIMATOR_ID.to_owned();
            return run;
        }
        let before_tokens = estimate_tokens(&request.content) as u64;
        let content_type = detect(&request.content);
        if !request.capabilities.replace_output
            || request.content_origin == ContentOrigin::FileContent
            || request.content.chars().count() < config.min_input_chars
        {
            return passthrough(request, before_tokens, content_type, None);
        }

        let json_candidate = config.force_json
            || content_type == ContentType::Json
            || is_wrapped_structured_json(&request.content);
        if !json_candidate {
            return passthrough(request, before_tokens, content_type, None);
        }

        let attached_store = if config.stash_enabled
            && config.compression_enabled
            && request.capabilities.publish_retrieve_tool
        {
            stash_store
        } else {
            None
        };
        let context = JsonCompressionContext {
            stash: attached_store.map(AsRef::as_ref),
            allow_toon: config.allow_toon && request.capabilities.replace_with_text,
            preserve_top_level_shape: config.preserve_top_level_shape,
            min_toon_chars: config.min_toon_chars,
        };
        let outcome =
            match JsonCompressor::new(config.json.clone()).compress(&request.content, &context) {
                Ok(outcome) => outcome,
                Err(error) => {
                    let mut run = passthrough(request, before_tokens, content_type, None);
                    run.response.disposition = Disposition::Error;
                    run.response.diagnostic = Some(truncate_diagnostic(&error.to_string()));
                    return run;
                }
            };

        let mut ledger = StashLedger::default();
        for write in outcome.stash_writes {
            ledger.record(write);
        }
        let verdict = decide(&ArbitrationInput {
            original: &request.content,
            candidate: &outcome.output,
            has_operations: !outcome.operations.is_empty(),
            recoverability: outcome.recoverability,
            require_reversibility: config.require_reversibility && config.compression_enabled,
            dry_run: !config.compression_enabled,
            timed_out: started.elapsed() > config.timeout,
        });

        let store = attached_store.map(AsRef::as_ref);
        let (output, disposition, stash_keys) = match verdict {
            Verdict::Apply => {
                let keys = ledger.commit(&outcome.output, store);
                (outcome.output.clone(), Disposition::Applied, keys)
            }
            Verdict::DryRun => {
                ledger.rollback(store);
                (request.content.clone(), Disposition::DryRun, Vec::new())
            }
            Verdict::Reject(disposition) => {
                ledger.rollback(store);
                (request.content.clone(), disposition, Vec::new())
            }
        };
        let selected = matches!(verdict, Verdict::Apply | Verdict::DryRun);
        let after_tokens = if selected {
            estimate_tokens(&outcome.output) as u64
        } else {
            before_tokens
        };
        let response_operations = if matches!(verdict, Verdict::Apply) {
            legacy_chain(&outcome.operations)
        } else {
            Vec::new()
        };
        let reversibility = if selected {
            protocol_reversibility(outcome.recoverability)
        } else {
            Reversibility::Lossless
        };
        let unrecoverable_truncations = if !outcome.operations.contains(&JsonOperation::Truncation)
            || !config.compression_enabled
        {
            None
        } else if attached_store.is_some() || selected {
            Some(outcome.metrics.unrecoverable_truncations)
        } else {
            None
        };
        let store_attached = attached_store.is_some();
        PostToolRun {
            response: CompressionResponse {
                protocol_version: PROTOCOL_VERSION,
                output,
                disposition,
                content_type: Some(ContentType::Json.wire_str().to_owned()),
                compressor_chain: response_operations,
                reversibility,
                before_tokens,
                after_tokens,
                stash_keys,
                tokenizer_id: TOKENIZER_ID.to_owned(),
                diagnostic: None,
            },
            candidate: Some(outcome.output),
            operations: outcome.operations,
            stash_writes: store_attached.then(|| ledger.live_writes()),
            stash_errors: store_attached.then(|| outcome.metrics.stash_errors + ledger.errors()),
            stash_size: attached_store.map(|store| store.len()),
            unrecoverable_truncations,
        }
    }
}

fn is_wrapped_structured_json(content: &str) -> bool {
    let Ok(Value::String(inner)) = serde_json::from_str(content) else {
        return false;
    };
    matches!(
        serde_json::from_str::<Value>(&inner),
        Ok(Value::Object(_) | Value::Array(_))
    )
}

fn passthrough(
    request: &CompressionRequest,
    before_tokens: u64,
    content_type: ContentType,
    diagnostic: Option<String>,
) -> PostToolRun {
    let mut response = CompressionResponse::passthrough(request, before_tokens);
    response.content_type = Some(content_type.wire_str().to_owned());
    response.diagnostic = diagnostic;
    PostToolRun {
        response,
        candidate: None,
        operations: Vec::new(),
        stash_writes: None,
        stash_errors: None,
        stash_size: None,
        unrecoverable_truncations: None,
    }
}

fn legacy_chain(operations: &[JsonOperation]) -> Vec<String> {
    let mut chain = Vec::new();
    if operations.iter().any(|operation| {
        matches!(
            operation,
            JsonOperation::Cleanup | JsonOperation::Truncation
        )
    }) {
        chain.push("response-cleanup".to_owned());
    }
    if operations.contains(&JsonOperation::Toon) {
        chain.push("toon".to_owned());
    }
    chain
}

fn protocol_reversibility(recoverability: Recoverability) -> Reversibility {
    match recoverability {
        Recoverability::Lossless => Reversibility::Lossless,
        Recoverability::Retrievable => Reversibility::Retrievable,
        Recoverability::Unrecoverable => Reversibility::Unrecoverable,
    }
}

const TRUNCATION_SUFFIX: &str = " [truncated]";

fn truncate_diagnostic(message: &str) -> String {
    if message.len() <= DIAGNOSTIC_MAX_BYTES {
        return message.to_owned();
    }
    let mut end = DIAGNOSTIC_MAX_BYTES - TRUNCATION_SUFFIX.len();
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{TRUNCATION_SUFFIX}", &message[..end])
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokenless_ccr::{InMemoryStore, StashError, StashWrite};
    use tokenless_protocol::{Capabilities, Seam};

    use super::*;

    #[derive(Default)]
    struct CountingStore {
        inner: InMemoryStore,
        stash_calls: AtomicUsize,
        delete_calls: AtomicUsize,
    }

    impl StashStore for CountingStore {
        fn stash(&self, payload: &str) -> Result<StashWrite, StashError> {
            self.stash_calls.fetch_add(1, Ordering::Relaxed);
            self.inner.stash(payload)
        }

        fn retrieve(&self, hash: &str) -> Result<Option<String>, StashError> {
            self.inner.retrieve(hash)
        }

        fn len(&self) -> usize {
            self.inner.len()
        }

        fn evict_expired(&self) -> Result<usize, StashError> {
            self.inner.evict_expired()
        }

        fn delete(&self, hash: &str, generation: u64) -> Result<bool, StashError> {
            self.delete_calls.fetch_add(1, Ordering::Relaxed);
            self.inner.delete(hash, generation)
        }
    }

    fn request(content: &str) -> CompressionRequest {
        let mut request = CompressionRequest::new(content, "test", Seam::PostTool);
        request.capabilities = Capabilities {
            replace_output: true,
            publish_retrieve_tool: true,
            replace_with_text: true,
        };
        request
    }

    fn config(timeout: Duration, truncate_arrays_at: usize) -> PostToolPipelineConfig {
        PostToolPipelineConfig {
            timeout,
            max_input_bytes: 1024 * 1024,
            min_input_chars: 0,
            compression_enabled: true,
            stash_enabled: true,
            require_reversibility: false,
            force_json: true,
            preserve_top_level_shape: false,
            allow_toon: false,
            min_toon_chars: usize::MAX,
            json: JsonCompressionConfig {
                truncate_arrays_at,
                array_tail_preserve: 0,
                ..JsonCompressionConfig::default()
            },
        }
    }

    #[test]
    fn one_json_domain_trace_reaches_one_stash_commit() {
        let input = serde_json::to_string(&serde_json::json!({
            "debug": "discarded noise",
            "items": (0..12)
                .map(|index| format!("item-{index}-{}", "x".repeat(80)))
                .collect::<Vec<_>>(),
        }))
        .unwrap();
        let concrete = Arc::new(CountingStore::default());
        let store: Arc<dyn StashStore> = concrete.clone();

        let run = PostToolPipeline::run(
            &request(&input),
            &config(Duration::from_secs(1), 2),
            Some(&store),
        );

        assert_eq!(run.response.disposition, Disposition::Applied);
        assert_eq!(
            run.operations,
            [JsonOperation::Cleanup, JsonOperation::Truncation]
        );
        assert_eq!(run.response.compressor_chain, ["response-cleanup"]);
        assert_eq!(run.response.stash_keys.len(), 1);
        assert_eq!(concrete.stash_calls.load(Ordering::Relaxed), 1);
        assert_eq!(concrete.delete_calls.load(Ordering::Relaxed), 0);
        assert_eq!(concrete.len(), 1);
    }

    #[test]
    fn rejected_json_candidate_is_arbitrated_and_rolled_back_once() {
        let concrete = Arc::new(CountingStore::default());
        let store: Arc<dyn StashStore> = concrete.clone();
        let run = PostToolPipeline::run(
            &request(r#"["a","b"]"#),
            &config(Duration::from_secs(1), 1),
            Some(&store),
        );

        assert_eq!(run.response.disposition, Disposition::NoSavings);
        assert_eq!(concrete.stash_calls.load(Ordering::Relaxed), 1);
        assert_eq!(concrete.delete_calls.load(Ordering::Relaxed), 1);
        assert_eq!(concrete.len(), 0);
    }

    #[test]
    fn timed_out_json_candidate_is_rolled_back_once() {
        let input = serde_json::to_string(
            &(0..12)
                .map(|index| format!("item-{index}-{}", "x".repeat(80)))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let concrete = Arc::new(CountingStore::default());
        let store: Arc<dyn StashStore> = concrete.clone();
        let run = PostToolPipeline::run(&request(&input), &config(Duration::ZERO, 2), Some(&store));

        assert_eq!(run.response.disposition, Disposition::Timeout);
        assert_eq!(concrete.stash_calls.load(Ordering::Relaxed), 1);
        assert_eq!(concrete.delete_calls.load(Ordering::Relaxed), 1);
        assert_eq!(concrete.len(), 0);
    }

    #[test]
    fn quoted_json_scalar_passes_through() {
        let input = serde_json::to_string(&"x".repeat(5_000)).unwrap();
        let mut config = config(Duration::from_secs(1), 2);
        config.force_json = false;

        let run = PostToolPipeline::run(&request(&input), &config, None);

        assert_eq!(run.response.disposition, Disposition::Passthrough);
        assert_eq!(run.response.output, input);
        assert!(run.operations.is_empty());
    }

    #[test]
    fn oversized_passthrough_identifies_the_byte_estimator() {
        let input = "界".repeat(4);
        let mut config = config(Duration::from_secs(1), 2);
        config.max_input_bytes = input.len() - 1;

        let run = PostToolPipeline::run(&request(&input), &config, None);

        assert_eq!(run.response.disposition, Disposition::Passthrough);
        assert_eq!(run.response.before_tokens, 3);
        assert_eq!(run.response.after_tokens, 3);
        assert_ne!(run.response.before_tokens, estimate_tokens(&input) as u64);
        assert_eq!(run.response.tokenizer_id, BYTE_ESTIMATOR_ID);
    }
}
