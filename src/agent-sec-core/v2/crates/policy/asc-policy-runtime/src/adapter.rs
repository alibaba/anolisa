use crate::model::{AdapterAccepted, AdapterCommand};

/// Stable Adapter dispatch errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdapterDispatchError {
    /// No concrete Adapter is composed into the daemon.
    #[error("adapter unavailable")]
    Unavailable,
    /// A transient dispatch failure may be retried with the same operation ID.
    #[error("adapter dispatch is retryable")]
    Retryable,
    /// The Adapter rejected the target-independent command.
    #[error("adapter rejected the command")]
    Rejected,
}

/// Port implemented by a future daemon Adapter component.
pub trait PolicyAdapter: Send + Sync {
    /// Submits one idempotent target-independent command.
    ///
    /// # Errors
    /// Returns unavailable, retryable, or permanent Adapter rejection.
    fn submit(&self, command: &AdapterCommand) -> Result<AdapterAccepted, AdapterDispatchError>;
}

/// Production-safe placeholder until a concrete Adapter is implemented.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnavailablePolicyAdapter;

impl PolicyAdapter for UnavailablePolicyAdapter {
    fn submit(&self, _command: &AdapterCommand) -> Result<AdapterAccepted, AdapterDispatchError> {
        Err(AdapterDispatchError::Unavailable)
    }
}
