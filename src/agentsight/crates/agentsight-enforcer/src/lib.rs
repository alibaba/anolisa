//! Privileged enforcement service for AgentSight.

#[cfg(feature = "actplane")]
mod actplane;
mod backend;
mod event_hub;
#[cfg(feature = "mock-backend")]
mod mock;
mod service;

#[cfg(feature = "actplane")]
pub use actplane::ActPlaneBackend;
pub use backend::{BackendError, EnforcementBackend};
pub use event_hub::EventHub;
#[cfg(feature = "mock-backend")]
pub use mock::MockBackend;
pub use service::{EnforcerService, ServiceError};
