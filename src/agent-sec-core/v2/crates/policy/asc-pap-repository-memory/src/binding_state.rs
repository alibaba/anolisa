use crate::{BindingStateData, ProcessLocalPapRepository, State};
use asc_foundation_types::ResourceId;
use asc_policy_repository::{
    BindingStateRepository, BindingStateSnapshot, BindingStateWrite, StoreError, WriteResult,
};
use asc_policy_types::{error::Validate, target::Presence};
use std::sync::Mutex;

impl ProcessLocalPapRepository {
    /// Seeds complete records for local composition/contract tests. This is not
    /// a disk loader or evidence of crash recovery. Duplicate identities fail.
    /// # Errors
    /// Returns invalid on malformed records or duplicate target identities.
    pub fn with_binding_states(records: Vec<BindingStateSnapshot>) -> Result<Self, StoreError> {
        let mut state = State::default();
        for record in records {
            record.binding.validate().map_err(|_| StoreError::Invalid)?;
            for (index, deployment) in record.deployments.iter().enumerate() {
                if deployment.presence == Presence::Absent
                    || record.deployments[..index]
                        .iter()
                        .any(|d| d.target.same_identity(&deployment.target))
                {
                    return Err(StoreError::Invalid);
                }
            }
            let id = record.binding.spec.binding_id.to_string();
            if state
                .bindings
                .insert(id.clone(), record.binding.clone())
                .is_some()
            {
                return Err(StoreError::Invalid);
            }
            state.binding_states.insert(
                id,
                BindingStateData {
                    runtime: record.runtime,
                    deployments: record.deployments,
                    last_write: None,
                },
            );
        }
        Ok(Self {
            state: Mutex::new(state),
        })
    }
}

impl BindingStateRepository for ProcessLocalPapRepository {
    fn get_binding_state(
        &self,
        id: &ResourceId,
    ) -> Result<Option<BindingStateSnapshot>, StoreError> {
        let state = self.state.lock().map_err(|_| StoreError::Unavailable)?;
        Ok(snapshot(&state, id.as_str()))
    }
    fn compare_exchange_binding_state(
        &self,
        expected: &BindingStateSnapshot,
        write: &BindingStateWrite,
    ) -> Result<WriteResult, StoreError> {
        let id = expected.binding.spec.binding_id.as_str();
        if let Some(next) = &write.next {
            if next.binding.spec.binding_id != expected.binding.spec.binding_id {
                return Err(StoreError::Invalid);
            }
            next.binding.validate().map_err(|_| StoreError::Invalid)?;
        }
        let mut state = self.state.lock().map_err(|_| StoreError::Unavailable)?;
        if let Some(last) = state
            .binding_states
            .get(id)
            .and_then(|r| r.last_write.as_ref())
            && last.write_id == write.write_id
        {
            return if last == write {
                Ok(WriteResult::AlreadyApplied)
            } else {
                Err(StoreError::Invalid)
            };
        }
        let current = snapshot(&state, id);
        if current.is_none() && write.next.is_none() {
            return Ok(WriteResult::AlreadyApplied);
        }
        if current.as_ref() != Some(expected) {
            return Ok(WriteResult::Conflict);
        }
        if let Some(next) = &write.next {
            save(&mut state, next.clone()).last_write = Some(write.clone());
        } else {
            state.bindings.remove(id);
            state.binding_states.remove(id);
        }
        Ok(WriteResult::Applied)
    }
}
fn snapshot(state: &State, id: &str) -> Option<BindingStateSnapshot> {
    let binding = state.bindings.get(id)?.clone();
    let data = state.binding_states.get(id);
    Some(BindingStateSnapshot {
        binding,
        runtime: data.map(|d| d.runtime.clone()).unwrap_or_default(),
        deployments: data.map(|d| d.deployments.clone()).unwrap_or_default(),
    })
}

fn save(state: &mut State, record: BindingStateSnapshot) -> &mut BindingStateData {
    let id = record.binding.spec.binding_id.to_string();
    state.bindings.insert(id.clone(), record.binding);
    let data = state.binding_states.entry(id).or_default();
    data.runtime = record.runtime;
    data.deployments = record.deployments;
    data
}
