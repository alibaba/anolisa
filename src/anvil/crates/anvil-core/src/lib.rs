// SPDX-License-Identifier: Apache-2.0
//! anvil-core: shared types and v0.1 in-memory implementations for the
//! anvil sandbox-orchestration daemon.
//!
//! This crate intentionally has no I/O surface beyond JSON/TOML on local
//! filesystems. Network/UDS surfaces are implemented in the `anvil` daemon
//! crate. Modules map 1:1 to the functional breakdown:
//!
//! - [`config`]: daemon TOML configuration
//! - [`policy`]: workload class + policy file schema
//! - [`backend`]: backend kinds + selection / fallback
//! - [`lifecycle`]: sandbox state machine + JSON persistence
//! - [`pool`]: warm-pool key/stat/manager
//! - [`template`]: template registry + refcnt + GC
//! - [`kernel`]: kernel hook registry, per-hook mutex
//! - [`error`]: unified [`AnvilError`] error enum

pub mod backend;
pub mod config;
pub mod error;
pub mod kernel;
pub mod lifecycle;
pub mod policy;
pub mod pool;
pub mod template;

pub use error::{AnvilError, Result};
