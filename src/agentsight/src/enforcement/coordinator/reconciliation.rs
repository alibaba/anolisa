//! Desired-state reconciliation after an enforcer reconnects.

use std::collections::HashMap;

use agentsight_enforcement_protocol::{ApplyPolicy, Binding, BindingState};
use uuid::Uuid;

use super::{EnforcementClient, EnforcementCoordinatorError, EnforcementError, EnforcementStore};

const MAX_OPERATOR_MESSAGE_CHARS: usize = 512;
const REJECTION_FALLBACK: &str = "enforcer rejected desired binding without operator-safe detail";

/// Operations used to reconcile one persisted desired-state generation.
pub(super) trait DesiredStateClient {
    /// Lists bindings currently acknowledged by the enforcer.
    fn bindings(&self) -> Result<Vec<Binding>, EnforcementError>;

    /// Applies one missing desired binding.
    fn apply(&self, request: ApplyPolicy) -> Result<Binding, EnforcementError>;

    /// Detaches one actual binding that is not desired.
    fn detach(&self, binding_id: Uuid) -> Result<(), EnforcementError>;
}

impl DesiredStateClient for EnforcementClient {
    fn bindings(&self) -> Result<Vec<Binding>, EnforcementError> {
        EnforcementClient::bindings(self)
    }

    fn apply(&self, request: ApplyPolicy) -> Result<Binding, EnforcementError> {
        EnforcementClient::apply(self, request)
    }

    fn detach(&self, binding_id: Uuid) -> Result<(), EnforcementError> {
        EnforcementClient::detach(self, binding_id)
    }
}

/// Reconciles persisted desired bindings against one backend snapshot.
///
/// A typed remote apply rejection is terminal for only that binding. Transport,
/// protocol, response-correlation, and persistence errors abort the generation.
///
/// # Errors
///
/// Returns an enforcer or persistence error that invalidates the generation.
pub(super) fn reconcile_desired_state<C: DesiredStateClient + ?Sized>(
    client: &C,
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

fn persist_reconciled_apply(
    store: &EnforcementStore,
    request: ApplyPolicy,
    result: Result<Binding, EnforcementError>,
) -> Result<(), EnforcementCoordinatorError> {
    match result {
        Ok(acknowledged) => store.upsert_binding(&acknowledged)?,
        Err(EnforcementError::Remote { code, message }) => {
            store.upsert_binding(&remote_failure_binding(request, &code, &message))?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

/// Builds the bounded operator-safe failed state for a typed remote rejection.
pub(super) fn remote_failure_binding(request: ApplyPolicy, code: &str, message: &str) -> Binding {
    Binding {
        request,
        state: BindingState::Failed,
        message: Some(remote_rejection_message(code, message)),
        domain_id: None,
    }
}

fn remote_rejection_message(code: &str, _message: &str) -> String {
    let operator_message = match code {
        "binding_conflict" => "enforcer rejected desired binding: binding conflict",
        "missing_binding" => "enforcer rejected desired binding: binding is missing",
        "stale_process" => "enforcer rejected desired binding: process identity is stale",
        "compile_failure" => "enforcer rejected desired binding: policy compilation failed",
        "kernel_failure" => "enforcer rejected desired binding: kernel attachment failed",
        _ => REJECTION_FALLBACK,
    };
    sanitize_operator_message(operator_message)
}

fn sanitize_operator_message(message: &str) -> String {
    let sanitized: String = message
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(MAX_OPERATOR_MESSAGE_CHARS)
        .collect();
    let sanitized = sanitized.trim();
    if sanitized.is_empty() {
        REJECTION_FALLBACK.into()
    } else {
        sanitized.into()
    }
}

fn is_active_desired(state: BindingState) -> bool {
    matches!(
        state,
        BindingState::Pending | BindingState::Enforced | BindingState::Degraded
    )
}

#[cfg(test)]
#[path = "reconciliation_tests.rs"]
mod tests;
