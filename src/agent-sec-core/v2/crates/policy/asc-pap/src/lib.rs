//! Policy authoring, lowering, and immutable Policy/Scope revisions.

#![forbid(unsafe_code)]

mod error;
mod model;
mod repository;
mod scope;
mod service;

pub use error::PapError;
pub use model::{Page, PolicyRevisionState, PreparedPolicy, PreparedScope, ScopeRevisionState};
pub use repository::PapRepository;
pub use scope::{
    NamespaceChange, NestedExecutionDomains, ProcessMembership, ScopeActivation, ScopeEndCondition,
    ScopeKind, ScopeLifetime, ScopeSelector, ScopeTemplate,
};
pub use service::PapService;
