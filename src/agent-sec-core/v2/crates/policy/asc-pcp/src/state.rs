//! Reconciliation decisions over generic aggregate reads and CAS writes.
use crate::model::PendingWrite;
use crate::{
    AttemptOutcome, BindingStateRepository, BindingStateWrite, Deployment, ExecutionSlot,
    ExpectedBinding, Presence, ReconcileRecord, RetryPolicy, SavedApply, StoreError, TargetRef,
    WriteResult,
};
use asc_foundation_types::ResourceId;
use asc_policy_types::binding::BindingStatus;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Process-local execution ownership and pending outcomes. The composition root
/// MUST share one instance across every reconciler accessing the same store.
/// It is independent of storage and survives replacement of a worker instance.
#[derive(Debug, Default)]
pub struct ReconcileExecution {
    slots: Mutex<BTreeMap<String, Arc<Mutex<ExecutionSlot>>>>,
}
impl ReconcileExecution {
    /// Inspect an existing slot without allocating for unknown Binding IDs.
    /// # Errors
    /// Returns an execution ownership lock failure.
    pub(crate) fn execution_slot(
        &self,
        id: &ResourceId,
    ) -> Result<Option<Arc<Mutex<ExecutionSlot>>>, StoreError> {
        Ok(self
            .slots
            .lock()
            .map_err(|_| StoreError::Unavailable)?
            .get(id.as_str())
            .cloned())
    }
    pub(crate) fn retire(
        &self,
        id: &ResourceId,
        slot: &Arc<Mutex<ExecutionSlot>>,
    ) -> Result<(), StoreError> {
        let mut slots = self.slots.lock().map_err(|_| StoreError::Unavailable)?;
        if slots
            .get(id.as_str())
            .is_some_and(|current| Arc::ptr_eq(current, slot))
        {
            slots.remove(id.as_str());
        }
        Ok(())
    }
    pub(crate) fn slot(&self, id: &ResourceId) -> Result<Arc<Mutex<ExecutionSlot>>, StoreError> {
        Ok(self
            .slots
            .lock()
            .map_err(|_| StoreError::Unavailable)?
            .entry(id.to_string())
            .or_default()
            .clone())
    }
}

pub(crate) struct ReconcileState {
    pub repository: Arc<dyn BindingStateRepository>,
}
impl ReconcileState {
    pub fn read(&self, id: &ResourceId) -> Result<Option<ReconcileRecord>, StoreError> {
        self.repository.get_binding_state(id)
    }
    pub fn claim(
        &self,
        record: &ReconcileRecord,
        now: u64,
        policy: RetryPolicy,
    ) -> Result<Option<ReconcileRecord>, StoreError> {
        let Some(next) = claim(record.clone(), now, policy)? else {
            return Ok(None);
        };
        let write = BindingStateWrite::new(next.clone());
        match self
            .repository
            .compare_exchange_binding_state(record, &write)?
        {
            WriteResult::Applied | WriteResult::AlreadyApplied => Ok(Some(next)),
            WriteResult::Conflict => Ok(None),
        }
    }
    pub fn register(
        &self,
        expected: &ExpectedBinding,
        prepared: Option<&SavedApply>,
        targets: &[TargetRef],
    ) -> Result<bool, StoreError> {
        for _ in 0..16 {
            let Some(record) = self.read(&expected.id)? else {
                return Ok(false);
            };
            let Some(next) = register(record.clone(), expected, prepared, targets)? else {
                return Ok(false);
            };
            let write = BindingStateWrite::new(next);
            if self
                .repository
                .compare_exchange_binding_state(&record, &write)?
                != WriteResult::Conflict
            {
                return Ok(true);
            }
        }
        Err(StoreError::Unavailable)
    }

    pub fn finish(
        &self,
        outcome: &AttemptOutcome,
        completion: &mut Option<PendingWrite>,
    ) -> Result<bool, StoreError> {
        // A conflict is recomputed against the latest aggregate. Bound contention
        // work per call; retain the outcome for a later bookkeeping-only retry.
        for _ in 0..16 {
            if completion.is_none() {
                let Some(record) = self.read(&outcome.expected.id)? else {
                    return Ok(false);
                };
                let (next, matched) = finish(record.clone(), outcome)?;
                *completion = Some(PendingWrite {
                    expected: record,
                    write: if matched && outcome.next_status == BindingStatus::Deleted {
                        BindingStateWrite::delete()
                    } else {
                        BindingStateWrite::new(next)
                    },
                    matched,
                });
            }
            let pending = completion.as_ref().ok_or(StoreError::Invalid)?;
            match self
                .repository
                .compare_exchange_binding_state(&pending.expected, &pending.write)?
            {
                WriteResult::Applied => {
                    let matched = pending.matched;
                    *completion = None;
                    return Ok(matched);
                }
                // Recheck the latest intent on the next call; never replay a
                // removed Absent target or overwrite a newer PAP write.
                WriteResult::AlreadyApplied => {
                    let removed = pending.write.next.is_none();
                    *completion = None;
                    return Ok(removed);
                }
                WriteResult::Conflict => *completion = None,
            }
        }
        Err(StoreError::Unavailable)
    }
}

fn claim(
    mut record: ReconcileRecord,
    now: u64,
    policy: RetryPolicy,
) -> Result<Option<ReconcileRecord>, StoreError> {
    crate::retry::validate(policy)?;
    if record.runtime.next_attempt_at.is_some_and(|at| at > now) {
        return Ok(None);
    }
    let Ok(running) = record.binding.status.start_reconcile() else {
        return Ok(None);
    };
    let policy = record.runtime.retry_policy.unwrap_or(policy);
    crate::retry::validate(policy)?;
    if record.runtime.attempts_started >= policy.max_attempts {
        return Ok(None);
    }
    record.binding.status = running;
    record.runtime.retry_policy = Some(policy);
    record.runtime.attempts_started += 1;
    record.runtime.next_attempt_at = None;
    Ok(Some(record))
}
fn register(
    mut record: ReconcileRecord,
    expected: &ExpectedBinding,
    prepared: Option<&SavedApply>,
    targets: &[TargetRef],
) -> Result<Option<ReconcileRecord>, StoreError> {
    if !expected.matches(&record.binding) {
        return Ok(None);
    }
    if !expected.status.is_reconciling() {
        return Err(StoreError::Invalid);
    }
    if let Some(saved) = prepared {
        if expected.status != BindingStatus::Applying
            || saved.revision != expected.revision
            || record
                .runtime
                .prepared
                .as_ref()
                .is_some_and(|p| p.revision == saved.revision && p != saved)
        {
            return Err(StoreError::Invalid);
        }
        record.runtime.prepared = Some(saved.clone());
    }
    for (index, target) in targets.iter().enumerate() {
        if targets[..index].iter().any(|t| t.same_identity(target)) {
            return Err(StoreError::Invalid);
        }
        let new_target = prepared.is_some_and(|p| p.prepared.target == *target);
        if let Some(deployment) = record
            .deployments
            .iter_mut()
            .find(|d| d.target.same_identity(target))
        {
            if !new_target && deployment.target != *target {
                return Err(StoreError::Invalid);
            }
            deployment.presence = Presence::Unknown;
            if new_target {
                deployment.target = target.clone();
                deployment.revision = expected.revision;
            }
        } else if new_target {
            record.deployments.push(Deployment {
                target: target.clone(),
                revision: expected.revision,
                presence: Presence::Unknown,
                last_confirmed: None,
            });
        } else {
            return Err(StoreError::Invalid);
        }
    }
    if prepared.is_some_and(|p| !targets.contains(&p.prepared.target)) {
        return Err(StoreError::Invalid);
    }
    record
        .deployments
        .sort_by(|a, b| (&a.target.route, &a.target.id).cmp(&(&b.target.route, &b.target.id)));
    Ok(Some(record))
}
fn finish(
    mut record: ReconcileRecord,
    outcome: &AttemptOutcome,
) -> Result<(ReconcileRecord, bool), StoreError> {
    if !outcome.expected.status.is_reconciling() {
        return Err(StoreError::Invalid);
    }
    outcome
        .expected
        .status
        .validate_successor(outcome.next_status)
        .map_err(|_| StoreError::Invalid)?;
    let pending = matches!(
        outcome.next_status,
        BindingStatus::PendingApply | BindingStatus::PendingDelete
    );
    let success = matches!(
        outcome.next_status,
        BindingStatus::Ready | BindingStatus::Deleted
    );
    if outcome.next_status == outcome.expected.status
        || pending != outcome.next_attempt_at.is_some()
        || success != outcome.error.is_none()
    {
        return Err(StoreError::Invalid);
    }
    // Validate all evidence before touching the live state. Even a stale
    // lifecycle may only report registered, not newer, target identities.
    for (index, observation) in outcome.observations.iter().enumerate() {
        if outcome.observations[..index]
            .iter()
            .any(|o| o.target.same_identity(&observation.target))
            || !record.deployments.iter().any(|d| {
                d.target == observation.target
                    && d.revision.get() <= outcome.expected.revision.get()
            })
        {
            return Err(StoreError::Invalid);
        }
    }
    for observation in &outcome.observations {
        if observation.presence == Presence::Absent {
            record
                .deployments
                .retain(|d| !d.target.same_identity(&observation.target));
        } else if let Some(deployment) = record
            .deployments
            .iter_mut()
            .find(|d| d.target.same_identity(&observation.target))
        {
            deployment.presence = observation.presence;
            if observation.presence == Presence::Present {
                deployment.last_confirmed = Some(Presence::Present);
            }
        }
    }
    let matches = outcome.expected.matches(&record.binding);
    if matches {
        if outcome.next_status == BindingStatus::Deleted {
            if !record.deployments.is_empty() {
                return Err(StoreError::Invalid);
            }
            // The caller atomically removes the aggregate. There is no Deleted
            // snapshot to persist, but all evidence must still be validated.
            return Ok((record, true));
        }
        if outcome.next_status == BindingStatus::Ready
            && !record.runtime.prepared.as_ref().is_some_and(|saved| {
                saved.revision == outcome.expected.revision
                    && record.deployments.len() == 1
                    && record.deployments[0].target == saved.prepared.target
                    && record.deployments[0].presence == Presence::Present
            })
        {
            return Err(StoreError::Invalid);
        }
        record.binding.status = outcome.next_status;
        record.runtime.next_attempt_at = outcome.next_attempt_at;
        record.runtime.last_error.clone_from(&outcome.error);
    }
    Ok((record, matches))
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
