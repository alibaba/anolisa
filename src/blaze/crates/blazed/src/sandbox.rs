// SPDX-License-Identifier: Apache-2.0
//! Managed sandbox lifecycle and runtime ownership.

mod manager;
mod storage_sync;
pub(crate) mod template;

pub use manager::{CreateSandbox, SandboxManager, SandboxManagerInit};
pub(crate) use storage_sync::StorageSyncLoop;
