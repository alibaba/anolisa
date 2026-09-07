//! Runtime-owned PostTool content dispatch and arbitration.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokenless_ccr::{InMemoryStore, StashStore, StashWrite};
use tokenless_compressors::{
    BuildLogCompressor, BuildLogOperation, JsonCompressionConfig, JsonCompressionContext,
    JsonCompressor, JsonOperation,
};
use tokenless_protocol::{
    AppliedOperation, BYTE_ESTIMATOR_ID, ContentOrigin, ContentType, Disposition, PostToolRequest,
    PostToolResponse, Recoverability, TOKENIZER_ID, ToolResultStatus, estimate_tokens,
    estimate_tokens_from_bytes,
};

use super::arbitration::{ArbitrationInput, Verdict, decide};
use super::content::detect;
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
    pub(crate) response: PostToolResponse,
    pub(crate) candidate: Option<String>,
    pub(crate) operations: Vec<AppliedOperation>,
    pub(crate) stash_writes: Option<usize>,
    pub(crate) stash_errors: Option<usize>,
    pub(crate) stash_size: Option<usize>,
    pub(crate) unrecoverable_truncations: Option<usize>,
}

/// Runtime-owned PostTool pipeline for statically dispatched content domains.
pub(crate) struct PostToolPipeline;

/// Error returned when the selected domain compressor fails.
#[derive(Debug, thiserror::Error)]
#[error("PostTool pipeline failed: {0}")]
pub(crate) struct PostToolPipelineError(String);

impl PostToolPipeline {
    pub(crate) fn run(
        request: &PostToolRequest,
        config: &PostToolPipelineConfig,
        stash_store: Option<&Arc<dyn StashStore>>,
    ) -> Result<PostToolRun, PostToolPipelineError> {
        let started = Instant::now();

        if request.content.len() > config.max_input_bytes {
            let mut run = passthrough(
                request,
                estimate_tokens_from_bytes(request.content.len()) as u64,
                ContentType::Unknown,
            );
            run.response.tokenizer_id = BYTE_ESTIMATOR_ID.to_owned();
            return Ok(run);
        }
        let before_tokens = estimate_tokens(&request.content) as u64;
        let content_type = detect(&request.content);
        if request.status == ToolResultStatus::Error {
            return Ok(passthrough(request, before_tokens, content_type));
        }
        if !request.capabilities.replace_output
            || request.content_origin == ContentOrigin::FileContent
            || request.content.chars().count() < config.min_input_chars
        {
            return Ok(passthrough(request, before_tokens, content_type));
        }

        let json_candidate = config.force_json
            || content_type == ContentType::Json
            || is_wrapped_structured_json(&request.content);
        let build_log_candidate = content_type == ContentType::BuildLog
            && request.content_origin == ContentOrigin::CommandOutput;
        if !json_candidate && !build_log_candidate {
            return Ok(passthrough(request, before_tokens, content_type));
        }

        let dry_run_store = (!config.compression_enabled
            && config.stash_enabled
            && request.capabilities.recovery.is_available()
            && stash_store.is_some())
        .then(InMemoryStore::new);
        let attached_store: Option<&dyn StashStore> =
            if config.stash_enabled && request.capabilities.recovery.is_available() {
                if config.compression_enabled {
                    stash_store.map(AsRef::as_ref)
                } else {
                    dry_run_store.as_ref().map(|store| store as &dyn StashStore)
                }
            } else {
                None
            };
        let candidate = if json_candidate {
            let context = JsonCompressionContext {
                recovery: &request.capabilities.recovery,
                stash: attached_store,
                allow_toon: config.allow_toon && request.capabilities.replace_with_text,
                preserve_top_level_shape: config.preserve_top_level_shape,
                min_toon_chars: config.min_toon_chars,
                allow_unrecoverable: !config.require_reversibility || !config.compression_enabled,
            };
            let outcome = JsonCompressor::new(config.json.clone())
                .compress(&request.content, &context)
                .map_err(|error| PostToolPipelineError(error.to_string()))?;
            DomainCandidate {
                output: outcome.output,
                operations: json_operations(&outcome.operations),
                recoverability: outcome.recoverability,
                stash_writes: outcome.stash_writes,
                stash_errors: outcome.metrics.stash_errors,
                unrecoverable_truncations: outcome
                    .operations
                    .contains(&JsonOperation::Truncation)
                    .then_some(outcome.metrics.unrecoverable_truncations),
            }
        } else {
            let outcome = BuildLogCompressor.compress_with_recovery(
                &request.content,
                attached_store,
                &request.capabilities.recovery,
            );
            DomainCandidate {
                output: outcome.output,
                operations: build_log_operations(&outcome.operations),
                recoverability: outcome.recoverability,
                stash_writes: outcome.stash_writes,
                stash_errors: outcome.metrics.stash_errors,
                unrecoverable_truncations: None,
            }
        };

        let mut ledger = StashLedger::default();
        for write in candidate.stash_writes {
            ledger.record(write);
        }
        let verdict = decide(&ArbitrationInput {
            original: &request.content,
            candidate: &candidate.output,
            has_operations: !candidate.operations.is_empty(),
            recoverability: candidate.recoverability,
            require_reversibility: config.require_reversibility && config.compression_enabled,
            dry_run: !config.compression_enabled,
            timed_out: started.elapsed() > config.timeout,
        });

        let store = attached_store;
        let (output, disposition, stash_keys) = match verdict {
            Verdict::Apply => {
                let keys = ledger.commit(&candidate.output, store, &request.capabilities.recovery);
                (candidate.output.clone(), Disposition::Applied, keys)
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
            estimate_tokens(&candidate.output) as u64
        } else {
            before_tokens
        };
        let response_operations = if matches!(verdict, Verdict::Apply) {
            candidate.operations.clone()
        } else {
            Vec::new()
        };
        let recoverability = if matches!(verdict, Verdict::Apply) {
            protocol_recoverability(candidate.recoverability)
        } else {
            Recoverability::Lossless
        };
        let unrecoverable_truncations = if !config.compression_enabled {
            None
        } else {
            candidate
                .unrecoverable_truncations
                .filter(|_| attached_store.is_some() || selected)
        };
        let persistent_store_attached = config.compression_enabled && attached_store.is_some();
        Ok(PostToolRun {
            response: PostToolResponse {
                output,
                disposition,
                content_type: Some(if json_candidate {
                    ContentType::Json
                } else {
                    ContentType::BuildLog
                }),
                applied_operations: response_operations,
                recoverability,
                before_tokens,
                after_tokens,
                stash_keys,
                tokenizer_id: TOKENIZER_ID.to_owned(),
                additional_context: None,
            },
            candidate: Some(candidate.output),
            operations: candidate.operations,
            stash_writes: persistent_store_attached.then(|| ledger.live_writes()),
            stash_errors: persistent_store_attached
                .then(|| candidate.stash_errors + ledger.errors()),
            stash_size: if persistent_store_attached {
                attached_store.map(StashStore::len)
            } else {
                None
            },
            unrecoverable_truncations,
        })
    }
}

struct DomainCandidate {
    output: String,
    operations: Vec<AppliedOperation>,
    recoverability: tokenless_compressors::Recoverability,
    stash_writes: Vec<StashWrite>,
    stash_errors: usize,
    unrecoverable_truncations: Option<usize>,
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
    request: &PostToolRequest,
    before_tokens: u64,
    content_type: ContentType,
) -> PostToolRun {
    let mut response = PostToolResponse::passthrough(request, before_tokens);
    response.content_type = Some(content_type);
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

fn json_operations(operations: &[JsonOperation]) -> Vec<AppliedOperation> {
    operations
        .iter()
        .map(|operation| match operation {
            JsonOperation::Cleanup => AppliedOperation::JsonCleanup,
            JsonOperation::RecordReduction => AppliedOperation::JsonRecordReduction,
            JsonOperation::Truncation => AppliedOperation::JsonTruncation,
            JsonOperation::Toon => AppliedOperation::Toon,
        })
        .collect()
}

fn build_log_operations(operations: &[BuildLogOperation]) -> Vec<AppliedOperation> {
    operations
        .iter()
        .map(|operation| match operation {
            BuildLogOperation::TerminalCleanup => AppliedOperation::TerminalCleanup,
            BuildLogOperation::ProgressReduction => AppliedOperation::BuildLogReduction,
        })
        .collect()
}

fn protocol_recoverability(
    recoverability: tokenless_compressors::Recoverability,
) -> Recoverability {
    match recoverability {
        tokenless_compressors::Recoverability::Lossless => Recoverability::Lossless,
        tokenless_compressors::Recoverability::Retrievable => Recoverability::Retrievable,
        tokenless_compressors::Recoverability::Unrecoverable => Recoverability::Unrecoverable,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokenless_ccr::{InMemoryStore, StashError, StashWrite};
    use tokenless_protocol::{
        OutputOptimization, PostToolCapabilities, ResultKind, ToolResultStatus,
    };

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

    fn request(content: &str) -> PostToolRequest {
        PostToolRequest {
            result_kind: ResultKind::Tool,
            tool_name: "Bash".into(),
            content: content.into(),
            status: ToolResultStatus::Success,
            content_origin: ContentOrigin::CommandOutput,
            output_optimization: OutputOptimization::None,
            capabilities: PostToolCapabilities {
                replace_output: true,
                recovery: tokenless_protocol::RecoveryMethod::Shell,
                replace_with_text: true,
            },
        }
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

    fn record_array(count: usize) -> String {
        serde_json::to_string(
            &(0..count)
                .map(|index| {
                    serde_json::json!({
                        "id": index,
                        "message": format!("record-{index}-{}", "x".repeat(80)),
                        "status": "ok"
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap()
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
        )
        .unwrap();

        assert_eq!(run.response.disposition, Disposition::Applied);
        assert_eq!(
            run.operations,
            [
                AppliedOperation::JsonCleanup,
                AppliedOperation::JsonTruncation
            ]
        );
        assert_eq!(
            run.response.applied_operations,
            [
                AppliedOperation::JsonCleanup,
                AppliedOperation::JsonTruncation
            ]
        );
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
        )
        .unwrap();

        assert_eq!(run.response.disposition, Disposition::NoSavings);
        assert_eq!(concrete.stash_calls.load(Ordering::Relaxed), 1);
        assert_eq!(concrete.delete_calls.load(Ordering::Relaxed), 1);
        assert_eq!(concrete.len(), 0);
    }

    #[test]
    fn record_reduction_has_one_final_commit_and_protocol_operation() {
        let concrete = Arc::new(CountingStore::default());
        let store: Arc<dyn StashStore> = concrete.clone();
        let run = PostToolPipeline::run(
            &request(&record_array(40)),
            &config(Duration::from_secs(1), 32),
            Some(&store),
        )
        .unwrap();

        assert_eq!(run.response.disposition, Disposition::Applied);
        assert_eq!(run.operations, [AppliedOperation::JsonRecordReduction]);
        assert_eq!(
            run.response.applied_operations,
            [AppliedOperation::JsonRecordReduction]
        );
        assert_eq!(run.response.recoverability, Recoverability::Retrievable);
        assert_eq!(run.response.stash_keys.len(), 1);
        assert_eq!(concrete.stash_calls.load(Ordering::Relaxed), 1);
        assert_eq!(concrete.delete_calls.load(Ordering::Relaxed), 0);
        assert_eq!(concrete.len(), 1);
    }

    #[test]
    fn record_reduction_dry_run_uses_only_a_temporary_store() {
        let concrete = Arc::new(CountingStore::default());
        let store: Arc<dyn StashStore> = concrete.clone();
        let mut dry_run = config(Duration::from_secs(1), 32);
        dry_run.compression_enabled = false;
        dry_run.require_reversibility = true;

        let input = record_array(40);
        let run = PostToolPipeline::run(&request(&input), &dry_run, Some(&store)).unwrap();

        assert_eq!(run.response.disposition, Disposition::DryRun);
        assert_eq!(run.response.output, input);
        assert_eq!(run.operations, [AppliedOperation::JsonRecordReduction]);
        assert!(run.response.after_tokens < run.response.before_tokens);
        assert_eq!(concrete.stash_calls.load(Ordering::Relaxed), 0);
        assert_eq!(concrete.delete_calls.load(Ordering::Relaxed), 0);
        assert_eq!(concrete.len(), 0);
    }

    #[test]
    fn record_candidate_with_no_savings_rolls_back_its_single_write() {
        let input = serde_json::to_string(&vec![serde_json::json!({}); 33]).unwrap();
        let concrete = Arc::new(CountingStore::default());
        let store: Arc<dyn StashStore> = concrete.clone();
        let mut no_cleanup = config(Duration::from_secs(1), 32);
        no_cleanup.json.drop_empty_fields = false;

        let run = PostToolPipeline::run(&request(&input), &no_cleanup, Some(&store)).unwrap();

        assert_eq!(run.response.disposition, Disposition::NoSavings);
        assert_eq!(run.response.output, input);
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
        let run = PostToolPipeline::run(&request(&input), &config(Duration::ZERO, 2), Some(&store))
            .unwrap();

        assert_eq!(run.response.disposition, Disposition::Timeout);
        assert_eq!(concrete.stash_calls.load(Ordering::Relaxed), 1);
        assert_eq!(concrete.delete_calls.load(Ordering::Relaxed), 1);
        assert_eq!(concrete.len(), 0);
    }

    #[test]
    fn dry_run_reports_recoverability_for_the_emitted_original() {
        let input = serde_json::to_string(
            &(0..12)
                .map(|index| format!("item-{index}-{}", "x".repeat(80)))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let mut dry_run = config(Duration::from_secs(1), 2);
        dry_run.compression_enabled = false;
        dry_run.stash_enabled = false;

        let run = PostToolPipeline::run(&request(&input), &dry_run, None).unwrap();

        assert_eq!(run.response.disposition, Disposition::DryRun);
        assert_eq!(run.response.output, input);
        assert!(run.response.applied_operations.is_empty());
        assert_eq!(run.response.recoverability, Recoverability::Lossless);
        assert!(run.response.after_tokens < run.response.before_tokens);
    }

    fn build_log() -> String {
        let mut output = "$ cargo build\n".to_owned();
        for index in 0..30 {
            output.push_str(&format!(
                "Compiling package-{index:03} v0.1.{index} with extended progress output\n"
            ));
        }
        output.push_str("Finished `dev` profile [unoptimized] target(s) in 1.2s\n");
        output
    }

    fn go_test_log() -> String {
        (0..30)
            .map(|index| format!("ok  \tgithub.com/acme/pkg{index:02}\t0.{index:03}s\n"))
            .collect()
    }

    fn build_log_config() -> PostToolPipelineConfig {
        let mut config = config(Duration::from_secs(1), 32);
        config.force_json = false;
        config.require_reversibility = true;
        config
    }

    #[test]
    fn one_build_log_domain_reaches_one_final_commit() {
        let concrete = Arc::new(CountingStore::default());
        let store: Arc<dyn StashStore> = concrete.clone();
        let run = PostToolPipeline::run(&request(&build_log()), &build_log_config(), Some(&store))
            .unwrap();

        assert_eq!(run.response.disposition, Disposition::Applied);
        assert_eq!(run.response.content_type, Some(ContentType::BuildLog));
        assert_eq!(run.operations, [AppliedOperation::BuildLogReduction]);
        assert_eq!(
            run.response.applied_operations,
            [AppliedOperation::BuildLogReduction]
        );
        assert_eq!(run.response.recoverability, Recoverability::Retrievable);
        assert_eq!(run.response.stash_keys.len(), 1);
        assert_eq!(concrete.stash_calls.load(Ordering::Relaxed), 1);
        assert_eq!(concrete.delete_calls.load(Ordering::Relaxed), 0);
        assert_eq!(concrete.len(), 1);
    }

    #[test]
    fn native_go_test_rows_reach_build_log_reduction() {
        let concrete = Arc::new(CountingStore::default());
        let store: Arc<dyn StashStore> = concrete.clone();
        let run =
            PostToolPipeline::run(&request(&go_test_log()), &build_log_config(), Some(&store))
                .unwrap();

        assert_eq!(run.response.disposition, Disposition::Applied);
        assert_eq!(run.response.content_type, Some(ContentType::BuildLog));
        assert_eq!(run.operations, [AppliedOperation::BuildLogReduction]);
        assert_eq!(run.response.recoverability, Recoverability::Retrievable);
        assert_eq!(run.response.stash_keys.len(), 1);
        assert_eq!(concrete.stash_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn build_log_dry_run_uses_only_the_temporary_store() {
        let concrete = Arc::new(CountingStore::default());
        let store: Arc<dyn StashStore> = concrete.clone();
        let mut config = build_log_config();
        config.compression_enabled = false;
        let input = build_log();

        let run = PostToolPipeline::run(&request(&input), &config, Some(&store)).unwrap();

        assert_eq!(run.response.disposition, Disposition::DryRun);
        assert_eq!(run.response.output, input);
        assert_eq!(run.operations, [AppliedOperation::BuildLogReduction]);
        assert!(run.response.after_tokens < run.response.before_tokens);
        assert_eq!(concrete.stash_calls.load(Ordering::Relaxed), 0);
        assert_eq!(concrete.delete_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn build_log_from_non_command_origin_passes_through() {
        let mut request = request(&build_log());
        request.content_origin = ContentOrigin::ApiResponse;
        let run = PostToolPipeline::run(&request, &build_log_config(), None).unwrap();
        assert_eq!(run.response.disposition, Disposition::Passthrough);
        assert_eq!(run.response.content_type, Some(ContentType::BuildLog));
        assert!(run.operations.is_empty());
    }

    #[test]
    fn quoted_json_scalar_passes_through() {
        let input = serde_json::to_string(&"x".repeat(5_000)).unwrap();
        let mut config = config(Duration::from_secs(1), 2);
        config.force_json = false;

        let run = PostToolPipeline::run(&request(&input), &config, None).unwrap();

        assert_eq!(run.response.disposition, Disposition::Passthrough);
        assert_eq!(run.response.output, input);
        assert!(run.operations.is_empty());
    }

    #[test]
    fn oversized_passthrough_identifies_the_byte_estimator() {
        let input = "界".repeat(4);
        let mut config = config(Duration::from_secs(1), 2);
        config.max_input_bytes = input.len() - 1;

        let run = PostToolPipeline::run(&request(&input), &config, None).unwrap();

        assert_eq!(run.response.disposition, Disposition::Passthrough);
        assert_eq!(run.response.before_tokens, 3);
        assert_eq!(run.response.after_tokens, 3);
        assert_ne!(run.response.before_tokens, estimate_tokens(&input) as u64);
        assert_eq!(run.response.tokenizer_id, BYTE_ESTIMATOR_ID);
    }
}
