/// Failure returned while validating, lowering, or composing policy input.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// A shared Canonical IR invariant failed.
    #[error("invalid policy: {0}")]
    InvalidPolicy(#[from] asc_policy_types::ValidationError),
    /// A template-specific invariant failed.
    #[error("invalid template at {path}: {message}")]
    InvalidTemplate {
        /// Stable field path.
        path: String,
        /// Human-readable detail.
        message: String,
    },
    /// A generated local identifier is not representable on the wire.
    #[error("cannot generate identifier: {0}")]
    Identifier(String),
    /// Inputs use different Canonical IR profiles.
    #[error("policies use incompatible Canonical IR profiles")]
    ProfileMismatch,
    /// An immutable policy identity was reused for different content.
    #[error("policy {policy_id} revision {revision} has conflicting content")]
    ConflictingRevision {
        /// Stable policy identity.
        policy_id: String,
        /// Immutable revision.
        revision: u64,
    },
    /// Failure guarantees cannot be safely merged.
    #[error("policies use incompatible failure guarantees")]
    IncompatibleFailurePolicy,
    /// Composition requires at least one policy.
    #[error("at least one policy is required")]
    EmptyPolicySet,
}
