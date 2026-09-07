use asc_foundation_types::{ResourceId, Revision};
use asc_policy_types::binding::{BindingStatus, BindingView};
use asc_policy_types::policy::PreparedPolicy;
use asc_policy_types::scope::PreparedScope;

use crate::error::PapError;
use crate::model::{Page, PolicyRevisionState, ScopeRevisionState};

/// Persistence port owned by PAP.
///
/// TODO(policy-pagination-bounds): before a concrete repository is exposed
/// through a bounded daemon transport, pass a server-owned aggregate byte
/// budget through all three list paths. Define encoded-size accounting so an
/// implementation can reject an individually oversized first record before
/// decoding/materializing it and stop before a page exceeds the remaining
/// budget.
///
/// All list implementations must apply pagination in the repository. They
/// first order the complete matching result as documented by the individual
/// method, then skip `offset` records and return at most `limit` records.
/// Identity ordering is the lexicographic byte order of `ResourceId::as_str()`.
/// `Page::total` is the matching count before `offset` and `limit` are applied.
/// These values are per-query inputs and must not be persisted.
pub trait PapRepository: Send + Sync {
    /// Creates or replaces the current Policy record.
    ///
    /// Implementations must atomically accept a changed record only when its
    /// revision is exactly the next never-reused revision for the Policy identity.
    /// An exact replay of the current record is idempotent; every other stale,
    /// reused, or skipped revision must return [`PapError::Conflict`]. A successful
    /// changed write replaces the previously retained content; only the current
    /// record remains.
    ///
    /// # Errors
    /// Returns conflict, serialization, or persistence failures.
    fn put_policy(&self, policy: &PreparedPolicy) -> Result<PreparedPolicy, PapError>;

    /// Gets Policy allocation state and its optional current record.
    ///
    /// # Errors
    /// Returns a persistence failure when the query cannot complete.
    fn get_policy_revision_state(
        &self,
        id: &ResourceId,
    ) -> Result<Option<PolicyRevisionState>, PapError>;

    /// Gets the current Policy only when its revision equals `revision`.
    ///
    /// # Errors
    /// Returns not-found or persistence failures.
    fn get_policy(&self, id: &ResourceId, revision: Revision) -> Result<PreparedPolicy, PapError>;

    /// Lists current Policy records ordered by Policy identity ascending.
    ///
    /// # Errors
    /// Returns a persistence failure when the query cannot complete.
    fn list_policies(&self, limit: u32, offset: u32) -> Result<Page<PreparedPolicy>, PapError>;

    /// Deletes the current Policy content when its revision equals `revision`.
    ///
    /// Implementations retain the allocation head as a tombstone so a later
    /// update of the same identity cannot reuse the deleted revision.
    ///
    /// # Errors
    /// Returns not-found, conflict, or persistence failures.
    fn delete_policy_revision(
        &self,
        id: &ResourceId,
        revision: Revision,
    ) -> Result<PreparedPolicy, PapError>;

    /// Creates or replaces the current Scope record.
    ///
    /// Implementations must atomically accept a changed record only when its
    /// revision is exactly the next never-reused revision for the Scope identity.
    /// An exact replay of the current record is idempotent; every other stale,
    /// reused, or skipped revision must return [`PapError::Conflict`]. A successful
    /// changed write replaces the previously retained content; only the current
    /// record remains.
    ///
    /// # Errors
    /// Returns conflict, serialization, or persistence failures.
    fn put_scope(&self, scope: &PreparedScope) -> Result<PreparedScope, PapError>;

    /// Gets Scope allocation state and its optional current record.
    ///
    /// # Errors
    /// Returns a persistence failure when the query cannot complete.
    fn get_scope_revision_state(
        &self,
        id: &ResourceId,
    ) -> Result<Option<ScopeRevisionState>, PapError>;

    /// Gets the current Scope only when its revision equals `revision`.
    ///
    /// # Errors
    /// Returns not-found or persistence failures.
    fn get_scope(&self, id: &ResourceId, revision: Revision) -> Result<PreparedScope, PapError>;

    /// Lists current Scope records ordered by Scope identity ascending.
    ///
    /// # Errors
    /// Returns a persistence failure when the query cannot complete.
    fn list_scopes(&self, limit: u32, offset: u32) -> Result<Page<PreparedScope>, PapError>;

    /// Deletes the current Scope content when its revision equals `revision`.
    ///
    /// Implementations retain the allocation head as a tombstone so a later
    /// update of the same identity cannot reuse the deleted revision.
    ///
    /// # Errors
    /// Returns not-found, conflict, or persistence failures.
    fn delete_scope_revision(
        &self,
        id: &ResourceId,
        revision: Revision,
    ) -> Result<PreparedScope, PapError>;

    /// Creates or atomically updates the single current Binding record.
    ///
    /// A new Binding starts at revision 1 in `PENDING_APPLY`. An update must use
    /// exactly the next never-reused revision and must enter `PENDING_APPLY` or
    /// `PENDING_DELETE`. The update replaces the complete current spec and
    /// status atomically; older Binding records are not retained. Implementations
    /// reject updates while the current status is `APPLYING` or `DELETING`.
    ///
    /// TODO(policy-reconciliation): this method is the transaction boundary
    /// that must later atomically persist a durable reconcile intent and its
    /// ordering/CAS token together with the new spec/current-status pointer.
    /// No outbox is written in the PAP-only phase.
    ///
    /// # Errors
    /// Returns operation-in-progress, conflict, serialization, or persistence
    /// failures.
    fn update_binding(&self, binding: &BindingView) -> Result<BindingView, PapError>;

    /// Compare-and-swaps worker status for the current Binding revision.
    ///
    /// Implementations must atomically require the current spec and status to
    /// equal `binding_revision` and `expected_status`, then call
    /// `expected_status.validate_successor(next_status)` or enforce an
    /// equivalent predicate. No `PreparedBinding` is rewritten.
    ///
    /// Request transitions use [`PapRepository::update_binding`] and allocate a
    /// new revision; this method only persists Reconciler transitions within one
    /// current revision.
    ///
    /// # Errors
    /// Returns conflict or persistence failures.
    fn update_binding_status(
        &self,
        id: &ResourceId,
        binding_revision: Revision,
        expected_status: BindingStatus,
        next_status: BindingStatus,
    ) -> Result<BindingStatus, PapError>;

    /// Gets the current Binding spec and status as a read-only aggregate.
    ///
    /// # Errors
    /// Returns not-found or persistence failures.
    fn get_binding(&self, id: &ResourceId) -> Result<BindingView, PapError>;

    /// Lists current Binding specs and status ordered by Binding identity
    /// ascending.
    ///
    /// # Errors
    /// Returns a persistence failure when the query cannot complete.
    fn list_bindings(&self, limit: u32, offset: u32) -> Result<Page<BindingView>, PapError>;
}
