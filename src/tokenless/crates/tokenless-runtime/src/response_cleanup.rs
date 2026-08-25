//! Pipeline adapter for the existing JSON response cleanup.
//!
//! [`ResponseCleanup`] puts [`ResponseCompressor`] behind the pipeline's
//! [`Compressor`] trait (roadmap §5.3: move existing response cleanup behind
//! the new compressor interface), registered as
//! [`tokenless_pipeline::RESPONSE_CLEANUP`]. It lives here rather than in
//! the pipeline crate because the Runtime owns the stash store and the
//! per-call configuration the compressor is built from.

use std::cell::RefCell;

use tokenless_ccr::{StashStore, StashWrite};
use tokenless_pipeline::{
    CompressError, CompressOutcome, Compressor, CompressorSpec, RESPONSE_CLEANUP,
};
use tokenless_protocol::Reversibility;
use tokenless_schema::ResponseCompressor;

/// One-shot adapter: built per request from [`crate::CompressOptions`], with
/// the stash already attached (the [`ResponseCompressor`] API takes an
/// `Arc`-owned store at construction, not per call).
pub(crate) struct ResponseCleanup {
    inner: ResponseCompressor,
    /// Whether `inner` was built with a stash store. Without one, every
    /// truncation is unrecoverable no matter what the counters say.
    stash_attached: bool,
    /// The serialized candidate of the last `compress` call. The legacy
    /// measurement channel for [`crate::CompressResult::compressed_output`]
    /// (dry-run statistics record the predicted candidate); removed with the
    /// statistics migration (roadmap §5.5).
    candidate: RefCell<Option<String>>,
    /// The stash writes of the last `compress` call, retained so the entry
    /// router can roll them back when it rejects a pipeline-applied
    /// candidate after its own acceptance checks (the ledger inside
    /// [`tokenless_pipeline::run`] only rolls back the pipeline's own
    /// rejections).
    writes: RefCell<Vec<StashWrite>>,
}

impl ResponseCleanup {
    pub(crate) fn new(inner: ResponseCompressor, stash_attached: bool) -> Self {
        Self {
            inner,
            stash_attached,
            candidate: RefCell::new(None),
            writes: RefCell::new(Vec::new()),
        }
    }

    /// Failed stash operations of the last `compress` call.
    pub(crate) fn stash_errors(&self) -> usize {
        self.inner.stash_errors()
    }

    /// Successful stash writes of the last `compress` call: unique created
    /// rows plus every refresh of a pre-existing key, matching the metric
    /// documented for [`ResponseCompressor::stash_writes`].
    pub(crate) fn stash_writes(&self) -> usize {
        self.inner.stash_writes()
    }

    /// Truncations without a retrievable marker in the last `compress` call.
    pub(crate) fn unrecoverable_truncations(&self) -> usize {
        self.inner.unrecoverable_truncations()
    }

    /// Takes the retained candidate of the last `compress` call.
    pub(crate) fn take_candidate(&self) -> Option<String> {
        self.candidate.take()
    }

    /// Takes the retained stash writes of the last `compress` call.
    pub(crate) fn take_writes(&self) -> Vec<StashWrite> {
        self.writes.take()
    }
}

impl Compressor for ResponseCleanup {
    fn spec(&self) -> &CompressorSpec {
        &RESPONSE_CLEANUP
    }

    // `_stash` is deliberately unused: the store is attached at construction
    // (see the struct doc), and the caller passes that same store to
    // `tokenless_pipeline::run`, so the ledger's rollback targets exactly
    // the rows these writes created.
    fn compress(
        &self,
        content: &str,
        _stash: Option<&dyn StashStore>,
    ) -> Result<CompressOutcome, CompressError> {
        let value: serde_json::Value = serde_json::from_str(content)
            .map_err(|error| CompressError::Failed(format!("JSON parse error: {error}")))?;
        let compressed = self.inner.compress(&value);
        let output = serde_json::to_string(&compressed)
            .map_err(|error| CompressError::Failed(format!("serialize failed: {error}")))?;

        // Dropped fields, nulls, and empties keep the pre-pipeline judgment
        // that they are structural cleanup, not content loss: only actual
        // truncation degrades the claim. Truncation is retrievable exactly
        // when a store is attached and every truncated payload landed in it.
        let reversibility = if self.inner.truncations() == 0 {
            Reversibility::Lossless
        } else if !self.stash_attached || self.inner.unrecoverable_truncations() > 0 {
            Reversibility::Unrecoverable
        } else {
            Reversibility::Retrievable
        };

        self.candidate.replace(Some(output.clone()));
        let stash_writes = self.inner.take_stash_writes();
        self.writes.replace(stash_writes.clone());
        Ok(CompressOutcome {
            output,
            reversibility,
            stash_writes,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokenless_ccr::InMemoryStore;

    use super::*;

    fn compress(adapter: &ResponseCleanup, content: &str) -> CompressOutcome {
        match adapter.compress(content, None) {
            Ok(outcome) => outcome,
            Err(error) => panic!("adapter failed: {error}"),
        }
    }

    #[test]
    fn cleanup_without_truncation_claims_lossless() {
        let adapter = ResponseCleanup::new(ResponseCompressor::new(), false);
        let outcome = compress(&adapter, r#"{"value":1,"noise":null,"debug":"x"}"#);
        assert_eq!(outcome.output, r#"{"value":1}"#);
        assert_eq!(outcome.reversibility, Reversibility::Lossless);
        assert!(outcome.stash_writes.is_empty());
        assert_eq!(adapter.take_candidate().as_deref(), Some(r#"{"value":1}"#));
    }

    #[test]
    fn truncation_without_a_store_claims_unrecoverable() {
        let compressor = ResponseCompressor::new().with_truncate_strings_at(20);
        let adapter = ResponseCleanup::new(compressor, false);
        let input = format!(r#"{{"tail":"{}"}}"#, "x".repeat(200));
        let outcome = compress(&adapter, &input);
        assert_eq!(outcome.reversibility, Reversibility::Unrecoverable);
        assert!(outcome.stash_writes.is_empty());
    }

    #[test]
    fn stashed_truncation_claims_retrievable_and_reports_its_writes() {
        let store = Arc::new(InMemoryStore::new());
        let compressor = ResponseCompressor::new()
            .with_truncate_strings_at(80)
            .with_stash_store(store.clone());
        let adapter = ResponseCleanup::new(compressor, true);
        let input = format!(r#"{{"tail":"{}"}}"#, "x".repeat(400));
        let outcome = compress(&adapter, &input);
        assert_eq!(outcome.reversibility, Reversibility::Retrievable);
        assert_eq!(outcome.stash_writes.len(), 1);
        assert!(outcome.stash_writes[0].created);
        assert!(
            outcome
                .output
                .contains(&tokenless_ccr::marker_for(&outcome.stash_writes[0].key))
        );
    }

    #[test]
    fn non_json_content_is_a_compressor_error() {
        let adapter = ResponseCleanup::new(ResponseCompressor::new(), false);
        assert!(adapter.compress("not json", None).is_err());
    }
}
