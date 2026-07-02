//! Stash store trait and error types.
//!
//! The trait is deliberately dependency-free so compressors can hold an
//! `Option<Arc<dyn StashStore>>` without pulling in any backend crate.

/// A reversible stash of compressed-out payloads.
///
/// `stash` stores a payload and returns its BLAKE3-derived key; the caller
/// injects a `<<tokenless:KEY>>` marker into the compressed output so the LLM
/// can request the original back via `retrieve`. Keeping `stash` responsible
/// for key derivation (rather than accepting a caller-supplied hash) removes a
/// injection footgun: callers cannot mismatch a marker from its payload.
pub trait StashStore: Send + Sync {
    /// Stash `payload`, returning its key. Re-stashing the same payload is
    /// idempotent (same key) and refreshes the entry's expiry.
    fn stash(&self, payload: &str) -> Result<String, StashError>;

    /// Retrieve a stashed payload by key. Returns `Ok(None)` if the key is
    /// absent or the entry has expired.
    fn retrieve(&self, hash: &str) -> Result<Option<String>, StashError>;

    /// Number of live (non-expired) entries. For observability/stats only.
    fn len(&self) -> usize;

    /// Whether the store holds no live entries.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drop all expired entries and return how many were removed.
    fn evict_expired(&self) -> Result<usize, StashError>;
}

/// Errors a stash backend can surface. Kept minimal: backends map their
/// concrete error types into `Backend` with a human-readable message so the
/// trait stays free of backend-specific dependencies.
#[derive(Debug, thiserror::Error)]
pub enum StashError {
    /// A backend-specific failure (DB error, IO error, etc.).
    #[error("stash backend error: {0}")]
    Backend(String),
}

#[cfg(feature = "sqlite")]
impl From<rusqlite::Error> for StashError {
    fn from(e: rusqlite::Error) -> Self {
        StashError::Backend(e.to_string())
    }
}
