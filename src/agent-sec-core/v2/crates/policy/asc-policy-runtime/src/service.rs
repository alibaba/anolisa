use std::sync::Arc;

use asc_foundation_types::{ResourceId, Revision};
use asc_pap::PapRepository;
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::adapter::PolicyAdapter;
use crate::error::RuntimeError;
use crate::model::{BindingDesiredState, OperationState, PreparedBinding, ReconcileOperation};
use crate::repository::RuntimeRepository;

/// Binding application service.
pub struct PolicyRuntime<P, R, A> {
    pap_repository: Arc<P>,
    runtime_repository: Arc<R>,
    adapter: Arc<A>,
}

impl<P, R, A> Clone for PolicyRuntime<P, R, A> {
    fn clone(&self) -> Self {
        Self {
            pap_repository: Arc::clone(&self.pap_repository),
            runtime_repository: Arc::clone(&self.runtime_repository),
            adapter: Arc::clone(&self.adapter),
        }
    }
}

impl<P, R, A> PolicyRuntime<P, R, A>
where
    P: PapRepository,
    R: RuntimeRepository,
    A: PolicyAdapter,
{
    /// Creates the runtime around explicit ports.
    pub fn new(pap_repository: Arc<P>, runtime_repository: Arc<R>, adapter: Arc<A>) -> Self {
        Self {
            pap_repository,
            runtime_repository,
            adapter,
        }
    }

    /// Resolves immutable PAP revisions into a complete Binding snapshot and persists it.
    ///
    /// # Errors
    /// Returns reference, not-found, revision-allocation, or persistence errors.
    pub fn put_binding(
        &self,
        binding_id: Option<&ResourceId>,
        policy_id: &ResourceId,
        policy_revision: Revision,
        scope_id: &ResourceId,
        scope_revision: Revision,
    ) -> Result<ReconcileOperation, RuntimeError> {
        let policy = self
            .pap_repository
            .get_policy(policy_id, policy_revision)
            .map_err(RuntimeError::Pap)?;
        let scope = self
            .pap_repository
            .get_scope(scope_id, scope_revision)
            .map_err(RuntimeError::Pap)?;
        let update_existing = binding_id.is_some();
        let mut selected_id = binding_id.cloned().unwrap_or_else(generated_resource_id);

        loop {
            let current = match self.runtime_repository.get_binding(&selected_id) {
                Ok(current) => Some(current),
                Err(RuntimeError::NotFound) => None,
                Err(error) => return Err(error),
            };
            if update_existing && current.is_none() {
                return Err(RuntimeError::NotFound);
            }
            if !update_existing && current.is_some() {
                selected_id = generated_resource_id();
                continue;
            }
            let binding_revision = next_revision(current.as_ref())?;
            let binding = PreparedBinding {
                binding_id: selected_id.clone(),
                binding_revision,
                policy: policy.clone(),
                scope: scope.clone(),
                desired_state: BindingDesiredState::Ready,
            };
            let operation = operation_for(&binding)?;
            let expected = current.as_ref().map(|value| value.binding_revision);
            match self
                .runtime_repository
                .accept_binding(&binding, &operation, expected)
            {
                Err(RuntimeError::PreconditionFailed) if !update_existing => {
                    selected_id = generated_resource_id();
                }
                Err(RuntimeError::PreconditionFailed | RuntimeError::IdempotencyConflict) => {}
                result => return result,
            }
        }
    }

    /// Atomically accepts a Binding removal operation using the last stored snapshot.
    ///
    /// # Errors
    /// Returns not-found, revision-allocation, or persistence errors.
    pub fn delete_binding(
        &self,
        binding_id: &ResourceId,
    ) -> Result<ReconcileOperation, RuntimeError> {
        loop {
            let mut binding = self.runtime_repository.get_binding(binding_id)?;
            let expected = binding.binding_revision;
            binding.binding_revision = next_revision(Some(&binding))?;
            binding.desired_state = BindingDesiredState::Absent;
            let operation = operation_for(&binding)?;
            match self
                .runtime_repository
                .accept_binding(&binding, &operation, Some(expected))
            {
                Err(RuntimeError::PreconditionFailed | RuntimeError::IdempotencyConflict) => {}
                result => return result,
            }
        }
    }

    /// Dispatches at most one queued operation.
    ///
    /// # Errors
    /// Returns persistence errors. Adapter failures are persisted as operation states.
    pub fn dispatch_once(&self) -> Result<Option<ReconcileOperation>, RuntimeError> {
        let Some(command) = self.runtime_repository.claim_next()? else {
            return Ok(None);
        };
        let outcome = self.adapter.submit(&command);
        self.runtime_repository
            .finish_dispatch(&command.operation_id, outcome)
            .map(Some)
    }

    /// Returns the runtime repository for query use cases.
    pub fn repository(&self) -> &R {
        &self.runtime_repository
    }
}

fn digest<T: Serialize>(value: &T) -> Result<String, RuntimeError> {
    let bytes = serde_json::to_vec(value).map_err(RuntimeError::Serialization)?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn generated_resource_id() -> ResourceId {
    ResourceId::new(Uuid::new_v4().to_string()).expect("UUID is a valid resource id")
}

fn next_revision(current: Option<&PreparedBinding>) -> Result<Revision, RuntimeError> {
    let value = current.map_or(1, |binding| {
        binding.binding_revision.get().checked_add(1).unwrap_or(0)
    });
    Revision::new(value).map_err(|_| RuntimeError::PreconditionFailed)
}

fn operation_for(binding: &PreparedBinding) -> Result<ReconcileOperation, RuntimeError> {
    Ok(ReconcileOperation {
        operation_id: generated_resource_id(),
        binding_id: binding.binding_id.clone(),
        binding_revision: binding.binding_revision,
        request_digest: digest(binding)?,
        state: OperationState::Queued,
        stage: "dispatch_adapter".to_owned(),
        error_code: None,
    })
}
