use asc_foundation_types::{ResourceId, Revision};

use crate::error::PapError;
use crate::model::{Page, PolicyRevisionState, PreparedPolicy, PreparedScope, ScopeRevisionState};

/// Persistence boundary owned by PAP.
pub trait PapRepository: Send + Sync {
    /// Inserts an immutable policy revision or returns the identical revision.
    ///
    /// # Errors
    /// Returns a conflict, serialization, or persistence error.
    fn put_policy(&self, policy: &PreparedPolicy) -> Result<PreparedPolicy, PapError>;
    /// Gets allocation state and the latest retained revision, when the identity exists.
    ///
    /// # Errors
    /// Returns a persistence error when the query cannot complete.
    fn get_policy_revision_state(
        &self,
        id: &ResourceId,
    ) -> Result<Option<PolicyRevisionState>, PapError>;
    /// Gets one policy revision.
    ///
    /// # Errors
    /// Returns not-found or persistence errors.
    fn get_policy(&self, id: &ResourceId, revision: Revision) -> Result<PreparedPolicy, PapError>;
    /// Lists policy revisions.
    ///
    /// # Errors
    /// Returns a persistence error when the query cannot complete.
    fn list_policies(
        &self,
        limit: u32,
        offset: u64,
        max_item_bytes: usize,
    ) -> Result<Page<PreparedPolicy>, PapError>;
    /// Deletes the complete content of one exact policy revision.
    ///
    /// # Errors
    /// Returns not-found or persistence errors.
    fn delete_policy_revision(
        &self,
        id: &ResourceId,
        revision: Revision,
    ) -> Result<PreparedPolicy, PapError>;
    /// Inserts an immutable scope revision or returns the identical revision.
    ///
    /// # Errors
    /// Returns a conflict, serialization, or persistence error.
    fn put_scope(&self, scope: &PreparedScope) -> Result<PreparedScope, PapError>;
    /// Gets allocation state and the latest retained Scope revision.
    ///
    /// # Errors
    /// Returns a persistence error when the query cannot complete.
    fn get_scope_revision_state(
        &self,
        id: &ResourceId,
    ) -> Result<Option<ScopeRevisionState>, PapError>;
    /// Gets one scope revision.
    ///
    /// # Errors
    /// Returns not-found or persistence errors.
    fn get_scope(&self, id: &ResourceId, revision: Revision) -> Result<PreparedScope, PapError>;
    /// Lists scope revisions.
    ///
    /// # Errors
    /// Returns a persistence error when the query cannot complete.
    fn list_scopes(
        &self,
        limit: u32,
        offset: u64,
        max_item_bytes: usize,
    ) -> Result<Page<PreparedScope>, PapError>;
    /// Deletes one exact Scope revision.
    ///
    /// # Errors
    /// Returns not-found or persistence errors.
    fn delete_scope_revision(
        &self,
        id: &ResourceId,
        revision: Revision,
    ) -> Result<PreparedScope, PapError>;
}
