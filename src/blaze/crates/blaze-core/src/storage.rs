// SPDX-License-Identifier: Apache-2.0
//! Generic storage provider abstraction.
//!
//! Different providers may offer different performance characteristics
//! (warm pools, copy-on-write, content-addressable dedup) but present
//! a uniform interface to the daemon layer.

use std::path::PathBuf;

use async_trait::async_trait;
use thiserror::Error;

use crate::error::{BlazeError, Result};

/// A storage slot allocated for one sandbox instance.
///
/// This capability is runtime-only. Persist the stable `id`, then ask the
/// configured provider to reconstruct every path after restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageSlot {
    /// Stable identifier used to reconstruct paths after daemon restart.
    pub id: String,
    /// Writable root filesystem exposed to the backend.
    pub rootfs_path: PathBuf,
    /// Base or merged guest memory file exposed to the backend.
    pub mem_path: PathBuf,
    /// Cumulative memory delta relative to the base image.
    pub mem_diff_path: PathBuf,
    /// Cumulative root filesystem delta relative to the base image.
    pub rootfs_diff_path: PathBuf,
    /// Provider-owned directory containing all slot artifacts.
    pub instance_dir: PathBuf,
}

/// Pool readiness status.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct PoolStatus {
    pub ready: usize,
    pub capacity: usize,
    pub pending: usize,
    /// Slots retained because backend or storage cleanup must be retried.
    pub quarantined: usize,
}

/// Options for acquiring a storage slot.
#[derive(Debug, Clone)]
pub struct AcquireOpts {
    /// Stable sandbox identifier. Providers must reject path components.
    pub instance_id: String,
    /// Logical root filesystem size in bytes.
    pub rootfs_size: u64,
    /// Logical guest memory file size in bytes.
    pub mem_size: u64,
}

/// Storage allocation failure with an optional residual slot owner.
///
/// A provider returns `residual` only when rollback could not remove resources
/// that were created for this request. The caller must retain the stable slot
/// ID until a later release succeeds.
#[derive(Debug, Error)]
#[error("{source}")]
pub struct StorageAcquireError {
    #[source]
    source: BlazeError,
    residual: Option<StorageSlot>,
}

impl StorageAcquireError {
    /// Build a failure after the provider confirmed that no resources remain.
    pub fn clean(source: BlazeError) -> Self {
        Self {
            source,
            residual: None,
        }
    }

    /// Build a failure that transfers residual slot ownership to the caller.
    pub fn with_residual(source: BlazeError, residual: StorageSlot) -> Self {
        Self {
            source,
            residual: Some(residual),
        }
    }

    /// Split the original provider error from any residual slot owner.
    pub fn into_parts(self) -> (BlazeError, Option<StorageSlot>) {
        (self.source, self.residual)
    }
}

impl From<BlazeError> for StorageAcquireError {
    fn from(source: BlazeError) -> Self {
        Self::clean(source)
    }
}

/// Generic storage backend trait.
#[async_trait]
pub trait StorageProvider: Send + Sync {
    /// Probe whether this provider is available in the current environment.
    async fn probe(&self) -> Result<bool>;

    /// Acquire a ready storage slot (may come from a warm pool).
    async fn acquire(
        &self,
        opts: &AcquireOpts,
    ) -> std::result::Result<StorageSlot, StorageAcquireError>;

    /// Release a storage slot (cleanup all associated resources).
    async fn release(&self, slot: StorageSlot) -> Result<()>;

    /// Release a slot using only its stable identifier during crash recovery.
    ///
    /// Providers whose `release` operation is idempotent for a missing slot
    /// should override this method. The default requires reconstruction first.
    async fn release_by_id(&self, instance_id: &str) -> Result<()> {
        let slot = self.reconstruct(instance_id).await?;
        self.release(slot).await
    }

    /// Reconstruct a previously allocated slot from a stable instance id.
    ///
    /// Implementations must derive every returned path from their configured
    /// root and must not trust persisted path strings.
    async fn reconstruct(&self, instance_id: &str) -> Result<StorageSlot>;

    /// Synchronize already-written provider artifacts to persistent storage.
    ///
    /// This operation persists the files and directory metadata that already
    /// belong to `slot` and are visible to the provider call. Artifact updates
    /// that race with one call may become visible in that call or a later one.
    ///
    /// The daemon may stop waiting at its configured deadline, but keeps the
    /// future supervised under slot ownership until it completes. A later
    /// synchronization or cleanup must remain safe after completion.
    async fn sync_artifacts(&self, slot: &StorageSlot) -> Result<()>;

    /// Query warm pool status.
    fn pool_status(&self) -> PoolStatus;

    /// Drain all ready slots from the warm pool.
    async fn drain_pool(&self) -> Result<usize>;
}
