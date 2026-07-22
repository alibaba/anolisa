//! Local security evidence, risk-case models, and SQLite persistence.

mod containment;
mod coordinator;
mod query;
mod store;

pub use containment::enforcer::{
    ContainmentEnforcer, ContainmentEnforcerError, ContainmentReadinessLease, StampedBinding,
    StampedBindings, stable_readiness_lease,
};
pub use containment::{
    ContainmentCandidate, ContainmentCoordinator, ContainmentError, ContainmentPlan,
    ContainmentRequest,
};
pub use coordinator::{SecurityCoordinator, SecurityCoordinatorError};
pub use query::{
    ContainmentAction, ContainmentFailureStage, ContainmentLifecycle, RiskCase, RiskCaseDetail,
    RiskCaseStatus, RiskSeverity, SecurityCountBy, SecurityEventFilter, SecurityEventPage,
    SecuritySummary,
};
pub use store::{
    ContainmentActivationResult, ContainmentClaimResult, SecurityEventStore, SecurityStore,
    SecurityStoreError,
};
