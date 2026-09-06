use asc_policy_types::error::ValidationError;

/// Stable PAP failure categories independent of transport and persistence.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum PapError {
    /// A shared identifier cannot be represented by a required domain type.
    #[error("invalid identifier: {0}")]
    InvalidIdentifier(String),
    /// Human-readable Policy name validation failed.
    #[error("invalid policy name: {0}")]
    InvalidPolicyName(String),
    /// Policy authoring or lowering validation failed.
    #[error("invalid policy: {0}")]
    InvalidPolicy(ValidationError),
    /// Scope authoring validation failed.
    #[error("invalid scope: {0}")]
    InvalidScope(ValidationError),
    /// Binding construction validation failed.
    #[error("invalid binding: {0}")]
    InvalidBinding(ValidationError),
    /// Pagination parameters are outside the bounded PAP query contract.
    #[error("invalid pagination: limit must be between 1 and 1000")]
    InvalidPagination,
    /// An immutable identity or concurrent revision precondition conflicted.
    #[error("immutable revision conflict")]
    Conflict,
    /// A changed desired-state request cannot interrupt target-side work.
    #[error("binding reconciliation operation is in progress")]
    OperationInProgress,
    /// The requested exact record does not exist.
    #[error("record not found")]
    NotFound,
    /// A Binding references a Policy revision that is neither current nor in its snapshot.
    #[error("referenced policy revision not found")]
    ReferencedPolicyRevisionNotFound,
    /// A Binding references a Scope revision that is neither current nor in its snapshot.
    #[error("referenced scope revision not found")]
    ReferencedScopeRevisionNotFound,
    /// No further positive `u32` revision can be allocated.
    #[error("revision space exhausted")]
    RevisionExhausted,
    /// JSON serialization failed while computing canonical content identity.
    #[error("serialization failed")]
    Serialization,
    /// Persistence failed without exposing implementation details.
    #[error("persistence failed")]
    Persistence,
}
