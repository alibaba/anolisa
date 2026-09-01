//! Durable Binding intent and the seam implemented by a future enforcement Adapter.

#![forbid(unsafe_code)]

mod adapter;
mod error;
mod model;
mod repository;
mod service;
pub mod testing;

pub use adapter::{AdapterDispatchError, PolicyAdapter, UnavailablePolicyAdapter};
pub use error::RuntimeError;
pub use model::{
    AdapterAccepted, AdapterCommand, BindingDesiredState, OperationState, PreparedBinding,
    ReconcileOperation,
};
pub use repository::RuntimeRepository;
pub use service::PolicyRuntime;
