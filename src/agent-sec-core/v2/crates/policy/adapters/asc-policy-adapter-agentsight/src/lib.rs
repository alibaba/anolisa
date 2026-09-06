//! Pure Binding-to-AgentSight/ActPlane translation.
//!
//! This crate translates a complete immutable `PreparedBinding` into a
//! deterministic target plan. It performs no persistence, HTTP, process
//! inspection, or policy attachment.

#![forbid(unsafe_code)]

mod plan;
mod translate;

pub use plan::{
    ACTPLANE_POLICY_MEDIA_TYPE, AGENTSIGHT_BINDING_PLAN_FORMAT,
    AGENTSIGHT_BINDING_PLAN_SCHEMA_VERSION, AgentSightBindingPlan, AgentSightPolicyPlan,
    AgentSightScopePlan, AgentSightSourceBinding,
};
pub use translate::AgentSightAdapter;
