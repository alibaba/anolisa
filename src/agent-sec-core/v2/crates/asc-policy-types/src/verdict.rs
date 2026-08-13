//! Verdict model (§3.4): binary decidable core (`Decision`), obligation
//! channel (`Obligation`), RFC 6902 patch, and the deterministic five-value
//! projection ALLOW/DENY/MODIFY/STEP_UP/DEFER (Table 5). Merge law ordering
//! lives in asc-policy-ir; this module only carries the output contract.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::attribute::AttributeRequirement;
use crate::event::EventTrust;
use crate::primitives::{DecisionId, PolicyRevision, RuleId, Timestamp};
use crate::subject::SkillRef;
use crate::token::ResumeToken;

/// Decidable decision kernel: strictly binary (decision D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Action may proceed (possibly after patch application).
    Allow,
    /// Action is rejected; StepUp/Defer obligations may open a resume path.
    Deny,
}

/// Audit severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Informational.
    Low,
    /// Noteworthy.
    Medium,
    /// Significant risk.
    High,
    /// Immediate operator attention.
    Critical,
}

/// Named audit sink.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuditSink(pub String);

/// Approver reference for StepUp obligations.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApproverRef(pub String);

/// Deferred-work queue reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QueueRef(pub String);

/// Alert delivery channel.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AlertChannel(pub String);

/// Behavior on StepUp/Defer timeout. Escalate never relaxes semantics: the
/// deny-biased baseline holds, only the handling priority changes (§8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailAction {
    /// Final rejection; the resume token is voided.
    Deny,
    /// Keep denying, add Quarantine + operator alert, requeue at higher
    /// priority for manual handling.
    Escalate,
}

/// Enforcement action of a kernel rule. `Notify` until interface C confirms
/// deny capability and receipt SLA (Table 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessAction {
    /// Report matches without blocking.
    Notify,
    /// Block matching actions.
    Deny,
}

/// Interface C rule spec: the subject/match/action(/ttl) tuple handed to the
/// OS Harness; TTL is carried by the enclosing KernelRule obligation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessRuleSpec {
    /// Rule subject, e.g. `session:<id>`.
    pub subject: String,
    /// Bounded match expression (pre-compiled globs and prefix matches only;
    /// no regex enters the kernel plan, §5.3).
    #[serde(rename = "match")]
    pub match_spec: serde_json::Value,
    /// Enforcement action.
    pub action: HarnessAction,
}

/// Scope of a Quarantine obligation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuarantineScope {
    /// Downgrade a skill's ledger trust state.
    SkillDegrade {
        /// Skill to downgrade.
        skill: SkillRef,
    },
    /// Freeze the session process tree (cgroup v2 freezer, recoverable).
    SessionFreeze,
}

/// Obligation channel (§3.4). Prerequisite obligations (tightening
/// KernelRule, Quarantine) must complete before the verdict is returned and
/// fail as Deny; post-hoc obligations (Audit, EmitAlert, ScoreDelta,
/// Feedback) run async and never retroactively change the decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Obligation {
    /// Record an audit entry.
    Audit {
        /// Audit severity.
        severity: Severity,
        /// Target sink.
        sink: AuditSink,
    },
    /// Require human approval; deny-biased until approved (§8.3).
    StepUp {
        /// Who may approve.
        approver: ApproverRef,
        /// Prompt shown on the approval channel.
        prompt: String,
        /// Resumption credential.
        resume_token: ResumeToken,
        /// Approval window.
        timeout: Duration,
        /// Behavior when the window elapses.
        on_timeout: FailAction,
    },
    /// Queue for later processing; deny-biased while queued.
    Defer {
        /// Target queue.
        queue: QueueRef,
        /// Processing deadline.
        deadline: Timestamp,
        /// Behavior when the deadline elapses.
        on_deadline: FailAction,
    },
    /// Emit an operator alert.
    EmitAlert {
        /// Delivery channel.
        channel: AlertChannel,
        /// Message template.
        template: String,
    },
    /// Lower the session trust score. Decrease-only: in-session score is
    /// monotonically non-increasing; recovery is a cross-session calibration
    /// flow, never an obligation.
    ScoreDelta {
        /// Amount to decrease.
        decrease: u32,
        /// Machine-readable reason.
        reason_code: String,
    },
    /// Push a tightening rule to the OS Harness via interface C.
    KernelRule {
        /// Rule to push.
        rule: HarnessRuleSpec,
        /// Rule lifetime.
        ttl: Duration,
    },
    /// Quarantine: skill downgrade or session freeze request.
    Quarantine {
        /// Quarantine scope.
        scope: QuarantineScope,
    },
    /// Response-ladder first rung: feed guidance back to the agent.
    Feedback {
        /// Message injected into the agent context.
        to_agent: String,
    },
}

/// Discriminant of [`Obligation`], used by adapter descriptors and audit
/// summaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationKind {
    /// [`Obligation::Audit`].
    Audit,
    /// [`Obligation::StepUp`].
    StepUp,
    /// [`Obligation::Defer`].
    Defer,
    /// [`Obligation::EmitAlert`].
    EmitAlert,
    /// [`Obligation::ScoreDelta`].
    ScoreDelta,
    /// [`Obligation::KernelRule`].
    KernelRule,
    /// [`Obligation::Quarantine`].
    Quarantine,
    /// [`Obligation::Feedback`].
    Feedback,
}

impl Obligation {
    /// Returns the discriminant of this obligation.
    pub fn kind(&self) -> ObligationKind {
        match self {
            Obligation::Audit { .. } => ObligationKind::Audit,
            Obligation::StepUp { .. } => ObligationKind::StepUp,
            Obligation::Defer { .. } => ObligationKind::Defer,
            Obligation::EmitAlert { .. } => ObligationKind::EmitAlert,
            Obligation::ScoreDelta { .. } => ObligationKind::ScoreDelta,
            Obligation::KernelRule { .. } => ObligationKind::KernelRule,
            Obligation::Quarantine { .. } => ObligationKind::Quarantine,
            Obligation::Feedback { .. } => ObligationKind::Feedback,
        }
    }
}

/// Explanation entry for a verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reason {
    /// Machine-readable reason code.
    pub code: String,
    /// Rule that produced this reason.
    pub rule_id: RuleId,
    /// References to supporting evidence records.
    pub evidence_refs: Vec<String>,
    /// Human-readable explanation.
    pub message: String,
}

/// Trust score change output by a stateful adjudicator. Non-positive
/// semantics: in-session scores only decrease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustDelta {
    /// Amount to decrease.
    pub decrease: u32,
    /// Machine-readable reason.
    pub reason_code: String,
}

/// Five-value projection of a verdict (Table 5), for the external API and
/// AARM R4 alignment. A pure function of the verdict; introduces no state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VerdictProjection {
    /// Allow, possibly with audit/feedback obligations.
    Allow,
    /// Final rejection.
    Deny,
    /// Allow the patched action; the patch must pass target-schema checks.
    Modify,
    /// Deny-biased pending approval; resume via token.
    StepUp,
    /// Deny-biased while queued; deadline applies `on_deadline`.
    Defer,
}

/// Adjudication output (§3.4): binary kernel plus obligation channel and
/// optional RFC 6902 patch. Patches may only tighten or rewrite parameters,
/// never widen resource scope (compile-time whitelist, runtime PEP re-check).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    /// Verdict schema version; currently 1.
    pub schema_version: u16,
    /// Decision this verdict answers.
    pub decision_id: DecisionId,
    /// Binary decidable kernel.
    pub decision: Decision,
    /// RFC 6902 patch; meaningful only with `Decision::Allow` (=> MODIFY).
    pub patch: Option<json_patch::Patch>,
    /// Obligation channel.
    pub obligations: Vec<Obligation>,
    /// Explanation entries.
    pub reasons: Vec<Reason>,
    /// Context that would complete the adjudication; trigger data for
    /// STEP_UP/DEFER (§7.3).
    pub missing_context: Vec<AttributeRequirement>,
    /// Confidence from stateful adjudicator plugins, when involved.
    pub confidence: Option<f32>,
    /// Trust score change from the Intent Firewall (decrease-only).
    pub trust_delta: Option<TrustDelta>,
    /// Lowest evidence assurance this verdict relied on.
    pub provenance_floor: EventTrust,
    /// Revision the verdict was computed under.
    pub policy_revision: PolicyRevision,
    /// Result validity; expired verdicts require re-adjudication.
    pub valid_until: Option<Timestamp>,
}

impl Verdict {
    /// Projects the verdict onto the five-value external semantics per
    /// Table 5. StepUp takes precedence over Defer, mirroring the merge law.
    pub fn project(&self) -> VerdictProjection {
        match self.decision {
            Decision::Deny => {
                let mut has_defer = false;
                for obligation in &self.obligations {
                    match obligation.kind() {
                        ObligationKind::StepUp => return VerdictProjection::StepUp,
                        ObligationKind::Defer => has_defer = true,
                        _ => {}
                    }
                }
                if has_defer {
                    VerdictProjection::Defer
                } else {
                    VerdictProjection::Deny
                }
            }
            Decision::Allow => {
                if self.patch.is_some() {
                    VerdictProjection::Modify
                } else {
                    VerdictProjection::Allow
                }
            }
        }
    }
}
