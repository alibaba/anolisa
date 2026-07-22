//! Bounded restart recovery and expiry handling for containment actions.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use agentsight_enforcement_protocol::Binding;

use super::{
    ContainmentAction, ContainmentActivationResult, ContainmentCoordinator, ContainmentError,
    ContainmentFailureStage, ContainmentLifecycle, RiskCaseStatus, SecurityStoreError,
    acknowledgement_matches, enforce_request, now_ns, resolve_policy, sanitize_failure,
    validate_process_identity,
};

const DUE_BATCH_LIMIT: usize = 100;
const DETACH_MAX_RETRIES: u32 = 5;
const SECOND_NS: u64 = 1_000_000_000;

impl ContainmentCoordinator {
    /// Reconciles at most one bounded batch of due persisted actions.
    ///
    /// # Errors
    ///
    /// Returns the first typed store, enforcer, or recovery failure after
    /// continuing to process the rest of the fetched batch.
    pub fn reconcile_once(&self, current_time_ns: u64) -> Result<(), ContainmentError> {
        let actions = self
            .store
            .due_containment_actions(current_time_ns, DUE_BATCH_LIMIT)?;
        let mut first_error = None;
        for action in actions {
            let result = self
                .store
                .claim_containment_reconciliation(&action, current_time_ns)
                .map_err(ContainmentError::from)
                .and_then(|claimed| match claimed {
                    Some(claimed) => self.reconcile_claimed(claimed, current_time_ns),
                    None => Ok(()),
                });
            if let Err(error) = result
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Starts one stoppable background reconciliation worker.
    ///
    /// # Errors
    ///
    /// Returns [`ContainmentError::AlreadyRunning`] for duplicate workers or
    /// [`ContainmentError::ReconcilerThread`] when spawning the thread fails.
    pub fn start_reconciler(&self, interval: Duration) -> Result<JoinHandle<()>, ContainmentError> {
        if self
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ContainmentError::AlreadyRunning);
        }
        self.stop.store(false, Ordering::Release);
        let interval = interval.max(Duration::from_millis(10));
        let store = Arc::clone(&self.store);
        let enforcer = Arc::clone(&self.enforcer);
        let stop = Arc::clone(&self.stop);
        let running = Arc::clone(&self.running);
        thread::Builder::new()
            .name("agentsight-containment-reconciler".into())
            .spawn(move || {
                let coordinator = ContainmentCoordinator {
                    store,
                    enforcer,
                    stop: Arc::clone(&stop),
                    running: Arc::clone(&running),
                };
                while !stop.load(Ordering::Acquire) {
                    if let Err(error) = coordinator.reconcile_once(now_ns()) {
                        log::error!("containment reconciliation failed: {error}");
                    }
                    sleep_until_stopped(&stop, interval);
                }
                running.store(false, Ordering::Release);
            })
            .map_err(|error| {
                self.running.store(false, Ordering::Release);
                ContainmentError::ReconcilerThread(error)
            })
    }

    /// Requests the background reconciliation worker to stop.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    fn reconcile_claimed(
        &self,
        action: ContainmentAction,
        current_time_ns: u64,
    ) -> Result<(), ContainmentError> {
        match action.lifecycle_state {
            ContainmentLifecycle::Pending => self.reconcile_pending(action, current_time_ns),
            ContainmentLifecycle::Expiring => self.reconcile_detach(action, current_time_ns),
            ContainmentLifecycle::Active
            | ContainmentLifecycle::Expired
            | ContainmentLifecycle::Failed => Err(SecurityStoreError::InvalidData(format!(
                "containment action {} has invalid claimed lifecycle {:?}",
                action.action_id, action.lifecycle_state
            ))
            .into()),
        }
    }

    fn reconcile_pending(
        &self,
        mut action: ContainmentAction,
        current_time_ns: u64,
    ) -> Result<(), ContainmentError> {
        let claimed_at_ns = action.updated_at_ns;
        let bindings = match self.enforcer.bindings() {
            Ok(bindings) => bindings,
            Err(message) => {
                let reason = sanitize_failure(&message);
                action.failure_stage = Some(ContainmentFailureStage::Reconcile);
                action.failure_reason = Some(reason.clone());
                self.finish_claimed(&action, ContainmentLifecycle::Pending, claimed_at_ns)?;
                return Err(ContainmentError::Enforcer(reason));
            }
        };
        let exact = exact_binding(&bindings, action.binding_id);
        if exact.is_err() {
            return self.fail_pending(
                action,
                claimed_at_ns,
                "enforcer returned duplicate containment binding identities",
                true,
                current_time_ns,
            );
        }
        let exact = exact.ok().flatten();
        let detail = self.store.case_detail(action.case_id)?;
        if !matches!(
            detail.case.status,
            RiskCaseStatus::Open | RiskCaseStatus::Confirmed
        ) {
            return self.fail_pending(
                action,
                claimed_at_ns,
                "source case is no longer eligible for containment recovery",
                exact.is_some(),
                current_time_ns,
            );
        }
        let Some(context) = resolve_policy(detail, bindings) else {
            return self.fail_pending(
                action,
                claimed_at_ns,
                "original audit binding provenance is unavailable",
                exact.is_some(),
                current_time_ns,
            );
        };
        if context.detail.case.agent_id != action.agent_id
            || context.source_path != action.source_path
        {
            return self.fail_pending(
                action,
                claimed_at_ns,
                "persisted containment identity does not match source provenance",
                exact.is_some(),
                current_time_ns,
            );
        }
        let Some(request) = enforce_request(
            &context,
            action.binding_id,
            action.root_pid,
            action.process_start_time,
        ) else {
            return self.fail_pending(
                action,
                claimed_at_ns,
                "source policy cannot be reconstructed exactly",
                exact.is_some(),
                current_time_ns,
            );
        };

        let acknowledgement = match exact {
            Some(binding) if acknowledgement_matches(&binding, &request) => binding,
            Some(_) => {
                return self.fail_pending(
                    action,
                    claimed_at_ns,
                    "existing containment binding does not match durable intent",
                    true,
                    current_time_ns,
                );
            }
            None => {
                if validate_process_identity(action.root_pid, action.process_start_time).is_err() {
                    return self.fail_pending(
                        action,
                        claimed_at_ns,
                        "persisted containment process identity is stale",
                        false,
                        current_time_ns,
                    );
                }
                match self.enforcer.apply_credential_policy(request.clone()) {
                    Ok(binding) => binding,
                    Err(message) => {
                        let reason = sanitize_failure(&message);
                        action.lifecycle_state = ContainmentLifecycle::Failed;
                        action.failure_stage = Some(ContainmentFailureStage::Reconcile);
                        action.failure_reason = Some(reason.clone());
                        action.next_retry_at_ns = None;
                        self.finish_claimed(&action, ContainmentLifecycle::Pending, claimed_at_ns)?;
                        return Err(ContainmentError::Enforcer(reason));
                    }
                }
            }
        };
        if !acknowledgement_matches(&acknowledgement, &request) {
            return self.fail_pending(
                action,
                claimed_at_ns,
                "enforcer returned an invalid recovery acknowledgement",
                true,
                current_time_ns,
            );
        }
        match self
            .store
            .activate_containment_action(action.action_id, action.updated_at_ns)
        {
            Ok(ContainmentActivationResult::Activated) => Ok(()),
            Ok(ContainmentActivationResult::CaseIneligible(_)) => self.fail_pending(
                action,
                claimed_at_ns,
                "source case changed eligibility during containment recovery",
                true,
                current_time_ns,
            ),
            Err(error) => {
                let reason = format!("transactional recovery activation failed: {error}");
                self.fail_pending(action, claimed_at_ns, &reason, true, current_time_ns)
            }
        }
    }

    fn fail_pending(
        &self,
        mut action: ContainmentAction,
        claimed_at_ns: u64,
        reason: &str,
        cleanup_binding: bool,
        current_time_ns: u64,
    ) -> Result<(), ContainmentError> {
        let reason = sanitize_failure(reason);
        if cleanup_binding {
            return match self.enforcer.detach(action.binding_id) {
                Ok(()) => {
                    action.lifecycle_state = ContainmentLifecycle::Failed;
                    action.failure_stage = Some(ContainmentFailureStage::Reconcile);
                    action.failure_reason = Some(reason.clone());
                    action.next_retry_at_ns = None;
                    self.finish_claimed(&action, ContainmentLifecycle::Pending, claimed_at_ns)?;
                    Err(ContainmentError::RecoveryFailed {
                        action_id: action.action_id,
                        reason,
                    })
                }
                Err(message) => {
                    let detach_reason =
                        sanitize_failure(&format!("{reason}; detach failed: {message}"));
                    self.record_detach_failure(
                        &mut action,
                        ContainmentLifecycle::Pending,
                        claimed_at_ns,
                        current_time_ns,
                        detach_reason.clone(),
                    )?;
                    Err(ContainmentError::CleanupRequired {
                        action_id: action.action_id,
                        binding_id: action.binding_id,
                        reason: detach_reason,
                    })
                }
            };
        }
        action.lifecycle_state = ContainmentLifecycle::Failed;
        action.failure_stage = Some(ContainmentFailureStage::Reconcile);
        action.failure_reason = Some(reason.clone());
        action.next_retry_at_ns = None;
        self.finish_claimed(&action, ContainmentLifecycle::Pending, claimed_at_ns)?;
        Err(ContainmentError::RecoveryFailed {
            action_id: action.action_id,
            reason,
        })
    }

    fn reconcile_detach(
        &self,
        mut action: ContainmentAction,
        current_time_ns: u64,
    ) -> Result<(), ContainmentError> {
        let claimed_at_ns = action.updated_at_ns;
        match self.enforcer.detach(action.binding_id) {
            Ok(()) => {
                action.lifecycle_state = ContainmentLifecycle::Expired;
                action.failure_stage = None;
                action.failure_reason = None;
                action.next_retry_at_ns = None;
                self.finish_claimed(&action, ContainmentLifecycle::Expiring, claimed_at_ns)
            }
            Err(message) => self.record_detach_failure(
                &mut action,
                ContainmentLifecycle::Expiring,
                claimed_at_ns,
                current_time_ns,
                sanitize_failure(&message),
            ),
        }
    }

    fn record_detach_failure(
        &self,
        action: &mut ContainmentAction,
        claimed_lifecycle: ContainmentLifecycle,
        claimed_at_ns: u64,
        current_time_ns: u64,
        reason: String,
    ) -> Result<(), ContainmentError> {
        action.attempt_count = action.attempt_count.saturating_add(1);
        action.failure_stage = Some(ContainmentFailureStage::Detach);
        action.failure_reason = Some(reason);
        if action.attempt_count > DETACH_MAX_RETRIES {
            action.lifecycle_state = ContainmentLifecycle::Failed;
            action.next_retry_at_ns = None;
        } else {
            action.lifecycle_state = ContainmentLifecycle::Expiring;
            action.next_retry_at_ns =
                Some(current_time_ns.saturating_add(detach_retry_delay_ns(action.attempt_count)));
        }
        self.finish_claimed(action, claimed_lifecycle, claimed_at_ns)
    }

    fn finish_claimed(
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
        Err(SecurityStoreError::InvalidData(format!(
            "containment action {} changed while reconciliation was in progress",
            action.action_id
        ))
        .into())
    }
}

fn exact_binding(bindings: &[Binding], binding_id: uuid::Uuid) -> Result<Option<Binding>, ()> {
    let mut matching = bindings
        .iter()
        .filter(|binding| binding.request.binding_id == binding_id);
    let first = matching.next().cloned();
    if matching.next().is_some() {
        return Err(());
    }
    Ok(first)
}

fn detach_retry_delay_ns(attempt_count: u32) -> u64 {
    let shift = attempt_count.saturating_sub(1).min(4);
    SECOND_NS.saturating_mul(1_u64 << shift)
}

fn sleep_until_stopped(stop: &std::sync::atomic::AtomicBool, duration: Duration) {
    let mut remaining = duration;
    while !stop.load(Ordering::Acquire) && !remaining.is_zero() {
        let step = remaining.min(Duration::from_millis(50));
        thread::sleep(step);
        remaining = remaining.saturating_sub(step);
    }
}
