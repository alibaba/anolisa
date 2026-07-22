//! Case-level orchestration for upgrading audit evidence to enforcement.

mod policy;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

use agentsight_enforcement_protocol::{ApplyCredentialPolicy, Binding};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use self::policy::{
    ResolvedPolicy, acknowledgement_matches, enforce_request, live_candidates, resolve_policy,
    select_candidate,
};
use super::{
    ContainmentAction, ContainmentActivationResult, ContainmentClaimResult,
    ContainmentFailureStage, ContainmentLifecycle, RiskCaseDetail, RiskCaseStatus, SecurityStore,
    SecurityStoreError,
};
use crate::enforcement::EnforcementCoordinator;

const DEFAULT_DURATION_SECS: u64 = 900;
const MIN_DURATION_SECS: u64 = 60;
const MAX_DURATION_SECS: u64 = 86_400;

/// Enforcement operations required by containment orchestration.
pub trait ContainmentEnforcer: Send + Sync {
    /// Compiles and applies one product-level credential policy.
    fn apply_credential_policy(&self, request: ApplyCredentialPolicy) -> Result<Binding, String>;
    /// Detaches a previously applied binding.
    fn detach(&self, binding_id: Uuid) -> Result<(), String>;
    /// Lists persisted enforcement bindings.
    fn bindings(&self) -> Result<Vec<Binding>, String>;
}

impl ContainmentEnforcer for EnforcementCoordinator {
    fn apply_credential_policy(&self, request: ApplyCredentialPolicy) -> Result<Binding, String> {
        EnforcementCoordinator::apply_credential_policy(self, request)
            .map_err(|error| error.to_string())
    }

    fn detach(&self, binding_id: Uuid) -> Result<(), String> {
        EnforcementCoordinator::detach(self, binding_id).map_err(|error| error.to_string())
    }

    fn bindings(&self) -> Result<Vec<Binding>, String> {
        EnforcementCoordinator::bindings(self).map_err(|error| error.to_string())
    }
}

/// User-selected process and duration for one containment request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainmentRequest {
    /// Selected process-tree root.
    pub root_pid: i32,
    /// Temporary duration, or `None` for explicit persistent enforcement.
    pub duration_secs: Option<u64>,
}

/// Live process identity eligible to receive the case-derived policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainmentCandidate {
    /// Product Agent identifier.
    pub agent_id: String,
    /// Candidate process-tree root.
    pub root_pid: i32,
    /// Linux process start time paired with `root_pid`.
    pub process_start_time: u64,
    /// Human-readable Agent or process name.
    pub display_name: String,
}

/// Case-derived data needed to confirm a containment request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainmentPlan {
    /// Source audit case.
    pub case_id: Uuid,
    /// Canonical source path recovered from the original persisted binding.
    pub source_path: String,
    /// Process identity from the original binding.
    pub original_target: Option<ContainmentCandidate>,
    /// Whether the original PID still has its recorded start time.
    pub original_target_valid: bool,
    /// Current same-Agent replacement candidates.
    pub candidates: Vec<ContainmentCandidate>,
    /// Safe temporary duration shown by the confirmation UI.
    pub default_duration_secs: u64,
    /// Smallest accepted temporary duration.
    pub min_duration_secs: u64,
    /// Largest accepted temporary duration.
    pub max_duration_secs: u64,
    /// Most recent action, when one already exists.
    pub existing_action: Option<ContainmentAction>,
}

/// Typed failures at the case, process, store, and enforcer boundaries.
#[derive(Debug, Error)]
pub enum ContainmentError {
    /// The requested case does not exist.
    #[error("risk case {0} does not exist")]
    MissingCase(Uuid),
    /// Original policy provenance cannot safely produce an enforce policy.
    #[error("source policy for case {0} is unavailable")]
    SourcePolicyUnavailable(Uuid),
    /// The selected PID is missing, protected, recycled, or not an approved candidate.
    #[error("root process {0} is stale")]
    RootProcessStale(i32),
    /// Multiple trusted candidates identify the same selected PID.
    #[error("multiple trusted candidates identify root process {0}")]
    AmbiguousCandidate(i32),
    /// The case review state forbids containment.
    #[error("case {case_id} cannot be contained from state {status:?}")]
    IneligibleCase {
        /// Requested case.
        case_id: Uuid,
        /// Current ineligible review state.
        status: RiskCaseStatus,
    },
    /// A temporary duration falls outside the approved bounds.
    #[error("duration must be null or between 60 and 86400 seconds")]
    InvalidDuration,
    /// The authenticated requester identity is unsafe to persist.
    #[error("requested_by must be 1 to 128 bytes without control characters")]
    InvalidRequestedBy,
    /// An active lifecycle action exists with a different target or duration.
    #[error("containment action {0} already targets this case with different parameters")]
    IncompatibleAction(Uuid),
    /// A compatible action is still awaiting an enforcement acknowledgement.
    #[error("containment action {0} is still in progress")]
    ContainmentInProgress(Uuid),
    /// A compatible action is already detaching and cannot be reported active.
    #[error("containment action {0} is expiring")]
    ContainmentExpiring(Uuid),
    /// Human review made the case ineligible while enforcement was applying.
    #[error("case {case_id} changed to {status:?} while containment was applying")]
    CaseEligibilityChanged {
        /// Requested case.
        case_id: Uuid,
        /// Review state observed by the activation transaction.
        status: RiskCaseStatus,
    },
    /// The privileged enforcement boundary failed or returned an invalid acknowledgement.
    #[error("enforcement unavailable: {0}")]
    Enforcer(String),
    /// Local security persistence failed.
    #[error(transparent)]
    Store(#[from] SecurityStoreError),
}

/// Coordinates provenance recovery, durable intent, and enforced acknowledgement.
pub struct ContainmentCoordinator {
    store: Arc<SecurityStore>,
    enforcer: Arc<dyn ContainmentEnforcer>,
    stop: Arc<AtomicBool>,
}

impl ContainmentCoordinator {
    /// Creates a coordinator without starting background reconciliation.
    pub fn new(store: Arc<SecurityStore>, enforcer: Arc<dyn ContainmentEnforcer>) -> Self {
        Self {
            store,
            enforcer,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Builds a containment plan solely from the case's persisted binding provenance.
    ///
    /// # Errors
    /// Returns a typed case, policy-provenance, store, or enforcer error.
    pub fn plan(
        &self,
        case_id: Uuid,
        candidates: Vec<ContainmentCandidate>,
    ) -> Result<ContainmentPlan, ContainmentError> {
        let context = self.case_context(case_id)?;
        let candidates = live_candidates(&context.detail.case.agent_id, candidates)?;
        let request = &context.binding.request;
        let original_target = ContainmentCandidate {
            agent_id: request.agent_id.clone(),
            root_pid: request.root_pid,
            process_start_time: request.process_start_time,
            display_name: context
                .detail
                .evidence
                .iter()
                .find_map(|event| event.identity.agent_name.clone())
                .unwrap_or_else(|| request.agent_id.clone()),
        };
        let original_target_valid = candidates.iter().any(|candidate| {
            candidate.root_pid == original_target.root_pid
                && candidate.process_start_time == original_target.process_start_time
        });
        Ok(ContainmentPlan {
            case_id,
            source_path: context.source_path,
            original_target: Some(original_target),
            original_target_valid,
            candidates,
            default_duration_secs: DEFAULT_DURATION_SECS,
            min_duration_secs: MIN_DURATION_SECS,
            max_duration_secs: MAX_DURATION_SECS,
            existing_action: self.store.latest_containment_action(case_id)?,
        })
    }

    /// Persists pending intent, applies enforcement, and confirms after acknowledgement.
    ///
    /// `None` is an explicit persistent duration; no request-side default is applied.
    ///
    /// # Errors
    /// Returns a typed validation, case, policy-provenance, store, or enforcer error.
    pub fn contain(
        &self,
        case_id: Uuid,
        request: ContainmentRequest,
        candidates: &[ContainmentCandidate],
        requested_by: &str,
    ) -> Result<ContainmentAction, ContainmentError> {
        validate_duration(request.duration_secs)?;
        let requested_by = validate_requested_by(requested_by)?;
        let detail = self.case_detail(case_id)?;
        let selected = select_candidate(&detail.case.agent_id, request.root_pid, candidates)?;
        if let Some(existing) = self.store.latest_containment_action(case_id)?
            && live_lifecycle(existing.lifecycle_state)
        {
            return existing_action(existing, &request, selected.process_start_time);
        }
        let context = self.context_from_detail(detail)?;

        let now = now_ns();
        let binding_id = Uuid::new_v4();
        let apply = enforce_request(
            &context,
            binding_id,
            request.root_pid,
            selected.process_start_time,
        )
        .ok_or(ContainmentError::SourcePolicyUnavailable(case_id))?;
        let expires_at_ns = request
            .duration_secs
            .map(|duration| now.saturating_add(duration.saturating_mul(1_000_000_000)));
        let mut action = ContainmentAction {
            action_id: Uuid::new_v4(),
            case_id,
            binding_id,
            agent_id: context.detail.case.agent_id.clone(),
            root_pid: request.root_pid,
            process_start_time: selected.process_start_time,
            source_path: context.source_path,
            duration_secs: request.duration_secs,
            expires_at_ns,
            lifecycle_state: ContainmentLifecycle::Pending,
            blocked_at_ns: None,
            requested_by,
            failure_stage: None,
            failure_reason: None,
            attempt_count: 0,
            next_retry_at_ns: None,
            created_at_ns: now,
            updated_at_ns: now,
        };
        match self.store.claim_containment_action(&action)? {
            ContainmentClaimResult::Claimed => {}
            ContainmentClaimResult::Existing(existing) => {
                return existing_action(*existing, &request, selected.process_start_time);
            }
            ContainmentClaimResult::CaseIneligible(status) => {
                return Err(ContainmentError::IneligibleCase { case_id, status });
            }
        }

        let acknowledgement = match self.enforcer.apply_credential_policy(apply.clone()) {
            Ok(binding) => binding,
            Err(message) => return self.attach_failed(action, &message),
        };
        if !acknowledgement_matches(&acknowledgement, &apply) {
            let message = "enforcer returned an invalid binding acknowledgement";
            self.detach_and_fail(&mut action, ContainmentFailureStage::Attach, message)?;
            return Err(ContainmentError::Enforcer(message.into()));
        }

        action.updated_at_ns = now_ns();
        match self
            .store
            .activate_containment_action(action.action_id, action.updated_at_ns)
        {
            Ok(ContainmentActivationResult::Activated) => {
                action.lifecycle_state = ContainmentLifecycle::Active;
                Ok(action)
            }
            Ok(ContainmentActivationResult::CaseIneligible(status)) => {
                self.detach_and_fail(
                    &mut action,
                    ContainmentFailureStage::Reconcile,
                    "case eligibility changed while enforcement was applying",
                )?;
                Err(ContainmentError::CaseEligibilityChanged { case_id, status })
            }
            Err(error) => {
                self.detach_and_fail(
                    &mut action,
                    ContainmentFailureStage::Reconcile,
                    &format!("transactional activation failed: {error}"),
                )?;
                Err(error.into())
            }
        }
    }

    fn case_context(&self, case_id: Uuid) -> Result<ResolvedPolicy, ContainmentError> {
        let detail = self.case_detail(case_id)?;
        self.context_from_detail(detail)
    }

    fn case_detail(&self, case_id: Uuid) -> Result<RiskCaseDetail, ContainmentError> {
        let detail = match self.store.case_detail(case_id) {
            Ok(detail) => detail,
            Err(SecurityStoreError::MissingCase(_)) => {
                return Err(ContainmentError::MissingCase(case_id));
            }
            Err(error) => return Err(error.into()),
        };
        if !matches!(
            detail.case.status,
            RiskCaseStatus::Open | RiskCaseStatus::Confirmed
        ) {
            return Err(ContainmentError::IneligibleCase {
                case_id,
                status: detail.case.status,
            });
        }
        Ok(detail)
    }

    fn context_from_detail(
        &self,
        detail: RiskCaseDetail,
    ) -> Result<ResolvedPolicy, ContainmentError> {
        let case_id = detail.case.case_id;
        let bindings = self
            .enforcer
            .bindings()
            .map_err(|message| ContainmentError::Enforcer(sanitize_failure(&message)))?;
        resolve_policy(detail, bindings).ok_or(ContainmentError::SourcePolicyUnavailable(case_id))
    }

    fn attach_failed(
        &self,
        mut action: ContainmentAction,
        message: &str,
    ) -> Result<ContainmentAction, ContainmentError> {
        let message = sanitize_failure(message);
        self.persist_failed(
            &mut action,
            ContainmentFailureStage::Attach,
            message.clone(),
        )?;
        Err(ContainmentError::Enforcer(message))
    }

    fn detach_and_fail(
        &self,
        action: &mut ContainmentAction,
        stage: ContainmentFailureStage,
        message: &str,
    ) -> Result<(), ContainmentError> {
        let reason = match self.enforcer.detach(action.binding_id) {
            Ok(()) => sanitize_failure(message),
            Err(error) => sanitize_failure(&format!("{message}; detach failed: {error}")),
        };
        self.persist_failed(action, stage, reason)
    }

    fn persist_failed(
        &self,
        action: &mut ContainmentAction,
        stage: ContainmentFailureStage,
        reason: String,
    ) -> Result<(), ContainmentError> {
        action.lifecycle_state = ContainmentLifecycle::Failed;
        action.failure_stage = Some(stage);
        action.failure_reason = Some(reason);
        action.updated_at_ns = now_ns();
        if !self.store.update_containment_action(action)? {
            return Err(SecurityStoreError::InvalidData(format!(
                "containment action {} disappeared while recording failure",
                action.action_id
            ))
            .into());
        }
        Ok(())
    }
}

impl Drop for ContainmentCoordinator {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
    }
}

fn live_lifecycle(lifecycle: ContainmentLifecycle) -> bool {
    matches!(
        lifecycle,
        ContainmentLifecycle::Pending
            | ContainmentLifecycle::Active
            | ContainmentLifecycle::Expiring
    )
}

fn existing_action(
    existing: ContainmentAction,
    request: &ContainmentRequest,
    process_start_time: u64,
) -> Result<ContainmentAction, ContainmentError> {
    if existing.root_pid != request.root_pid
        || existing.process_start_time != process_start_time
        || existing.duration_secs != request.duration_secs
    {
        return Err(ContainmentError::IncompatibleAction(existing.action_id));
    }
    match existing.lifecycle_state {
        ContainmentLifecycle::Active => Ok(existing),
        ContainmentLifecycle::Pending => {
            Err(ContainmentError::ContainmentInProgress(existing.action_id))
        }
        ContainmentLifecycle::Expiring => {
            Err(ContainmentError::ContainmentExpiring(existing.action_id))
        }
        ContainmentLifecycle::Expired | ContainmentLifecycle::Failed => {
            Err(ContainmentError::IncompatibleAction(existing.action_id))
        }
    }
}

fn validate_duration(duration_secs: Option<u64>) -> Result<(), ContainmentError> {
    if duration_secs.is_some_and(|value| !(MIN_DURATION_SECS..=MAX_DURATION_SECS).contains(&value))
    {
        return Err(ContainmentError::InvalidDuration);
    }
    Ok(())
}

fn validate_requested_by(requested_by: &str) -> Result<String, ContainmentError> {
    if requested_by.len() > 128 || requested_by.chars().any(char::is_control) {
        return Err(ContainmentError::InvalidRequestedBy);
    }
    let requested_by = requested_by.trim();
    if requested_by.is_empty() {
        return Err(ContainmentError::InvalidRequestedBy);
    }
    Ok(requested_by.to_string())
}

fn sanitize_failure(message: &str) -> String {
    let sanitized: String = message
        .chars()
        .filter(|character| !character.is_control())
        .take(512)
        .collect();
    if sanitized.is_empty() {
        "enforcer operation failed without detail".into()
    } else {
        sanitized
    }
}

fn now_ns() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    u64::try_from(nanos).unwrap_or(u64::MAX)
}
