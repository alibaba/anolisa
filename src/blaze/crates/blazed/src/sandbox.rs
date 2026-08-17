// SPDX-License-Identifier: Apache-2.0
//! Managed sandbox lifecycle and runtime ownership.

mod checkpoint;
mod manager;
mod restore;
mod storage_sync;
pub(crate) mod template;

pub use manager::{CreateSandbox, SandboxManager, SandboxManagerInit};
pub use restore::{RestoreSandbox, RestoreSandboxResult};
pub(crate) use storage_sync::StorageSyncLoop;
