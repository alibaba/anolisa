//! Case-level orchestration for upgrading audit evidence to enforcement.

mod policy;
mod reconciler;
mod worker;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use agentsight_enforcement_protocol::{ApplyCredentialPolicy, Binding};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use self::policy::{
    ResolvedPolicy, acknowledgement_matches, enforce_request, existing_action, live_candidates,
    live_lifecycle, resolve_policy, sanitize_failure, select_candidate, validate_duration,
    validate_process_identity, validate_requested_by,
};
use super::{
    ContainmentAction, ContainmentActivationResult, ContainmentClaimResult,
    ContainmentFailureStage, ContainmentLifecycle, RiskCaseDetail, RiskCaseStatus, SecurityStore,
    SecurityStoreError,
};
use crate::enforcement::{
    EnforcementCoordinator, EnforcementCoordinatorError, EnforcementError, read_process_start_time,
};

const DEFAULT_DURATION_SECS: u64 = 900;
const MIN_DURATION_SECS: u64 = 60;
const MAX_DURATION_SECS: u64 = 86_400;
const CLEANUP_RETRY_DELAY_NS: u64 = 1_000_000_000;

/// Typed failures returned by the containment enforcement boundary.
#[derive(Debug, Error)]
pub enum ContainmentEnforcerError {
    /// Readiness, transport, or local enforcement state is temporarily unavailable.
    #[error("{0}")]
    Unavailable(String),
    /// The enforcement boundary rejected or contradicted the durable request.
    #[error("{0}")]
    Rejected(String),
}

/// Enforcement operations required by containment orchestration.
pub trait ContainmentEnforcer: Send + Sync {
    /// Compiles and applies one product-level credential policy.
    fn apply_credential_policy(
        &self,
        request: ApplyCredentialPolicy,
    ) -> Result<Binding, ContainmentEnforcerError>;
    /// Detaches a previously applied binding.
    fn detach(&self, binding_id: Uuid) -> Result<(), String>;
    /// Lists persisted enforcement bindings.
    fn bindings(&self) -> Result<Vec<Binding>, ContainmentEnforcerError>;
}

impl ContainmentEnforcer for EnforcementCoordinator {
    fn apply_credential_policy(
        &self,
        request: ApplyCredentialPolicy,
    ) -> Result<Binding, ContainmentEnforcerError> {
        EnforcementCoordinator::apply_credential_policy(self, request)
            .map_err(containment_enforcer_error)
    }

    fn detach(&self, binding_id: Uuid) -> Result<(), String> {
        EnforcementCoordinator::detach(self, binding_id).map_err(|error| error.to_string())
    }

    fn bindings(&self) -> Result<Vec<Binding>, ContainmentEnforcerError> {
        EnforcementCoordinator::bindings(self).map_err(containment_enforcer_error)
    }
}

fn containment_enforcer_error(error: EnforcementCoordinatorError) -> ContainmentEnforcerError {
    let unavailable = matches!(
        &error,
        EnforcementCoordinatorError::IngestionUnavailable
            | EnforcementCoordinatorError::Store(_)
            | EnforcementCoordinatorError::Thread(_)
            | EnforcementCoordinatorError::Client(
                EnforcementError::Io(_) | EnforcementError::Disconnected
            )
    );
    let message = error.to_string();
    if unavailable {
        ContainmentEnforcerError::Unavailable(message)
    } else {
        ContainmentEnforcerError::Rejected(message)
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
    /// Detachment failed, so the binding may still enforce until reconciliation succeeds.
    #[error(
        "containment action {action_id} requires cleanup; binding {binding_id} may remain attached: {reason}"
    )]
    CleanupRequired {
        /// Action retaining the case-level uniqueness claim.
        action_id: Uuid,
        /// Binding whose attachment state is uncertain.
        binding_id: Uuid,
        /// Sanitized detachment failure detail.
        reason: String,
    },
    /// The privileged enforcement boundary failed or returned an invalid acknowledgement.
    #[error("enforcement unavailable: {0}")]
    Enforcer(String),
    /// Local security persistence failed.
    #[error(transparent)]
    Store(#[from] SecurityStoreError),
    /// A second background reconciler was requested for this coordinator.
    #[error("containment reconciler is already running")]
    AlreadyRunning,
    /// Spawning the background reconciler failed.
    #[error("failed to start containment reconciler: {0}")]
    ReconcilerThread(std::io::Error),
    /// Persisted provenance could not safely restore a pending action.
    #[error("containment action {action_id} recovery failed: {reason}")]
    RecoveryFailed {
        /// Pending action that could not be recovered.
        action_id: Uuid,
        /// Sanitized actionable failure detail.
        reason: String,
    },
    /// Another worker replaced the lifecycle claim while an operation was in flight.
    #[error("containment action {0} reconciliation claim was lost")]
    ClaimLost(Uuid),
    /// Malformed due rows were quarantined after valid rows continued.
    #[error("quarantined {count} corrupt containment action(s)")]
    CorruptActions {
        /// Number of malformed rows quarantined from this bounded batch.
        count: usize,
    },
}

/// Coordinates provenance recovery, durable intent, and enforced acknowledgement.
pub struct ContainmentCoordinator {
    store: Arc<SecurityStore>,
    enforcer: Arc<dyn ContainmentEnforcer>,
    worker: Arc<worker::ReconcilerControl>,
}

impl ContainmentCoordinator {
    /// Creates a coordinator without starting background reconciliation.
    pub fn new(store: Arc<SecurityStore>, enforcer: Arc<dyn ContainmentEnforcer>) -> Self {
        Self {
            store,
            enforcer,
            worker: Arc::new(worker::ReconcilerControl::new()),
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
        let original_target_valid = read_process_start_time(original_target.root_pid)
            .is_ok_and(|actual| actual == original_target.process_start_time);
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
        if let Some(existing) = self.store.latest_containment_action(case_id)?
            && live_lifecycle(existing.lifecycle_state)
        {
            if existing.root_pid != request.root_pid
                || existing.duration_secs != request.duration_secs
            {
                return Err(ContainmentError::IncompatibleAction(existing.action_id));
            }
            let process_start_time =
                validate_process_identity(existing.root_pid, existing.process_start_time)?;
            return existing_action(existing, &request, process_start_time);
        }
        let context = self.context_from_detail(detail)?;
        let process_start_time = if request.root_pid == context.binding.request.root_pid {
            validate_process_identity(request.root_pid, context.binding.request.process_start_time)?
        } else {
            select_candidate(&context.detail.case.agent_id, request.root_pid, candidates)?
                .process_start_time
        };

        let now = now_ns();
        let binding_id = Uuid::new_v4();
        let apply = enforce_request(&context, binding_id, request.root_pid, process_start_time)
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
            process_start_time,
            source_path: context.source_path,
            duration_secs: request.duration_secs,
            expires_at_ns,
            lifecycle_state: ContainmentLifecycle::Pending,
            blocked_at_ns: None,
            requested_by,
            failure_stage: None,
            failure_reason: None,
            attempt_count: 0,
            next_retry_at_ns: Some(now),
            created_at_ns: now,
            updated_at_ns: now,
        };
        match self.store.claim_containment_action(&action)? {
            ContainmentClaimResult::Claimed => {}
            ContainmentClaimResult::Existing(existing) => {
                return existing_action(*existing, &request, process_start_time);
            }
            ContainmentClaimResult::CaseIneligible(status) => {
                return Err(ContainmentError::IneligibleCase { case_id, status });
            }
        }

        let acknowledgement = match self.enforcer.apply_credential_policy(apply.clone()) {
            Ok(binding) => binding,
            Err(error) => return self.attach_failed(action, &error.to_string()),
        };
        if !acknowledgement_matches(&acknowledgement, &apply) {
            let message = "enforcer returned an invalid binding acknowledgement";
            self.detach_and_fail(&mut action, ContainmentFailureStage::Attach, message)?;
            return Err(ContainmentError::Enforcer(message.into()));
        }

        let claimed_at_ns = action.updated_at_ns;
        let activated_at_ns = now_ns().max(claimed_at_ns.saturating_add(1));
        match self.store.activate_containment_action(
            action.action_id,
            claimed_at_ns,
            activated_at_ns,
        ) {
            Ok(ContainmentActivationResult::Activated) => {
                action.lifecycle_state = ContainmentLifecycle::Active;
                action.next_retry_at_ns = None;
                action.updated_at_ns = activated_at_ns;
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
            Ok(ContainmentActivationResult::LostClaim) => {
                Err(ContainmentError::ClaimLost(action.action_id))
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
            .map_err(|error| ContainmentError::Enforcer(sanitize_failure(&error.to_string())))?;
        resolve_policy(detail, bindings).ok_or(ContainmentError::SourcePolicyUnavailable(case_id))
    }

    fn attach_failed(
        &self,
        mut action: ContainmentAction,
        message: &str,
    ) -> Result<ContainmentAction, ContainmentError> {
        let message = sanitize_failure(message);
        let claimed_at_ns = action.updated_at_ns;
        action.lifecycle_state = ContainmentLifecycle::Failed;
        action.failure_stage = Some(ContainmentFailureStage::Attach);
        action.failure_reason = Some(message.clone());
        action.next_retry_at_ns = None;
        action.updated_at_ns = now_ns().max(claimed_at_ns.saturating_add(1));
        self.finish_direct_claim(&action, ContainmentLifecycle::Pending, claimed_at_ns)?;
        Err(ContainmentError::Enforcer(message))
    }

    fn detach_and_fail(
        &self,
        action: &mut ContainmentAction,
        stage: ContainmentFailureStage,
        message: &str,
    ) -> Result<(), ContainmentError> {
        let reason = sanitize_failure(message);
        let claimed_at_ns = action.updated_at_ns;
        let current_time_ns = now_ns();
        let Some(mut cleanup) = self.store.begin_containment_cleanup(
            action,
            ContainmentLifecycle::Pending,
            claimed_at_ns,
            current_time_ns,
            reason.clone(),
        )?
        else {
            return Err(ContainmentError::ClaimLost(action.action_id));
        };
        let cleanup_claim_ns = cleanup.updated_at_ns;
        match self.enforcer.detach(action.binding_id) {
            Ok(()) => {
                cleanup.lifecycle_state = ContainmentLifecycle::Failed;
                cleanup.failure_stage = Some(stage);
                cleanup.failure_reason = Some(reason);
                cleanup.next_retry_at_ns = None;
                cleanup.updated_at_ns = now_ns().max(cleanup_claim_ns.saturating_add(1));
                self.finish_direct_claim(
                    &cleanup,
                    ContainmentLifecycle::Expiring,
                    cleanup_claim_ns,
                )?;
                *action = cleanup;
                Ok(())
            }
            Err(error) => {
                let reason = sanitize_failure(&format!("{message}; detach failed: {error}"));
                cleanup.failure_reason = Some(reason.clone());
                cleanup.attempt_count = cleanup.attempt_count.saturating_add(1);
                cleanup.next_retry_at_ns =
                    Some(current_time_ns.saturating_add(CLEANUP_RETRY_DELAY_NS));
                cleanup.updated_at_ns = now_ns().max(cleanup_claim_ns.saturating_add(1));
                self.finish_direct_claim(
                    &cleanup,
                    ContainmentLifecycle::Expiring,
                    cleanup_claim_ns,
                )?;
                *action = cleanup;
                Err(ContainmentError::CleanupRequired {
                    action_id: action.action_id,
                    binding_id: action.binding_id,
                    reason,
                })
            }
        }
    }

    fn finish_direct_claim(
        &self,
        action: &ContainmentAction,
        claimed_lifecycle: ContainmentLifecycle,
        claimed_at_ns: u64,
    ) -> Result<(), ContainmentError> {
        if self
            .store
            .finish_containment_reconciliation(action, claimed_lifecycle, claimed_at_ns)?
        {
            return Ok(());
        }
        Err(ContainmentError::ClaimLost(action.action_id))
    }
}

fn now_ns() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    u64::try_from(nanos).unwrap_or(u64::MAX)
}
