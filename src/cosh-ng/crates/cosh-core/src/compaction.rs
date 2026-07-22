//! Model-aware session context compaction.
//!
//! Compaction reduces the provider-visible context of a long session while
//! keeping the persisted transcript complete and the session identity
//! unchanged. The transcript in [`crate::session::PersistedSession::messages`]
//! stays append-only; compaction only maintains an optional projection
//! ([`CompactionState`]) that tells the runtime to replace an already
//! summarized transcript prefix with a bounded structured snapshot.

mod boundary;
mod budget;
mod engine;
mod projection;
mod summarize;

pub use budget::{ContextBudget, ModelCapability};
pub use projection::CompactionState;

pub(crate) use budget::{estimate_messages_tokens, estimate_text_tokens};
pub(crate) use engine::{compact_in_memory, run_compact_cli};
pub(crate) use projection::{effective_messages, sanitize_loaded_state};
