//! Reversible compression stash for tokenless (Compress-Cache-Retrieve).
//!
//! When a compressor truncates content, the original payload is stashed under
//! a BLAKE3-derived key and a `<<tokenless:KEY>>` marker is inserted in the
//! compressed output. The LLM can quote the marker back to retrieve the
//! original, so compression stays reversible end-to-end even though the inline
//! representation is lossy.
//!
//! The store is injected into compressors as `Option<Arc<dyn StashStore>>`;
//! when absent, compressors take their original (lossy, non-retrievable) path.
//! This keeps the stash off the core compression path unless explicitly enabled.
//!
//! # Backends
//!
//! - [`InMemoryStore`]: in-process, no dependencies. Tests and single-process
//!   runs only — state is lost when the process exits, so it does not work
//!   across the fork+exec'd hook calls.
//! - [`SqliteStore`] (feature `sqlite`, on by default): persists to a file so
//!   state survives across processes. The recommended backend for the
//!   production hook path.

pub mod backends;
pub mod key;
pub mod marker;
pub mod store;

pub use backends::in_memory::InMemoryStore;
#[cfg(feature = "sqlite")]
pub use backends::sqlite::SqliteStore;
pub use key::compute_key;
pub use marker::{MARKER_PREFIX, MARKER_SUFFIX, extract_hash, marker_for, parse_marker};
pub use store::{StashError, StashStore};
