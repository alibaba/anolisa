//! `ContextProvider` trait, `PipDescriptor` and `AttributeBundle` (§7.1),
//! matching the six built-in providers of Table 7 (taint/session/trust/
//! ledger/evidence/detector), each with declared unavailability semantics.

use std::sync::Arc;
use std::time::Duration;

use asc_policy_types::attribute::{Attribute, AttributeRequirement};
use asc_policy_types::subject::SubjectRef;
use async_trait::async_trait;

/// Latency class of a provider (Table 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LatencyClass {
    /// In-process lookup.
    Local,
    /// Cross-process IPC (e.g. `taint_query`).
    Ipc,
    /// Model inference service (Tier C).
    Model,
}

/// How long fetched attributes stay valid for caching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreshnessSpec {
    /// Maximum attribute age before a refetch is required.
    pub max_age: Duration,
}

/// Declared behavior when the provider is unavailable (Table 7/8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PipFailMode {
    /// Report missing context; dependent predicates degrade per rule.
    MissingContext,
    /// Return a conservative default value and audit.
    DefaultValue,
    /// Hard failure; hard-domain decisions convert to deny.
    Fail,
}

/// Provider self-description used by the ContextBroker for routing, budget
/// accounting and failure handling.
#[derive(Debug, Clone)]
pub struct PipDescriptor {
    /// Owned attribute namespace: `taint` / `trust` / `ledger` / `evidence`
    /// / `detector` / `session`.
    pub namespace: Arc<str>,
    /// Latency class.
    pub latency_class: LatencyClass,
    /// Attribute cacheability.
    pub freshness: FreshnessSpec,
    /// Unavailability semantics.
    pub on_unavailable: PipFailMode,
}

/// Batched attribute request assembled from the hit rules' requirement set.
#[derive(Debug, Clone, PartialEq)]
pub struct AttributeQuery {
    /// Subject to resolve attributes for.
    pub subject: SubjectRef,
    /// Requested attributes with their assurance floors.
    pub requirements: Vec<AttributeRequirement>,
}

/// Fetch result: resolved attributes plus what could not be provided.
/// Missing entries flow into `Verdict::missing_context` (§7.3).
#[derive(Debug, Clone, PartialEq)]
pub struct AttributeBundle {
    /// Resolved attributes, each carrying its assurance level.
    pub attributes: Vec<Attribute>,
    /// Requirements this provider could not satisfy.
    pub missing: Vec<AttributeRequirement>,
}

/// PIP failures; mapped to decision behavior via the failure semantics
/// matrix (Table 8), never silently swallowed.
#[derive(Debug, thiserror::Error)]
pub enum PipError {
    /// Provider did not answer within its budget.
    #[error("provider `{namespace}` timed out")]
    Timeout {
        /// Namespace of the timed-out provider.
        namespace: String,
    },
    /// Provider is unreachable.
    #[error("provider `{namespace}` unavailable: {detail}")]
    Unavailable {
        /// Namespace of the unavailable provider.
        namespace: String,
        /// Failure detail.
        detail: String,
    },
    /// Internal provider error.
    #[error("provider internal error: {0}")]
    Internal(String),
}

/// Attribute source (§7.1). Implementations run their timeouts on the
/// runtime injected by asc-policyd and must attach assurance levels to every
/// attribute they return.
#[async_trait]
pub trait ContextProvider: Send + Sync {
    /// Provider self-description.
    fn descriptor(&self) -> PipDescriptor;

    /// Resolves the requested attributes for a subject.
    ///
    /// # Errors
    /// Returns [`PipError`] on timeout or unavailability; the ContextBroker
    /// converts it per the provider's declared [`PipFailMode`].
    async fn fetch(&self, query: AttributeQuery) -> Result<AttributeBundle, PipError>;
}
