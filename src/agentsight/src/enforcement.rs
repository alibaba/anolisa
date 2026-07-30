//! AgentSight-side coordination for the privileged enforcement service.

mod client;
mod coordinator;
mod store;

pub use agentsight_enforcement_protocol::{
    ApplyPolicy, Binding, BindingState, Effect, HealthStatus, ViolationEvent,
};
pub use client::{
    EnforcementClient, EnforcementError, SecurityEventSubscription, ViolationSubscription,
};
pub use coordinator::{EnforcementCoordinator, EnforcementCoordinatorError};
pub use store::{EnforcementStore, EnforcementStoreError};
