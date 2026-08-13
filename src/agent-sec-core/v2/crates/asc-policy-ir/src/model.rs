//! IR data structures (§5.2): `PolicyIr`, `RuleIr`, `PredicateIr`,
//! `TraceAutomatonIr`, `CapabilityIr` and the rule inverted index. New
//! variants are append-only; the PDP rejects rules with unknown variants at
//! load time instead of skipping them (§5.4).

use std::time::Duration;

use asc_policy_types::event::EventTrust;
use asc_policy_types::primitives::{Digest, PolicyRevision, RuleId};
use asc_policy_types::request::InterventionPoint;
use asc_policy_types::subject::ActionKind;
use asc_policy_types::verdict::{
    AlertChannel, ApproverRef, AuditSink, FailAction, HarnessRuleSpec, QueueRef, Severity,
};
use serde::{Deserialize, Serialize};

use crate::tier::Tier;

/// Trace automaton identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AutomatonId(pub String);

/// Detector binding identifier (Tier C).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DetectorId(pub String);

/// Attribute path within the closed namespace set (§4.2).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AttrPath(pub String);

/// Taint/content label set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LabelSet(pub Vec<String>);

/// Hash-pinned reference to a static dataset in the bundle data layer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DatasetRef {
    /// Dataset name within the bundle.
    pub name: String,
    /// Content hash of the dataset.
    pub hash: Digest,
}

/// Default effect of a policy domain when no rule hits (§3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DomainDefault {
    /// Default deny (capability domain).
    Deny,
    /// Default pass without granting new permissions (detection domain).
    Pass,
    /// Cedar-style: at least one permit and no forbid (authorization domain).
    PermitWithForbidOverride,
}

/// Per-domain defaults declared in the bundle manifest and frozen into IR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainDefaults {
    /// File/network/exec/credential capability domain.
    pub capability: DomainDefault,
    /// Behavior detection domain.
    pub detection: DomainDefault,
    /// Authorization domain.
    pub authorization: DomainDefault,
}

/// Typed literal in a predicate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueIr {
    /// Boolean literal.
    Bool(bool),
    /// Integer literal.
    Int(i64),
    /// Floating-point literal.
    Float(f64),
    /// String literal.
    Str(String),
    /// String list literal.
    StrList(Vec<String>),
}

/// Comparison operators from the closed operator set (§4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CmpOp {
    /// Equal.
    Eq,
    /// Not equal.
    Ne,
    /// Less than.
    Lt,
    /// Less than or equal.
    Le,
    /// Greater than.
    Gt,
    /// Greater than or equal.
    Ge,
    /// Member of a literal list.
    In,
    /// Not a member of a literal list.
    NotIn,
    /// Glob match (the only pattern form allowed into Tier A).
    MatchesGlob,
    /// Linear-time regex match; Tier B only (§4.2).
    MatchesRe,
}

/// Set operators against hash-pinned datasets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SetOp {
    /// Non-empty intersection.
    Intersects,
    /// Subset relation.
    SubsetOf,
}

/// Degraded effect applied when a predicate cannot be evaluated as declared
/// (`onInsufficientAssurance`, `onBudgetExceeded`); never allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackEffect {
    /// Degrade to deny.
    Deny,
    /// Degrade to step-up approval.
    StepUp,
    /// Degrade to audit only (advisory domains).
    Audit,
}

/// Bounded predicate tree (§5.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PredicateIr {
    /// Always true.
    True,
    /// Attribute comparison.
    Cmp {
        /// Attribute path.
        attr: AttrPath,
        /// Comparison operator.
        op: CmpOp,
        /// Right-hand literal.
        value: ValueIr,
    },
    /// Attribute-to-dataset set operation.
    SetOp {
        /// Attribute path.
        attr: AttrPath,
        /// Set operator.
        op: SetOp,
        /// Dataset operand.
        set: DatasetRef,
    },
    /// Taint label intersection with an assurance floor (interface B).
    TaintIntersect {
        /// Attribute path carrying taint labels.
        attr: AttrPath,
        /// Labels to intersect with.
        labels: LabelSet,
        /// Minimum provenance assurance required.
        min_assurance: EventTrust,
        /// Effect when assurance is insufficient.
        on_insufficient: FallbackEffect,
    },
    /// Tier C detector reference, evaluated asynchronously under budget.
    DetectorRef {
        /// Bound detector.
        binding: DetectorId,
        /// Score threshold for a hit.
        threshold: f32,
        /// Evaluation budget.
        budget: Duration,
        /// Effect when the budget is exceeded; never allow.
        on_budget_exceeded: FallbackEffect,
    },
    /// Conjunction.
    And(Vec<PredicateIr>),
    /// Disjunction.
    Or(Vec<PredicateIr>),
    /// Negation.
    Not(Box<PredicateIr>),
}

/// Compiled subject matcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectMatcherIr {
    /// Matches every session (P0 `session: "*"`).
    Any,
    /// Glob over session ids.
    SessionGlob(String),
}

/// Compiled trigger: which intervention/action/tool combinations arm a rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerIr {
    /// Intervention point the rule fires on.
    pub intervention: InterventionPoint,
    /// Action family filter, when constrained.
    pub action_kind: Option<ActionKind>,
    /// Tool name filter; empty means any tool.
    pub tools: Vec<String>,
}

/// Rule effect under the merge law. `Ord` encodes Table 6 priority:
/// greater values take precedence, so `max()` implements the merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectIr {
    /// Explicit allow (lowest priority).
    Allow,
    /// Audit-only effect.
    Audit,
    /// Allow with parameter rewrite (patch).
    Modify,
    /// Deny-biased queueing.
    Defer,
    /// Deny-biased approval requirement.
    StepUp,
    /// Rejection; absorbs everything else (highest priority).
    Deny,
}

/// Compile-time quarantine scope selector; the runtime
/// `QuarantineScope` (§3.4) is assembled from this plus the triggering
/// subject (skill ref or session).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuarantineScopeKind {
    /// Downgrade the triggering skill's ledger trust state.
    SkillDegrade,
    /// Freeze the triggering session (cgroup v2 freezer).
    SessionFreeze,
}

/// Compiled obligation template. Runtime-only material (resume tokens) is
/// assembled by the PDP at verdict time, not stored in IR.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationIr {
    /// Audit obligation.
    Audit {
        /// Severity to record.
        severity: Severity,
        /// Target sink.
        sink: AuditSink,
    },
    /// Step-up approval template; the resume token is issued at assembly.
    StepUp {
        /// Approver reference.
        approver: ApproverRef,
        /// Prompt template shown on the approval channel; assembly resolves
        /// placeholders from the triggering request.
        prompt: String,
        /// Approval window.
        timeout: Duration,
        /// Timeout behavior.
        on_timeout: FailAction,
    },
    /// Deferred processing template.
    Defer {
        /// Target queue.
        queue: QueueRef,
        /// Relative deadline from verdict time.
        deadline: Duration,
        /// Deadline behavior.
        on_deadline: FailAction,
    },
    /// Alert emission template.
    EmitAlert {
        /// Delivery channel.
        channel: AlertChannel,
        /// Message template.
        template: String,
    },
    /// Trust score decrease.
    ScoreDelta {
        /// Amount to decrease.
        decrease: u32,
        /// Machine-readable reason.
        reason_code: String,
    },
    /// Kernel tightening rule (always also a Tier A entry).
    KernelRule {
        /// Rule to push via interface C.
        rule: HarnessRuleSpec,
        /// Rule lifetime.
        ttl: Duration,
    },
    /// Quarantine request template. The concrete target (which skill, which
    /// session) is resolved from the triggering subject at assembly.
    Quarantine {
        /// Which quarantine form to assemble.
        scope: QuarantineScopeKind,
    },
    /// Agent feedback template.
    Feedback {
        /// Message injected into the agent context.
        to_agent: String,
    },
}

/// Compiled rule (§5.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleIr {
    /// Rule identifier.
    pub rule_id: RuleId,
    /// Execution tier; rules containing Tier C sub-predicates are Tier B
    /// overall with the C sub-items marked inside the predicate.
    pub tier: Tier,
    /// Subject matcher.
    pub subject_matcher: SubjectMatcherIr,
    /// Trigger.
    pub trigger: TriggerIr,
    /// Bounded predicate tree.
    pub predicate: PredicateIr,
    /// Effect under the merge law.
    pub effect: EffectIr,
    /// Obligation templates.
    pub obligations: Vec<ObligationIr>,
    /// Patch application order within the same effect; does not affect the
    /// merge law.
    pub priority: u16,
    /// Lowest evidence assurance this rule may act on.
    pub provenance_requirement: EventTrust,
}

/// Inverted-index entry over (intervention, action kind, tool).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleIndexEntry {
    /// Indexed intervention point.
    pub intervention: InterventionPoint,
    /// Indexed action family, when constrained.
    pub action_kind: Option<ActionKind>,
    /// Indexed tool name, when constrained.
    pub tool: Option<String>,
    /// Rules armed for this key.
    pub rules: Vec<RuleId>,
}

/// Inverted index for O(hit) rule lookup on the decision path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleIndex {
    /// Index entries.
    pub entries: Vec<RuleIndexEntry>,
}

/// Automaton state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateIr {
    /// State id, unique within the automaton; state 0 is initial.
    pub id: u32,
    /// Whether reaching this state accepts (fires `on_accept`).
    pub accepting: bool,
}

/// Event pattern a transition matches on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventPatternIr {
    /// `AgentEvent` variant name the transition consumes.
    pub event_type: String,
}

/// Automaton transition with a guard predicate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionIr {
    /// Source state id.
    pub from: u32,
    /// Target state id.
    pub to: u32,
    /// Event pattern consumed.
    pub on: EventPatternIr,
    /// Guard evaluated against the matched event.
    pub guard: PredicateIr,
}

/// Resource bounds of an automaton, fixed at compile time and enforced at
/// runtime (§6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowSpec {
    /// Event window TTL (default 30 minutes).
    pub ttl: Duration,
    /// Per-session instance cap (default 256); exceeding triggers
    /// effect-split backpressure.
    pub max_instances: u32,
}

/// Compiled trace automaton (§5.2), partitioned by session at runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceAutomatonIr {
    /// Automaton identifier.
    pub automaton_id: AutomatonId,
    /// States, including initial and accepting.
    pub states: Vec<StateIr>,
    /// Transitions.
    pub transitions: Vec<TransitionIr>,
    /// Resource bounds.
    pub window: WindowSpec,
    /// Effect and obligations fired on acceptance.
    pub on_accept: (EffectIr, Vec<ObligationIr>),
    /// Lowest evidence assurance the automaton may act on.
    pub provenance_requirement: EventTrust,
}

/// Tier C detector binding declared in the bundle data layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectorBinding {
    /// Detector identifier.
    pub id: DetectorId,
    /// Detector/model version; part of the detector cache key (§6.4).
    pub version: String,
    /// Default evaluation budget.
    pub budget: Duration,
}

/// Compiled capability profile: Tier A output for sandbox configuration and
/// interface C rule push (§4.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityIr {
    /// Profile name.
    pub name: String,
    /// Subject the profile applies to.
    pub subject: SubjectMatcherIr,
    /// Compiled kernel rule specs; entries that cannot map to a target
    /// enforcement plane are excluded here and forced into audit (§4.3).
    pub kernel_rules: Vec<HarnessRuleSpec>,
}

/// Source location of a compiled rule, for explain output and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplainEntry {
    /// Rule identifier.
    pub rule_id: RuleId,
    /// Source file within the bundle.
    pub source: String,
    /// 1-based line number of the rule definition.
    pub line: u32,
}

/// Explain index mapping compiled rules back to bundle sources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplainIndex {
    /// Explain entries.
    pub entries: Vec<ExplainEntry>,
}

/// Complete compiled policy (§5.2): the only artifact the PDP evaluates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyIr {
    /// IR schema version, independent of the bundle format (§5.4).
    pub schema_version: u16,
    /// Revision this IR was compiled from.
    pub revision: PolicyRevision,
    /// Per-domain defaults.
    pub domain_defaults: DomainDefaults,
    /// Inverted rule index.
    pub rule_index: RuleIndex,
    /// Compiled rules.
    pub rules: Vec<RuleIr>,
    /// Compiled trace automata (P1).
    pub automata: Vec<TraceAutomatonIr>,
    /// Compiled capability profiles.
    pub capabilities: Vec<CapabilityIr>,
    /// Tier C detector bindings.
    pub detectors: Vec<DetectorBinding>,
    /// Hash-pinned static datasets.
    pub datasets: Vec<DatasetRef>,
    /// Explain index.
    pub explain: ExplainIndex,
}
