use asc_foundation_types::{ResourceId, Revision};
use asc_pap::Page;

use crate::adapter::AdapterDispatchError;
use crate::error::RuntimeError;
use crate::model::{AdapterAccepted, AdapterCommand, PreparedBinding, ReconcileOperation};

/// Runtime persistence boundary. Implementations atomically store Binding snapshots, operations,
/// outbox items, and audit records. They do not resolve or revalidate PAP references.
pub trait RuntimeRepository: Send + Sync {
    /// Creates or idempotently returns a durable operation for an already-resolved Binding.
    ///
    /// # Errors
    /// Returns idempotency, CAS, serialization, or persistence errors.
    fn accept_binding(
        &self,
        binding: &PreparedBinding,
        operation: &ReconcileOperation,
        expected_binding_revision: Option<Revision>,
    ) -> Result<ReconcileOperation, RuntimeError>;
    /// Gets one prepared Binding.
    ///
    /// # Errors
    /// Returns not-found or persistence errors.
    fn get_binding(&self, id: &ResourceId) -> Result<PreparedBinding, RuntimeError>;
    /// Lists prepared Bindings.
    ///
    /// # Errors
    /// Returns a persistence error when the query cannot complete.
    fn list_bindings(
        &self,
        limit: u32,
        offset: u64,
        max_item_bytes: usize,
    ) -> Result<Page<PreparedBinding>, RuntimeError>;
    /// Gets one operation.
    ///
    /// # Errors
    /// Returns not-found or persistence errors.
    fn get_operation(&self, id: &ResourceId) -> Result<ReconcileOperation, RuntimeError>;
    /// Claims the next queued operation and returns its command.
    ///
    /// # Errors
    /// Returns serialization or persistence errors.
    fn claim_next(&self) -> Result<Option<AdapterCommand>, RuntimeError>;
    /// Persists the outcome of one Adapter submission.
    ///
    /// # Errors
    /// Returns not-found, serialization, or persistence errors.
    fn finish_dispatch(
        &self,
        operation_id: &ResourceId,
        outcome: Result<AdapterAccepted, AdapterDispatchError>,
    ) -> Result<ReconcileOperation, RuntimeError>;
}
