//! `StatefulAdjudicator` plugin contract and framework-managed `SessionSlot`
//! state (§6.1-§6.2). Plugins hold no interior mutable state; per-session
//! slots are sharded by (adjudicator_id, session_id) with single-writer
//! serialization, and state versions feed the decision cache key.

use std::any::Any;

use asc_policy_types::attribute::Attribute;
use asc_policy_types::event::EventEnvelope;
use asc_policy_types::request::DecisionRequest;
use asc_policy_types::verdict::{Decision, Obligation, Reason, TrustDelta};

/// Stateful adjudicator plugin identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AdjudicatorId(pub String);

/// Framework-managed per-session plugin state. The framework increments
/// the version after every `observe`/`adjudicate` call and folds it into
/// the decision cache key — plugins neither see nor can forget this.
#[derive(Default)]
pub struct SessionSlot {
    /// Maintained exclusively by the framework; private so plugins holding
    /// `&mut SessionSlot` cannot tamper with cache-key inputs.
    state_version: u64,
    /// Plugin-owned state. Must stay bounded: state size decoupled from
    /// event rate (§6.1).
    pub state: Option<Box<dyn Any + Send>>,
}

impl SessionSlot {
    /// Creates an empty slot at version 0.
    pub fn new() -> Self {
        Self::default()
    }

    /// Read-only view of the framework-maintained version counter.
    pub fn state_version(&self) -> u64 {
        self.state_version
    }

    /// Advances the version after a plugin call; feeds the decision cache
    /// key (§6.4).
    // Crate-private so plugins can never bump versions themselves; the
    // session-queue runtime consuming this lands with P0.
    #[allow(dead_code)]
    pub(crate) fn bump_version(&mut self) {
        self.state_version = self.state_version.wrapping_add(1);
    }
}

/// Read-only view of PIP-fetched context handed to plugins; plugins never
/// perform their own IO (§6.2).
#[derive(Debug, Clone, Copy)]
pub struct ContextView<'a> {
    /// Attributes fetched for the current request/event.
    pub attributes: &'a [Attribute],
}

/// Intent assessment produced by the Intent Firewall (§6.1).
#[derive(Debug, Clone, PartialEq)]
pub struct IntentAssessment {
    /// Coarse intent class label.
    pub class: String,
    /// Session drift score.
    pub drift_score: f32,
    /// Whether declared intent matches observed behavior, when checkable.
    pub declared_vs_observed: Option<bool>,
}

/// Signal a stateful plugin feeds into the merger; never a verdict by
/// itself (§6.1).
#[derive(Debug, Clone, PartialEq)]
pub struct AdjudicationSignal {
    /// Proposed decision and obligations, when the plugin has an opinion.
    pub proposal: Option<(Decision, Vec<Obligation>)>,
    /// Confidence in the proposal.
    pub confidence: f32,
    /// Trust score change; written back by the framework with monotonicity
    /// enforcement (in-session decrease only), never by the plugin (§6.2).
    pub trust_delta: Option<TrustDelta>,
    /// Intent assessment, when computed.
    pub intent: Option<IntentAssessment>,
    /// Explanation entries contributed to the verdict.
    pub reasons: Vec<Reason>,
}

/// Stateful adjudicator plugin (§6.1). Intent Firewall is the first
/// implementation; later on-device-model session adjudicators mount via the
/// same trait. Plugins take `&self`: session state arrives as an exclusive
/// `&mut SessionSlot` borrow, single-writer per session, no plugin locking.
pub trait StatefulAdjudicator: Send + Sync {
    /// Stable plugin identifier.
    fn id(&self) -> AdjudicatorId;

    /// Observes an event and updates session state (intent tracking, drift
    /// windows). Must keep state bounded regardless of event rate.
    fn observe(&self, event: &EventEnvelope, state: &mut SessionSlot, ctx: &ContextView<'_>);

    /// Produces a signal for the current request; the signal enters the
    /// merger and never becomes a verdict directly.
    fn adjudicate(
        &self,
        req: &DecisionRequest,
        state: &mut SessionSlot,
        ctx: &ContextView<'_>,
    ) -> AdjudicationSignal;
}
