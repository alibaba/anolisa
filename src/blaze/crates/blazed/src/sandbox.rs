// SPDX-License-Identifier: Apache-2.0
//! Managed sandbox lifecycle and runtime ownership.

mod manager;

pub use manager::{CreateSandbox, SandboxManager, SandboxManagerInit};
