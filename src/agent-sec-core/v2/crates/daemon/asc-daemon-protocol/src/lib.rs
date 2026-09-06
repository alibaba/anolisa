//! First-version PAP administration wire contracts for the local daemon.
//!
//! This crate owns only untrusted serialized values. It reuses stable Policy,
//! Scope, Binding, identifier, and revision types rather than defining daemon
//! DTO copies. PAP execution, authorization, persistence, and transport live
//! in higher layers.

#![forbid(unsafe_code)]

mod common;
mod envelope;
pub mod method;
mod pap;
mod response;

pub use common::{ListParams, ListResult, ResourceParams, RevisionParams};
pub use envelope::DaemonRequest;
pub use pap::{
    CreateBindingParams, CreatePolicyParams, CreateScopeParams, UpdateBindingParams,
    UpdatePolicyParams, UpdateScopeParams,
};
pub use response::{
    DaemonError, DaemonResponse, ErrorCode, ErrorResponse, RequestId, SuccessResponse, error_code,
};
