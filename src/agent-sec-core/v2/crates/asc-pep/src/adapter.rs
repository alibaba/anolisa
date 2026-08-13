//! `EnforcementAdapter` trait, `EnforcementReceipt` (§8.1) and descriptor
//! types for the built-in adapters of Table 9. Each adapter declares its
//! supported intervention points, obligation kinds and idempotency.

use asc_policy_types::primitives::{DecisionId, Digest, Timestamp};
use asc_policy_types::request::InterventionPoint;
use asc_policy_types::subject::{ActionKind, ResourceRef};
use asc_policy_types::verdict::{ObligationKind, Verdict};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Enforcement adapter identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PepId(pub String);

/// Identifier of an assembled obligation instance within a verdict.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObligationId(pub String);

/// The concrete action a verdict is enforced against; its digest is compared
/// with the receipt to detect confused-PEP execution (T5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionRef {
    /// Action family.
    pub kind: ActionKind,
    /// Target resource.
    pub resource: ResourceRef,
    /// Digest of the action arguments, when applicable.
    pub args_digest: Option<Digest>,
}

/// Adapter self-description used by the obligation dispatcher for routing.
#[derive(Debug, Clone)]
pub struct PepDescriptor {
    /// Adapter identifier.
    pub adapter: PepId,
    /// Intervention points the adapter covers.
    pub intervention_points: Vec<InterventionPoint>,
    /// Obligation kinds the adapter can fulfil.
    pub obligation_kinds: Vec<ObligationKind>,
    /// Whether `enforce` may be retried safely.
    pub idempotent: bool,
}

/// Enforcement outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnforceOutcome {
    /// All obligations applied.
    Applied,
    /// Some obligations applied; failures listed by omission from
    /// `applied_obligations`.
    PartiallyApplied,
    /// Nothing applied.
    Failed,
}

/// Interface C acknowledgment passed through in the receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessAck {
    /// Whether the OS Harness accepted the rule.
    pub accepted: bool,
    /// Harness-side detail, when provided.
    pub detail: Option<String>,
}

/// Enforcement receipt (§8.1): one link of the evidence chain from
/// decision_id to who adjudicated, which plane enforced and whether L0
/// confirmed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnforcementReceipt {
    /// Decision this receipt belongs to.
    pub decision_id: DecisionId,
    /// Adapter that enforced.
    pub adapter: PepId,
    /// Enforcement outcome.
    pub outcome: EnforceOutcome,
    /// Obligations actually applied.
    pub applied_obligations: Vec<ObligationId>,
    /// Interface C acknowledgment, when a kernel rule was pushed.
    pub kernel_ack: Option<HarnessAck>,
    /// Enforcement time.
    pub ts: Timestamp,
}

/// PEP failures. Prerequisite-obligation failures are treated as Deny by
/// the caller (deny-biased PEP, Table 8).
#[derive(Debug, thiserror::Error)]
pub enum PepError {
    /// The verdict references obligations this adapter does not support.
    #[error("unsupported obligation kind for adapter `{adapter}`")]
    Unsupported {
        /// Adapter that rejected the obligation.
        adapter: String,
    },
    /// The target enforcement plane rejected or failed the action.
    #[error("enforcement failed: {0}")]
    EnforcementFailed(String),
    /// Receipt could not be produced; treated as failure (receipts are
    /// mandatory evidence).
    #[error("receipt unavailable: {0}")]
    ReceiptUnavailable(String),
}

/// Enforcement adapter (§8.1). Adapters run least-privileged and must
/// return a receipt for every enforcement attempt.
#[async_trait]
pub trait EnforcementAdapter: Send + Sync {
    /// Adapter self-description.
    fn descriptor(&self) -> PepDescriptor;

    /// Enforces a verdict against an action and returns the receipt.
    ///
    /// # Errors
    /// Returns [`PepError`]; failures on prerequisite obligations are
    /// treated as Deny by the dispatcher.
    async fn enforce(
        &self,
        verdict: &Verdict,
        action: &ActionRef,
    ) -> Result<EnforcementReceipt, PepError>;
}
