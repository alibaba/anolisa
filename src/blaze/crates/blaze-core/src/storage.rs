// SPDX-License-Identifier: Apache-2.0
//! Generic storage provider abstraction.
//!
//! Different providers may offer different performance characteristics
//! (warm pools, copy-on-write, content-addressable dedup) but present
//! a uniform interface to the daemon layer.

use std::path::PathBuf;

use async_trait::async_trait;

use crate::error::Result;

/// A storage slot allocated for one sandbox instance.
#[derive(Debug, Clone)]
pub struct StorageSlot {
    pub id: String,
    pub rootfs_path: PathBuf,
    pub mem_path: PathBuf,
    pub instance_dir: PathBuf,
}

/// Pool readiness status.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct PoolStatus {
    pub ready: usize,
    pub capacity: usize,
    pub pending: usize,
}

/// Options for acquiring a storage slot.
#[derive(Debug, Clone)]
pub struct AcquireOpts {
    pub instance_id: String,
    pub rootfs_size: u64,
    pub mem_size: u64,
}

/// Generic storage backend trait.
#[async_trait]
pub trait StorageProvider: Send + Sync {
    /// Probe whether this provider is available in the current environment.
    async fn probe(&self) -> Result<bool>;

    /// Acquire a ready storage slot (may come from a warm pool).
    async fn acquire(&self, opts: &AcquireOpts) -> Result<StorageSlot>;

    /// Release a storage slot (cleanup all associated resources).
    async fn release(&self, slot: StorageSlot) -> Result<()>;

    /// Flush dirty data to persistent storage (implementation may be no-op).
    async fn flush_dirty(&self, slot: &StorageSlot) -> Result<()>;

    /// Query warm pool status.
    fn pool_status(&self) -> PoolStatus;

    /// Drain all ready slots from the warm pool.
    async fn drain_pool(&self) -> Result<usize>;
}
