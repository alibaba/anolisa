//! Stateful application API shared by Tokenless frontends.
//!
//! The runtime composes the shared compression pipeline, reversible SQLite
//! stash, and statistics without depending on a command-line or
//! language-binding layer. Response compression routes through
//! [`tokenless_pipeline::run`], with the existing cleanup registered as the
//! first entry of the production registry (roadmap §5.3).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use thiserror::Error;
use tokenless_ccr::{SqliteStore, StashError, StashStore, StashWrite, extract_hash, is_valid_hash};
use tokenless_pipeline::PipelineConfig;
use tokenless_protocol::{Capabilities, CompressionRequest, Seam};
use tokenless_schema::{ResponseCompressor, SchemaCompressor};
use tokenless_stats::{
    CompressionMode, OperationType, SlsWriter, StatsRecord, StatsRecorder, ensure_state_dir,
    estimate_tokens, get_home_dir, resolve_data_dir, validate_data_dir, validate_database_path,
};

mod response_cleanup;

use response_cleanup::ResponseCleanup;

/// Why a compression attempt did or did not replace the input: the protocol
/// disposition vocabulary, re-exported verbatim so CLI, Runtime, and
/// language bindings share one set of names and wire strings (roadmap §5.6).
pub use tokenless_protocol::Disposition;

/// Maximum accepted response size, matching the standalone CLI input limit.
pub const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;

/// Runtime construction options for state and observability.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Explicit state directory. `None` uses `TOKENLESS_DATA_DIR`, then the
    /// passwd-backed home directory's `.tokenless` child.
    pub data_dir: Option<PathBuf>,
    /// Whether successful compression savings are stored in `stats.db`.
    pub stats_enabled: bool,
    /// Whether successful compression savings are emitted to the SLS writer.
    pub sls_enabled: bool,
    /// Whether compressed output is returned. Disabled mode calculates and
    /// records predicted savings but returns the original input.
    pub compression_enabled: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            data_dir: None,
            stats_enabled: true,
            sls_enabled: false,
            compression_enabled: true,
        }
    }
}

/// Per-call response compression controls.
#[derive(Debug, Clone)]
pub struct CompressOptions {
    /// Maximum string character count before truncation.
    pub truncate_strings_at: Option<usize>,
    /// Maximum array item count before truncation.
    pub truncate_arrays_at: Option<usize>,
    /// Items preserved from the tail of truncated arrays.
    ///
    /// When set, truncated arrays keep this many trailing items in addition
    /// to the head, with a truncation marker between them. `None` uses the
    /// compressor default.
    pub array_tail_preserve: Option<usize>,
    /// Maximum JSON nesting depth before truncation.
    pub max_depth: Option<usize>,
    /// Whether reversible stash markers may be created.
    pub stash_enabled: bool,
    /// Preserve the original response when stash is unavailable or a write
    /// fails. Framework adapters should enable this to avoid lossy fallback.
    pub require_reversible: bool,
}

impl Default for CompressOptions {
    fn default() -> Self {
        Self {
            truncate_strings_at: None,
            truncate_arrays_at: None,
            array_tail_preserve: None,
            max_depth: None,
            stash_enabled: true,
            require_reversible: false,
        }
    }
}

/// Identifiers attached to one compression statistics record.
#[derive(Debug, Clone)]
pub struct Attribution {
    /// Agent or integration identifier.
    pub agent_id: String,
    /// Optional conversation/session identifier.
    pub session_id: Option<String>,
    /// Optional tool-call identifier.
    pub tool_use_id: Option<String>,
}

impl Attribution {
    /// Create attribution for an agent without session or tool-call IDs.
    pub fn new(agent_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            session_id: None,
            tool_use_id: None,
        }
    }
}

/// Structured response from one compression attempt.
#[derive(Debug, Clone)]
pub struct CompressResult {
    /// Text that the caller should pass to the model.
    pub output: String,
    /// Compact candidate calculated before dry-run or fail-open policy; the
    /// original input when no candidate ran (passthrough, timeout, error).
    /// Retained as the legacy measurement channel — dry-run statistics
    /// record the predicted candidate from it — and scheduled for removal
    /// with the statistics migration (roadmap §5.5).
    pub compressed_output: String,
    /// Policy decision applied to the candidate.
    pub disposition: Disposition,
    /// Estimated tokens in the original input.
    pub before_tokens: usize,
    /// Estimated tokens in `compressed_output`.
    pub after_tokens: usize,
    /// Number of successful stash writes, or `None` when no store was attached.
    pub stash_writes: Option<usize>,
    /// Number of failed stash writes, or `None` when no store was attached.
    pub stash_errors: Option<usize>,
    /// Number of truncations without a retrievable marker, or `None` when no
    /// store was attached.
    pub unrecoverable_truncations: Option<usize>,
    /// Live stash entry count after compression, or `None` without a store.
    pub stash_size: Option<usize>,
}

impl CompressResult {
    /// Whether the caller-visible output is the compressed candidate.
    pub fn applied(&self) -> bool {
        self.disposition == Disposition::Applied
    }
}

/// Errors surfaced by the reusable runtime API.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RuntimeError {
    /// Input exceeded the public runtime limit.
    #[error("input exceeds {limit_mib} MiB limit")]
    InputTooLarge {
        /// Configured limit in mebibytes.
        limit_mib: usize,
    },
    /// Response input was not JSON.
    #[error("JSON parse error: {0}")]
    InvalidJson(#[from] serde_json::Error),
    /// State directory or database path violated the shared path policy.
    #[error("invalid Tokenless state path: {0}")]
    InvalidStatePath(#[from] tokenless_stats::path_policy::PathPolicyError),
    /// State directory creation failed.
    #[error("failed to create Tokenless state directory '{}': {source}", path.display())]
    CreateStateDirectory {
        /// Directory that could not be created.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// A response could not be serialized after compression.
    #[error("failed to serialize compressed response: {0}")]
    Serialize(serde_json::Error),
    /// A JSON value could not be encoded as TOON.
    #[error("TOON encode failed: {0}")]
    ToonEncode(String),
    /// Retrieve input was neither a marker nor a bare 24-character hash.
    #[error("invalid stash hash: {value:?} (expected 24 hex chars or a <<tokenless:HASH>> marker)")]
    InvalidHash {
        /// Rejected retrieve input.
        value: String,
    },
    /// The stash backend is unavailable.
    #[error("stash unavailable: {0}")]
    StashUnavailable(String),
    /// The requested stash entry does not exist or has expired.
    #[error("no stashed payload for hash: {hash}")]
    StashEntryNotFound {
        /// Normalized stash hash.
        hash: String,
    },
    /// The stash backend failed while retrieving an entry.
    #[error("stash retrieve failed: {0}")]
    StashRetrieve(String),
}

/// Stateful Tokenless service for in-process callers.
pub struct TokenlessRuntime {
    config: RuntimeConfig,
    data_dir: PathBuf,
    stash_store: Option<Arc<dyn StashStore>>,
    stash_error: Option<String>,
    stats_recorder: Option<StatsRecorder>,
    stats_error: Option<String>,
}

impl TokenlessRuntime {
    /// Open reusable stash and statistics state.
    ///
    /// Invalid directory paths fail construction. SQLite open failures remain
    /// observable through `stash_error()` / `stats_error()` so compression can
    /// follow its configured fail-open policy.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the state directory violates the shared
    /// path policy or cannot be created.
    pub fn new(config: RuntimeConfig) -> Result<Self, RuntimeError> {
        let data_dir = resolve_runtime_data_dir(config.data_dir.as_deref())?;
        ensure_state_dir(&data_dir).map_err(|source| RuntimeError::CreateStateDirectory {
            path: data_dir.clone(),
            source,
        })?;

        let stash_path = validate_database_path(&data_dir.join("stash.db"), &[&data_dir])?;
        let (stash_store, stash_error) = match SqliteStore::new(&stash_path) {
            Ok(store) => (Some(Arc::new(store) as Arc<dyn StashStore>), None),
            Err(error) => (
                None,
                Some(format!(
                    "cannot open stash db at {}: {error}",
                    stash_path.display(),
                )),
            ),
        };

        let (stats_recorder, stats_error) = if config.stats_enabled {
            let stats_path = validate_database_path(&data_dir.join("stats.db"), &[&data_dir])?;
            match StatsRecorder::new(&stats_path) {
                Ok(recorder) => (Some(recorder), None),
                Err(error) => (
                    None,
                    Some(format!(
                        "cannot open stats db at {}: {error}",
                        stats_path.display(),
                    )),
                ),
            }
        } else {
            (None, None)
        };

        Ok(Self {
            config,
            data_dir,
            stash_store,
            stash_error,
            stats_recorder,
            stats_error,
        })
    }

    /// Compress one JSON response and record any effective savings.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] for oversized or invalid JSON input.
    pub fn compress_response(
        &self,
        input: &str,
        options: &CompressOptions,
        attribution: &Attribution,
    ) -> Result<CompressResult, RuntimeError> {
        let result = compress_response_with_store(
            input,
            options,
            self.config.compression_enabled,
            self.stash_store.as_ref(),
        )?;
        self.record_stats(OperationType::CompressResponse, input, &result, attribution);
        Ok(result)
    }

    /// Compress a Function Calling schema or top-level `tools` request with reversible markers.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] for oversized or invalid JSON input, or if the
    /// compressed schema cannot be serialized.
    pub fn compress_schema(
        &self,
        input: &str,
        attribution: &Attribution,
    ) -> Result<CompressResult, RuntimeError> {
        let result = compress_schema_with_store(
            input,
            self.config.compression_enabled,
            self.stash_store.as_ref(),
        )?;
        self.record_stats(OperationType::CompressSchema, input, &result, attribution);
        Ok(result)
    }

    /// Encode one JSON value as TOON when doing so reduces estimated tokens.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] for oversized or invalid JSON input, or when
    /// the TOON encoder rejects the value.
    pub fn compress_toon(
        &self,
        input: &str,
        attribution: &Attribution,
    ) -> Result<CompressResult, RuntimeError> {
        let result = compress_toon(input, self.config.compression_enabled)?;
        self.record_stats(OperationType::CompressToon, input, &result, attribution);
        Ok(result)
    }

    /// Retrieve a payload by bare hash or text containing a marker.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] for malformed input, unavailable state, a
    /// missing/expired entry, or a backend failure.
    pub fn retrieve(&self, hash_or_marker: &str) -> Result<String, RuntimeError> {
        let store = self.stash_store.as_deref().ok_or_else(|| {
            RuntimeError::StashUnavailable(
                self.stash_error
                    .clone()
                    .unwrap_or_else(|| "stash is not configured".to_string()),
            )
        })?;
        retrieve_from_store(store, hash_or_marker)
    }

    /// Validated state directory used by this runtime.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Whether the SQLite stash opened successfully.
    pub fn stash_available(&self) -> bool {
        self.stash_store.is_some()
    }

    /// Stash initialization failure, if any.
    pub fn stash_error(&self) -> Option<&str> {
        self.stash_error.as_deref()
    }

    /// Whether statistics were requested and their SQLite recorder opened.
    pub fn stats_available(&self) -> bool {
        self.stats_recorder.is_some()
    }

    /// Statistics initialization failure, if any.
    pub fn stats_error(&self) -> Option<&str> {
        self.stats_error.as_deref()
    }

    fn record_stats(
        &self,
        operation: OperationType,
        input: &str,
        result: &CompressResult,
        attribution: &Attribution,
    ) {
        if !self.config.stats_enabled && !self.config.sls_enabled {
            return;
        }
        let (after, after_tokens) = match result.disposition {
            Disposition::Applied | Disposition::DryRun => {
                (result.compressed_output.as_str(), result.after_tokens)
            }
            _ => (input, result.before_tokens),
        };
        let before_tokens = result.before_tokens;
        if after_tokens >= before_tokens {
            return;
        }

        let mode = if self.config.compression_enabled {
            CompressionMode::Active
        } else {
            CompressionMode::DryRun
        };
        let mut record = StatsRecord::new(
            operation,
            attribution.agent_id.clone(),
            input.len(),
            before_tokens,
            after.len(),
            after_tokens,
        )
        .with_before_text(input.to_string())
        .with_after_text(after.to_string())
        .with_source_pid(std::process::id() as i64)
        .with_mode(mode)
        .with_stash(result.stash_writes, result.stash_errors, result.stash_size);
        if let Some(session_id) = &attribution.session_id {
            record = record.with_session_id(session_id.clone());
        }
        if let Some(tool_use_id) = &attribution.tool_use_id {
            record = record.with_tool_use_id(tool_use_id.clone());
        }

        if let Some(recorder) = &self.stats_recorder {
            let _ = recorder.record(&record);
        }
        if self.config.sls_enabled {
            SlsWriter::new().write(&record);
        }
    }
}

/// Compress a Function Calling schema or top-level `tools` request using an optional stash store.
///
/// # Errors
///
/// Returns [`RuntimeError`] for oversized or invalid JSON input, or if the
/// compressed schema cannot be serialized.
pub fn compress_schema_with_store(
    input: &str,
    compression_enabled: bool,
    stash_store: Option<&Arc<dyn StashStore>>,
) -> Result<CompressResult, RuntimeError> {
    validate_input_size(input)?;
    let value: serde_json::Value = serde_json::from_str(input)?;
    let attached_store = if compression_enabled {
        stash_store
    } else {
        None
    };
    let mut compressor = SchemaCompressor::new();
    if let Some(store) = attached_store {
        compressor = compressor.with_stash_store(Arc::clone(store));
    }
    let compressed = compressor.compress(&value);
    let compressed_output = serde_json::to_string(&compressed).map_err(RuntimeError::Serialize)?;
    let before_tokens = estimate_tokens(input);
    let after_tokens = estimate_tokens(&compressed_output);
    let compression_stash_errors = attached_store.map(|_| compressor.stash_errors());
    let disposition = if after_tokens >= before_tokens {
        Disposition::NoSavings
    } else if !compression_enabled {
        Disposition::DryRun
    } else if attached_store.is_none() || compression_stash_errors.is_some_and(|count| count > 0) {
        Disposition::ReversibilityUnavailable
    } else {
        Disposition::Applied
    };
    let (stash_writes, stash_errors) = if disposition != Disposition::Applied {
        compressor.rollback_stash_writes();
        (
            attached_store.map(|_| compressor.stash_writes()),
            attached_store.map(|_| compressor.stash_errors()),
        )
    } else {
        let metrics = (
            attached_store.map(|_| compressor.stash_writes()),
            attached_store.map(|_| compressor.stash_errors()),
        );
        compressor.clear_stash_session();
        metrics
    };
    let stash_size = attached_store.map(|store| store.len());
    let output = if disposition == Disposition::Applied {
        compressed_output.clone()
    } else {
        input.to_string()
    };
    Ok(CompressResult {
        output,
        compressed_output,
        disposition,
        before_tokens,
        after_tokens,
        stash_writes,
        stash_errors,
        unrecoverable_truncations: None,
        stash_size,
    })
}

/// Encode JSON as TOON and apply the shared no-savings and dry-run policy.
///
/// # Errors
///
/// Returns [`RuntimeError`] for oversized or invalid JSON input, or when the
/// TOON encoder rejects the value.
pub fn compress_toon(
    input: &str,
    compression_enabled: bool,
) -> Result<CompressResult, RuntimeError> {
    validate_input_size(input)?;
    let value: serde_json::Value = serde_json::from_str(input)?;
    let compressed_output = toon_format::encode_default(&value)
        .map_err(|error| RuntimeError::ToonEncode(error.to_string()))?
        .trim_end()
        .to_string();
    let before_tokens = estimate_tokens(input);
    let after_tokens = estimate_tokens(&compressed_output);
    let disposition = if compressed_output.is_empty() || after_tokens >= before_tokens {
        Disposition::NoSavings
    } else if !compression_enabled {
        Disposition::DryRun
    } else {
        Disposition::Applied
    };
    let output = if disposition == Disposition::Applied {
        compressed_output.clone()
    } else {
        input.to_string()
    };
    Ok(CompressResult {
        output,
        compressed_output,
        disposition,
        before_tokens,
        after_tokens,
        stash_writes: None,
        stash_errors: None,
        unrecoverable_truncations: None,
        stash_size: None,
    })
}

fn validate_input_size(input: &str) -> Result<(), RuntimeError> {
    if input.len() > MAX_INPUT_BYTES {
        return Err(RuntimeError::InputTooLarge {
            limit_mib: MAX_INPUT_BYTES / (1024 * 1024),
        });
    }
    Ok(())
}

/// Overall pipeline budget for one response compression. The pre-pipeline
/// path had no timeout; this bound honors the one-budget contract (roadmap
/// §5.3) while sitting far above any observed in-process compression time,
/// so it only fires on pathological input. Timeout is fail-open: the
/// original content is returned.
const RESPONSE_PIPELINE_TIMEOUT: Duration = Duration::from_secs(10);

/// Forwards to the caller's store while counting the pipeline ledger's
/// deletes, which is what keeps `CompressResult`'s legacy stash metrics on
/// their pre-pipeline semantics: `stash_writes` reports the rows still live
/// after a rollback, and a failed rollback delete surfaces in
/// `stash_errors` so the CLI's stash-health warning still fires on orphaned
/// rows.
struct DeleteTracking<'a> {
    inner: &'a dyn StashStore,
    // Atomics because `StashStore` is `Sync`; this tracker never actually
    // crosses threads within one compress call.
    removed: AtomicUsize,
    failed: AtomicUsize,
}

impl StashStore for DeleteTracking<'_> {
    fn stash(&self, payload: &str) -> Result<StashWrite, StashError> {
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
        let result = self.inner.delete(hash, generation);
        match &result {
            Ok(true) => {
                self.removed.fetch_add(1, Ordering::Relaxed);
            }
            Ok(false) => {}
            Err(_) => {
                self.failed.fetch_add(1, Ordering::Relaxed);
            }
        }
        result
    }
}

/// Compress a response using an optional caller-owned stash store.
///
/// CLI and embedded frontends use this function to share the pipeline's
/// routing, staged execution, and end-to-end arbitration (roadmap §4.3):
/// no-savings fallback, dry-run behavior, reversibility policy, timeout,
/// and stash rollback all come from [`tokenless_pipeline::run`]. A failing
/// compression step is fail-open and reported through the disposition, not
/// as an error.
///
/// # Errors
///
/// Returns [`RuntimeError`] for oversized or invalid JSON input.
pub fn compress_response_with_store(
    input: &str,
    options: &CompressOptions,
    compression_enabled: bool,
    stash_store: Option<&Arc<dyn StashStore>>,
) -> Result<CompressResult, RuntimeError> {
    validate_input_size(input)?;
    // Boundary contract: response input must be JSON. The pipeline itself
    // routes non-JSON content to passthrough, so this validation is what
    // keeps invalid input a structured error for the CLI and bindings.
    serde_json::from_str::<serde::de::IgnoredAny>(input)?;

    let mut compressor = ResponseCompressor::new();
    if let Some(value) = options.truncate_strings_at {
        compressor = compressor.with_truncate_strings_at(value);
    }
    if let Some(value) = options.truncate_arrays_at {
        compressor = compressor.with_truncate_arrays_at(value);
    }
    if let Some(value) = options.array_tail_preserve {
        compressor = compressor.with_array_tail_preserve(value);
    }
    if let Some(value) = options.max_depth {
        compressor = compressor.with_max_depth(value);
    }

    // In dry-run no store is attached at all — the measured candidate is
    // never emitted, so writing stash rows (even rolled-back ones) would be
    // pure churn.
    let attached_store = if options.stash_enabled && compression_enabled {
        stash_store
    } else {
        None
    };
    if let Some(store) = attached_store {
        compressor = compressor.with_stash_store(Arc::clone(store));
    }
    let adapter = ResponseCleanup::new(compressor, attached_store.is_some());

    // Attribution reaches statistics separately until the §5.5 migration;
    // the in-process request carries no frontend identity.
    let mut request = CompressionRequest::new(input, "", Seam::PostTool);
    request.capabilities = Capabilities {
        replace_output: true,
        publish_retrieve_tool: attached_store.is_some(),
    };
    let config = PipelineConfig {
        timeout: RESPONSE_PIPELINE_TIMEOUT,
        // The pre-pipeline policy is "always try to shrink": a permanently
        // unmet size target keeps the cleanup's retrievable-lossy stage on.
        max_tokens: Some(0),
        // Reversibility is enforced only when something would be emitted;
        // in dry-run the pre-pipeline precedence (dry-run wins) applies.
        require_reversibility: options.require_reversible
            && options.stash_enabled
            && compression_enabled,
        dry_run: !compression_enabled,
    };
    let tracker = attached_store.map(|store| DeleteTracking {
        inner: store.as_ref(),
        removed: AtomicUsize::new(0),
        failed: AtomicUsize::new(0),
    });
    let response = tokenless_pipeline::run(
        &request,
        &[&adapter],
        tracker.as_ref().map(|tracker| tracker as &dyn StashStore),
        &config,
    );

    // Legacy measurement channel (see `CompressResult::compressed_output`):
    // the candidate the adapter produced, or the original when none ran.
    let compressed_output = adapter
        .take_candidate()
        .unwrap_or_else(|| input.to_string());
    // Both counts run the shared estimator locally, mirroring each other;
    // this also avoids converting the response's u64 counts back to usize.
    let before_tokens = estimate_tokens(input);
    let after_tokens = estimate_tokens(&compressed_output);
    // Rows still live after the ledger's rollback and orphan-commit deletes,
    // and write/delete failures combined — the pre-pipeline metric contract.
    let stash_writes = tracker.as_ref().map(|tracker| {
        adapter
            .stash_writes()
            .saturating_sub(tracker.removed.load(Ordering::Relaxed))
    });
    let stash_errors = tracker
        .as_ref()
        .map(|tracker| adapter.stash_errors() + tracker.failed.load(Ordering::Relaxed));
    let unrecoverable_truncations = attached_store.map(|_| adapter.unrecoverable_truncations());
    let stash_size = attached_store.map(|store| store.len());

    Ok(CompressResult {
        output: response.output,
        compressed_output,
        disposition: response.disposition,
        before_tokens,
        after_tokens,
        stash_writes,
        stash_errors,
        unrecoverable_truncations,
        stash_size,
    })
}

/// Retrieve from a caller-owned store using a bare hash or embedded marker.
///
/// # Errors
///
/// Returns [`RuntimeError`] for malformed input, a missing/expired entry, or a
/// backend failure.
pub fn retrieve_from_store(
    store: &dyn StashStore,
    hash_or_marker: &str,
) -> Result<String, RuntimeError> {
    let hash = match extract_hash(hash_or_marker) {
        Some(hash) => hash.to_ascii_lowercase(),
        None if is_valid_hash(hash_or_marker) => hash_or_marker.to_ascii_lowercase(),
        None => {
            return Err(RuntimeError::InvalidHash {
                value: hash_or_marker.to_string(),
            });
        }
    };
    match store.retrieve(&hash) {
        Ok(Some(payload)) => Ok(payload),
        Ok(None) => Err(RuntimeError::StashEntryNotFound { hash }),
        Err(error) => Err(RuntimeError::StashRetrieve(error.to_string())),
    }
}

fn resolve_runtime_data_dir(explicit: Option<&Path>) -> Result<PathBuf, RuntimeError> {
    if let Some(path) = explicit {
        return validate_data_dir(path).map_err(RuntimeError::from);
    }
    let home = get_home_dir();
    let trusted_home = (!home.is_empty()).then(|| Path::new(&home));
    let environment = std::env::var("TOKENLESS_DATA_DIR").ok();
    resolve_data_dir(trusted_home, environment.as_deref()).map_err(RuntimeError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokenless_ccr::{InMemoryStore, StashError};

    struct AlwaysFail;

    impl StashStore for AlwaysFail {
        fn stash(&self, _payload: &str) -> Result<tokenless_ccr::StashWrite, StashError> {
            Err(StashError::Backend("simulated".to_string()))
        }

        fn retrieve(&self, _hash: &str) -> Result<Option<String>, StashError> {
            Ok(None)
        }

        fn len(&self) -> usize {
            0
        }

        fn evict_expired(&self) -> Result<usize, StashError> {
            Ok(0)
        }

        fn delete(&self, _hash: &str, _generation: u64) -> Result<bool, StashError> {
            Ok(false)
        }
    }

    fn long_response() -> String {
        serde_json::to_string(&serde_json::json!({
            "items": (0..100).collect::<Vec<_>>(),
            "tail": format!("RECOVERY_SENTINEL={}\n", "x".repeat(200)),
        }))
        .unwrap()
    }

    #[test]
    fn compress_and_retrieve_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let payload = format!("RECOVERY_SENTINEL=ORCHID-7291\n{}", "世界".repeat(200));
        let input = serde_json::to_string(&serde_json::json!({ "tail": payload })).unwrap();
        let runtime = TokenlessRuntime::new(RuntimeConfig {
            data_dir: Some(directory.path().to_path_buf()),
            stats_enabled: false,
            ..RuntimeConfig::default()
        })
        .unwrap();
        let result = runtime
            .compress_response(
                &input,
                &CompressOptions {
                    truncate_strings_at: Some(80),
                    require_reversible: true,
                    ..CompressOptions::default()
                },
                &Attribution::new("test"),
            )
            .unwrap();
        assert!(result.applied());
        let hash = extract_hash(&result.output).unwrap();
        let restored = runtime.retrieve(&hash.to_ascii_uppercase()).unwrap();
        assert_eq!(restored, payload);
    }

    #[test]
    fn reversible_policy_preserves_input_without_store() {
        let input = long_response();
        let result = compress_response_with_store(
            &input,
            &CompressOptions {
                truncate_arrays_at: Some(2),
                require_reversible: true,
                ..CompressOptions::default()
            },
            true,
            None,
        )
        .unwrap();
        assert_eq!(result.disposition, Disposition::ReversibilityUnavailable);
        assert_eq!(result.output, input);
    }

    #[test]
    fn reversible_policy_preserves_input_after_string_stash_failure() {
        let input = serde_json::to_string(&serde_json::json!({ "tail": "x".repeat(400) })).unwrap();
        let store = Arc::new(AlwaysFail) as Arc<dyn StashStore>;
        let result = compress_response_with_store(
            &input,
            &CompressOptions {
                truncate_strings_at: Some(80),
                require_reversible: true,
                ..CompressOptions::default()
            },
            true,
            Some(&store),
        )
        .unwrap();

        assert_eq!(result.disposition, Disposition::ReversibilityUnavailable);
        assert_eq!(result.output, input);
        assert_eq!(result.stash_errors, Some(1));
        assert_eq!(result.unrecoverable_truncations, Some(1));
    }

    #[test]
    fn reversible_policy_preserves_input_after_depth_stash_failure() {
        let input = serde_json::to_string(&serde_json::json!({
            "nested": {"payload": "x".repeat(400)}
        }))
        .unwrap();
        let store = Arc::new(AlwaysFail) as Arc<dyn StashStore>;
        let result = compress_response_with_store(
            &input,
            &CompressOptions {
                max_depth: Some(0),
                require_reversible: true,
                ..CompressOptions::default()
            },
            true,
            Some(&store),
        )
        .unwrap();

        assert_eq!(result.disposition, Disposition::ReversibilityUnavailable);
        assert_eq!(result.output, input);
        assert_eq!(result.stash_errors, Some(1));
        assert_eq!(result.unrecoverable_truncations, Some(1));
    }

    #[test]
    fn reversible_policy_preserves_input_when_string_marker_cannot_fit() {
        let input = serde_json::to_string(&serde_json::json!({ "tail": "x".repeat(400) })).unwrap();
        let store = Arc::new(InMemoryStore::new()) as Arc<dyn StashStore>;
        let result = compress_response_with_store(
            &input,
            &CompressOptions {
                truncate_strings_at: Some(10),
                require_reversible: true,
                ..CompressOptions::default()
            },
            true,
            Some(&store),
        )
        .unwrap();

        assert_eq!(result.disposition, Disposition::ReversibilityUnavailable);
        assert_eq!(result.output, input);
        assert_eq!(result.stash_errors, Some(0));
        assert_eq!(result.unrecoverable_truncations, Some(1));
    }

    #[test]
    fn cli_policy_allows_lossy_output_without_store() {
        let input = long_response();
        let result = compress_response_with_store(
            &input,
            &CompressOptions {
                truncate_arrays_at: Some(2),
                ..CompressOptions::default()
            },
            true,
            None,
        )
        .unwrap();
        assert!(result.applied());
        assert_ne!(result.output, input);
        assert!(result.output.contains("truncated"));
    }

    #[test]
    fn dry_run_does_not_write_stash() {
        let input = long_response();
        let store = Arc::new(InMemoryStore::new()) as Arc<dyn StashStore>;
        let result = compress_response_with_store(
            &input,
            &CompressOptions {
                truncate_arrays_at: Some(2),
                ..CompressOptions::default()
            },
            false,
            Some(&store),
        )
        .unwrap();
        assert_eq!(result.disposition, Disposition::DryRun);
        assert_eq!(result.output, input);
        assert_eq!(store.len(), 0);
        assert_eq!(result.stash_writes, None);
    }

    #[test]
    fn no_savings_returns_original() {
        let input = r#"{"value":1}"#;
        let result =
            compress_response_with_store(input, &CompressOptions::default(), true, None).unwrap();
        assert_eq!(result.disposition, Disposition::NoSavings);
        assert_eq!(result.output, input);
    }

    #[test]
    fn no_savings_rolls_back_orphan_stash() {
        let input = r#"["a","b"]"#;
        let store = Arc::new(InMemoryStore::new()) as Arc<dyn StashStore>;
        let result = compress_response_with_store(
            input,
            &CompressOptions {
                truncate_arrays_at: Some(1),
                ..CompressOptions::default()
            },
            true,
            Some(&store),
        )
        .unwrap();
        assert_eq!(result.disposition, Disposition::NoSavings);
        assert_eq!(result.output, input);
        assert_eq!(store.len(), 0);
        assert_eq!(result.stash_writes, Some(0));
    }

    #[test]
    fn rollback_delete_failures_surface_in_stash_errors() {
        struct StashOkDeleteFails(InMemoryStore);

        impl StashStore for StashOkDeleteFails {
            fn stash(&self, payload: &str) -> Result<StashWrite, StashError> {
                self.0.stash(payload)
            }

            fn retrieve(&self, hash: &str) -> Result<Option<String>, StashError> {
                self.0.retrieve(hash)
            }

            fn len(&self) -> usize {
                self.0.len()
            }

            fn evict_expired(&self) -> Result<usize, StashError> {
                self.0.evict_expired()
            }

            fn delete(&self, _hash: &str, _generation: u64) -> Result<bool, StashError> {
                Err(StashError::Backend("simulated delete failure".to_string()))
            }
        }

        // Twelve short items: the truncation marker outgrows the removed
        // tail, so the candidate is rejected as no-savings after stashing.
        let input = r#"["a","b","c","d","e","f","g","h","i","j","k","l"]"#;
        let store = Arc::new(StashOkDeleteFails(InMemoryStore::new())) as Arc<dyn StashStore>;
        let result = compress_response_with_store(
            input,
            &CompressOptions {
                truncate_arrays_at: Some(1),
                array_tail_preserve: Some(0),
                ..CompressOptions::default()
            },
            true,
            Some(&store),
        )
        .unwrap();

        assert_eq!(result.disposition, Disposition::NoSavings);
        assert_eq!(result.output, input);
        // The rollback delete failed: the orphaned row is still live and the
        // failure is visible, so the CLI's stash-health warning fires.
        assert_eq!(result.stash_writes, Some(1));
        assert_eq!(result.stash_errors, Some(1));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn invalid_json_is_structured_error() {
        let error =
            compress_response_with_store("not json", &CompressOptions::default(), true, None)
                .unwrap_err();
        assert!(matches!(error, RuntimeError::InvalidJson(_)));
    }

    #[test]
    fn non_record_json_passes_through_untouched() {
        // Detection routes only record-shaped JSON ({...}/[...]) to the
        // cleanup; a scalar root passes through, where the pre-pipeline path
        // would truncate it. Deliberate: routing by detected content is the
        // §4.2 contract, and non-record roots wait for their own compressor.
        let input = serde_json::to_string(&"x".repeat(400)).unwrap();
        let result = compress_response_with_store(
            &input,
            &CompressOptions {
                truncate_strings_at: Some(80),
                ..CompressOptions::default()
            },
            true,
            None,
        )
        .unwrap();
        assert_eq!(result.disposition, Disposition::Passthrough);
        assert_eq!(result.output, input);
        assert_eq!(result.after_tokens, result.before_tokens);
    }

    #[test]
    fn pure_cleanup_savings_are_lossless_and_pass_required_reversibility() {
        // Dropped debug fields, nulls, and empties keep the pre-pipeline
        // judgment that they are cleanup, not content loss: with no
        // truncation the candidate is lossless and applies even where
        // reversible output is required and no stash exists. (The
        // pre-pipeline path rejected exactly this combination.)
        let input = serde_json::to_string(&serde_json::json!({
            "value": 1,
            "noise": null,
            "debug": "x".repeat(200),
        }))
        .unwrap();
        let result = compress_response_with_store(
            &input,
            &CompressOptions {
                require_reversible: true,
                ..CompressOptions::default()
            },
            true,
            None,
        )
        .unwrap();
        assert!(result.applied());
        assert!(!result.output.contains("debug"));
    }

    #[test]
    fn dry_run_wins_over_required_reversibility() {
        let input = long_response();
        let result = compress_response_with_store(
            &input,
            &CompressOptions {
                truncate_arrays_at: Some(2),
                require_reversible: true,
                ..CompressOptions::default()
            },
            false,
            None,
        )
        .unwrap();
        assert_eq!(result.disposition, Disposition::DryRun);
        assert_eq!(result.output, input);
        assert!(result.after_tokens < result.before_tokens);
    }

    #[test]
    fn runtime_records_attribution() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = TokenlessRuntime::new(RuntimeConfig {
            data_dir: Some(directory.path().to_path_buf()),
            stats_enabled: true,
            ..RuntimeConfig::default()
        })
        .unwrap();
        let result = runtime
            .compress_response(
                &long_response(),
                &CompressOptions {
                    truncate_arrays_at: Some(2),
                    require_reversible: true,
                    ..CompressOptions::default()
                },
                &Attribution {
                    agent_id: "agentscope".to_string(),
                    session_id: Some("session-a".to_string()),
                    tool_use_id: Some("tool-a".to_string()),
                },
            )
            .unwrap();
        assert!(result.applied());

        let recorder = StatsRecorder::new(directory.path().join("stats.db")).unwrap();
        let records = recorder.records_by_session("session-a", None).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].agent_id, "agentscope");
        assert_eq!(records[0].tool_use_id.as_deref(), Some("tool-a"));
    }

    #[test]
    fn runtime_records_the_result_token_estimates() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = TokenlessRuntime::new(RuntimeConfig {
            data_dir: Some(directory.path().to_path_buf()),
            stats_enabled: true,
            ..RuntimeConfig::default()
        })
        .unwrap();
        let input = serde_json::to_string(&serde_json::json!({
            "tail": "世界".repeat(300)
        }))
        .unwrap();
        let result = runtime
            .compress_response(
                &input,
                &CompressOptions {
                    truncate_strings_at: Some(80),
                    require_reversible: true,
                    ..CompressOptions::default()
                },
                &Attribution {
                    agent_id: "agentscope".to_string(),
                    session_id: Some("unicode-session".to_string()),
                    tool_use_id: None,
                },
            )
            .unwrap();
        assert!(result.applied());

        let recorder = StatsRecorder::new(directory.path().join("stats.db")).unwrap();
        let records = recorder
            .records_by_session("unicode-session", None)
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].before_tokens, result.before_tokens);
        assert_eq!(records[0].after_tokens, result.after_tokens);
    }

    #[test]
    fn schema_compression_is_reversible_and_attributed() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = TokenlessRuntime::new(RuntimeConfig {
            data_dir: Some(directory.path().to_path_buf()),
            stats_enabled: true,
            ..RuntimeConfig::default()
        })
        .unwrap();
        let description = format!("SCHEMA_SENTINEL {}", "details ".repeat(200));
        let input = serde_json::to_string(&serde_json::json!({
            "type": "function",
            "function": {
                "name": "lookup",
                "description": description,
                "parameters": {"type": "object", "properties": {}}
            }
        }))
        .unwrap();
        let attribution = Attribution {
            agent_id: "python-sdk".to_string(),
            session_id: Some("schema-session".to_string()),
            tool_use_id: None,
        };
        let result = runtime.compress_schema(&input, &attribution).unwrap();
        assert!(result.applied());
        let hash = extract_hash(&result.output).unwrap();
        assert_eq!(runtime.retrieve(hash).unwrap(), description);

        let recorder = StatsRecorder::new(directory.path().join("stats.db")).unwrap();
        let records = recorder.records_by_session("schema-session", None).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].operation, OperationType::CompressSchema);
    }

    #[test]
    fn schema_compression_handles_tools_request_container() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = TokenlessRuntime::new(RuntimeConfig {
            data_dir: Some(directory.path().to_path_buf()),
            ..RuntimeConfig::default()
        })
        .unwrap();
        let input = serde_json::to_string(&serde_json::json!({
            "model": "example-model",
            "tools": [{
                "type": "function",
                "function": {
                    "name": "lookup",
                    "description": "A".repeat(2000),
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "B".repeat(1000)
                            }
                        }
                    }
                }
            }]
        }))
        .unwrap();

        let result = runtime
            .compress_schema(&input, &Attribution::new("runtime-test"))
            .unwrap();
        assert!(result.applied());

        let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["model"], "example-model");
        assert!(
            output["tools"][0]["function"]["description"]
                .as_str()
                .unwrap()
                .chars()
                .count()
                <= 256
        );
        assert!(
            output["tools"][0]["function"]["parameters"]["properties"]["query"]["description"]
                .as_str()
                .unwrap()
                .chars()
                .count()
                <= 160
        );
    }

    #[test]
    fn schema_no_savings_rolls_back_stash() {
        let input = r#"{"type":"function","function":{"name":"small","parameters":{}}}"#;
        let store = Arc::new(InMemoryStore::new()) as Arc<dyn StashStore>;
        let result = compress_schema_with_store(input, true, Some(&store)).unwrap();
        assert_eq!(result.disposition, Disposition::NoSavings);
        assert_eq!(result.output, input);
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn toon_uses_only_a_smaller_encoding() {
        let input = serde_json::to_string(&serde_json::json!({
            "items": (0..100)
                .map(|index| serde_json::json!({"name": "same", "value": index}))
                .collect::<Vec<_>>()
        }))
        .unwrap();
        let result = compress_toon(&input, true).unwrap();
        assert!(result.applied());
        assert!(result.after_tokens < result.before_tokens);

        let tiny = "null";
        let result = compress_toon(tiny, true).unwrap();
        assert_eq!(result.disposition, Disposition::NoSavings);
        assert_eq!(result.output, tiny);
    }

    #[test]
    fn invalid_explicit_data_dir_is_rejected() {
        let error = TokenlessRuntime::new(RuntimeConfig {
            data_dir: Some(PathBuf::from("relative")),
            ..RuntimeConfig::default()
        })
        .err()
        .expect("relative state directory must fail");
        assert!(matches!(error, RuntimeError::InvalidStatePath(_)));
    }

    #[cfg(unix)]
    #[test]
    fn sqlite_open_failure_remains_fail_open() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        // Directory mode bits do not constrain euid 0; skip rather than
        // asserting a permission failure that cannot happen as root.
        if std::fs::File::create(directory.path().join(".probe")).is_ok() {
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            return;
        }
        let runtime = TokenlessRuntime::new(RuntimeConfig {
            data_dir: Some(directory.path().to_path_buf()),
            stats_enabled: false,
            ..RuntimeConfig::default()
        })
        .unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(!runtime.stash_available());
        assert!(runtime.stash_error().is_some());

        let input = long_response();
        let result = runtime
            .compress_response(
                &input,
                &CompressOptions {
                    truncate_arrays_at: Some(2),
                    require_reversible: true,
                    ..CompressOptions::default()
                },
                &Attribution::new("test"),
            )
            .unwrap();
        assert_eq!(result.output, input);
    }
}
