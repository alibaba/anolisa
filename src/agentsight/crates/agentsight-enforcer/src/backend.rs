//! Backend contract shared by the mock and ActPlane implementations.

use std::sync::mpsc::Receiver;

use agentsight_enforcement_protocol::{
    ApplyCredentialPolicy, ApplyPolicy, Binding, HealthStatus, SecurityEvent, ViolationEvent,
};
use thiserror::Error;
use uuid::Uuid;

/// Failures produced while managing a policy binding.
#[derive(Debug, Error)]
pub enum BackendError {
    /// A binding ID was reused with different desired state.
    #[error("binding {0} conflicts with active desired state")]
    BindingConflict(Uuid),
    /// The requested binding does not exist.
    #[error("binding {0} does not exist")]
    MissingBinding(Uuid),
    /// PID start time no longer matches the apply request.
    #[error("process {pid} is stale or has been reused")]
    StaleProcess {
        /// PID rejected by the backend.
        pid: i32,
    },
    /// ActPlane rejected the supplied policy source.
    #[error("policy compilation failed: {0}")]
    CompileFailure(String),
    /// Kernel attachment or runtime state management failed.
    #[error("kernel enforcement failed: {0}")]
    KernelFailure(String),
}

/// Privileged policy lifecycle and violation source.
pub trait EnforcementBackend: Send + Sync + 'static {
    /// Reports whether the backend can accept policy operations.
    ///
    /// # Errors
    ///
    /// Returns a backend error when readiness cannot be determined.
    fn health(&self) -> Result<HealthStatus, BackendError>;

    /// Applies desired policy state idempotently.
    ///
    /// # Errors
    ///
    /// Returns a typed conflict, process, compile, or kernel error.
    fn apply(&self, request: ApplyPolicy) -> Result<Binding, BackendError>;

    /// Compiles and applies a product-level credential-exfiltration policy.
    ///
    /// # Errors
    ///
    /// Returns a validation, compile, stale-process, or kernel error.
    fn apply_credential_policy(
        &self,
        request: ApplyCredentialPolicy,
    ) -> Result<Binding, BackendError>;

    /// Detaches one binding.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError::MissingBinding`] when the ID is unknown or a
    /// kernel error when detachment cannot be acknowledged.
    fn detach(&self, binding_id: Uuid) -> Result<(), BackendError>;

    /// Lists the backend's current bindings.
    ///
    /// # Errors
    ///
    /// Returns a backend error when state cannot be read safely.
    fn bindings(&self) -> Result<Vec<Binding>, BackendError>;

    /// Creates an independent bounded violation subscription.
    fn subscribe(&self) -> Receiver<ViolationEvent>;

    /// Creates an independent bounded normalized security-event subscription.
    fn subscribe_security_events(&self) -> Receiver<SecurityEvent>;
}
