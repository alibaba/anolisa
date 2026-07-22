//! AgentSight-side coordination for the privileged enforcement service.

mod client;
mod coordinator;
mod store;
mod target;

pub use agentsight_enforcement_protocol::{
    ApplyPolicy, Binding, BindingState, Effect, HealthStatus, ViolationEvent,
};
pub use client::{
    EnforcementClient, EnforcementError, SecurityEventSubscription, ViolationSubscription,
};
pub use coordinator::{EnforcementCoordinator, EnforcementCoordinatorError};
pub use store::{EnforcementStore, EnforcementStoreError};
pub(crate) use target::{canonical_policy_file, read_process_start_time};
