use asc_pap::PapError;

/// Stable runtime failure categories.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// Referenced authoring resource failed.
    #[error("PAP resource error: {0}")]
    Pap(PapError),
    /// Idempotency identity was reused for another request.
    #[error("operation id conflicts with another request")]
    IdempotencyConflict,
    /// Binding compare-and-swap precondition failed.
    #[error("binding revision precondition failed")]
    PreconditionFailed,
    /// Requested record does not exist.
    #[error("record not found")]
    NotFound,
    /// One record cannot fit within the caller's response-page byte budget.
    #[error("record exceeds response page byte budget")]
    ResponseTooLarge,
    /// Serialization failed.
    #[error("serialization failed: {0}")]
    Serialization(serde_json::Error),
    /// Persistence failed without leaking implementation detail.
    #[error("persistence failed")]
    Persistence,
}
