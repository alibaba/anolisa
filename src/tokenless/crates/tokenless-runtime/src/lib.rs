//! Stateful application API shared by Tokenless frontends.
//!
//! The runtime owns lifecycle orchestration, including the PostTool pipeline,
//! reversible SQLite stash, and statistics, without depending on a
//! command-line or language-binding layer.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokenless_ccr::{SqliteStore, StashStore, extract_hash, is_valid_hash};
use tokenless_compressors::JsonCompressionConfig;
use tokenless_protocol::{Capabilities, CompressionRequest, Seam};
use tokenless_schema::SchemaCompressor;
use tokenless_stats::{
    CompressionMode, OperationType, SlsWriter, StatsRecord, StatsRecorder, ensure_state_dir,
    estimate_tokens, get_home_dir, resolve_data_dir, validate_data_dir, validate_database_path,
};

mod entry;
mod post_tool;
mod taxonomy;

pub use entry::{EntryOptions, EntryOutcome, compress_with_store};
use post_tool::{PostToolPipeline, PostToolPipelineConfig};

/// Why a compression attempt did or did not replace the input: the protocol
/// disposition vocabulary, re-exported verbatim so CLI, Runtime, and
/// language bindings share one set of names and wire strings (roadmap §5.6).
pub use tokenless_protocol::Disposition;

/// Maximum accepted response size, matching the standalone CLI input limit.
pub const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;

/// Minimum payload length (in characters) before TOON encoding runs.
///
/// TOON on small JSON saves only a few characters (observed ~0.3% below
/// ~500 chars) while the per-event encode cost stays the same, so smaller
/// payloads pass through untouched. Keeping the threshold here lets the
/// `compress-toon` CLI and the [`TokenlessRuntime::compress_toon`] SDK
/// path behave exactly like the adapter hook layer, which skips payloads
/// under the same threshold before invoking the CLI.
pub const MIN_TOON_CHARS: usize = 500;

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
    /// Payloads shorter than [`MIN_TOON_CHARS`] characters pass through
    /// unchanged, matching the adapter hook layer.
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
        let result = compress_toon(input, self.config.compression_enabled, MIN_TOON_CHARS)?;
        self.record_stats(OperationType::CompressToon, input, &result, attribution);
        Ok(result)
    }

    /// Route one protocol request through the unified entry point and
    /// record any effective savings — the same pipeline the `tokenless
    /// compress` subcommand runs, called in-process (roadmap §5.4).
    pub fn compress(
        &self,
        request: &tokenless_protocol::CompressionRequest,
    ) -> tokenless_protocol::CompressionResponse {
        let options = EntryOptions {
            compression_enabled: self.config.compression_enabled,
            stash_enabled: true,
        };
        let outcome = compress_with_store(request, &options, self.stash_store.as_ref());
        record_compression(
            request,
            &outcome,
            self.stats_recorder.as_ref(),
            self.config.sls_enabled,
        );
        outcome.response
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
        retrieve_recorded(
            store,
            hash_or_marker,
            self.stats_recorder.as_ref(),
            "embedded",
        )
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
    Ok(finish_schema_compression(
        input,
        compressed_output,
        compression_enabled,
        attached_store,
        &compressor,
    ))
}

/// Applies the schema disposition ladder to an already-computed candidate:
/// no-savings by tokens, dry-run, reversibility-unavailable when no store is
/// attached or a stash write failed, then applied — rolling back or
/// committing the compressor's stash session accordingly. Shared by
/// [`compress_schema_with_store`] and the unified entry router so the
/// hand-rolled ladder exists once.
fn finish_schema_compression(
    input: &str,
    compressed_output: String,
    compression_enabled: bool,
    attached_store: Option<&Arc<dyn StashStore>>,
    compressor: &SchemaCompressor,
) -> CompressResult {
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
    CompressResult {
        output,
        compressed_output,
        disposition,
        before_tokens,
        after_tokens,
        stash_writes,
        stash_errors,
        unrecoverable_truncations: None,
        stash_size,
    }
}

/// Encode JSON as TOON and apply the shared minimum-length gate,
/// no-savings, and dry-run policies.
///
/// Payloads with fewer than `min_toon_chars` characters pass through
/// unchanged as [`Disposition::Passthrough`] because TOON savings on small
/// JSON are near-zero (see [`MIN_TOON_CHARS`], the shared default used by
/// the CLI and the adapter hooks). Pass `0` to encode any payload that
/// yields token savings, restoring the pre-gate behavior. The input is
/// still validated as JSON, so short invalid payloads fail instead of
/// passing through.
///
/// # Errors
///
/// Returns [`RuntimeError`] for oversized or invalid JSON input, and for
/// TOON encode failures on payloads that clear the minimum-length gate.
pub fn compress_toon(
    input: &str,
    compression_enabled: bool,
    min_toon_chars: usize,
) -> Result<CompressResult, RuntimeError> {
    validate_input_size(input)?;
    let before_tokens = estimate_tokens(input);
    // Parse before the minimum-length gate: invalid JSON must fail with
    // `RuntimeError::InvalidJson` (CLI exit code 2) regardless of payload
    // size. The gate only skips TOON encoding; it never exempts input from
    // JSON validation.
    let value: serde_json::Value = serde_json::from_str(input)?;
    // Character count, not byte length: the adapter hooks measure the same
    // threshold in Unicode code points, so CJK payloads gate identically.
    if input.chars().count() < min_toon_chars {
        return Ok(CompressResult {
            output: input.to_string(),
            compressed_output: input.to_string(),
            disposition: Disposition::Passthrough,
            before_tokens,
            after_tokens: before_tokens,
            stash_writes: None,
            stash_errors: None,
            unrecoverable_truncations: None,
            stash_size: None,
        });
    }
    let compressed_output = toon_format::encode_default(&value)
        .map_err(|error| RuntimeError::ToonEncode(error.to_string()))?
        .trim_end()
        .to_string();
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

/// Overall budget for one in-process PostTool compression.
const RESPONSE_PIPELINE_TIMEOUT: Duration = Duration::from_secs(10);

/// Compress a response using an optional caller-owned stash store.
///
/// CLI and embedded frontends use the same Runtime-owned JSON PostTool
/// pipeline. The explicit API validates JSON at its boundary; optional
/// compression failures remain fail-open inside the pipeline.
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

    // Attribution reaches statistics separately until the §5.5 migration;
    // the in-process request carries no frontend identity.
    let mut request = CompressionRequest::new(input, "", Seam::PostTool);
    request.capabilities = Capabilities {
        replace_output: true,
        publish_retrieve_tool: options.stash_enabled
            && compression_enabled
            && stash_store.is_some(),
        // The legacy path never emits a non-JSON encoding; callers own
        // their envelopes.
        replace_with_text: false,
    };
    let run = PostToolPipeline::run(
        &request,
        &PostToolPipelineConfig {
            timeout: RESPONSE_PIPELINE_TIMEOUT,
            max_input_bytes: MAX_INPUT_BYTES,
            min_input_chars: 0,
            compression_enabled,
            stash_enabled: options.stash_enabled,
            require_reversibility: options.require_reversible,
            force_json: true,
            preserve_top_level_shape: false,
            allow_toon: false,
            min_toon_chars: MIN_TOON_CHARS,
            json: json_config(options),
        },
        stash_store,
    );

    // Legacy measurement channel (see `CompressResult::compressed_output`):
    // the candidate the adapter produced, or the original when none ran.
    let compressed_output = run.candidate.unwrap_or_else(|| input.to_string());
    // Both counts run the shared estimator locally, mirroring each other;
    // this also avoids converting the response's u64 counts back to usize.
    let before_tokens = estimate_tokens(input);
    let after_tokens = estimate_tokens(&compressed_output);

    let stash_attached = run.stash_writes.is_some();
    Ok(CompressResult {
        output: run.response.output,
        compressed_output,
        disposition: run.response.disposition,
        before_tokens,
        after_tokens,
        stash_writes: run.stash_writes,
        stash_errors: run.stash_errors,
        unrecoverable_truncations: stash_attached
            .then(|| run.unrecoverable_truncations.unwrap_or(0)),
        stash_size: run.stash_size,
    })
}

fn json_config(options: &CompressOptions) -> JsonCompressionConfig {
    let defaults = JsonCompressionConfig::default();
    JsonCompressionConfig {
        truncate_strings_at: options
            .truncate_strings_at
            .unwrap_or(defaults.truncate_strings_at),
        truncate_arrays_at: options
            .truncate_arrays_at
            .unwrap_or(defaults.truncate_arrays_at),
        array_tail_preserve: options
            .array_tail_preserve
            .unwrap_or(defaults.array_tail_preserve),
        max_depth: options.max_depth.unwrap_or(defaults.max_depth),
        ..defaults
    }
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
    retrieve_recorded(store, hash_or_marker, None, "")
}

/// Normalize retrieve input — a bare hash or text containing a
/// `<<tokenless:HASH>>` marker — to the lowercase stash key.
///
/// # Errors
///
/// Returns [`RuntimeError::InvalidHash`] when neither form is present.
pub fn normalize_hash(hash_or_marker: &str) -> Result<String, RuntimeError> {
    match extract_hash(hash_or_marker) {
        Some(hash) => Ok(hash.to_ascii_lowercase()),
        None if is_valid_hash(hash_or_marker) => Ok(hash_or_marker.to_ascii_lowercase()),
        None => Err(RuntimeError::InvalidHash {
            value: hash_or_marker.to_string(),
        }),
    }
}

/// The single retrieval implementation with §4.6 event recording, shared by
/// the CLI (`"cli"`), the MCP server (`"mcp"`), and
/// [`TokenlessRuntime::retrieve`] (`"embedded"`). Every store lookup records
/// one `retrieve_events` row — `hit`, `miss`, or `error` — when a recorder
/// is attached; invalid input never reaches the store and records nothing.
/// Recording is fail-silent and never affects the returned payload or error.
///
/// # Errors
///
/// Returns [`RuntimeError`] for malformed input, a missing/expired entry,
/// or a backend failure.
pub fn retrieve_recorded(
    store: &dyn StashStore,
    hash_or_marker: &str,
    recorder: Option<&StatsRecorder>,
    source: &str,
) -> Result<String, RuntimeError> {
    let hash = normalize_hash(hash_or_marker)?;
    let result = store.retrieve(&hash);
    if let Some(recorder) = recorder {
        let (outcome, payload_tokens) = match &result {
            Ok(Some(payload)) => ("hit", Some(estimate_tokens(payload) as i64)),
            Ok(None) => ("miss", None),
            Err(_) => ("error", None),
        };
        let tokenizer_id = payload_tokens
            .is_some()
            .then_some(tokenless_protocol::TOKENIZER_ID);
        let _ =
            recorder.record_retrieve_event(&hash, outcome, source, payload_tokens, tokenizer_id);
    }
    match result {
        Ok(Some(payload)) => Ok(payload),
        Ok(None) => Err(RuntimeError::StashEntryNotFound { hash }),
        Err(error) => Err(RuntimeError::StashRetrieve(error.to_string())),
    }
}

/// The single §5.5 recording path for unified-entry compressions, shared by
/// [`TokenlessRuntime::compress`] and the CLI `compress` subcommand.
///
/// Writes at most one `stats` row per invocation — only when the invocation
/// measured a saving — carrying the §4.6 attribution columns, then attaches
/// one `compression_artifacts` row per stash key emitted by an applied
/// result, owned by the chain's stash-writing compressor (its first
/// retrievable-lossy id). `retrieve_events` and artifacts live only
/// in stats.db; SLS mirrors the scalar record fields. Fail-silent
/// throughout.
pub fn record_compression(
    request: &CompressionRequest,
    outcome: &EntryOutcome,
    recorder: Option<&StatsRecorder>,
    sls_enabled: bool,
) {
    if recorder.is_none() && !sls_enabled {
        return;
    }
    let response = &outcome.response;
    let before = &request.content;
    let after = &outcome.stats.measured_text;
    let before_tokens = estimate_tokens(before);
    let after_tokens = estimate_tokens(after);
    if after_tokens >= before_tokens {
        return;
    }

    let mode = match response.disposition {
        Disposition::DryRun => CompressionMode::DryRun,
        _ => CompressionMode::Active,
    };
    let chain_json = (!response.compressor_chain.is_empty())
        .then(|| serde_json::to_string(&response.compressor_chain).ok())
        .flatten();
    let mut record = StatsRecord::new(
        outcome.stats.op.clone(),
        request.agent_id.clone(),
        before.len(),
        before_tokens,
        after.len(),
        after_tokens,
    )
    .with_before_text(before.clone())
    .with_after_text(after.clone())
    .with_source_pid(std::process::id() as i64)
    .with_mode(mode)
    .with_stash(
        outcome.stash_writes,
        outcome.stash_errors,
        outcome.stash_size,
    )
    .with_entry_metadata(
        request.seam.wire_str(),
        response.content_type.clone(),
        Some(request.content_origin.wire_str().to_owned()),
        chain_json,
        response.tokenizer_id.clone(),
        outcome.stats.unrecoverable_truncations.map(|n| n as i64),
    );
    if let Some(session_id) = &request.session_id {
        record = record.with_session_id(session_id.clone());
    }
    if let Some(tool_use_id) = &request.tool_use_id {
        record = record.with_tool_use_id(tool_use_id.clone());
    }

    if let Some(recorder) = recorder
        && let Ok(stats_id) = recorder.record(&record)
        && response.disposition == Disposition::Applied
        && !response.stash_keys.is_empty()
        && let Some(compressor_id) = stash_owning_compressor(&response.compressor_chain)
    {
        let _ = recorder.record_artifacts(stats_id, compressor_id, &response.stash_keys);
    }
    if sls_enabled {
        SlsWriter::new().write(&record);
    }
}

/// The v1 chain element that owns an applied result's Stash keys.
fn stash_owning_compressor(chain: &[String]) -> Option<&str> {
    chain
        .iter()
        .find(|id| matches!(id.as_str(), "response-cleanup" | "schema-compress"))
        .or_else(|| chain.first())
        .map(String::as_str)
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
    use tokenless_ccr::{InMemoryStore, StashError, StashWrite};
    use tokenless_protocol::ContentOrigin;

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
        assert_eq!(result.unrecoverable_truncations, None);
    }

    #[test]
    fn explicit_response_api_removes_top_level_nulls() {
        let mut value = serde_json::Map::new();
        value.insert(
            "keep".to_owned(),
            serde_json::Value::String("value".to_owned()),
        );
        for index in 0..80 {
            value.insert(format!("null_{index}"), serde_json::Value::Null);
        }
        let input = serde_json::to_string(&value).unwrap();

        let result =
            compress_response_with_store(&input, &CompressOptions::default(), true, None).unwrap();

        assert_eq!(result.disposition, Disposition::Applied);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&result.output).unwrap(),
            serde_json::json!({"keep": "value"})
        );
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
    fn explicit_json_api_compresses_string_roots() {
        // JSON scalars remain valid input to the explicit JSON API even
        // though the general PostTool detector only dispatches structured
        // JSON and wrapped structured output.
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
        assert_eq!(result.disposition, Disposition::Applied);
        assert_ne!(result.output, input);
        assert!(result.after_tokens < result.before_tokens);
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
        let result = compress_toon(&input, true, MIN_TOON_CHARS).unwrap();
        assert!(result.applied());
        assert!(result.after_tokens < result.before_tokens);

        // A tiny payload never reaches the encoder now: the shared
        // minimum-length gate passes it through untouched.
        let tiny = "null";
        let result = compress_toon(tiny, true, MIN_TOON_CHARS).unwrap();
        assert_eq!(result.disposition, Disposition::Passthrough);
        assert_eq!(result.output, tiny);

        // The same tiny payload encodes when the gate is disabled.
        let result = compress_toon(tiny, true, 0).unwrap();
        assert_eq!(result.disposition, Disposition::NoSavings);
        assert_eq!(result.output, tiny);
    }

    #[test]
    fn toon_skips_short_payloads_with_savings() {
        // Uniform rows compress well under TOON, so without the gate this
        // payload would be Applied; the minimum-length check must win.
        let input = serde_json::to_string(&serde_json::json!({
            "items": (0..10)
                .map(|index| serde_json::json!({"name": "same", "value": index}))
                .collect::<Vec<_>>()
        }))
        .unwrap();
        assert!(input.chars().count() < MIN_TOON_CHARS);
        let gated = compress_toon(&input, true, MIN_TOON_CHARS).unwrap();
        assert_eq!(gated.disposition, Disposition::Passthrough);
        assert_eq!(gated.output, input);
        assert_eq!(gated.before_tokens, gated.after_tokens);

        let forced = compress_toon(&input, true, 0).unwrap();
        assert!(forced.applied());
        assert!(forced.after_tokens < forced.before_tokens);
    }

    #[test]
    fn toon_min_chars_boundary_is_inclusive() {
        // Build {"pad":"aaa..."} with exactly MIN_TOON_CHARS characters.
        let overhead = r#"{"pad":""}"#.chars().count();
        let padded = format!(r#"{{"pad":"{}"}}"#, "a".repeat(MIN_TOON_CHARS - overhead));
        assert_eq!(padded.chars().count(), MIN_TOON_CHARS);
        // At exactly the threshold the gate must not fire; the savings
        // comparison alone decides the disposition.
        let result = compress_toon(&padded, true, MIN_TOON_CHARS).unwrap();
        assert_ne!(result.disposition, Disposition::Passthrough);
    }

    #[test]
    fn toon_gate_does_not_skip_json_validation() {
        // Short invalid input must fail with InvalidJson even under the
        // default gate: the gate skips TOON encoding, not JSON validation.
        let error = compress_toon("not json", true, MIN_TOON_CHARS).unwrap_err();
        assert!(matches!(error, RuntimeError::InvalidJson(_)));

        // The same contract holds with the gate disabled.
        let error = compress_toon("not json", true, 0).unwrap_err();
        assert!(matches!(error, RuntimeError::InvalidJson(_)));
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

    fn entry_request(content: &str, seam: Seam) -> CompressionRequest {
        let mut request = CompressionRequest::new(content, "test-agent", seam);
        request.capabilities.replace_output = true;
        request.session_id = Some("session-r".into());
        request.tool_use_id = Some("tool-r".into());
        request
    }

    fn verbose_tools_json() -> String {
        let description =
            "Read a file from the workspace and return its contents as text. ".repeat(12);
        serde_json::to_string(&serde_json::json!([
            {"type": "function", "function": {"name": "read_file", "description": description,
             "parameters": {"type": "object", "properties": {}}}},
        ]))
        .unwrap()
    }

    fn compressible_api_json() -> String {
        serde_json::to_string(&serde_json::json!({
            "url": "https://example.com/data",
            "status": 200,
            "debug": "trace=9f2e11c0 backend_latency_ms=184 retries=0 tls=reused pool=warm shard=eu-central-1a cache=miss",
            "results": (0..6).map(|i| serde_json::json!({
                "name": format!("pkg-{i}"),
                "version": "1.0.0",
                "license": null,
                "homepage": "",
            })).collect::<Vec<_>>(),
            "count": 6,
        }))
        .unwrap()
    }

    const ENTRY_ENABLED: EntryOptions = EntryOptions {
        compression_enabled: true,
        stash_enabled: true,
    };

    #[test]
    fn record_compression_writes_attribution_and_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = StatsRecorder::new(directory.path().join("stats.db")).unwrap();
        let store: Arc<dyn StashStore> = Arc::new(InMemoryStore::new());

        let request = entry_request(&verbose_tools_json(), Seam::BeforeModel);
        let outcome = compress_with_store(&request, &ENTRY_ENABLED, Some(&store));
        assert_eq!(outcome.response.disposition, Disposition::Applied);
        record_compression(&request, &outcome, Some(&recorder), false);

        let records = recorder.records_by_session("session-r", None).unwrap();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.operation, OperationType::CompressSchema);
        assert_eq!(record.seam.as_deref(), Some("before_model"));
        assert_eq!(record.content_type, None);
        assert_eq!(
            record.compressor_chain.as_deref(),
            Some(r#"["schema-compress"]"#)
        );
        assert_eq!(
            record.tokenizer_id.as_deref(),
            Some(tokenless_protocol::TOKENIZER_ID)
        );
        assert_eq!(record.mode, CompressionMode::Active);

        let expected: Vec<(i64, String, String)> = outcome
            .response
            .stash_keys
            .iter()
            .map(|key| (record.id, key.clone(), "schema-compress".to_string()))
            .collect();
        assert!(!expected.is_empty());
        let conn = rusqlite::Connection::open(directory.path().join("stats.db")).unwrap();
        let rows: Vec<(i64, String, String)> = conn
            .prepare(
                "SELECT stats_id, hash, compressor_id FROM compression_artifacts ORDER BY hash",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let mut expected_sorted = expected;
        expected_sorted.sort_by(|a, b| a.1.cmp(&b.1));
        assert_eq!(rows, expected_sorted);
    }

    /// Unsupported text domains produce neither compression rows nor
    /// artifacts while the first Runtime-owned Pipeline ships JSON only.
    #[test]
    fn passthrough_text_produces_no_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = StatsRecorder::new(directory.path().join("stats.db")).unwrap();
        let store: Arc<dyn StashStore> = Arc::new(InMemoryStore::new());

        let mut lines: Vec<String> = (0..4).map(|i| format!("$ cargo build step {i}")).collect();
        lines.extend(
            (0..70).map(|i| format!("\u{1b}[1m\u{1b}[32m   Compiling\u{1b}[0m pkg{i:03} v0.1.{i}")),
        );
        lines.push("error[E0308]: mismatched types".to_string());
        lines.extend((0..12).map(|i| format!("summary tail line {i}")));
        let content = lines.join("\n") + "\n";

        let mut request = entry_request(&content, Seam::PostTool);
        request.tool_name = Some("Bash".into());
        request.capabilities.replace_with_text = true;
        request.capabilities.publish_retrieve_tool = true;
        let outcome = compress_with_store(&request, &ENTRY_ENABLED, Some(&store));
        assert_eq!(outcome.response.disposition, Disposition::Passthrough);
        assert!(outcome.response.compressor_chain.is_empty());
        assert!(outcome.response.stash_keys.is_empty());
        record_compression(&request, &outcome, Some(&recorder), false);

        let conn = rusqlite::Connection::open(directory.path().join("stats.db")).unwrap();
        let artifacts: i64 = conn
            .query_row("SELECT COUNT(*) FROM compression_artifacts", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(artifacts, 0);
    }

    #[test]
    fn record_compression_attributes_the_detected_content_type() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = StatsRecorder::new(directory.path().join("stats.db")).unwrap();

        let mut request = entry_request(&compressible_api_json(), Seam::PostTool);
        request.tool_name = Some("WebFetch".into());
        let outcome = compress_with_store(&request, &ENTRY_ENABLED, None);
        assert_eq!(outcome.response.disposition, Disposition::Applied);
        record_compression(&request, &outcome, Some(&recorder), false);

        let records = recorder.records_by_session("session-r", None).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].seam.as_deref(), Some("post_tool"));
        assert!(records[0].content_type.is_some());
        assert!(records[0].compressor_chain.is_some());
        // An undeclared origin lands as `unspecified`, not NULL.
        assert_eq!(records[0].content_origin.as_deref(), Some("unspecified"));
    }

    #[test]
    fn record_compression_carries_the_declared_origin_to_the_column() {
        // `record_compression` is the only writer of `content_origin`; this
        // pins the wire string end to end, through the column §4.7 will read
        // to tell a protected-path passthrough from an ordinary no-savings
        // row. `ApiResponse` because it is off the release gate, so the row
        // exists to carry the value.
        let directory = tempfile::tempdir().unwrap();
        let recorder = StatsRecorder::new(directory.path().join("stats.db")).unwrap();

        let mut request = entry_request(&compressible_api_json(), Seam::PostTool);
        request.content_origin = ContentOrigin::ApiResponse;
        let outcome = compress_with_store(&request, &ENTRY_ENABLED, None);
        assert_eq!(outcome.response.disposition, Disposition::Applied);
        record_compression(&request, &outcome, Some(&recorder), false);

        let records = recorder.records_by_session("session-r", None).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].content_origin.as_deref(), Some("api_response"));
    }

    #[test]
    fn record_compression_counts_unmarked_truncations_without_a_store() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = StatsRecorder::new(directory.path().join("stats.db")).unwrap();

        // 200 rows against the shell threshold (128 head + 8 tail): the
        // cleanup truncates the array, and with no store attached the drop
        // is unmarked. publish_retrieve_tool stays false, so even an
        // available store would not be attached.
        let content = serde_json::to_string(&serde_json::json!({
            "records": (0..200).map(|i| serde_json::json!({
                "id": i, "name": format!("row-{i}"),
            })).collect::<Vec<_>>(),
        }))
        .unwrap();
        let mut request = entry_request(&content, Seam::PostTool);
        request.tool_name = Some("Bash".into());
        let outcome = compress_with_store(&request, &ENTRY_ENABLED, None);
        assert_eq!(outcome.response.disposition, Disposition::Applied);
        assert_eq!(outcome.stats.unrecoverable_truncations, Some(1));
        record_compression(&request, &outcome, Some(&recorder), false);
        let records = recorder.records_by_session("session-r", None).unwrap();
        assert_eq!(records[0].unrecoverable_truncations, Some(1));

        // Dry-run attaches no store either, but records NULL: a count there
        // would misstate what an active run with stash attached emits.
        let dry = compress_with_store(
            &request,
            &EntryOptions {
                compression_enabled: false,
                stash_enabled: true,
            },
            None,
        );
        assert_eq!(dry.response.disposition, Disposition::DryRun);
        assert_eq!(dry.stats.unrecoverable_truncations, None);
    }

    #[test]
    fn record_compression_dry_run_rows_carry_no_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = StatsRecorder::new(directory.path().join("stats.db")).unwrap();

        let request = entry_request(&verbose_tools_json(), Seam::BeforeModel);
        let outcome = compress_with_store(
            &request,
            &EntryOptions {
                compression_enabled: false,
                stash_enabled: true,
            },
            None,
        );
        assert_eq!(outcome.response.disposition, Disposition::DryRun);
        record_compression(&request, &outcome, Some(&recorder), false);

        let records = recorder.records_by_session("session-r", None).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].mode, CompressionMode::DryRun);
        let conn = rusqlite::Connection::open(directory.path().join("stats.db")).unwrap();
        let artifacts: i64 = conn
            .query_row("SELECT COUNT(*) FROM compression_artifacts", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(artifacts, 0);
    }

    #[test]
    fn record_compression_skips_rows_without_savings() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = StatsRecorder::new(directory.path().join("stats.db")).unwrap();

        let request = entry_request("plain text passthrough content", Seam::PostTool);
        let outcome = compress_with_store(&request, &ENTRY_ENABLED, None);
        assert_eq!(outcome.response.disposition, Disposition::Passthrough);
        record_compression(&request, &outcome, Some(&recorder), false);
        assert_eq!(recorder.count().unwrap(), 0);
    }

    #[test]
    fn retrieve_recorded_logs_each_store_outcome() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = StatsRecorder::new(directory.path().join("stats.db")).unwrap();
        let store = InMemoryStore::new();
        let write = store.stash("recorded payload").unwrap();

        let payload = retrieve_recorded(&store, &write.key, Some(&recorder), "cli").unwrap();
        assert_eq!(payload, "recorded payload");
        let miss = retrieve_recorded(&store, &"0".repeat(24), Some(&recorder), "mcp");
        assert!(matches!(miss, Err(RuntimeError::StashEntryNotFound { .. })));

        struct FailingRetrieve;
        impl StashStore for FailingRetrieve {
            fn stash(&self, _payload: &str) -> Result<tokenless_ccr::StashWrite, StashError> {
                unreachable!()
            }
            fn retrieve(&self, _hash: &str) -> Result<Option<String>, StashError> {
                Err(StashError::Backend("simulated".to_string()))
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
        let error = retrieve_recorded(&FailingRetrieve, &write.key, Some(&recorder), "embedded");
        assert!(matches!(error, Err(RuntimeError::StashRetrieve(_))));

        // Invalid input never reaches the store: no event.
        let invalid = retrieve_recorded(&store, "not-a-hash", Some(&recorder), "cli");
        assert!(matches!(invalid, Err(RuntimeError::InvalidHash { .. })));

        let totals = recorder.retrieve_totals().unwrap();
        assert_eq!(totals.hits, 1);
        assert_eq!(totals.misses, 1);
        assert_eq!(totals.errors, 1);
        assert!(totals.retrieved_tokens > 0);
    }
}
