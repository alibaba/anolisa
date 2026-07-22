//! Bounded restart recovery and expiry handling for containment actions.

use agentsight_enforcement_protocol::Binding;

use crate::security::store::DueContainmentAction;

use super::{
    ContainmentAction, ContainmentActivationResult, ContainmentCoordinator, ContainmentEnforcer,
    ContainmentEnforcerError, ContainmentError, ContainmentFailureStage, ContainmentLifecycle,
    RiskCaseStatus, SecurityStore, SecurityStoreError, acknowledgement_matches, enforce_request,
    resolve_policy, sanitize_failure, validate_process_identity,
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
        reconcile_batch(&self.store, self.enforcer.as_ref(), current_time_ns)
    }
}

pub(super) fn reconcile_batch(
    store: &SecurityStore,
    enforcer: &dyn ContainmentEnforcer,
    current_time_ns: u64,
) -> Result<(), ContainmentError> {
    Reconciler { store, enforcer }.reconcile_once(current_time_ns)
}

struct Reconciler<'a> {
    store: &'a SecurityStore,
    enforcer: &'a dyn ContainmentEnforcer,
}

impl Reconciler<'_> {
    fn reconcile_once(&self, current_time_ns: u64) -> Result<(), ContainmentError> {
        let candidates = self
            .store
            .due_containment_candidates(current_time_ns, DUE_BATCH_LIMIT)?;
        let mut first_error = None;
        let mut corrupt_count = 0;
        for candidate in candidates {
            let action = match candidate {
                DueContainmentAction::Valid(action) => action,
                DueContainmentAction::Corrupt { action_key, reason } => {
                    corrupt_count += 1;
                    if let Err(error) = self.store.quarantine_containment_action(
                        &action_key,
                        &reason,
                        current_time_ns,
                    ) && first_error.is_none()
                    {
                        first_error = Some(error.into());
                    }
                    continue;
                }
            };
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
        if corrupt_count > 0 {
            return Err(ContainmentError::CorruptActions {
                count: corrupt_count,
            });
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
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
            Err(ContainmentEnforcerError::Unavailable(message)) => {
                return self.record_pending_unavailable(
                    &mut action,
                    claimed_at_ns,
                    current_time_ns,
                    message,
                );
            }
            Err(ContainmentEnforcerError::Rejected(message)) => {
                return self.fail_pending(action, claimed_at_ns, &message, false, current_time_ns);
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
        if action
            .expires_at_ns
            .is_some_and(|expires_at_ns| expires_at_ns <= current_time_ns)
        {
            if exact.is_some() {
                return self.expire_pending_binding(action, claimed_at_ns, current_time_ns);
            }
            action.lifecycle_state = ContainmentLifecycle::Expired;
            action.failure_stage = None;
            action.failure_reason = None;
            action.next_retry_at_ns = None;
            return self.finish_claimed(&action, ContainmentLifecycle::Pending, claimed_at_ns);
        }
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
                    Err(ContainmentEnforcerError::Unavailable(message)) => {
                        return self.record_pending_unavailable(
                            &mut action,
                            claimed_at_ns,
                            current_time_ns,
                            message,
                        );
                    }
                    Err(ContainmentEnforcerError::Rejected(message)) => {
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
        match self.store.activate_containment_action(
            action.action_id,
            claimed_at_ns,
            current_time_ns.max(claimed_at_ns.saturating_add(1)),
        ) {
            Ok(ContainmentActivationResult::Activated) => Ok(()),
            Ok(ContainmentActivationResult::CaseIneligible(_)) => self.fail_pending(
                action,
                claimed_at_ns,
                "source case changed eligibility during containment recovery",
                true,
                current_time_ns,
            ),
            Ok(ContainmentActivationResult::LostClaim) => {
                Err(ContainmentError::ClaimLost(action.action_id))
            }
            Err(error) => {
                let reason = format!("transactional recovery activation failed: {error}");
                self.fail_pending(action, claimed_at_ns, &reason, true, current_time_ns)
            }
        }
    }

    fn record_pending_unavailable(
        &self,
        action: &mut ContainmentAction,
        claimed_at_ns: u64,
        current_time_ns: u64,
        reason: String,
    ) -> Result<(), ContainmentError> {
        let reason = sanitize_failure(&reason);
        action.lifecycle_state = ContainmentLifecycle::Pending;
        action.failure_stage = Some(ContainmentFailureStage::Reconcile);
        action.failure_reason = Some(reason.clone());
        action.attempt_count = action.attempt_count.saturating_add(1);
        action.next_retry_at_ns =
            Some(current_time_ns.saturating_add(retry_delay_ns(action.attempt_count)));
        self.finish_claimed(action, ContainmentLifecycle::Pending, claimed_at_ns)?;
        Err(ContainmentError::Enforcer(reason))
    }

    fn expire_pending_binding(
        &self,
        action: ContainmentAction,
        claimed_at_ns: u64,
        current_time_ns: u64,
    ) -> Result<(), ContainmentError> {
        let Some(mut cleanup) = self.store.begin_containment_cleanup(
            &action,
            ContainmentLifecycle::Pending,
            claimed_at_ns,
            current_time_ns,
            "expired pending containment binding requires detachment".into(),
        )?
        else {
            return Err(ContainmentError::ClaimLost(action.action_id));
        };
        let cleanup_claim_ns = cleanup.updated_at_ns;
        match self.enforcer.detach(action.binding_id) {
            Ok(()) => {
                cleanup.lifecycle_state = ContainmentLifecycle::Expired;
                cleanup.failure_stage = None;
                cleanup.failure_reason = None;
                cleanup.next_retry_at_ns = None;
                self.finish_claimed(&cleanup, ContainmentLifecycle::Expiring, cleanup_claim_ns)
            }
            Err(message) => self.record_detach_failure(
                &mut cleanup,
                ContainmentLifecycle::Expiring,
                cleanup_claim_ns,
                current_time_ns,
                sanitize_failure(&message),
            ),
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
            let Some(mut cleanup) = self.store.begin_containment_cleanup(
                &action,
                ContainmentLifecycle::Pending,
                claimed_at_ns,
                current_time_ns,
                reason.clone(),
            )?
            else {
                return Err(ContainmentError::ClaimLost(action.action_id));
            };
            let cleanup_claim_ns = cleanup.updated_at_ns;
            return match self.enforcer.detach(action.binding_id) {
                Ok(()) => {
                    cleanup.lifecycle_state = ContainmentLifecycle::Failed;
                    cleanup.failure_stage = Some(ContainmentFailureStage::Reconcile);
                    cleanup.failure_reason = Some(reason.clone());
                    cleanup.next_retry_at_ns = None;
                    self.finish_claimed(
                        &cleanup,
                        ContainmentLifecycle::Expiring,
                        cleanup_claim_ns,
                    )?;
                    Err(ContainmentError::RecoveryFailed {
                        action_id: cleanup.action_id,
                        reason,
                    })
                }
                Err(message) => {
                    let detach_reason =
                        sanitize_failure(&format!("{reason}; detach failed: {message}"));
                    self.record_detach_failure(
                        &mut cleanup,
                        ContainmentLifecycle::Expiring,
                        cleanup_claim_ns,
                        current_time_ns,
                        detach_reason.clone(),
                    )?;
                    Err(ContainmentError::CleanupRequired {
                        action_id: cleanup.action_id,
                        binding_id: cleanup.binding_id,
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
                Some(current_time_ns.saturating_add(retry_delay_ns(action.attempt_count)));
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
        Err(ContainmentError::ClaimLost(action.action_id))
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

fn retry_delay_ns(attempt_count: u32) -> u64 {
    let shift = attempt_count.saturating_sub(1).min(4);
    SECOND_NS.saturating_mul(1_u64 << shift)
}
