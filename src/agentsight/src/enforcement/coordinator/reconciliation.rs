//! Desired-state reconciliation after an enforcer reconnects.

use std::collections::HashMap;

use agentsight_enforcement_protocol::{ApplyPolicy, Binding, BindingState};

use super::{EnforcementClient, EnforcementCoordinatorError, EnforcementError, EnforcementStore};

pub(super) fn reconcile_desired_state(
    client: &EnforcementClient,
    store: &EnforcementStore,
) -> Result<(), EnforcementCoordinatorError> {
    let desired = store.bindings()?;
    let actual = client.bindings()?;
    let desired_by_id: HashMap<_, _> = desired
        .iter()
        .map(|binding| (binding.request.binding_id, binding))
        .collect();
    let mut retained_actual = HashMap::new();

    for binding in actual {
        let binding_id = binding.request.binding_id;
        let matches_active_desired = desired_by_id.get(&binding_id).is_some_and(|desired| {
            is_active_desired(desired.state) && desired.request == binding.request
        });
        if matches_active_desired {
            retained_actual.insert(binding_id, binding);
        } else {
            client.detach(binding_id)?;
        }
    }

    for mut binding in desired {
        let binding_id = binding.request.binding_id;
        if is_active_desired(binding.state) {
            match retained_actual.remove(&binding_id) {
                Some(actual) => store.upsert_binding(&actual)?,
                None => persist_reconciled_apply(
                    store,
                    binding.request.clone(),
                    client.apply(binding.request.clone()),
                )?,
            }
        } else if binding.state == BindingState::Detaching {
            binding.state = BindingState::Detached;
            binding.message = None;
            binding.domain_id = None;
            store.upsert_binding(&binding)?;
        }
    }
    Ok(())
}

pub(super) fn persist_reconciled_apply(
    store: &EnforcementStore,
    request: ApplyPolicy,
    result: Result<Binding, EnforcementError>,
) -> Result<(), EnforcementCoordinatorError> {
    match result {
        Ok(acknowledged) => store.upsert_binding(&acknowledged)?,
        Err(EnforcementError::Remote { code, message }) => {
            store.upsert_binding(&Binding {
                request,
                state: BindingState::Failed,
                message: Some(format!("enforcer rejected request ({code}): {message}")),
                domain_id: None,
            })?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn is_active_desired(state: BindingState) -> bool {
    matches!(
        state,
        BindingState::Pending | BindingState::Enforced | BindingState::Degraded
    )
}
