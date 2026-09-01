use std::sync::Arc;

use asc_foundation_types::{ResourceId, Revision};
use asc_pap::{Page, PapRepository, PapService, PreparedPolicy, PreparedScope, ScopeSelector};
use asc_policy_engine::PolicyTemplate;
use asc_policy_runtime::{
    PolicyAdapter, PolicyRuntime, PreparedBinding, ReconcileOperation, RuntimeRepository,
};

use super::PolicyError;

/// Transport-independent Policy administration and Binding use cases.
pub struct PolicyService<S, A> {
    pap: PapService<S>,
    runtime: PolicyRuntime<S, S, A>,
}

impl<S, A> Clone for PolicyService<S, A> {
    fn clone(&self) -> Self {
        Self {
            pap: self.pap.clone(),
            runtime: self.runtime.clone(),
        }
    }
}

impl<S, A> PolicyService<S, A>
where
    S: PapRepository + RuntimeRepository,
    A: PolicyAdapter,
{
    /// Composes one Policy service from explicit ports.
    pub fn new(store: Arc<S>, adapter: Arc<A>) -> Self {
        Self {
            pap: PapService::new(Arc::clone(&store)),
            runtime: PolicyRuntime::new(Arc::clone(&store), store, adapter),
        }
    }

    /// Converges a Policy identity to one complete desired template.
    ///
    /// # Errors
    /// Returns Policy validation, conflict, or persistence errors.
    pub fn put_policy(
        &self,
        id: Option<&ResourceId>,
        name: &str,
        template: &PolicyTemplate,
    ) -> Result<PreparedPolicy, PolicyError> {
        Ok(self.pap.put_policy(id, name, template)?)
    }

    /// Reads one Policy revision.
    ///
    /// # Errors
    /// Returns not-found or persistence errors.
    pub fn get_policy(
        &self,
        id: &ResourceId,
        revision: Revision,
    ) -> Result<PreparedPolicy, PolicyError> {
        Ok(self.pap.repository().get_policy(id, revision)?)
    }

    /// Lists Policy revisions.
    ///
    /// # Errors
    /// Returns a persistence error when the query cannot complete.
    pub fn list_policies(
        &self,
        limit: u32,
        offset: u64,
        max_item_bytes: usize,
    ) -> Result<Page<PreparedPolicy>, PolicyError> {
        Ok(self
            .pap
            .repository()
            .list_policies(limit, offset, max_item_bytes)?)
    }

    /// Deletes one exact Policy revision while preserving all other revisions.
    ///
    /// # Errors
    /// Returns not-found or persistence errors.
    pub fn delete_policy_revision(
        &self,
        id: &ResourceId,
        revision: Revision,
    ) -> Result<PreparedPolicy, PolicyError> {
        Ok(self.pap.repository().delete_policy_revision(id, revision)?)
    }

    /// Converges a Scope identity to one simple selector intent.
    ///
    /// # Errors
    /// Returns Scope validation, conflict, or persistence errors.
    pub fn put_scope(
        &self,
        id: Option<&ResourceId>,
        selector: &ScopeSelector,
    ) -> Result<PreparedScope, PolicyError> {
        Ok(self.pap.put_scope(id, selector)?)
    }

    /// Reads one Scope revision.
    ///
    /// # Errors
    /// Returns not-found or persistence errors.
    pub fn get_scope(
        &self,
        id: &ResourceId,
        revision: Revision,
    ) -> Result<PreparedScope, PolicyError> {
        Ok(self.pap.repository().get_scope(id, revision)?)
    }

    /// Lists Scope revisions.
    ///
    /// # Errors
    /// Returns a persistence error when the query cannot complete.
    pub fn list_scopes(
        &self,
        limit: u32,
        offset: u64,
        max_item_bytes: usize,
    ) -> Result<Page<PreparedScope>, PolicyError> {
        Ok(self
            .pap
            .repository()
            .list_scopes(limit, offset, max_item_bytes)?)
    }

    /// Deletes one exact Scope revision.
    ///
    /// # Errors
    /// Returns not-found or persistence errors.
    pub fn delete_scope_revision(
        &self,
        id: &ResourceId,
        revision: Revision,
    ) -> Result<PreparedScope, PolicyError> {
        Ok(self.pap.repository().delete_scope_revision(id, revision)?)
    }

    /// Accepts a durable Binding revision and returns its user-facing desired state.
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
    ) -> Result<PreparedBinding, PolicyError> {
        let operation = self.runtime.put_binding(
            binding_id,
            policy_id,
            policy_revision,
            scope_id,
            scope_revision,
        )?;
        Ok(self
            .runtime
            .repository()
            .get_binding(&operation.binding_id)?)
    }

    /// Reads one Binding.
    ///
    /// # Errors
    /// Returns not-found or persistence errors.
    pub fn get_binding(&self, id: &ResourceId) -> Result<PreparedBinding, PolicyError> {
        Ok(self.runtime.repository().get_binding(id)?)
    }

    /// Accepts a durable Binding removal revision and returns its desired state.
    ///
    /// # Errors
    /// Returns not-found, revision-allocation, or persistence errors.
    pub fn delete_binding(&self, id: &ResourceId) -> Result<PreparedBinding, PolicyError> {
        let operation = self.runtime.delete_binding(id)?;
        Ok(self
            .runtime
            .repository()
            .get_binding(&operation.binding_id)?)
    }

    /// Lists Bindings.
    ///
    /// # Errors
    /// Returns a persistence error when the query cannot complete.
    pub fn list_bindings(
        &self,
        limit: u32,
        offset: u64,
        max_item_bytes: usize,
    ) -> Result<Page<PreparedBinding>, PolicyError> {
        Ok(self
            .runtime
            .repository()
            .list_bindings(limit, offset, max_item_bytes)?)
    }

    /// Dispatches at most one durable outbox operation through the Adapter port.
    ///
    /// # Errors
    /// Returns Adapter or persistence errors for the claimed operation.
    pub fn dispatch_once(&self) -> Result<Option<ReconcileOperation>, PolicyError> {
        Ok(self.runtime.dispatch_once()?)
    }
}
