//! Local security evidence, risk-case models, and SQLite persistence.

mod coordinator;
mod query;
mod store;

pub use coordinator::{SecurityCoordinator, SecurityCoordinatorError};
pub use query::{
    ContainmentAction, ContainmentFailureStage, ContainmentLifecycle, RiskCase, RiskCaseDetail,
    RiskCaseStatus, RiskSeverity, SecurityCountBy, SecurityEventFilter, SecurityEventPage,
    SecuritySummary,
};
pub use store::{SecurityEventStore, SecurityStore, SecurityStoreError};
