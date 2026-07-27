// SPDX-License-Identifier: Apache-2.0
//! File-based storage provider: creates per-instance directories with
//! rootfs and memory files on a local filesystem. This is the simplest
//! provider — no CoW, no dedup, no warm pool — suitable for development
//! and single-node deployments.

use std::path::PathBuf;

use async_trait::async_trait;

use blaze_core::error::{BlazeError, Result};
use blaze_core::storage::{AcquireOpts, PoolStatus, StorageProvider, StorageSlot};

/// A trivial filesystem-based storage provider. Each [`acquire`] call
/// creates a fresh directory with empty rootfs and memfile placeholders.
pub struct FileStorageProvider {
    base_dir: PathBuf,
}

impl FileStorageProvider {
    /// Create a new provider rooted at `base_dir`. The directory need not
    /// exist yet — [`probe`] checks and [`acquire`] creates subdirs.
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }
}

#[async_trait]
impl StorageProvider for FileStorageProvider {
    async fn probe(&self) -> Result<bool> {
        Ok(self.base_dir.exists())
    }

    async fn acquire(&self, opts: &AcquireOpts) -> Result<StorageSlot> {
        // Validate instance_id: must be a single path component (no /, .., absolute paths)
        if opts.instance_id.is_empty()
            || opts.instance_id.contains('/')
            || opts.instance_id.contains('\\')
            || opts.instance_id == ".."
            || opts.instance_id == "."
            || std::path::Path::new(&opts.instance_id).is_absolute()
        {
            return Err(BlazeError::StorageError {
                msg: format!(
                    "invalid instance_id '{}': must be a single path component",
                    opts.instance_id
                ),
            });
        }

        let instance_dir = self.base_dir.join(&opts.instance_id);

        // Atomic: create_dir fails with AlreadyExists if concurrent acquire races
        match tokio::fs::create_dir(&instance_dir).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(BlazeError::StorageError {
                    msg: format!(
                        "acquire '{}': instance directory already exists",
                        opts.instance_id
                    ),
                });
            }
            Err(e) => {
                return Err(BlazeError::StorageError {
                    msg: format!("acquire '{}': create dir: {}", opts.instance_id, e),
                });
            }
        }

        // Create rootfs + mem; rollback dir on failure
        let rootfs_path = instance_dir.join("rootfs.ext4");
        let mem_path = instance_dir.join("mem.bin");

        let result = async {
            let f = tokio::fs::File::create(&rootfs_path).await?;
            if opts.rootfs_size > 0 {
                f.set_len(opts.rootfs_size).await?;
            }
            let f = tokio::fs::File::create(&mem_path).await?;
            if opts.mem_size > 0 {
                f.set_len(opts.mem_size).await?;
            }
            Ok::<(), std::io::Error>(())
        }
        .await;

        if let Err(e) = result {
            // Rollback: remove the directory we just created
            let rollback_msg = match tokio::fs::remove_dir_all(&instance_dir).await {
                Ok(()) => "rolled back".to_string(),
                Err(cleanup_err) => format!(
                    "rollback failed (residual dir {}): {}",
                    instance_dir.display(),
                    cleanup_err
                ),
            };
            return Err(BlazeError::StorageError {
                msg: format!(
                    "acquire '{}': file setup failed, {}: {}",
                    opts.instance_id, rollback_msg, e
                ),
            });
        }

        Ok(StorageSlot {
            id: opts.instance_id.clone(),
            rootfs_path,
            mem_path,
            instance_dir,
        })
    }

    async fn release(&self, slot: StorageSlot) -> Result<()> {
        // Re-derive the canonical path from base_dir + slot.id
        // Do NOT trust slot.instance_dir (it could be externally constructed)
        if slot.id.is_empty()
            || slot.id.contains('/')
            || slot.id.contains('\\')
            || slot.id == ".."
            || slot.id == "."
        {
            return Err(BlazeError::StorageError {
                msg: format!("release '{}': path escapes base_dir", slot.id),
            });
        }
        let canonical_dir = self.base_dir.join(&slot.id);
        if !canonical_dir.starts_with(&self.base_dir) || canonical_dir == self.base_dir {
            return Err(BlazeError::StorageError {
                msg: format!("release '{}': path escapes base_dir", slot.id),
            });
        }
        if canonical_dir.exists() {
            tokio::fs::remove_dir_all(&canonical_dir)
                .await
                .map_err(|e| BlazeError::StorageError {
                    msg: format!("release '{}': {}", slot.id, e),
                })?;
        }
        Ok(())
    }

    async fn flush_dirty(&self, _slot: &StorageSlot) -> Result<()> {
        // No-op for the basic file provider.
        Ok(())
    }

    fn pool_status(&self) -> PoolStatus {
        PoolStatus::default()
    }

    async fn drain_pool(&self) -> Result<usize> {
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn probe_existing_dir_returns_true() {
        let tmp = tempfile::TempDir::new().unwrap();
        let provider = FileStorageProvider::new(tmp.path().to_path_buf());
        assert!(provider.probe().await.unwrap());
    }

    #[tokio::test]
    async fn probe_missing_dir_returns_false() {
        let provider =
            FileStorageProvider::new(PathBuf::from("/nonexistent/blaze-test-storage-probe"));
        assert!(!provider.probe().await.unwrap());
    }

    #[tokio::test]
    async fn acquire_creates_slot_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let provider = FileStorageProvider::new(tmp.path().to_path_buf());
        let opts = AcquireOpts {
            instance_id: "test-inst-001".to_string(),
            rootfs_size: 1024,
            mem_size: 512,
        };
        let slot = provider.acquire(&opts).await.unwrap();
        assert_eq!(slot.id, "test-inst-001");
        assert!(slot.rootfs_path.exists());
        assert!(slot.mem_path.exists());
        assert!(slot.instance_dir.exists());
        // Verify sparse file lengths match requested sizes
        assert_eq!(
            tokio::fs::metadata(&slot.rootfs_path).await.unwrap().len(),
            1024
        );
        assert_eq!(
            tokio::fs::metadata(&slot.mem_path).await.unwrap().len(),
            512
        );
    }

    #[tokio::test]
    async fn release_removes_instance_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let provider = FileStorageProvider::new(tmp.path().to_path_buf());
        let opts = AcquireOpts {
            instance_id: "test-inst-release".to_string(),
            rootfs_size: 1024,
            mem_size: 512,
        };
        let slot = provider.acquire(&opts).await.unwrap();
        let dir = slot.instance_dir.clone();
        assert!(dir.exists());
        provider.release(slot).await.unwrap();
        assert!(!dir.exists());
    }

    #[tokio::test]
    async fn pool_status_returns_defaults() {
        let tmp = tempfile::TempDir::new().unwrap();
        let provider = FileStorageProvider::new(tmp.path().to_path_buf());
        let status = provider.pool_status();
        assert_eq!(status.ready, 0);
        assert_eq!(status.capacity, 0);
        assert_eq!(status.pending, 0);
        assert_eq!(provider.drain_pool().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn release_rejects_forged_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let fp = FileStorageProvider::new(dir.path().to_path_buf());
        let forged_slot = StorageSlot {
            id: "../../etc".into(),
            rootfs_path: PathBuf::from("/etc/passwd"),
            mem_path: PathBuf::from("/etc/shadow"),
            instance_dir: PathBuf::from("/etc"),
        };
        assert!(fp.release(forged_slot).await.is_err());
    }

    #[tokio::test]
    async fn acquire_rejects_duplicate_id() {
        let dir = tempfile::TempDir::new().unwrap();
        let fp = FileStorageProvider::new(dir.path().to_path_buf());
        let opts = AcquireOpts {
            instance_id: "dup-1".into(),
            rootfs_size: 64,
            mem_size: 32,
        };

        // First acquire succeeds
        let _ = fp.acquire(&opts).await.unwrap();

        // Second acquire with same ID fails
        let r = fp.acquire(&opts).await;
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn acquire_rejects_path_traversal() {
        let tmp = tempfile::TempDir::new().unwrap();
        let provider = FileStorageProvider::new(tmp.path().to_path_buf());

        // Absolute path
        let r = provider
            .acquire(&AcquireOpts {
                instance_id: "/etc/passwd".into(),
                rootfs_size: 0,
                mem_size: 0,
            })
            .await;
        assert!(r.is_err());

        // Parent traversal
        let r = provider
            .acquire(&AcquireOpts {
                instance_id: "../escape".into(),
                rootfs_size: 0,
                mem_size: 0,
            })
            .await;
        assert!(r.is_err());

        // Slash in middle
        let r = provider
            .acquire(&AcquireOpts {
                instance_id: "foo/bar".into(),
                rootfs_size: 0,
                mem_size: 0,
            })
            .await;
        assert!(r.is_err());

        // Empty string
        let r = provider
            .acquire(&AcquireOpts {
                instance_id: "".into(),
                rootfs_size: 0,
                mem_size: 0,
            })
            .await;
        assert!(r.is_err());

        // Dot-dot
        let r = provider
            .acquire(&AcquireOpts {
                instance_id: "..".into(),
                rootfs_size: 0,
                mem_size: 0,
            })
            .await;
        assert!(r.is_err());
    }
}
