//! Transactional `SQLite` persistence for authored policy state and Adapter outbox work.

#![forbid(unsafe_code)]

mod pap_repository;
mod runtime_repository;
mod schema;
mod sql;
mod store;

pub use store::{SqlitePolicyStore, StoreOpenError};
