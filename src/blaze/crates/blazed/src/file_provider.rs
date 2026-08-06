// SPDX-License-Identifier: Apache-2.0
//! File-based storage provider: creates per-instance directories with
//! rootfs and memory files on a local filesystem. Base images and mutable
//! instance slots use separate roots.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use rustix::fs::{Mode, OFlags, open, openat};

use blaze_core::error::{BlazeError, Result};
use blaze_core::storage::{
    AcquireOpts, PoolStatus, StorageAcquireError, StorageProvider, StorageSlot,
};

/// A filesystem-based provider that copies base artifacts when available and
/// otherwise creates sparse rootfs and memory files at configured sizes.
pub struct FileStorageProvider {
    images_dir: PathBuf,
    instances_dir: PathBuf,
    #[cfg(test)]
    artifact_sync_open_hook: Option<std::sync::Arc<ArtifactSyncOpenHook>>,
}

#[cfg(test)]
pub(crate) struct ArtifactSyncOpenHook {
    opened: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}

#[cfg(test)]
impl ArtifactSyncOpenHook {
    pub(crate) fn new() -> Self {
        Self {
            opened: tokio::sync::Notify::new(),
            resume: tokio::sync::Notify::new(),
        }
    }

    pub(crate) async fn wait_until_open(&self) {
        self.opened.notified().await;
    }

    pub(crate) fn resume(&self) {
        self.resume.notify_one();
    }
}

impl FileStorageProvider {
    /// Create a provider with no separate image directory.
    ///
    /// This constructor is kept for focused tests. Daemon startup uses
    /// [`Self::with_images`] so immutable images and runtime slots cannot mix.
    #[cfg(test)]
    pub fn new(instances_dir: PathBuf) -> Self {
        Self {
            images_dir: instances_dir.clone(),
            instances_dir,
            artifact_sync_open_hook: None,
        }
    }

    /// Create a provider with distinct immutable image and runtime roots.
    pub fn with_images(images_dir: PathBuf, instances_dir: PathBuf) -> Self {
        Self {
            images_dir,
            instances_dir,
            #[cfg(test)]
            artifact_sync_open_hook: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_artifact_sync_open_hook(
        images_dir: PathBuf,
        instances_dir: PathBuf,
        hook: std::sync::Arc<ArtifactSyncOpenHook>,
    ) -> Self {
        Self {
            images_dir,
            instances_dir,
            artifact_sync_open_hook: Some(hook),
        }
    }

    fn slot_for_id(&self, instance_id: &str) -> Result<StorageSlot> {
        validate_instance_id(instance_id)?;
        let instance_dir = self.instances_dir.join(instance_id);
        if !instance_dir.starts_with(&self.instances_dir) || instance_dir == self.instances_dir {
            return Err(BlazeError::StorageError {
                msg: format!("slot '{instance_id}': path escapes instances_dir"),
            });
        }
        Ok(StorageSlot {
            id: instance_id.to_string(),
            rootfs_path: instance_dir.join("rootfs.ext4"),
            mem_path: instance_dir.join("mem.bin"),
            mem_diff_path: instance_dir.join("mem.diff"),
            rootfs_diff_path: instance_dir.join("rootfs.diff"),
            instance_dir,
        })
    }
}

#[derive(Clone, Copy)]
enum RequiredPathType {
    Directory,
    File,
}

impl RequiredPathType {
    fn description(self) -> &'static str {
        match self {
            Self::Directory => "directory",
            Self::File => "file",
        }
    }

    fn matches(self, metadata: &std::fs::Metadata) -> bool {
        match self {
            Self::Directory => metadata.is_dir(),
            Self::File => metadata.is_file(),
        }
    }
}

async fn require_slot_path(
    instance_id: &str,
    path: &Path,
    required_type: RequiredPathType,
) -> Result<()> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if required_type.matches(&metadata) => Ok(()),
        Ok(_) => Err(BlazeError::StorageIncomplete {
            instance_id: instance_id.to_string(),
            path: path.to_path_buf(),
            expected: required_type.description(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(BlazeError::StorageIncomplete {
                instance_id: instance_id.to_string(),
                path: path.to_path_buf(),
                expected: required_type.description(),
            })
        }
        Err(error) => Err(BlazeError::StorageError {
            msg: format!(
                "reconstruct '{instance_id}': inspect {}: {error}",
                path.display()
            ),
        }),
    }
}

#[async_trait]
impl StorageProvider for FileStorageProvider {
    async fn probe(&self) -> Result<bool> {
        Ok(self.images_dir.exists() && self.instances_dir.exists())
    }

    async fn acquire(
        &self,
        opts: &AcquireOpts,
    ) -> std::result::Result<StorageSlot, StorageAcquireError> {
        crate::failpoint::storage("storage-acquire")?;
        let slot = self.slot_for_id(&opts.instance_id)?;
        let instance_dir = slot.instance_dir.clone();

        // Atomic: create_dir fails with AlreadyExists if concurrent acquire races
        match tokio::fs::create_dir(&instance_dir).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(StorageAcquireError::clean(BlazeError::StorageError {
                    msg: format!(
                        "acquire '{}': instance directory already exists",
                        opts.instance_id
                    ),
                }));
            }
            Err(e) => {
                return Err(StorageAcquireError::clean(BlazeError::StorageError {
                    msg: format!("acquire '{}': create dir: {}", opts.instance_id, e),
                }));
            }
        }

        // Create rootfs + mem; rollback dir on failure
        let result = async {
            create_or_copy(
                &self.images_dir.join("rootfs.ext4"),
                &slot.rootfs_path,
                opts.rootfs_size,
            )
            .await?;
            create_or_copy(
                &self.images_dir.join("mem.bin"),
                &slot.mem_path,
                opts.mem_size,
            )
            .await?;
            tokio::fs::File::create(&slot.mem_diff_path).await?;
            tokio::fs::File::create(&slot.rootfs_diff_path).await?;
            crate::failpoint::storage("storage-acquire-artifacts")?;
            Ok::<(), BlazeError>(())
        }
        .await;

        if let Err(e) = result {
            let rollback = match crate::failpoint::storage("storage-acquire-rollback") {
                Ok(()) => tokio::fs::remove_dir_all(&instance_dir)
                    .await
                    .map_err(BlazeError::from),
                Err(error) => Err(error),
            };
            let source = match rollback {
                Ok(()) => BlazeError::StorageError {
                    msg: format!(
                        "acquire '{}': file setup failed, rolled back: {}",
                        opts.instance_id, e
                    ),
                },
                Err(cleanup) => {
                    return Err(StorageAcquireError::with_residual(
                        BlazeError::StorageError {
                            msg: format!(
                                "acquire '{}': file setup failed ({e}); rollback failed for {}: {cleanup}",
                                opts.instance_id,
                                instance_dir.display()
                            ),
                        },
                        slot,
                    ));
                }
            };
            return Err(StorageAcquireError::clean(source));
        }

        Ok(slot)
    }

    async fn release(&self, slot: StorageSlot) -> Result<()> {
        crate::failpoint::storage("storage-release")?;
        // Re-derive the canonical path from instances_dir + slot.id. Do not
        // trust path strings carried in a persisted or externally built slot.
        let canonical_dir = self.slot_for_id(&slot.id)?.instance_dir;
        if canonical_dir.exists() {
            tokio::fs::remove_dir_all(&canonical_dir)
                .await
                .map_err(|e| BlazeError::StorageError {
                    msg: format!("release '{}': {}", slot.id, e),
                })?;
        }
        Ok(())
    }

    async fn release_by_id(&self, instance_id: &str) -> Result<()> {
        let slot = self.slot_for_id(instance_id)?;
        self.release(slot).await
    }

    async fn reconstruct(&self, instance_id: &str) -> Result<StorageSlot> {
        let slot = self.slot_for_id(instance_id)?;
        require_slot_path(instance_id, &slot.instance_dir, RequiredPathType::Directory).await?;
        for path in [
            &slot.rootfs_path,
            &slot.mem_path,
            &slot.mem_diff_path,
            &slot.rootfs_diff_path,
        ] {
            require_slot_path(instance_id, path, RequiredPathType::File).await?;
        }
        Ok(slot)
    }

    async fn sync_artifacts(&self, slot: &StorageSlot) -> Result<()> {
        crate::failpoint::storage("sync-artifacts")?;
        // Never trust paths carried by a runtime or persisted slot. Rebuild
        // the complete provider-owned artifact set from the validated ID.
        let canonical = self.slot_for_id(&slot.id)?;
        let instance_dir = canonical.instance_dir.clone();
        let directory_fd = open_required_slot_path(
            &slot.id,
            &canonical.instance_dir,
            RequiredPathType::Directory,
            move || {
                open(
                    &instance_dir,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
            },
        )
        .await?;
        #[cfg(test)]
        if let Some(hook) = &self.artifact_sync_open_hook {
            hook.opened.notify_one();
            hook.resume.notified().await;
        }
        let directory_fd = Arc::new(directory_fd);
        for (name, path) in [
            ("rootfs.ext4", &canonical.rootfs_path),
            ("mem.bin", &canonical.mem_path),
            ("mem.diff", &canonical.mem_diff_path),
            ("rootfs.diff", &canonical.rootfs_diff_path),
        ] {
            let open_directory_fd = Arc::clone(&directory_fd);
            let file_fd =
                open_required_slot_path(&slot.id, path, RequiredPathType::File, move || {
                    openat(
                        &*open_directory_fd,
                        name,
                        OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                        Mode::empty(),
                    )
                })
                .await?;
            let file = tokio::fs::File::from_std(std::fs::File::from(file_fd));
            file.sync_all()
                .await
                .map_err(|error| BlazeError::StorageError {
                    msg: format!(
                        "sync artifacts '{}': sync {}: {error}",
                        slot.id,
                        path.display()
                    ),
                })?;
        }
        let directory_fd = Arc::try_unwrap(directory_fd).map_err(|_| BlazeError::StorageError {
            msg: format!(
                "sync artifacts '{}': directory descriptor remained shared after opening artifacts",
                slot.id
            ),
        })?;
        let directory = tokio::fs::File::from_std(std::fs::File::from(directory_fd));
        directory
            .sync_all()
            .await
            .map_err(|error| BlazeError::StorageError {
                msg: format!(
                    "sync artifacts '{}': sync directory {}: {error}",
                    slot.id,
                    canonical.instance_dir.display()
                ),
            })?;
        Ok(())
    }

    fn pool_status(&self) -> PoolStatus {
        PoolStatus::default()
    }
}

async fn open_required_slot_path<F>(
    instance_id: &str,
    path: &Path,
    required_type: RequiredPathType,
    open_path: F,
) -> Result<std::os::fd::OwnedFd>
where
    F: FnOnce() -> rustix::io::Result<std::os::fd::OwnedFd> + Send + 'static,
{
    let task_instance_id = instance_id.to_string();
    let task_path = path.to_path_buf();
    let join_instance_id = task_instance_id.clone();
    let join_path = task_path.clone();
    tokio::task::spawn_blocking(move || {
        let file = open_path().map_err(|error| {
            if matches!(
                error,
                rustix::io::Errno::NOENT | rustix::io::Errno::NOTDIR | rustix::io::Errno::LOOP
            ) {
                BlazeError::StorageIncomplete {
                    instance_id: task_instance_id.clone(),
                    path: task_path.clone(),
                    expected: required_type.description(),
                }
            } else {
                BlazeError::StorageError {
                    msg: format!(
                        "sync artifacts '{task_instance_id}': open {}: {error}",
                        task_path.display()
                    ),
                }
            }
        })?;
        let file = std::fs::File::from(file);
        let metadata = file.metadata().map_err(|error| BlazeError::StorageError {
            msg: format!(
                "sync artifacts '{task_instance_id}': inspect {}: {error}",
                task_path.display()
            ),
        })?;
        if !required_type.matches(&metadata) {
            return Err(BlazeError::StorageIncomplete {
                instance_id: task_instance_id,
                path: task_path,
                expected: required_type.description(),
            });
        }
        Ok(file.into())
    })
    .await
    .map_err(|error| BlazeError::StorageError {
        msg: format!(
            "sync artifacts '{join_instance_id}': open task for {} failed: {error}",
            join_path.display()
        ),
    })?
}

async fn create_or_copy(
    source: &std::path::Path,
    target: &std::path::Path,
    size: u64,
) -> std::io::Result<()> {
    if source.is_file() && source != target {
        tokio::fs::copy(source, target).await?;
        return Ok(());
    }
    let file = tokio::fs::File::create(target).await?;
    if size > 0 {
        file.set_len(size).await?;
    }
    Ok(())
}

fn validate_instance_id(instance_id: &str) -> Result<()> {
    if instance_id.is_empty()
        || instance_id.contains('/')
        || instance_id.contains('\\')
        || instance_id == ".."
        || instance_id == "."
        || std::path::Path::new(instance_id).is_absolute()
    {
        return Err(BlazeError::StorageError {
            msg: format!("invalid instance_id '{instance_id}': must be a single path component"),
        });
    }
    Ok(())
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

    #[test]
    fn pool_status_returns_current_capacity() {
        let tmp = tempfile::TempDir::new().unwrap();
        let provider = FileStorageProvider::new(tmp.path().to_path_buf());
        let status = provider.pool_status();
        assert_eq!(status.ready, 0);
        assert_eq!(status.capacity, 0);
        assert_eq!(status.pending, 0);
        assert_eq!(status.quarantined, 0);
    }

    #[tokio::test]
    async fn release_rejects_forged_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let fp = FileStorageProvider::new(dir.path().to_path_buf());
        let forged_slot = StorageSlot {
            id: "../../etc".into(),
            rootfs_path: PathBuf::from("/etc/passwd"),
            mem_path: PathBuf::from("/etc/shadow"),
            mem_diff_path: PathBuf::from("/etc/shadow"),
            rootfs_diff_path: PathBuf::from("/etc/passwd"),
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

    #[tokio::test]
    async fn reconstruct_derives_paths_from_id() {
        let temp = tempfile::TempDir::new().unwrap();
        let provider = FileStorageProvider::new(temp.path().to_path_buf());
        let slot = provider
            .acquire(&AcquireOpts {
                instance_id: "restore-me".into(),
                rootfs_size: 64,
                mem_size: 32,
            })
            .await
            .unwrap();
        let reconstructed = provider.reconstruct("restore-me").await.unwrap();
        assert_eq!(reconstructed, slot);
    }

    #[tokio::test]
    async fn reconstruct_classifies_missing_artifact_as_incomplete() {
        let temp = tempfile::TempDir::new().unwrap();
        let provider = FileStorageProvider::new(temp.path().to_path_buf());
        let slot = provider
            .acquire(&AcquireOpts {
                instance_id: "missing-artifact".into(),
                rootfs_size: 64,
                mem_size: 32,
            })
            .await
            .unwrap();
        tokio::fs::remove_file(&slot.mem_diff_path).await.unwrap();

        let error = provider
            .reconstruct("missing-artifact")
            .await
            .expect_err("missing artifact must invalidate the slot");

        assert!(matches!(
            error,
            BlazeError::StorageIncomplete {
                ref instance_id,
                ref path,
                expected: "file",
            } if instance_id == "missing-artifact" && path == &slot.mem_diff_path
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reconstruct_rejects_a_linked_slot_root() {
        use std::os::unix::fs::symlink;

        let storage = tempfile::TempDir::new().unwrap();
        let target = tempfile::TempDir::new().unwrap();
        for artifact in ["rootfs.ext4", "mem.bin", "mem.diff", "rootfs.diff"] {
            tokio::fs::write(target.path().join(artifact), b"external")
                .await
                .unwrap();
        }
        symlink(target.path(), storage.path().join("linked-slot")).unwrap();
        let provider = FileStorageProvider::new(storage.path().to_path_buf());

        let error = provider
            .reconstruct("linked-slot")
            .await
            .expect_err("linked slot root must be rejected");

        assert!(matches!(
            error,
            BlazeError::StorageIncomplete {
                ref instance_id,
                ref path,
                expected: "directory",
            } if instance_id == "linked-slot" && path == &storage.path().join("linked-slot")
        ));
        assert!(
            std::fs::symlink_metadata(storage.path().join("linked-slot"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(target.path().is_dir());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reconstruct_rejects_a_linked_slot_artifact() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::TempDir::new().unwrap();
        let provider = FileStorageProvider::new(temp.path().to_path_buf());
        let slot = provider
            .acquire(&AcquireOpts {
                instance_id: "linked-artifact".into(),
                rootfs_size: 64,
                mem_size: 32,
            })
            .await
            .unwrap();
        tokio::fs::remove_file(&slot.mem_diff_path).await.unwrap();
        let external = temp.path().join("external-memory-diff");
        tokio::fs::write(&external, b"external").await.unwrap();
        symlink(&external, &slot.mem_diff_path).unwrap();

        let error = provider
            .reconstruct("linked-artifact")
            .await
            .expect_err("linked artifact must be rejected");

        assert!(matches!(
            error,
            BlazeError::StorageIncomplete {
                ref instance_id,
                ref path,
                expected: "file",
            } if instance_id == "linked-artifact" && path == &slot.mem_diff_path
        ));
        assert!(
            std::fs::symlink_metadata(&slot.mem_diff_path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(external.is_file());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn slot_open_does_not_block_async_runtime() -> Result<()> {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc;
        use std::time::Duration;

        let temp = tempfile::tempdir()?;
        let path = temp.path().join("artifact");
        tokio::fs::write(&path, b"artifact").await?;

        let (opened_tx, opened_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let watchdog_release = release_tx.clone();
        let (watchdog_cancel_tx, watchdog_cancel_rx) = mpsc::channel();
        let watchdog_fired = Arc::new(AtomicBool::new(false));
        let watchdog_state = Arc::clone(&watchdog_fired);
        let watchdog = std::thread::spawn(move || {
            if watchdog_cancel_rx
                .recv_timeout(Duration::from_secs(2))
                .is_err()
            {
                watchdog_state.store(true, Ordering::SeqCst);
                let _ = watchdog_release.send(());
            }
        });

        let path_to_open = path.clone();
        let open_future = open_required_slot_path(
            "runtime-progress",
            &path,
            RequiredPathType::File,
            move || {
                let _ = opened_tx.send(());
                if release_rx.recv().is_err() {
                    return Err(rustix::io::Errno::INTR);
                }
                open(
                    &path_to_open,
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
            },
        );
        let runtime_progress = async {
            let opened = tokio::time::timeout(Duration::from_secs(4), opened_rx).await;
            assert!(
                matches!(opened, Ok(Ok(()))),
                "blocking slot open did not start"
            );
            tokio::task::yield_now().await;
            assert!(
                !watchdog_fired.load(Ordering::SeqCst),
                "blocking slot open stalled the current-thread runtime"
            );
            assert!(release_tx.send(()).is_ok(), "release slot open");
            assert!(
                watchdog_cancel_tx.send(()).is_ok(),
                "cancel slot-open watchdog"
            );
        };

        let (open_result, ()) = tokio::join!(open_future, runtime_progress);
        assert!(watchdog.join().is_ok(), "slot-open watchdog panicked");
        assert!(
            !watchdog_fired.load(Ordering::SeqCst),
            "slot-open watchdog released a blocked runtime"
        );
        open_result?;
        Ok(())
    }

    #[tokio::test]
    async fn sync_artifacts_rederives_canonical_paths_from_slot_id() {
        let temp = tempfile::TempDir::new().unwrap();
        let provider = FileStorageProvider::new(temp.path().to_path_buf());
        let slot = provider
            .acquire(&AcquireOpts {
                instance_id: "sync-canonical".into(),
                rootfs_size: 64,
                mem_size: 32,
            })
            .await
            .unwrap();
        tokio::fs::write(&slot.mem_diff_path, b"dirty-memory")
            .await
            .unwrap();
        tokio::fs::write(&slot.rootfs_diff_path, b"dirty-rootfs")
            .await
            .unwrap();

        let mut forged = slot.clone();
        forged.rootfs_path = PathBuf::from("/must/not/be/opened/rootfs");
        forged.mem_path = PathBuf::from("/must/not/be/opened/memory");
        forged.mem_diff_path = PathBuf::from("/must/not/be/opened/memory-diff");
        forged.rootfs_diff_path = PathBuf::from("/must/not/be/opened/rootfs-diff");
        forged.instance_dir = PathBuf::from("/must/not/be/opened");

        provider
            .sync_artifacts(&forged)
            .await
            .expect("provider uses canonical paths");
    }

    #[tokio::test]
    async fn sync_artifacts_rejects_incomplete_provider_slot() {
        let temp = tempfile::TempDir::new().unwrap();
        let provider = FileStorageProvider::new(temp.path().to_path_buf());
        let slot = provider
            .acquire(&AcquireOpts {
                instance_id: "sync-incomplete".into(),
                rootfs_size: 64,
                mem_size: 32,
            })
            .await
            .unwrap();
        tokio::fs::remove_file(&slot.mem_diff_path).await.unwrap();

        let error = provider
            .sync_artifacts(&slot)
            .await
            .expect_err("missing artifact must fail the sweep item");
        assert!(error.to_string().contains("mem.diff"), "{error}");
    }
}
