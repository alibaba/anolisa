//! `DecisionPoint` entry trait (`decide`/`observe`) and the decision flow:
//! inverted index lookup, batched PIP attribute fetch, Tier B evaluation,
//! adjudicator signals, merge law and obligation assembly (§6.1).

use asc_policy_types::event::EventEnvelope;
use asc_policy_types::primitives::PolicyRevision;
use asc_policy_types::request::DecisionRequest;
use asc_policy_types::verdict::Verdict;
use async_trait::async_trait;

/// PDP failures. Never surfaces raw panics: entry-boundary panics convert to
/// Deny (hard domains) or audit downgrade (advisory domains) before this
/// error is even constructed (§6.5).
#[derive(Debug, thiserror::Error)]
pub enum PdpError {
    /// Predicate or automaton evaluation failed; the caller applies the
    /// domain default per Table 8.
    #[error("evaluation failed: {0}")]
    Evaluation(String),
    /// No active policy revision is loaded.
    #[error("no active policy revision")]
    NoActiveRevision,
    /// Per-session queue is shut down or overloaded beyond backpressure.
    #[error("session queue unavailable: {0}")]
    QueueUnavailable(String),
}

/// The single PDP entry point (§6.1). `decide` and `observe` share one
/// serial queue per session: same-session items process in arrival order and
/// `decide` acts as a barrier for previously enqueued events.
#[async_trait]
pub trait DecisionPoint: Send + Sync {
    /// Adjudicates a single request and returns the verdict.
    ///
    /// # Errors
    /// Returns [`PdpError`] when evaluation cannot complete; callers apply
    /// the intervention point's default fail semantics.
    async fn decide(&self, req: DecisionRequest) -> Result<Verdict, PdpError>;

    /// Advances state only (automata, stateful plugins) without producing a
    /// verdict. Used for the `OsEventAsync` observation stream.
    ///
    /// # Errors
    /// Returns [`PdpError::QueueUnavailable`] when the session queue cannot
    /// accept the event.
    async fn observe(&self, event: EventEnvelope) -> Result<(), PdpError>;

    /// Currently active policy revision.
    fn active_revision(&self) -> PolicyRevision;
}
