//! The unified external-hook entry point (roadmap §5.4).
//!
//! One seam router behind `tokenless compress` and
//! [`crate::TokenlessRuntime::compress`]: JSON detection, tool threshold
//! selection, TOON selection, and final acceptance — previously five
//! scattered size checks across the common Python hooks and two CLI
//! subcommands — happen here exactly once. Non-JSON content routes to the
//! plain-text pipeline (terminal cleanup + build/log compressor, §6.1) when
//! the host offers a text slot. Adapters keep only envelope construction
//! (§4.5) — including unwrapping shell envelopes so their stdout/stderr text
//! arrives here as plain text.
//!
//! Every failure past request decoding is fail-open and reported through
//! the disposition: a failed optional compressor never blocks the agent
//! (§5.6).

use std::sync::Arc;

use tokenless_ccr::StashStore;
use tokenless_protocol::{
    CompressionRequest, CompressionResponse, ContentOrigin, Disposition, Reversibility, Seam,
};
use tokenless_schema::SchemaCompressor;
use tokenless_stats::{OperationType, estimate_tokens};

use crate::{
    CompressOptions, MAX_INPUT_BYTES, MIN_TOON_CHARS, ResponsePipelineRun,
    finish_schema_compression, run_response_pipeline, run_text_pipeline, taxonomy,
};

/// Minimum content size (Unicode scalar values, matching the Python hooks'
/// `len()`) for post-tool compression to be attempted at all.
const MIN_RESPONSE_CHARS: usize = 200;

// TOON selection gate: minimum candidate size for the TOON encoding pass,
// shared with the standalone compress-toon CLI/runtime path via
// [`crate::MIN_TOON_CHARS`].

/// Per-call behavior toggles resolved by the frontend from its config.
#[derive(Debug, Clone)]
pub struct EntryOptions {
    /// `false` measures and reports [`Disposition::DryRun`] while emitting
    /// the original content.
    pub compression_enabled: bool,
    /// `false` never attaches the stash, making truncations unrecoverable.
    pub stash_enabled: bool,
}

/// A [`CompressionResponse`] plus the payload the §5.5 recording path
/// ([`crate::record_compression`]) turns into one statistics row.
pub struct EntryOutcome {
    /// The protocol response to hand back to the adapter.
    pub response: CompressionResponse,
    /// Attribution consumed only by [`crate::record_compression`].
    pub(crate) stats: EntryStats,
    /// Successful stash writes still live after all rollbacks, or `None`
    /// when no store was attached.
    pub stash_writes: Option<usize>,
    /// Failed stash operations (writes and rollback deletes), or `None`
    /// when no store was attached.
    pub stash_errors: Option<usize>,
    /// Live stash entry count, or `None` when no store was attached.
    pub stash_size: Option<usize>,
}

/// Per-invocation statistics attribution of the winning path.
pub(crate) struct EntryStats {
    /// Historical operation type of the winning path: TOON win records as
    /// [`OperationType::CompressToon`], cleanup as `CompressResponse`,
    /// before-model as `CompressSchema`.
    pub(crate) op: OperationType,
    /// Measured candidate — meaningful in dry-run, where `response.output`
    /// is the original content.
    pub(crate) measured_text: String,
    /// Truncations without an emitted recovery marker; `None` for seams
    /// and dispositions that cannot truncate.
    pub(crate) unrecoverable_truncations: Option<usize>,
}

impl EntryOutcome {
    fn passthrough(request: &CompressionRequest, diagnostic: Option<String>) -> Self {
        let mut response =
            CompressionResponse::passthrough(request, estimate_tokens(&request.content) as u64);
        response.diagnostic = diagnostic;
        Self {
            response,
            stats: EntryStats {
                op: match request.seam {
                    Seam::BeforeModel => OperationType::CompressSchema,
                    _ => OperationType::CompressResponse,
                },
                measured_text: request.content.clone(),
                unrecoverable_truncations: None,
            },
            stash_writes: None,
            stash_errors: None,
            stash_size: None,
        }
    }
}

/// Routes one protocol request through the seam-appropriate compression
/// path and applies the single end-to-end acceptance.
pub fn compress_with_store(
    request: &CompressionRequest,
    options: &EntryOptions,
    stash_store: Option<&Arc<dyn StashStore>>,
) -> EntryOutcome {
    if request.content.len() > MAX_INPUT_BYTES {
        return EntryOutcome::passthrough(
            request,
            Some(format!(
                "input exceeds {} MiB limit",
                MAX_INPUT_BYTES / (1024 * 1024)
            )),
        );
    }
    // Principle 2: a compressor must not run when the adapter cannot apply
    // its result. Hosts without true output replacement stay passthrough.
    if !request.capabilities.replace_output {
        return EntryOutcome::passthrough(request, None);
    }
    match request.seam {
        Seam::PostTool => post_tool(request, options, stash_store),
        Seam::BeforeModel => before_model(request, options, stash_store),
        // Unimplemented seams route to passthrough (roadmap §5.2).
        Seam::PreTool | Seam::Proxy => EntryOutcome::passthrough(request, None),
    }
}

/// JSON detection with the hooks' string-unwrap semantics: content that is
/// a JSON-encoded string whose inner text is itself a JSON object or array
/// is unwrapped and compact-serialized; direct objects/arrays are used
/// verbatim. Anything else is not a compression subject.
fn normalize_content(content: &str) -> Option<(String, serde_json::Value)> {
    if content.starts_with('"')
        && let Ok(serde_json::Value::String(inner)) = serde_json::from_str(content)
    {
        return match serde_json::from_str::<serde_json::Value>(&inner) {
            Ok(value @ (serde_json::Value::Object(_) | serde_json::Value::Array(_))) => {
                // Compact, non-ASCII kept literal — the same shape the
                // hooks produced with `ensure_ascii=False` so the char
                // gates below measure identically.
                let compact = serde_json::to_string(&value).ok()?;
                Some((compact, value))
            }
            // A string payload without structured content is plain text.
            _ => None,
        };
    }
    match serde_json::from_str::<serde_json::Value>(content) {
        Ok(value @ (serde_json::Value::Object(_) | serde_json::Value::Array(_))) => {
            Some((content.to_string(), value))
        }
        _ => None,
    }
}

/// Port of the hooks' `_restore_dropped_schema_fields`: top-level keys the
/// cleanup dropped because they were empty are restored so replacement
/// output keeps a stable host tool schema (e.g. Bash's stdout/stderr).
/// Intentionally dropped non-empty fields (debug payloads) stay dropped.
fn restore_dropped_schema_fields(
    original: &serde_json::Map<String, serde_json::Value>,
    candidate: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut restored = candidate.clone();
    for (key, value) in original {
        if restored.contains_key(key) {
            continue;
        }
        let empty = value.is_null()
            || value.as_str() == Some("")
            || value.as_object().is_some_and(|map| map.is_empty())
            || value.as_array().is_some_and(|arr| arr.is_empty());
        if empty {
            restored.insert(key.clone(), value.clone());
        }
    }
    restored
}

/// The winning candidate of the post-tool ladder.
struct Winner {
    text: String,
    chain: Vec<String>,
    op: OperationType,
    reversibility: Reversibility,
    /// Whether the pipeline's candidate (and therefore its stash markers)
    /// is part of the emitted text.
    keeps_pipeline: bool,
}

fn post_tool(
    request: &CompressionRequest,
    options: &EntryOptions,
    stash_store: Option<&Arc<dyn StashStore>>,
) -> EntryOutcome {
    let Some((normalized, original_value)) = normalize_content(&request.content) else {
        return post_tool_text(request, options, stash_store);
    };
    let normalized_chars = normalized.chars().count();
    if normalized_chars < MIN_RESPONSE_CHARS {
        return EntryOutcome::passthrough(request, None);
    }

    let thresholds = taxonomy::thresholds_for(request.content_origin, request.tool_name.as_deref());
    let compress_options = CompressOptions {
        truncate_strings_at: Some(thresholds.truncate_strings_at),
        truncate_arrays_at: Some(thresholds.truncate_arrays_at),
        array_tail_preserve: None,
        max_depth: Some(thresholds.max_depth),
        // Markers are only retrievable when the host publishes a retrieve
        // tool; without one, attaching the stash would strand rows.
        stash_enabled: options.stash_enabled && request.capabilities.publish_retrieve_tool,
        require_reversible: false,
    };
    let mut pipeline_request = request.clone();
    pipeline_request.content = normalized.clone();
    let run = run_response_pipeline(
        &pipeline_request,
        &compress_options,
        options.compression_enabled,
        stash_store,
    );

    // The pipeline's token arbitration accepted the candidate (dry-run
    // reports the same acceptance without emitting).
    let pipeline_accepted = matches!(
        run.response.disposition,
        Disposition::Applied | Disposition::DryRun
    );

    // Character acceptance on top of token acceptance — the hooks' gates
    // compose both, in this order, and boundary inputs can differ.
    let cleanup_candidate = match &run.candidate {
        Some(candidate) if pipeline_accepted && candidate.chars().count() < normalized_chars => {
            if request.capabilities.replace_with_text {
                Some(candidate.clone())
            } else {
                // Structured slot: restore empty top-level schema fields,
                // which can cancel a marginal win.
                shape_structured_candidate(&original_value, candidate, normalized_chars)
            }
        }
        _ => None,
    };

    let winner = decide_post_tool_winner(
        request,
        &run,
        &normalized,
        &original_value,
        cleanup_candidate,
    );

    finish_post_tool(request, options, stash_store, run, winner)
}

/// The plain-text post-tool path (roadmap §6.1): terminal cleanup plus the
/// build/log compressor. Content arrives verbatim — no JSON-string unwrap
/// happens here (a JSON-quoted string stays one escaped line and never
/// engages the line-oriented engine).
fn post_tool_text(
    request: &CompressionRequest,
    options: &EntryOptions,
    stash_store: Option<&Arc<dyn StashStore>>,
) -> EntryOutcome {
    // Non-JSON output needs a text slot; a structured slot keeps its
    // envelope-owned JSON path (adapters own envelope knowledge, §4.5).
    if !request.capabilities.replace_with_text {
        return EntryOutcome::passthrough(request, None);
    }
    let content_chars = request.content.chars().count();
    if content_chars < MIN_RESPONSE_CHARS {
        return EntryOutcome::passthrough(request, None);
    }

    let run = run_text_pipeline(
        request,
        options.compression_enabled,
        // Markers are only retrievable when the host publishes a retrieve
        // tool; without one, attaching the stash would strand rows.
        options.stash_enabled && request.capabilities.publish_retrieve_tool,
        stash_store,
    );

    let pipeline_accepted = matches!(
        run.response.disposition,
        Disposition::Applied | Disposition::DryRun
    );
    // Character acceptance on top of token acceptance, composing the same
    // two gates as the JSON path.
    let winner = match &run.candidate {
        Some(candidate) if pipeline_accepted && candidate.chars().count() < content_chars => {
            Some(Winner {
                text: candidate.clone(),
                chain: run.chain.clone(),
                op: OperationType::CompressResponse,
                reversibility: run.response.reversibility,
                keeps_pipeline: true,
            })
        }
        _ => None,
    };
    finish_post_tool(request, options, stash_store, run, winner)
}

/// Restore-and-recheck for hosts whose replacement slot must keep a stable
/// JSON schema. Returns the accepted candidate text, or `None` when the
/// restore cancels the win.
fn shape_structured_candidate(
    original_value: &serde_json::Value,
    candidate: &str,
    normalized_chars: usize,
) -> Option<String> {
    let candidate_value: serde_json::Value = serde_json::from_str(candidate).ok()?;
    match (original_value, &candidate_value) {
        (serde_json::Value::Object(original), serde_json::Value::Object(compressed)) => {
            let restored = restore_dropped_schema_fields(original, compressed);
            let serialized = serde_json::to_string(&serde_json::Value::Object(restored)).ok()?;
            (serialized.chars().count() < normalized_chars).then_some(serialized)
        }
        // A non-object original (array) has no top-level schema to restore.
        _ => Some(candidate.to_string()),
    }
}

fn decide_post_tool_winner(
    request: &CompressionRequest,
    run: &ResponsePipelineRun,
    normalized: &str,
    original_value: &serde_json::Value,
    cleanup_candidate: Option<String>,
) -> Option<Winner> {
    // TOON runs outside the pipeline, so the release gate of §4.3 does not
    // reach it on its own — and `JsonRecords` is protected, which is exactly
    // the shape TOON encodes. Left alone it would re-encode a `package.json`
    // read from disk into a format the file does not have, which is the same
    // desynchronization the gate exists to prevent, arriving by another door.
    //
    // On the file-content path TOON may therefore only run on top of a
    // candidate the pipeline itself produced, which means on a released type.
    // No released type has a compressor today, so in practice this switches
    // TOON off for file reads outright; the condition reopens on its own the
    // day one lands. §9 removes the carve-out for good by moving TOON inside
    // the JSON compressor, where the gate covers it like any other.
    let released_by_the_pipeline =
        request.content_origin != ContentOrigin::FileContent || cleanup_candidate.is_some();
    // TOON is a non-JSON encoding: only hosts whose slot accepts arbitrary
    // text can apply it.
    let toon = if request.capabilities.replace_with_text && released_by_the_pipeline {
        let base_text = cleanup_candidate.as_deref().unwrap_or(normalized);
        let base_chars = base_text.chars().count();
        if base_chars >= MIN_TOON_CHARS {
            try_toon(
                base_text,
                original_value,
                cleanup_candidate.as_deref(),
                base_chars,
            )
        } else {
            None
        }
    } else {
        None
    };

    if let Some(toon_text) = toon {
        let keeps_pipeline = cleanup_candidate.is_some();
        return Some(Winner {
            text: toon_text,
            chain: if keeps_pipeline {
                vec!["response-cleanup".into(), "toon".into()]
            } else {
                vec!["toon".into()]
            },
            op: OperationType::CompressToon,
            // TOON is a decodable re-encoding: it degrades nothing beyond
            // what the cleanup already claimed.
            reversibility: if keeps_pipeline {
                run.response.reversibility
            } else {
                Reversibility::Lossless
            },
            keeps_pipeline,
        });
    }
    cleanup_candidate.map(|text| Winner {
        text,
        chain: vec!["response-cleanup".into()],
        op: OperationType::CompressResponse,
        reversibility: run.response.reversibility,
        keeps_pipeline: true,
    })
}

/// The TOON leg: encode, then apply the legacy token no-savings check and
/// the character comparison. Encoder failures are fail-open.
fn try_toon(
    base_text: &str,
    original_value: &serde_json::Value,
    cleanup_candidate: Option<&str>,
    base_chars: usize,
) -> Option<String> {
    // Reparse only when the base is the cleanup candidate; the original
    // value is already at hand otherwise.
    let parsed_candidate = match cleanup_candidate {
        Some(candidate) => Some(serde_json::from_str::<serde_json::Value>(candidate).ok()?),
        None => None,
    };
    let base_value = parsed_candidate.as_ref().unwrap_or(original_value);
    let encoded = toon_format::encode_default(base_value).ok()?;
    let toon = encoded.trim_end().to_string();
    if toon.is_empty() || estimate_tokens(&toon) >= estimate_tokens(base_text) {
        return None;
    }
    (toon.chars().count() < base_chars).then_some(toon)
}

fn finish_post_tool(
    request: &CompressionRequest,
    options: &EntryOptions,
    stash_store: Option<&Arc<dyn StashStore>>,
    run: ResponsePipelineRun,
    winner: Option<Winner>,
) -> EntryOutcome {
    let before_tokens = estimate_tokens(&request.content);
    let store_attached = run.stash_writes.is_some();

    // Stash rows whose markers do not reach the model are rolled back here,
    // mirroring the pipeline ledger's delete-by-generation discipline. This
    // covers both a full rejection and a TOON-over-original win after the
    // cleanup was rejected — cases where the old CLI-commits/hook-discards
    // split orphaned rows.
    let keeps_pipeline = winner.as_ref().is_some_and(|winner| winner.keeps_pipeline);
    let (removed, failed) = if !keeps_pipeline
        && !run.committed_writes.is_empty()
        && let Some(store) = stash_store
    {
        let mut removed = 0usize;
        let mut failed = 0usize;
        for write in &run.committed_writes {
            match store.delete(&write.key, write.generation) {
                Ok(true) => removed += 1,
                Ok(false) => {}
                Err(_) => failed += 1,
            }
        }
        (removed, failed)
    } else {
        (0, 0)
    };
    let stash_writes = run
        .stash_writes
        .map(|writes| writes.saturating_sub(removed));
    let stash_errors = run.stash_errors.map(|errors| errors + failed);
    let stash_size = if store_attached {
        stash_store.map(|store| store.len())
    } else {
        None
    };

    let mut response = run.response;
    match winner {
        Some(winner) => {
            let after_tokens = estimate_tokens(&winner.text) as u64;
            response.after_tokens = after_tokens;
            response.reversibility = winner.reversibility;
            response.compressor_chain = winner.chain;
            if !winner.keeps_pipeline {
                response.stash_keys = Vec::new();
            }
            let (output, disposition) = if options.compression_enabled {
                (winner.text.clone(), Disposition::Applied)
            } else {
                (request.content.clone(), Disposition::DryRun)
            };
            response.output = output;
            response.disposition = disposition;
            response.before_tokens = before_tokens as u64;
            // Truncations reach the model only while the cleanup candidate
            // is part of the emitted text; a TOON-over-original win
            // discarded them with the rollback above. Without an attached
            // store (retrieve unpublished, stash disabled or unavailable)
            // every truncation in an emitted candidate is unmarked; dry-run
            // stays unmeasured (NULL) because it never attaches a store, so
            // a count would misstate what an active run with stash records.
            let unrecoverable_truncations = if !winner.keeps_pipeline {
                None
            } else if run.unrecoverable_truncations.is_some() {
                run.unrecoverable_truncations
            } else if options.compression_enabled {
                Some(run.truncations)
            } else {
                None
            };
            EntryOutcome {
                response,
                stats: EntryStats {
                    op: winner.op,
                    measured_text: winner.text,
                    unrecoverable_truncations,
                },
                stash_writes,
                stash_errors,
                stash_size,
            }
        }
        None => {
            // No candidate survived: emit the original. A candidate the
            // character gates rejected downgrades the pipeline's acceptance
            // to no-savings; other dispositions pass through unchanged.
            let disposition = match response.disposition {
                Disposition::Applied | Disposition::DryRun => Disposition::NoSavings,
                other => other,
            };
            response.output = request.content.clone();
            response.disposition = disposition;
            response.before_tokens = before_tokens as u64;
            response.after_tokens = before_tokens as u64;
            response.reversibility = Reversibility::Lossless;
            response.compressor_chain = Vec::new();
            response.stash_keys = Vec::new();
            EntryOutcome {
                response,
                stats: EntryStats {
                    op: OperationType::CompressResponse,
                    measured_text: request.content.clone(),
                    unrecoverable_truncations: None,
                },
                stash_writes,
                stash_errors,
                stash_size,
            }
        }
    }
}

fn before_model(
    request: &CompressionRequest,
    options: &EntryOptions,
    stash_store: Option<&Arc<dyn StashStore>>,
) -> EntryOutcome {
    let value = match serde_json::from_str::<serde_json::Value>(&request.content) {
        Ok(value @ (serde_json::Value::Object(_) | serde_json::Value::Array(_))) => value,
        // Fail-open boundary: schema requests carry JSON tool declarations;
        // anything else is not a compression subject.
        _ => return EntryOutcome::passthrough(request, None),
    };

    let attached_store = if options.compression_enabled && options.stash_enabled {
        stash_store
    } else {
        None
    };
    let mut compressor = SchemaCompressor::new();
    if let Some(store) = attached_store {
        compressor = compressor.with_stash_store(Arc::clone(store));
    }
    // An array compresses element-wise (the CLI `--batch` semantics the
    // schema hook has always used); a single declaration object as-is.
    let compressed_value = match &value {
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(|item| compressor.compress(item)).collect())
        }
        other => compressor.compress(other),
    };
    let Ok(compressed_output) = serde_json::to_string(&compressed_value) else {
        return EntryOutcome::passthrough(request, Some("serialize failed".into()));
    };
    // Capture before the disposition ladder rolls back or clears the
    // session: on Applied these are exactly the emitted keys (every schema
    // stash write has a marker in the applied output).
    let pending_keys = compressor.stash_keys();
    let result = finish_schema_compression(
        &request.content,
        compressed_output,
        options.compression_enabled,
        attached_store,
        &compressor,
    );

    let applied = result.disposition == Disposition::Applied;
    let measured = matches!(
        result.disposition,
        Disposition::Applied | Disposition::DryRun
    );
    let mut response =
        CompressionResponse::passthrough(request, estimate_tokens(&request.content) as u64);
    response.output = result.output.clone();
    response.disposition = result.disposition;
    response.compressor_chain = vec!["schema-compress".into()];
    response.after_tokens = if measured {
        result.after_tokens as u64
    } else {
        result.before_tokens as u64
    };
    response.reversibility = if applied && result.stash_writes.unwrap_or(0) > 0 {
        Reversibility::Retrievable
    } else {
        Reversibility::Lossless
    };
    if applied {
        response.stash_keys = pending_keys;
    }
    EntryOutcome {
        response,
        stats: EntryStats {
            op: OperationType::CompressSchema,
            measured_text: if measured {
                result.compressed_output
            } else {
                request.content.clone()
            },
            unrecoverable_truncations: None,
        },
        stash_writes: result.stash_writes,
        stash_errors: result.stash_errors,
        stash_size: result.stash_size,
    }
}

#[cfg(test)]
mod tests {
    use tokenless_ccr::InMemoryStore;
    use tokenless_protocol::PROTOCOL_VERSION;

    use super::*;

    const ENABLED: EntryOptions = EntryOptions {
        compression_enabled: true,
        stash_enabled: true,
    };
    const DRY_RUN: EntryOptions = EntryOptions {
        compression_enabled: false,
        stash_enabled: true,
    };

    fn request(content: &str, seam: Seam) -> CompressionRequest {
        let mut request = CompressionRequest::new(content, "test-agent", seam);
        request.capabilities.replace_output = true;
        request
    }

    fn post_tool_request(content: &str, tool_name: &str) -> CompressionRequest {
        let mut request = request(content, Seam::PostTool);
        request.tool_name = Some(tool_name.into());
        request
    }

    /// A compressible API payload: the non-empty debug field is dropped for
    /// a win that survives the structured-slot schema restore.
    fn compressible_object() -> String {
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

    /// Uniform records with nothing to clean up: cleanup yields no savings,
    /// but the shape is TOON-friendly and over the TOON gate.
    fn toon_only_object() -> String {
        serde_json::to_string(&serde_json::json!({
            "matches": (0..16).map(|i| serde_json::json!({
                "file": format!("src/deep/nested/module_{i:02}.rs"),
                "line": 100 + i * 13,
                "column": 5 + i % 9,
                "symbol": format!("handle_case_{i:02}"),
            })).collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    fn verbose_tools() -> String {
        let description =
            "Read a file from the workspace and return its contents as text. ".repeat(12);
        serde_json::to_string(&serde_json::json!([
            {"type": "function", "function": {"name": "read_file", "description": description,
             "parameters": {"type": "object", "properties": {}}}},
        ]))
        .unwrap()
    }

    #[test]
    fn unimplemented_seams_route_to_passthrough() {
        for seam in [Seam::PreTool, Seam::Proxy] {
            let outcome =
                compress_with_store(&request(&compressible_object(), seam), &ENABLED, None);
            assert_eq!(outcome.response.disposition, Disposition::Passthrough);
            assert_eq!(outcome.response.output, compressible_object());
        }
    }

    #[test]
    fn missing_replace_output_is_passthrough() {
        let mut req = post_tool_request(&compressible_object(), "WebFetch");
        req.capabilities.replace_output = false;
        let outcome = compress_with_store(&req, &ENABLED, None);
        assert_eq!(outcome.response.disposition, Disposition::Passthrough);
    }

    #[test]
    fn a_tool_name_no_longer_decides_on_its_own() {
        // The layer-1 skip list is gone (roadmap §6.3): a request naming a
        // read tool but declaring no origin is judged by its content like any
        // other. Adapters still prefilter these before spawning, so this is
        // not a change any host sees — it is a change to what this API means.
        let outcome = compress_with_store(
            &post_tool_request(&compressible_object(), "Read"),
            &ENABLED,
            None,
        );
        assert_eq!(outcome.response.disposition, Disposition::Applied);
    }

    /// The same request with an origin declared.
    fn origin_request(content: &str, tool: &str, origin: ContentOrigin) -> CompressionRequest {
        let mut request = post_tool_request(content, tool);
        request.content_origin = origin;
        request
    }

    /// A build log committed to this repository, read back the way an agent
    /// would read any tracked file.
    const TRACKED_BUILD_LOG: &str =
        include_str!("../../tokenless-compressors/tests/fixtures/build_logs/cargo_failure.txt");

    #[test]
    fn a_build_log_read_from_disk_is_not_rewritten() {
        // `BuildLog` was released until review found it shares the flaw that
        // keeps `JsonRecords` protected: the detector scores content alone, so
        // this fixture — or a contributor doc quoting a compiler twice, see
        // `prose_carrying_two_generic_markers_is_a_known_detection_boundary` —
        // sits in the bucket beside the output of the build that just ran.
        let store: Arc<dyn StashStore> = Arc::new(InMemoryStore::new());

        let mut file = text_request(TRACKED_BUILD_LOG);
        file.content_origin = ContentOrigin::FileContent;
        let file = compress_with_store(&file, &ENABLED, Some(&store));
        assert_eq!(file.response.disposition, Disposition::Passthrough);
        assert_eq!(file.response.output, TRACKED_BUILD_LOG);
        assert!(file.response.compressor_chain.is_empty());
        assert!(file.response.stash_keys.is_empty());
        // Protected means full passthrough: nothing was stashed either.
        assert_eq!(store.len(), 0);

        // The same bytes as command output still compress. Measured on the
        // released list this replaces: the file path ran `["terminal-cleanup",
        // "build-log"]`, stripped SGR and folded a run of lines behind a
        // retrieval marker — a view the file on disk does not have, which the
        // model's next exact-match edit would be written against.
        let mut command = text_request(TRACKED_BUILD_LOG);
        command.content_origin = ContentOrigin::CommandOutput;
        let command = compress_with_store(&command, &ENABLED, Some(&store));
        assert_eq!(command.response.disposition, Disposition::Applied);
        assert_ne!(command.response.output, TRACKED_BUILD_LOG);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn an_authored_json_config_read_from_disk_is_not_rewritten() {
        // The taxonomy has one bucket for all JSON, so releasing it would hand
        // the response cleanup a hand-authored `package.json`: it drops null
        // and empty fields, and the model's next exact-match edit then fails
        // against a file it believes it read. Measured before this test
        // existed: `"description"` and `"license"` were both removed.
        let package_json = serde_json::to_string_pretty(&serde_json::json!({
            "name": "my-app",
            "version": "1.0.0",
            "description": "",
            "license": null,
            "keywords": [],
            "dependencies": (0..40)
                .map(|i| (format!("dep-{i:02}"), serde_json::json!("^1.0.0")))
                .collect::<serde_json::Map<_, _>>(),
        }))
        .unwrap();

        // A text slot, where the cleanup's removals are final — a structured
        // slot restores top-level fields on the way out, which hides the
        // damage for that shape but not for this one. It is also the slot
        // TOON needs, so the byte-identical assertion below pins the entry's
        // carve-out too: no pipeline candidate on a file read, no TOON.
        let mut file = origin_request(&package_json, "Read", ContentOrigin::FileContent);
        file.capabilities.replace_with_text = true;
        let outcome = compress_with_store(&file, &ENABLED, None);
        assert_eq!(outcome.response.disposition, Disposition::Passthrough);
        assert_eq!(outcome.response.output, package_json);

        // The same bytes as command output: still compressed, and the fields
        // do go. The gate is about where the content came from, not what it
        // is — and this is what the file path was about to do.
        let mut command = origin_request(&package_json, "Bash", ContentOrigin::CommandOutput);
        command.capabilities.replace_with_text = true;
        let outcome = compress_with_store(&command, &ENABLED, None);
        assert_eq!(outcome.response.disposition, Disposition::Applied);
        assert!(!outcome.response.output.contains("description"));
        assert!(!outcome.response.output.contains("license"));
    }

    /// Long enough to engage the generic mode, and detected as `PlainText` —
    /// the release list protects it.
    fn prose_text() -> String {
        (0..120)
            .map(|i| format!("record {i} holding some ordinary content\n"))
            .collect()
    }

    #[test]
    fn protected_content_from_a_file_passes_through_byte_identical() {
        // The detector cannot tell prose from source code in a language it
        // does not know, and a rewritten copy of either breaks the model's
        // next exact-match edit against the file.
        let prose = prose_text();
        let store: Arc<dyn StashStore> = Arc::new(InMemoryStore::new());

        let mut command = text_request(&prose);
        command.content_origin = ContentOrigin::CommandOutput;
        let command = compress_with_store(&command, &ENABLED, Some(&store));
        assert_eq!(command.response.disposition, Disposition::Applied);

        let mut file = text_request(&prose);
        file.content_origin = ContentOrigin::FileContent;
        let before = store.len();
        let file = compress_with_store(&file, &ENABLED, Some(&store));
        assert_eq!(file.response.disposition, Disposition::Passthrough);
        assert_eq!(file.response.output, prose);
        assert!(file.response.compressor_chain.is_empty());
        assert!(file.response.stash_keys.is_empty());
        // Protected means full passthrough: not even the lossless stage ran.
        assert_eq!(store.len(), before);
    }

    #[test]
    fn an_undeclared_origin_never_reaches_the_release_gate() {
        // The migration's inert default: the same protected content, with no
        // origin declared, compresses exactly as it did before this change.
        let store: Arc<dyn StashStore> = Arc::new(InMemoryStore::new());
        let outcome = compress_with_store(&text_request(&prose_text()), &ENABLED, Some(&store));
        assert_eq!(outcome.response.disposition, Disposition::Applied);
    }

    #[test]
    fn non_json_and_scalar_content_pass_through() {
        let text = "plain build log without any JSON structure ".repeat(10);
        for content in [text.as_str(), "12345678", "\"a JSON string of plain text\""] {
            let outcome = compress_with_store(&post_tool_request(content, "Bash"), &ENABLED, None);
            assert_eq!(outcome.response.disposition, Disposition::Passthrough);
        }
    }

    /// A post-tool request whose host offers a text slot and a retrieve
    /// tool — the shape the hook sends after unwrapping a shell envelope.
    fn text_request(content: &str) -> CompressionRequest {
        let mut request = post_tool_request(content, "Bash");
        request.capabilities.replace_with_text = true;
        request.capabilities.publish_retrieve_tool = true;
        request
    }

    fn build_log_text() -> String {
        let mut lines: Vec<String> = (0..4).map(|i| format!("$ cargo build step {i}")).collect();
        lines.extend((0..70).map(|i| format!("   Compiling pkg{i:03} v0.1.{i}")));
        lines.push("error[E0308]: mismatched types in src/main.rs".to_string());
        lines.extend((0..12).map(|i| format!("summary tail line {i}")));
        lines.join("\n") + "\n"
    }

    #[test]
    fn text_slot_build_log_applies_and_round_trips() {
        let store: Arc<dyn StashStore> = Arc::new(InMemoryStore::new());
        let req = text_request(&build_log_text());
        let outcome = compress_with_store(&req, &ENABLED, Some(&store));

        assert_eq!(outcome.response.disposition, Disposition::Applied);
        assert!(
            outcome
                .response
                .compressor_chain
                .contains(&"build-log".to_string())
        );
        assert_eq!(outcome.stats.op, OperationType::CompressResponse);
        assert!(!outcome.response.stash_keys.is_empty());
        assert!(outcome.response.output.contains("error[E0308]"));
        assert_eq!(outcome.stats.unrecoverable_truncations, Some(0));
        assert!(outcome.stash_writes.is_some_and(|writes| writes > 0));
        // Every emitted marker is backed by a byte-exact retrievable slice
        // of the original content.
        for key in &outcome.response.stash_keys {
            assert!(
                outcome
                    .response
                    .output
                    .contains(&tokenless_ccr::marker_for(key))
            );
            let payload = store.retrieve(key).unwrap().expect("stashed payload");
            assert!(build_log_text().contains(&payload));
        }
    }

    #[test]
    fn text_without_a_text_slot_stays_passthrough() {
        let mut req = post_tool_request(&build_log_text(), "Bash");
        req.capabilities.publish_retrieve_tool = true;
        let outcome = compress_with_store(&req, &ENABLED, None);
        assert_eq!(outcome.response.disposition, Disposition::Passthrough);
        assert_eq!(outcome.response.output, build_log_text());
    }

    #[test]
    fn short_text_is_passthrough() {
        let outcome = compress_with_store(&text_request("error: boom\n"), &ENABLED, None);
        assert_eq!(outcome.response.disposition, Disposition::Passthrough);
    }

    #[test]
    fn text_dry_run_measures_without_a_store_and_keeps_the_chain() {
        let req = text_request(&build_log_text());
        let outcome = compress_with_store(&req, &DRY_RUN, None);

        assert_eq!(outcome.response.disposition, Disposition::DryRun);
        assert_eq!(outcome.response.output, build_log_text());
        assert!(
            outcome
                .response
                .compressor_chain
                .contains(&"build-log".to_string())
        );
        // Markers are measurement-only: content-derived keys, no writes.
        assert!(outcome.stats.measured_text.contains("<<tokenless:"));
        assert!(outcome.response.stash_keys.is_empty());
        assert_eq!(outcome.stash_writes, None);
        assert_eq!(outcome.stats.unrecoverable_truncations, None);
    }

    #[test]
    fn text_dry_run_chain_omits_a_rolled_back_cleanup() {
        // A parameterless SGR is three characters, and `heuristic-v1` counts
        // `chars.div_ceil(4)`: at a character count that is a multiple of
        // four, removing it leaves the count unchanged, so the pipeline
        // reverts the whole lossless stage and the build/log engine runs on
        // the uncleaned text. The measurement chain must then name only what
        // actually shaped the candidate.
        let mut content = build_log_text().replacen("$ cargo", "$ \u{1b}[mcargo", 1);
        while content.chars().count() % 4 != 0 {
            content.push(' ');
        }
        let outcome = compress_with_store(&text_request(&content), &DRY_RUN, None);

        assert_eq!(outcome.response.disposition, Disposition::DryRun);
        assert_eq!(outcome.response.compressor_chain, ["build-log"]);
        // The engine saw the escape because the cleanup was rolled back.
        assert!(outcome.stats.measured_text.contains('\u{1b}'));
    }

    #[test]
    fn text_active_without_stash_applies_cleanup_only() {
        let mut lines: Vec<String> = (0..40)
            .map(|i| format!("\u{1b}[1m\u{1b}[32m   Compiling\u{1b}[0m pkg{i:03} v0.1.{i}"))
            .collect();
        lines.push("\u{1b}[1m    Finished\u{1b}[0m `release` profile in 12.02s".to_string());
        let content = lines.join("\n") + "\n";
        let options = EntryOptions {
            compression_enabled: true,
            stash_enabled: false,
        };
        let outcome = compress_with_store(&text_request(&content), &options, None);

        assert_eq!(outcome.response.disposition, Disposition::Applied);
        assert_eq!(outcome.response.compressor_chain, ["terminal-cleanup"]);
        assert!(!outcome.response.output.contains('\u{1b}'));
        assert!(!outcome.response.output.contains("<<tokenless:"));
        assert!(outcome.response.stash_keys.is_empty());
        // The lossy stage was excluded, so the emitted candidate holds no
        // unmarked omissions.
        assert_eq!(outcome.stats.unrecoverable_truncations, Some(0));
    }

    #[test]
    fn long_non_log_text_gets_the_generic_mode() {
        let store: Arc<dyn StashStore> = Arc::new(InMemoryStore::new());
        let prose: String = (0..120)
            .map(|i| format!("record {i} holding some ordinary content\n"))
            .collect();
        let outcome = compress_with_store(&text_request(&prose), &ENABLED, Some(&store));

        assert_eq!(outcome.response.disposition, Disposition::Applied);
        assert_eq!(outcome.response.compressor_chain, ["build-log"]);
        assert_eq!(outcome.response.stash_keys.len(), 1);
        assert!(
            outcome
                .response
                .output
                .contains("… (omitted 40 lines, run: tokenless retrieve")
        );
    }

    #[test]
    fn size_gate_counts_code_points_not_bytes() {
        // 98 chars but 278 bytes: under the gate only when counted in
        // Unicode scalar values, like the Python hooks' len().
        let content = format!(r#"{{"k":"{}"}}"#, "你".repeat(90));
        assert!(content.len() > MIN_RESPONSE_CHARS);
        assert!(content.chars().count() < MIN_RESPONSE_CHARS);
        let outcome = compress_with_store(&post_tool_request(&content, "WebFetch"), &ENABLED, None);
        assert_eq!(outcome.response.disposition, Disposition::Passthrough);
    }

    #[test]
    fn structured_slot_win_restores_empty_fields_and_drops_debug() {
        let outcome = compress_with_store(
            &post_tool_request(&compressible_object(), "WebFetch"),
            &ENABLED,
            None,
        );
        assert_eq!(outcome.response.disposition, Disposition::Applied);
        assert_eq!(outcome.response.compressor_chain, ["response-cleanup"]);
        assert_eq!(outcome.stats.op, OperationType::CompressResponse);
        let output: serde_json::Value = serde_json::from_str(&outcome.response.output).unwrap();
        assert!(output.get("debug").is_none());
        // Nested empties stay dropped; only top-level schema fields return.
        assert!(output["results"][0].get("license").is_none());
        assert!(outcome.response.after_tokens < outcome.response.before_tokens);
    }

    #[test]
    fn restore_cancelling_win_is_no_savings_for_structured_slots() {
        // Only empty top-level fields are droppable: the restore puts every
        // one of them back, cancelling the win.
        let content = serde_json::to_string(&serde_json::json!({
            "stdout": "line of output. ".repeat(20),
            "stderr": "",
            "metadata": null,
            "warnings": [],
            "env": {},
        }))
        .unwrap();
        let outcome = compress_with_store(&post_tool_request(&content, "Bash"), &ENABLED, None);
        assert_eq!(outcome.response.disposition, Disposition::NoSavings);
        assert_eq!(outcome.response.output, content);
        assert!(outcome.response.compressor_chain.is_empty());

        // A text slot keeps the unrestored candidate instead.
        let mut text_slot = post_tool_request(&content, "Bash");
        text_slot.capabilities.replace_with_text = true;
        let outcome = compress_with_store(&text_slot, &ENABLED, None);
        assert_eq!(outcome.response.disposition, Disposition::Applied);
        let output: serde_json::Value = serde_json::from_str(&outcome.response.output).unwrap();
        assert!(output.get("stderr").is_none());
    }

    #[test]
    fn toon_runs_only_for_text_slots() {
        let mut text_slot = post_tool_request(&toon_only_object(), "mcp__code_search");
        text_slot.capabilities.replace_with_text = true;
        let outcome = compress_with_store(&text_slot, &ENABLED, None);
        assert_eq!(outcome.response.disposition, Disposition::Applied);
        assert_eq!(outcome.response.compressor_chain, ["toon"]);
        assert_eq!(outcome.stats.op, OperationType::CompressToon);
        assert_eq!(outcome.response.reversibility, Reversibility::Lossless);
        assert!(!outcome.response.output.starts_with('{'));

        // The same content on a structured slot: cleanup finds nothing and
        // TOON never runs.
        let outcome = compress_with_store(
            &post_tool_request(&toon_only_object(), "mcp__code_search"),
            &ENABLED,
            None,
        );
        assert_eq!(outcome.response.disposition, Disposition::NoSavings);
        assert_eq!(outcome.response.output, toon_only_object());
    }

    #[test]
    fn toon_composes_with_an_accepted_cleanup() {
        // Like compressible_object, but large enough that the cleaned
        // candidate stays over the 500-char TOON gate.
        let content = serde_json::to_string(&serde_json::json!({
            "debug": "trace=9f2e11c0 backend_latency_ms=184 retries=0 cache=miss",
            "results": (0..16).map(|i| serde_json::json!({
                "name": format!("package-{i:02}"),
                "version": format!("1.{i}.0"),
                "license": null,
                "homepage": "",
            })).collect::<Vec<_>>(),
        }))
        .unwrap();
        let mut req = post_tool_request(&content, "WebFetch");
        req.capabilities.replace_with_text = true;
        let outcome = compress_with_store(&req, &ENABLED, None);
        assert_eq!(outcome.response.disposition, Disposition::Applied);
        assert_eq!(
            outcome.response.compressor_chain,
            ["response-cleanup", "toon"]
        );
        assert_eq!(outcome.stats.op, OperationType::CompressToon);
    }

    #[test]
    fn string_wrapped_json_is_unwrapped_before_compression() {
        let wrapped = serde_json::to_string(&toon_only_object()).unwrap();
        assert!(wrapped.starts_with('"'));
        let mut req = post_tool_request(&wrapped, "mcp__code_search");
        req.capabilities.replace_with_text = true;
        let outcome = compress_with_store(&req, &ENABLED, None);
        assert_eq!(outcome.response.disposition, Disposition::Applied);
        assert_eq!(outcome.response.compressor_chain, ["toon"]);
    }

    #[test]
    fn dry_run_measures_the_candidate_without_emitting_it() {
        let content = compressible_object();
        let outcome = compress_with_store(&post_tool_request(&content, "WebFetch"), &DRY_RUN, None);
        assert_eq!(outcome.response.disposition, Disposition::DryRun);
        assert_eq!(outcome.response.output, content);
        assert_ne!(outcome.stats.measured_text, content);
        assert!(outcome.response.after_tokens < outcome.response.before_tokens);
        assert_eq!(outcome.stash_writes, None);
    }

    #[test]
    fn rejected_candidates_roll_back_their_stash_rows() {
        // Drive finish_post_tool directly: constructing a real payload whose
        // candidate wins tokens but loses the char gate is not stable across
        // estimator tweaks, while the rollback contract itself is exact.
        let store: Arc<dyn StashStore> = Arc::new(InMemoryStore::new());
        let write = store.stash("stashed payload").unwrap();
        assert_eq!(store.len(), 1);
        let req = post_tool_request(&compressible_object(), "WebFetch");
        let run = ResponsePipelineRun {
            response: {
                let mut response = CompressionResponse::passthrough(&req, 10);
                response.disposition = Disposition::Applied;
                response.stash_keys = vec![write.key.clone()];
                response
            },
            candidate: Some("{}".into()),
            committed_writes: vec![write],
            stash_writes: Some(1),
            stash_errors: Some(0),
            unrecoverable_truncations: Some(0),
            stash_size: Some(1),
            truncations: 0,
            chain: vec![],
        };
        let outcome = finish_post_tool(&req, &ENABLED, Some(&store), run, None);
        assert_eq!(outcome.response.disposition, Disposition::NoSavings);
        assert_eq!(store.len(), 0, "rejected rows must be rolled back");
        assert_eq!(outcome.stash_writes, Some(0));
        assert!(outcome.response.stash_keys.is_empty());
    }

    #[test]
    fn oversized_content_passes_through_with_a_diagnostic() {
        let content = format!(r#"{{"k":"{}"}}"#, "x".repeat(MAX_INPUT_BYTES));
        let outcome = compress_with_store(&post_tool_request(&content, "Bash"), &ENABLED, None);
        assert_eq!(outcome.response.disposition, Disposition::Passthrough);
        assert!(outcome.response.diagnostic.is_some());
    }

    #[test]
    fn schema_array_compresses_element_wise_with_markers() {
        let store: Arc<dyn StashStore> = Arc::new(InMemoryStore::new());
        let content = verbose_tools();
        let outcome = compress_with_store(
            &request(&content, Seam::BeforeModel),
            &ENABLED,
            Some(&store),
        );
        assert_eq!(outcome.response.disposition, Disposition::Applied);
        assert_eq!(outcome.response.compressor_chain, ["schema-compress"]);
        assert_eq!(outcome.stats.op, OperationType::CompressSchema);
        assert_eq!(outcome.response.reversibility, Reversibility::Retrievable);
        let output: serde_json::Value = serde_json::from_str(&outcome.response.output).unwrap();
        assert!(output.is_array());
        assert!(outcome.response.output.contains("<<tokenless:"));
        assert!(outcome.response.after_tokens < outcome.response.before_tokens);
        // Applied schema results expose their emitted keys for the
        // artifacts ledger; every key's marker is in the output.
        assert!(!outcome.response.stash_keys.is_empty());
        for key in &outcome.response.stash_keys {
            assert!(outcome.response.output.contains(key.as_str()));
        }
    }

    #[test]
    fn schema_without_a_store_reports_reversibility_unavailable() {
        let outcome = compress_with_store(
            &request(&verbose_tools(), Seam::BeforeModel),
            &ENABLED,
            None,
        );
        assert_eq!(
            outcome.response.disposition,
            Disposition::ReversibilityUnavailable
        );
        assert_eq!(outcome.response.output, verbose_tools());
    }

    #[test]
    fn schema_no_savings_returns_the_original() {
        let store: Arc<dyn StashStore> = Arc::new(InMemoryStore::new());
        let content = r#"[{"type":"function","function":{"name":"ping","description":"Check connectivity.","parameters":{"type":"object","properties":{}}}}]"#;
        let outcome =
            compress_with_store(&request(content, Seam::BeforeModel), &ENABLED, Some(&store));
        assert_eq!(outcome.response.disposition, Disposition::NoSavings);
        assert_eq!(outcome.response.output, content);
        assert_eq!(store.len(), 0, "no-savings rolls the stash session back");
        assert!(
            outcome.response.stash_keys.is_empty(),
            "unapplied schema results expose no artifact keys"
        );
    }

    #[test]
    fn schema_dry_run_measures_without_emitting() {
        let content = verbose_tools();
        let outcome = compress_with_store(&request(&content, Seam::BeforeModel), &DRY_RUN, None);
        assert_eq!(outcome.response.disposition, Disposition::DryRun);
        assert_eq!(outcome.response.output, content);
        assert_ne!(outcome.stats.measured_text, content);
    }

    #[test]
    fn schema_non_json_content_is_passthrough() {
        let outcome = compress_with_store(
            &request("not json at all", Seam::BeforeModel),
            &ENABLED,
            None,
        );
        assert_eq!(outcome.response.disposition, Disposition::Passthrough);
    }

    #[test]
    fn request_version_is_the_protocol_version() {
        let outcome = compress_with_store(
            &post_tool_request(&compressible_object(), "WebFetch"),
            &ENABLED,
            None,
        );
        assert_eq!(outcome.response.protocol_version, PROTOCOL_VERSION);
    }
}
