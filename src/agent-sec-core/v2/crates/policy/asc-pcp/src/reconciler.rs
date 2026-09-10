use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;

use asc_foundation_types::ResourceId;
use asc_policy_types::binding::BindingStatus;
use asc_policy_types::target::TranslationOutcome;

use crate::{
    AttemptOutcome, BindingStateRepository, Clock, DeploymentReport, Disposition, ExecutionSlot,
    ExpectedBinding, Failure, FailureKind, Observation, Presence, ReconcileExecution,
    ReconcileRecord, RetryPolicy, SavedApply, StoreError, TargetBindingAdapter,
    TargetDeploymentClient, TargetRef,
};

/// Pure orchestration over repository, Adapter and Client ports. Clones/other
/// instances share the execution context supplied by the composition root.
pub struct BindingReconciler {
    state: crate::state::ReconcileState,
    execution: Arc<ReconcileExecution>,
    adapter: Arc<dyn TargetBindingAdapter>,
    clients: BTreeMap<String, Arc<dyn TargetDeploymentClient>>,
    apply_route: String,
    clock: Arc<dyn Clock>,
    retry: RetryPolicy,
}

impl BindingReconciler {
    /// Routes are stable configuration references, never endpoint credentials.
    /// Share one `ReconcileExecution` across all workers for a store; use separate
    /// contexts for separate stores. Pending results survive worker replacement.
    /// # Errors
    /// Rejects invalid retry configuration or a missing Apply Client.
    pub fn new(
        repository: Arc<dyn BindingStateRepository>,
        adapter: Arc<dyn TargetBindingAdapter>,
        clients: BTreeMap<String, Arc<dyn TargetDeploymentClient>>,
        apply_route: String,
        clock: Arc<dyn Clock>,
        retry: RetryPolicy,
        execution: Arc<ReconcileExecution>,
    ) -> Result<Self, StoreError> {
        crate::retry::validate(retry)?;
        if !clients.contains_key(&apply_route) {
            return Err(StoreError::Invalid);
        }
        Ok(Self {
            state: crate::state::ReconcileState { repository },
            execution,
            adapter,
            clients,
            apply_route,
            clock,
            retry,
        })
    }

    /// Executes at most one due attempt, or retries a previously failed result
    /// transaction. The returned retry deadline is for the caller's timer.
    ///
    /// This synchronous call is intentionally not cancellation-safe by dropping
    /// an async wrapper: callers must retain/join their blocking task. No task is
    /// spawned or detached here, and the core execution slot spans the entire call.
    /// # Errors
    /// Storage errors never imply remote failure or success. A failed completion
    /// stays in the shared slot; invoke again to retry bookkeeping without I/O.
    /// # Panics
    /// Resumes a port panic after attempting terminal bookkeeping and releasing
    /// execution ownership. The caller must observe the failed task and update
    /// service health. Unwinding never proves remote absence.
    pub fn reconcile(&self, id: &ResourceId) -> Result<Disposition, StoreError> {
        let shared = if let Some(shared) = self.execution.execution_slot(id)? {
            shared
        } else {
            if self.state.read(id)?.is_none() {
                return Ok(Disposition::Skipped);
            }
            self.execution.slot(id)?
        };
        let mut slot = shared.lock().map_err(|_| StoreError::Unavailable)?;
        // The guard stays outside the unwind boundary. A failed call cannot
        // poison execution ownership or release it before terminal bookkeeping.
        let result = catch_unwind(AssertUnwindSafe(|| self.reconcile_locked(id, &mut slot)));
        match result {
            Ok(result) => {
                if slot.removed && slot.pending.is_none() && slot.completion.is_none() {
                    self.execution.retire(id, &shared)?;
                }
                result
            }
            Err(payload) => {
                // Preserve an actual completion over the provisional panic
                // failure. If storage fails too, retain it for bookkeeping-only
                // retry. The repository must maintain its atomicity on unwind.
                let _ = catch_unwind(AssertUnwindSafe(|| {
                    if let Some(outcome) = slot.pending.clone() {
                        let disposition = self.commit(&outcome, &mut slot.completion)?;
                        slot.removed = outcome.next_status == BindingStatus::Deleted
                            && disposition == Disposition::Completed;
                        slot.pending = None;
                    }
                    Ok::<_, StoreError>(())
                }));
                if slot.removed && slot.pending.is_none() && slot.completion.is_none() {
                    let _ = self.execution.retire(id, &shared);
                }
                drop(slot);
                // The owner still observes task failure and updates health.
                resume_unwind(payload)
            }
        }
    }

    fn reconcile_locked(
        &self,
        id: &ResourceId,
        slot: &mut ExecutionSlot,
    ) -> Result<Disposition, StoreError> {
        if let Some(outcome) = slot.pending.as_ref() {
            let disposition = self.commit(outcome, &mut slot.completion)?;
            slot.removed = outcome.next_status == BindingStatus::Deleted
                && disposition == Disposition::Completed;
            slot.pending = None;
            return Ok(disposition);
        }
        let Some(record) = self.state.read(id)? else {
            slot.removed = true;
            return Ok(Disposition::Skipped);
        };
        if !matches!(
            record.binding.status,
            BindingStatus::PendingApply | BindingStatus::PendingDelete
        ) || record
            .runtime
            .next_attempt_at
            .is_some_and(|at| at > self.clock.now_ms())
        {
            return Ok(Disposition::Skipped);
        }
        let mut expected = ExpectedBinding::from_binding(&record.binding);
        expected.status = expected
            .status
            .start_reconcile()
            .map_err(|_| StoreError::Invalid)?;
        // Install before claim: a repository wrapper may panic after committing
        // the claim but before returning it. CAS prevents failing unclaimed work.
        slot.pending = Some(AttemptOutcome {
            next_status: expected
                .status
                .fail_reconcile()
                .map_err(|_| StoreError::Invalid)?,
            expected,
            observations: vec![],
            next_attempt_at: None,
            error: Some(Failure::new(
                FailureKind::Rejected,
                "RECONCILE_WORKER_PANICKED",
            )),
        });
        let claimed = self.state.claim(&record, self.clock.now_ms(), self.retry);
        let claimed = match claimed {
            Ok(Some(claimed)) => claimed,
            other => {
                slot.pending = None;
                return other.map(|_| Disposition::Superseded);
            }
        };
        let report = match self.execute(&claimed) {
            Ok(Some(report)) => report,
            Ok(None) => {
                slot.pending = None;
                return Ok(Disposition::Superseded);
            }
            Err(error) => {
                // No modifying call follows a failed registration. Keep the
                // claimed attempt recoverable without leaving APPLYING wedged.
                slot.pending = Some(self.outcome(
                    &claimed,
                    DeploymentReport {
                        observations: vec![],
                        error: Some(retryable("RECONCILE_STORAGE_ERROR")),
                    },
                ));
                return Err(error);
            }
        };
        slot.pending = Some(self.outcome(&claimed, report));
        let disposition = self.commit(
            slot.pending.as_ref().ok_or(StoreError::Invalid)?,
            &mut slot.completion,
        )?;
        slot.removed = slot.pending.as_ref().is_some_and(|outcome| {
            outcome.next_status == BindingStatus::Deleted && disposition == Disposition::Completed
        });
        slot.pending = None;
        Ok(disposition)
    }

    fn execute(&self, record: &ReconcileRecord) -> Result<Option<DeploymentReport>, StoreError> {
        if record.binding.status == BindingStatus::Deleting {
            return self.delete(record);
        }
        let saved = match self.prepare(record) {
            Ok(saved) => saved,
            Err(error) => {
                return Ok(Some(DeploymentReport {
                    observations: vec![],
                    error: Some(error),
                }));
            }
        };
        let target = &saved.prepared.target;
        let Some(client) = self.clients.get(&target.route) else {
            return Ok(Some(failed("RECONCILE_TARGET_UNAVAILABLE")));
        };
        let previous: Vec<_> = record
            .deployments
            .iter()
            .filter(|d| !d.target.same_identity(target))
            .map(|d| d.target.clone())
            .collect();
        // Cross-route migration needs an explicit multi-PEP contract. Never
        // reinterpret old cleanup bytes with the newly configured Client.
        if previous.iter().any(|old| old.route != target.route) {
            return Ok(Some(failed("RECONCILE_TARGET_UNAVAILABLE")));
        }
        let mut touched = previous.clone();
        touched.push(target.clone());
        if !self.state.register(
            &ExpectedBinding::from_binding(&record.binding),
            Some(&saved),
            &touched,
        )? {
            return Ok(None);
        }
        let report = if saved.is_update {
            client.update(&previous, &saved.prepared)
        } else {
            client.create(&saved.prepared)
        };
        let required: Vec<_> = touched
            .iter()
            .map(|t| Observation {
                target: t.clone(),
                presence: if t.same_identity(target) {
                    Presence::Present
                } else {
                    Presence::Absent
                },
            })
            .collect();
        Ok(Some(validate_report(report, &required)))
    }

    fn prepare(&self, record: &ReconcileRecord) -> Result<SavedApply, Failure> {
        if let Some(saved) = &record.runtime.prepared
            && saved.revision == record.binding.spec.binding_revision
        {
            return Ok(saved.clone());
        }
        let plan = match self.adapter.translate(&record.binding.spec) {
            Ok(TranslationOutcome::Translated(plan)) => plan,
            Ok(TranslationOutcome::Rejected(rejection)) => {
                return Err(Failure::new(FailureKind::Rejected, &rejection.code));
            }
            Err(fault) => return Err(retryable(&fault.code)),
        };
        let client = self
            .clients
            .get(&self.apply_route)
            .ok_or_else(|| retryable("RECONCILE_TARGET_UNAVAILABLE"))?;
        let prepared = client
            .prepare_apply(&plan)
            .map_err(|error| Failure::new(error.kind, &error.code))?;
        if prepared.target.route != self.apply_route
            || prepared.target.id.is_empty()
            || prepared.format.is_empty()
        {
            return Err(Failure::new(
                FailureKind::Rejected,
                "RECONCILE_INVALID_PREPARED",
            ));
        }
        Ok(SavedApply {
            revision: record.binding.spec.binding_revision,
            is_update: !record.deployments.is_empty(),
            plan,
            prepared,
        })
    }

    fn delete(&self, record: &ReconcileRecord) -> Result<Option<DeploymentReport>, StoreError> {
        let targets: Vec<_> = record
            .deployments
            .iter()
            .map(|d| d.target.clone())
            .collect();
        if !self.state.register(
            &ExpectedBinding::from_binding(&record.binding),
            None,
            &targets,
        )? {
            return Ok(None);
        }
        let mut groups: BTreeMap<&str, Vec<TargetRef>> = BTreeMap::new();
        for target in &targets {
            groups
                .entry(&target.route)
                .or_default()
                .push(target.clone());
        }
        let mut aggregate = DeploymentReport {
            observations: vec![],
            error: None,
        };
        for (route, targets) in groups {
            let report = self.clients.get(route).map_or_else(
                || failed("RECONCILE_TARGET_UNAVAILABLE"),
                |client| client.delete(&targets),
            );
            let required: Vec<_> = targets
                .into_iter()
                .map(|target| Observation {
                    target,
                    presence: Presence::Absent,
                })
                .collect();
            let report = validate_report(report, &required);
            aggregate.observations.extend(report.observations);
            if let Some(error) = report.error
                && (aggregate.error.is_none() || error.kind == FailureKind::Rejected)
            {
                aggregate.error = Some(error);
            }
        }
        Ok(Some(aggregate))
    }

    fn outcome(&self, record: &ReconcileRecord, report: DeploymentReport) -> AttemptOutcome {
        let status = record.binding.status;
        let policy = record.runtime.retry_policy.unwrap_or(self.retry);
        let (next_status, next_attempt_at) = match &report.error {
            None => (status.complete_reconcile(), None),
            Some(error)
                if error.kind == FailureKind::Retryable
                    && record.runtime.attempts_started < policy.max_attempts =>
            {
                (
                    status.retry_reconcile(),
                    Some(self.clock.now_ms().saturating_add(crate::retry::delay(
                        policy,
                        record.runtime.attempts_started,
                    ))),
                )
            }
            Some(_) => (status.fail_reconcile(), None),
        };
        AttemptOutcome {
            expected: ExpectedBinding::from_binding(&record.binding),
            observations: report.observations,
            // Only a successfully claimed running state reaches this function.
            next_status: next_status.unwrap_or(status),
            next_attempt_at,
            error: report.error,
        }
    }

    fn commit(
        &self,
        outcome: &AttemptOutcome,
        completion: &mut Option<crate::model::PendingWrite>,
    ) -> Result<Disposition, StoreError> {
        if !self.state.finish(outcome, completion)? {
            return Ok(Disposition::Superseded);
        }
        if let Some(at) = outcome.next_attempt_at {
            Ok(Disposition::RetryAt { at })
        } else if let Some(error) = &outcome.error {
            Ok(Disposition::Failed {
                error: error.clone(),
            })
        } else {
            Ok(Disposition::Completed)
        }
    }
}

fn retryable(code: &str) -> Failure {
    Failure::new(FailureKind::Retryable, code)
}

fn failed(code: &str) -> DeploymentReport {
    DeploymentReport {
        observations: vec![],
        error: Some(retryable(code)),
    }
}

fn validate_report(mut report: DeploymentReport, required: &[Observation]) -> DeploymentReport {
    for (index, observed) in report.observations.iter().enumerate() {
        if !required.iter().any(|r| r.target == observed.target)
            || report.observations[..index]
                .iter()
                .any(|o| o.target.same_identity(&observed.target))
        {
            // An invalid report provides no trustworthy clearance evidence.
            return failed("RECONCILE_INVALID_REPORT");
        }
    }
    report.error = report.error.map(|e| Failure::new(e.kind, &e.code));
    if report.error.is_none() && required.iter().any(|r| !report.observations.contains(r)) {
        report.error = Some(retryable("RECONCILE_UNCONFIRMED"));
    }
    report
}
