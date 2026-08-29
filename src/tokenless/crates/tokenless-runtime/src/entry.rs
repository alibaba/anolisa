//! The unified external-hook entry point (roadmap §5.4).
//!
//! One seam router backs `tokenless compress` and
//! [`crate::TokenlessRuntime::compress`]. Phase one routes JSON through the
//! Runtime-owned PostTool pipeline and passes every other content domain
//! through unchanged. Adapters keep only envelope construction.
//!
//! Every failure past request decoding is fail-open and reported through
//! the disposition: a failed optional compressor never blocks the agent
//! (§5.6).

use std::sync::Arc;

use tokenless_ccr::StashStore;
use tokenless_compressors::{JsonCompressionConfig, JsonOperation};
use tokenless_protocol::{
    CompressionRequest, CompressionResponse, Disposition, Reversibility, Seam,
};
use tokenless_schema::SchemaCompressor;
use tokenless_stats::{OperationType, estimate_tokens};

use crate::{
    MAX_INPUT_BYTES, MIN_TOON_CHARS, RESPONSE_PIPELINE_TIMEOUT, finish_schema_compression,
    post_tool::{PostToolPipeline, PostToolPipelineConfig},
    taxonomy,
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
    match request.seam {
        Seam::PostTool => post_tool(request, options, stash_store),
        Seam::BeforeModel => {
            if request.content.len() > MAX_INPUT_BYTES {
                EntryOutcome::passthrough(
                    request,
                    Some(format!(
                        "input exceeds {} MiB limit",
                        MAX_INPUT_BYTES / (1024 * 1024)
                    )),
                )
            } else if !request.capabilities.replace_output {
                EntryOutcome::passthrough(request, None)
            } else {
                before_model(request, options, stash_store)
            }
        }
        // Unimplemented seams route to passthrough (roadmap §5.2).
        Seam::PreTool | Seam::Proxy => EntryOutcome::passthrough(request, None),
    }
}

fn post_tool(
    request: &CompressionRequest,
    options: &EntryOptions,
    stash_store: Option<&Arc<dyn StashStore>>,
) -> EntryOutcome {
    let thresholds = taxonomy::thresholds_for(request.content_origin, request.tool_name.as_deref());
    let run = PostToolPipeline::run(
        request,
        &PostToolPipelineConfig {
            timeout: RESPONSE_PIPELINE_TIMEOUT,
            max_input_bytes: MAX_INPUT_BYTES,
            min_input_chars: MIN_RESPONSE_CHARS,
            compression_enabled: options.compression_enabled,
            stash_enabled: options.stash_enabled,
            require_reversibility: false,
            force_json: false,
            preserve_top_level_shape: !request.capabilities.replace_with_text,
            allow_toon: true,
            min_toon_chars: MIN_TOON_CHARS,
            json: JsonCompressionConfig {
                truncate_strings_at: thresholds.truncate_strings_at,
                truncate_arrays_at: thresholds.truncate_arrays_at,
                max_depth: thresholds.max_depth,
                ..JsonCompressionConfig::default()
            },
        },
        stash_store,
    );
    let measured = matches!(
        run.response.disposition,
        Disposition::Applied | Disposition::DryRun
    );
    let op = if run.operations.contains(&JsonOperation::Toon) {
        OperationType::CompressToon
    } else {
        OperationType::CompressResponse
    };
    EntryOutcome {
        stats: EntryStats {
            op,
            measured_text: if measured {
                run.candidate.unwrap_or_else(|| request.content.clone())
            } else {
                request.content.clone()
            },
            unrecoverable_truncations: run.unrecoverable_truncations,
        },
        response: run.response,
        stash_writes: run.stash_writes,
        stash_errors: run.stash_errors,
        stash_size: run.stash_size,
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
    use tokenless_protocol::{ContentOrigin, PROTOCOL_VERSION};

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
        // keeps JSON protected: the detector scores content alone, so
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

        // Phase one connects only JSON to the Runtime-owned pipeline. The
        // same bytes as command output therefore stay unchanged too.
        let mut command = text_request(TRACKED_BUILD_LOG);
        command.content_origin = ContentOrigin::CommandOutput;
        let command = compress_with_store(&command, &ENABLED, Some(&store));
        assert_eq!(command.response.disposition, Disposition::Passthrough);
        assert_eq!(command.response.output, TRACKED_BUILD_LOG);
        assert_eq!(store.len(), 0);
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
        assert_eq!(command.response.disposition, Disposition::Passthrough);

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
        // Origin does not opt an unsupported content domain into phase one.
        let store: Arc<dyn StashStore> = Arc::new(InMemoryStore::new());
        let outcome = compress_with_store(&text_request(&prose_text()), &ENABLED, Some(&store));
        assert_eq!(outcome.response.disposition, Disposition::Passthrough);
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
    fn text_slot_build_log_temporarily_passes_through() {
        let store: Arc<dyn StashStore> = Arc::new(InMemoryStore::new());
        let req = text_request(&build_log_text());
        let outcome = compress_with_store(&req, &ENABLED, Some(&store));

        assert_eq!(outcome.response.disposition, Disposition::Passthrough);
        assert_eq!(outcome.response.output, build_log_text());
        assert!(outcome.response.compressor_chain.is_empty());
        assert_eq!(outcome.stats.op, OperationType::CompressResponse);
        assert!(outcome.response.stash_keys.is_empty());
        assert_eq!(outcome.stats.unrecoverable_truncations, None);
        assert_eq!(outcome.stash_writes, None);
        assert_eq!(store.len(), 0);
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
    fn text_dry_run_is_also_passthrough_until_the_domain_is_connected() {
        let req = text_request(&build_log_text());
        let outcome = compress_with_store(&req, &DRY_RUN, None);

        assert_eq!(outcome.response.disposition, Disposition::Passthrough);
        assert_eq!(outcome.response.output, build_log_text());
        assert!(outcome.response.compressor_chain.is_empty());
        assert_eq!(outcome.stats.measured_text, build_log_text());
        assert!(outcome.response.stash_keys.is_empty());
        assert_eq!(outcome.stash_writes, None);
        assert_eq!(outcome.stats.unrecoverable_truncations, None);
    }

    #[test]
    fn text_dry_run_does_not_invoke_terminal_cleanup() {
        // A parameterless SGR is three characters, and `heuristic-v1` counts
        // `chars.div_ceil(4)`: at a character count that is a multiple of
        // four, removing it leaves the count unchanged, so the pipeline
        // reverts the whole lossless stage and the build/log engine runs on
        // the uncleaned text. The measurement chain must then name only what
        // actually shaped the candidate.
        let mut content = build_log_text().replacen("$ cargo", "$ \u{1b}[mcargo", 1);
        while !content.chars().count().is_multiple_of(4) {
            content.push(' ');
        }
        let outcome = compress_with_store(&text_request(&content), &DRY_RUN, None);

        assert_eq!(outcome.response.disposition, Disposition::Passthrough);
        assert!(outcome.response.compressor_chain.is_empty());
        assert!(outcome.stats.measured_text.contains('\u{1b}'));
    }

    #[test]
    fn text_active_without_stash_preserves_terminal_bytes() {
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

        assert_eq!(outcome.response.disposition, Disposition::Passthrough);
        assert!(outcome.response.compressor_chain.is_empty());
        assert!(outcome.response.output.contains('\u{1b}'));
        assert!(!outcome.response.output.contains("<<tokenless:"));
        assert!(outcome.response.stash_keys.is_empty());
        // The lossy stage was excluded, so the emitted candidate holds no
        // unmarked omissions.
        assert_eq!(outcome.stats.unrecoverable_truncations, None);
    }

    #[test]
    fn long_non_log_text_temporarily_passes_through() {
        let store: Arc<dyn StashStore> = Arc::new(InMemoryStore::new());
        let prose: String = (0..120)
            .map(|i| format!("record {i} holding some ordinary content\n"))
            .collect();
        let outcome = compress_with_store(&text_request(&prose), &ENABLED, Some(&store));

        assert_eq!(outcome.response.disposition, Disposition::Passthrough);
        assert_eq!(outcome.response.output, prose);
        assert!(outcome.response.compressor_chain.is_empty());
        assert!(outcome.response.stash_keys.is_empty());
        assert_eq!(store.len(), 0);
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
    fn oversized_content_passes_through_with_a_diagnostic() {
        let content = format!(r#"{{"k":"{}"}}"#, "x".repeat(MAX_INPUT_BYTES));
        let outcome = compress_with_store(&post_tool_request(&content, "Bash"), &ENABLED, None);
        assert_eq!(outcome.response.disposition, Disposition::Passthrough);
        assert_eq!(outcome.response.content_type.as_deref(), Some("unknown"));
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
