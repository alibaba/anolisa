//! Versioned NDJSON request and response contracts for the local daemon.

#![forbid(unsafe_code)]

mod auth;
mod common;
mod envelope;
mod frame;
mod health;
pub mod method;
mod policy;
mod response;

pub use auth::BearerAuth;
pub use common::{IdParams, ListParams, RevisionParams};
pub use envelope::DaemonRequest;
pub use frame::MAX_FRAME_BYTES;
pub use policy::{
    DeleteBindingParams, PolicyTemplateDto, PutBindingParams, PutPolicyParams, PutScopeParams,
    RevisionRefDto, ScopeSelectorDto, TrustedDestinationDto,
};
pub use response::{DaemonError, DaemonResponse};
