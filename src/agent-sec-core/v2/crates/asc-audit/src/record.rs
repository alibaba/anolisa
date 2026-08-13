//! `DecisionRecord` schema (§9.5): OPA EventV1-aligned with agent-context
//! extensions (attribute sources, per-tier timings, receipt linkage).

use asc_policy_types::event::EventTrust;
use asc_policy_types::primitives::{DecisionId, Digest, PolicyRevision, RuleId, Timestamp};
use asc_policy_types::request::InterventionPoint;
use asc_policy_types::subject::{SessionId, SubjectRef};
use asc_policy_types::verdict::{Decision, ObligationKind, Reason};
use serde::{Deserialize, Serialize};

/// Reference linking a decision record to its enforcement receipt, which is
/// written to a separate file and joined on decision_id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReceiptRef(pub String);

/// Compact verdict summary stored in the record; the full verdict is
/// reconstructible from decision + obligations kinds + reasons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerdictSummary {
    /// Binary decision.
    pub decision: Decision,
    /// Kinds of obligations attached.
    pub obligation_kinds: Vec<ObligationKind>,
    /// Explanation entries.
    pub reasons: Vec<Reason>,
}

/// Where each attribute came from: provider, fetch time and assurance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipSourceMeta {
    /// Attribute path.
    pub path: String,
    /// Provider namespace that supplied it.
    pub provider: String,
    /// When it was fetched.
    pub fetched_at: Timestamp,
    /// Assurance level it carried.
    pub assurance: EventTrust,
}

/// Per-stage decision latencies in microseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionTimings {
    /// Tier A interaction time.
    pub tier_a_us: u64,
    /// Tier B evaluation time.
    pub tier_b_us: u64,
    /// Tier C detector wait time.
    pub tier_c_us: u64,
    /// PIP attribute fetch time.
    pub pip_us: u64,
}

/// One adjudication audit record (§9.5). Full action inputs are stored
/// elsewhere; the record keeps only the digest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRecord {
    /// Record schema version; currently 1.
    pub schema_version: u16,
    /// Decision identity.
    pub decision_id: DecisionId,
    /// Record time.
    pub timestamp: Timestamp,
    /// Revision the decision was computed under.
    pub policy_revision: PolicyRevision,
    /// Session adjudicated.
    pub session_id: SessionId,
    /// Interception point.
    pub intervention_point: InterventionPoint,
    /// Decision subject.
    pub subject: SubjectRef,
    /// Digest of the full action input.
    pub action_digest: Digest,
    /// Verdict summary.
    pub verdict_summary: VerdictSummary,
    /// Rules that matched.
    pub matched_rules: Vec<RuleId>,
    /// Provenance of every attribute consulted.
    pub attribute_sources: Vec<PipSourceMeta>,
    /// Per-stage latencies.
    pub timings: DecisionTimings,
    /// Link to the enforcement receipt, forming the decision → enforcement
    /// evidence chain.
    pub enforcement_receipt: Option<ReceiptRef>,
    /// Hash of the previous record, forming the tamper-evident chain.
    pub prev_record_hash: [u8; 32],
}
