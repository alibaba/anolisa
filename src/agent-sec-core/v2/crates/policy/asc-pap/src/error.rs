use asc_policy_engine::EngineError;

/// Stable PAP failure categories.
#[derive(Debug, thiserror::Error)]
pub enum PapError {
    /// Invalid shared identifier.
    #[error("invalid identifier: {0}")]
    InvalidIdentifier(String),
    /// Invalid human-readable policy name.
    #[error("invalid policy name: {0}")]
    InvalidPolicyName(String),
    /// Policy lowering failed.
    #[error(transparent)]
    Engine(#[from] EngineError),
    /// Scope validation failed.
    #[error("invalid scope: {0}")]
    InvalidScope(String),
    /// Immutable identity was reused with different content.
    #[error("immutable revision conflict")]
    Conflict,
    /// Requested record does not exist.
    #[error("record not found")]
    NotFound,
    /// One record cannot fit within the caller's response-page byte budget.
    #[error("record exceeds response page byte budget")]
    ResponseTooLarge,
    /// JSON serialization failed.
    #[error("serialization failed: {0}")]
    Serialization(serde_json::Error),
    /// Persistence failed without leaking implementation details.
    #[error("persistence failed")]
    Persistence,
}
