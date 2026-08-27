//! Pipeline adapters for the terminal-cleanup and build/log engines.
//!
//! The engines live in `tokenless-compressors` (roadmap §6.1); these
//! adapters put them behind the pipeline's [`Compressor`] trait, mirroring
//! [`crate::response_cleanup::ResponseCleanup`]. They live here because the
//! Runtime owns the stash store and assembles the candidate pool per
//! request.

use std::cell::{Cell, RefCell};

use tokenless_ccr::{StashStore, StashWrite};
use tokenless_compressors::{BuildLogMode, clean_terminal, compress_log};
use tokenless_pipeline::{
    BUILD_LOG, CompressError, CompressOutcome, Compressor, CompressorSpec, ContentType,
    TERMINAL_CLEANUP, detect,
};
use tokenless_protocol::Reversibility;

/// Lossless level 1: ANSI colour and style codes, the only provably
/// non-semantic part of a terminal capture. No stash interaction at all.
#[derive(Default)]
pub(crate) struct TerminalCleanupAdapter {
    /// Output of the last `compress` call, for the entry's measurement
    /// channel when the lossy stage never runs.
    candidate: RefCell<Option<String>>,
    /// Whether the last `compress` call changed its input — the dry-run
    /// chain is rebuilt from these flags after the pipeline's rejection
    /// path cleared `compressor_chain`.
    changed: Cell<bool>,
}
// Both fields describe a contribution to the pipeline's candidate, so
// `discarded` clears them: a rolled-back cleanup shaped nothing.

impl TerminalCleanupAdapter {
    pub(crate) fn take_candidate(&self) -> Option<String> {
        self.candidate.take()
    }

    pub(crate) fn changed(&self) -> bool {
        self.changed.get()
    }
}

impl Compressor for TerminalCleanupAdapter {
    fn spec(&self) -> &CompressorSpec {
        &TERMINAL_CLEANUP
    }

    fn compress(
        &self,
        content: &str,
        _stash: Option<&dyn StashStore>,
    ) -> Result<CompressOutcome, CompressError> {
        let output = clean_terminal(content);
        self.changed.set(output != content);
        self.candidate.replace(Some(output.clone()));
        Ok(CompressOutcome {
            output,
            reversibility: Reversibility::Lossless,
            stash_writes: Vec::new(),
        })
    }

    fn discarded(&self) {
        self.changed.set(false);
        self.candidate.take();
    }
}

/// Retrievable-lossy level 2: the build/log engine. Writes go through the
/// pipeline-passed store, so the ledger's rollback targets exactly the rows
/// these writes created.
#[derive(Default)]
pub(crate) struct BuildLogAdapter {
    candidate: RefCell<Option<String>>,
    /// Stash writes of the last `compress` call, retained so the entry
    /// router can roll them back when its own acceptance checks reject a
    /// pipeline-applied candidate.
    writes: RefCell<Vec<StashWrite>>,
    changed: Cell<bool>,
    omitted_blocks: Cell<usize>,
    stash_write_count: Cell<usize>,
    stash_errors: Cell<usize>,
}

impl BuildLogAdapter {
    pub(crate) fn take_candidate(&self) -> Option<String> {
        self.candidate.take()
    }

    pub(crate) fn take_writes(&self) -> Vec<StashWrite> {
        self.writes.take()
    }

    pub(crate) fn changed(&self) -> bool {
        self.changed.get()
    }

    /// Gaps replaced by markers in the last `compress` call.
    pub(crate) fn omitted_blocks(&self) -> usize {
        self.omitted_blocks.get()
    }

    /// Successful stash writes of the last `compress` call.
    pub(crate) fn stash_writes(&self) -> usize {
        self.stash_write_count.get()
    }

    /// Failed stash attempts of the last `compress` call; their gaps stayed
    /// verbatim (fail-open per gap).
    pub(crate) fn stash_errors(&self) -> usize {
        self.stash_errors.get()
    }
}

impl Compressor for BuildLogAdapter {
    fn spec(&self) -> &CompressorSpec {
        &BUILD_LOG
    }

    fn compress(
        &self,
        content: &str,
        stash: Option<&dyn StashStore>,
    ) -> Result<CompressOutcome, CompressError> {
        // Deterministic re-detection on the (possibly cleaned) candidate:
        // real build logs get the classifier, other long plain text gets the
        // conservative generic head/tail mode. A log whose only detector
        // evidence was ANSI/CR degrades to generic after cleanup — still
        // compressed, just without the classifier.
        let mode = if detect(content) == ContentType::BuildLog {
            BuildLogMode::BuildLog
        } else {
            BuildLogMode::GenericText
        };
        let outcome = compress_log(content, mode, stash);
        // Omissions are per-gap fail-open: a failed stash keeps its gap
        // verbatim, so every emitted marker is backed. Without a store the
        // markers are measurement-only (dry-run) and the claim degrades.
        let reversibility = if outcome.omitted_blocks == 0 {
            // At most duplicate-run collapse happened; the repeat
            // annotation reconstructs it exactly.
            Reversibility::Lossless
        } else if outcome.retrievable {
            Reversibility::Retrievable
        } else {
            Reversibility::Unrecoverable
        };
        self.changed
            .set(outcome.output != content || !outcome.stash_writes.is_empty());
        self.omitted_blocks.set(outcome.omitted_blocks);
        self.stash_write_count.set(outcome.stash_writes.len());
        self.stash_errors.set(outcome.stash_errors);
        self.candidate.replace(Some(outcome.output.clone()));
        self.writes.replace(outcome.stash_writes.clone());
        Ok(CompressOutcome {
            output: outcome.output,
            reversibility,
            stash_writes: outcome.stash_writes,
        })
    }
}

#[cfg(test)]
mod tests {
    use tokenless_ccr::InMemoryStore;

    use super::*;

    fn cargo_like_log() -> String {
        let mut lines: Vec<String> = (0..3).map(|i| format!("$ cargo build step {i}")).collect();
        lines.extend((0..80).map(|i| format!("   Compiling pkg{i:03} v0.1.{i}")));
        lines.push("error[E0308]: mismatched types".to_string());
        lines.extend((0..12).map(|i| format!("tail summary line {i}")));
        lines.join("\n") + "\n"
    }

    #[test]
    fn terminal_cleanup_claims_lossless_and_tracks_change() {
        let adapter = TerminalCleanupAdapter::default();
        let outcome = adapter
            .compress("\u{1b}[1mBuild\u{1b}[0m ok\n", None)
            .unwrap();
        assert_eq!(outcome.output, "Build ok\n");
        assert_eq!(outcome.reversibility, Reversibility::Lossless);
        assert!(outcome.stash_writes.is_empty());
        assert!(adapter.changed());
        assert_eq!(adapter.take_candidate().as_deref(), Some("Build ok\n"));

        let outcome = adapter.compress("plain\n", None).unwrap();
        assert_eq!(outcome.output, "plain\n");
        assert!(!adapter.changed());
    }

    #[test]
    fn build_log_with_store_claims_retrievable_and_reports_writes() {
        let store = InMemoryStore::new();
        let adapter = BuildLogAdapter::default();
        let outcome = adapter.compress(&cargo_like_log(), Some(&store)).unwrap();
        assert_eq!(outcome.reversibility, Reversibility::Retrievable);
        assert!(!outcome.stash_writes.is_empty());
        assert!(outcome.output.contains("error[E0308]: mismatched types"));
        for write in &outcome.stash_writes {
            assert!(
                outcome
                    .output
                    .contains(&tokenless_ccr::marker_for(&write.key))
            );
        }
        assert_eq!(adapter.stash_writes(), outcome.stash_writes.len());
        assert_eq!(adapter.omitted_blocks(), outcome.stash_writes.len());
        assert!(adapter.changed());
    }

    #[test]
    fn build_log_without_store_degrades_to_unrecoverable_measurement() {
        let adapter = BuildLogAdapter::default();
        let outcome = adapter.compress(&cargo_like_log(), None).unwrap();
        assert_eq!(outcome.reversibility, Reversibility::Unrecoverable);
        assert!(outcome.stash_writes.is_empty());
        assert!(outcome.output.contains("<<tokenless:"));
    }

    #[test]
    fn short_input_is_a_lossless_no_op() {
        let adapter = BuildLogAdapter::default();
        let input = "error: one\nline two\n";
        let outcome = adapter.compress(input, None).unwrap();
        assert_eq!(outcome.output, input);
        assert_eq!(outcome.reversibility, Reversibility::Lossless);
        assert!(!adapter.changed());
        assert_eq!(adapter.omitted_blocks(), 0);
    }

    #[test]
    fn non_build_log_text_routes_to_generic_mode() {
        let store = InMemoryStore::new();
        let adapter = BuildLogAdapter::default();
        let prose: String = (0..120)
            .map(|i| format!("record {i} holding some ordinary content\n"))
            .collect();
        let outcome = adapter.compress(&prose, Some(&store)).unwrap();
        assert_eq!(outcome.reversibility, Reversibility::Retrievable);
        assert!(
            outcome
                .output
                .contains("… (omitted 40 lines, run: tokenless retrieve")
        );
        assert_eq!(adapter.omitted_blocks(), 1);
    }
}
