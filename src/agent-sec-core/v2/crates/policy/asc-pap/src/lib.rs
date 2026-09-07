//! Transport-independent Policy Administration Point use cases.
//!
//! PAP owns current-record Policy, Scope, and Binding CRUD with monotonic revisions.
//! Policy authoring is lowered synchronously through [`PolicyCompiler`].
//! A Binding revision is a complete immutable snapshot, while only the current
//! revision and its lifecycle status are retained by the Repository.
//! Target-specific translation, Adapter dispatch, and retries are intentionally
//! outside this crate.
//!
//! TODO(policy-reconciliation): before a reconciliation worker is introduced,
//! extend the Binding persistence transaction to atomically record a durable
//! reconcile intent fenced by the revision embedded in the current Binding.
//! This PAP-only phase deliberately implements neither an outbox nor a worker,
//! so accepted requests remain in `PENDING_APPLY` or `PENDING_DELETE`.

#![forbid(unsafe_code)]

mod compiler;
mod error;
mod model;
mod repository;
mod service;

pub use compiler::PolicyCompiler;
pub use error::PapError;
pub use model::{Page, PolicyRevisionState, ScopeRevisionState};
pub use repository::PapRepository;
pub use service::PapService;
