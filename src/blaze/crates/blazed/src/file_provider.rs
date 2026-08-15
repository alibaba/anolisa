// SPDX-License-Identifier: Apache-2.0
//! File-based storage provider: creates per-instance directories with
//! rootfs and memory files on a local filesystem. Base images and mutable
//! instance slots use separate roots; runtime pooling is owned by the daemon.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use rustix::fs::{
    AtFlags, Dir, DirEntry, FileType, FlockOperation, Mode, OFlags, RenameFlags, fchmod, flock,
    fstat, fsync, mkdirat, open, openat, renameat_with, statat, unlinkat,
};
use rustix::io::Errno;
use uuid::Uuid;

use blaze_core::error::{BlazeError, Result};
use blaze_core::storage::{
    AcquireOpts, PoolStatus, StorageAcquireError, StorageProvider, StorageSlot,
};

use crate::sandbox::template::opened_mount_id_for_owned_fd;

/// A filesystem-based provider that copies base artifacts when available and
/// otherwise creates sparse rootfs and memory files at configured sizes.
pub struct FileStorageProvider {
    images_dir: PathBuf,
    instances_dir: PathBuf,
    instances_owner: Option<Arc<std::os::fd::OwnedFd>>,
    #[cfg(test)]
    artifact_sync_open_hook: Option<std::sync::Arc<ArtifactSyncOpenHook>>,
    #[cfg(test)]
    acquire_blocking_hook: Option<std::sync::Arc<AcquireBlockingHook>>,
}

pub(crate) struct PreparedFileStorageProvider {
    resolved_instances_dir: PathBuf,
    owner: std::os::fd::OwnedFd,
}

pub(crate) struct PlannedFileStorageProvider {
    resolved_instances_dir: PathBuf,
    parent: std::os::fd::OwnedFd,
    parent_path: PathBuf,
    missing: Vec<OsString>,
}

#[cfg(test)]
pub(crate) struct ArtifactSyncOpenHook {
    opened: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}

#[cfg(test)]
struct AcquireBlockingHook {
    entered: tokio::sync::Notify,
    finished: tokio::sync::Notify,
    released: std::sync::Mutex<bool>,
    release: std::sync::Condvar,
}

#[cfg(test)]
impl AcquireBlockingHook {
    fn new() -> Self {
        Self {
            entered: tokio::sync::Notify::new(),
            finished: tokio::sync::Notify::new(),
            released: std::sync::Mutex::new(false),
            release: std::sync::Condvar::new(),
        }
    }

    fn pause_after_publish(&self) {
        self.entered.notify_one();
        let mut released = self.released.lock().expect("acquire hook lock");
        while !*released {
            released = self.release.wait(released).expect("acquire hook wait");
        }
    }

    async fn wait_until_entered(&self) {
        self.entered.notified().await;
    }

    async fn wait_until_finished(&self) {
        self.finished.notified().await;
    }

    fn resume(&self) {
        *self.released.lock().expect("acquire hook lock") = true;
        self.release.notify_one();
    }

    fn finish(&self) {
        self.finished.notify_one();
    }
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
    pub(crate) fn plan(instances_dir: PathBuf) -> Result<PlannedFileStorageProvider> {
        plan_instances_owner(&instances_dir)
    }

    pub(crate) fn planned_instances_dir(planned: &PlannedFileStorageProvider) -> &Path {
        &planned.resolved_instances_dir
    }

    pub(crate) fn prepare(
        planned: PlannedFileStorageProvider,
    ) -> Result<PreparedFileStorageProvider> {
        revalidate_instances_owner_plan(&planned)?;
        let resolved_instances_dir = planned.resolved_instances_dir;
        let owner =
            materialize_instances_owner(planned.parent, &planned.missing, &resolved_instances_dir)?;
        Ok(PreparedFileStorageProvider {
            resolved_instances_dir,
            owner,
        })
    }

    pub(crate) fn from_prepared(
        images_dir: PathBuf,
        prepared: PreparedFileStorageProvider,
    ) -> Self {
        let owner = Arc::new(prepared.owner);
        Self {
            images_dir,
            instances_dir: prepared.resolved_instances_dir,
            instances_owner: Some(owner),
            #[cfg(test)]
            artifact_sync_open_hook: None,
            #[cfg(test)]
            acquire_blocking_hook: None,
        }
    }

    /// Reopen a provider after a test has dropped every simulated-daemon owner.
    ///
    /// The parallel Rust test harness can fork a process-spawning sibling while
    /// this test still owns the provider root. The child briefly inherits the
    /// CLOEXEC flock until it calls exec, so a simulated restart may observe an
    /// exact owner-contention error after the old Rust owner has been dropped.
    /// Production startup remains single-attempt and fail-fast through
    /// [`Self::prepare`].
    #[cfg(test)]
    pub(crate) async fn reopen_after_simulated_restart(
        images_dir: PathBuf,
        instances_dir: PathBuf,
        retry_for: std::time::Duration,
    ) -> Result<Self> {
        let deadline = std::time::Instant::now() + retry_for;
        let mut planned = Self::plan(instances_dir.clone())?;
        let expected_root = Self::planned_instances_dir(&planned).to_path_buf();
        let exact_contention = format!(
            "storage instances root {} is already owned by another daemon",
            expected_root.display()
        );

        loop {
            match Self::prepare(planned) {
                Ok(prepared) => return Ok(Self::from_prepared(images_dir, prepared)),
                Err(error) => {
                    let is_exact_contention = matches!(
                        &error,
                        BlazeError::StorageError { msg } if msg == &exact_contention
                    );
                    if !is_exact_contention || std::time::Instant::now() >= deadline {
                        return Err(error);
                    }
                }
            }

            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            planned = Self::plan(instances_dir.clone())?;
            if Self::planned_instances_dir(&planned) != expected_root.as_path() {
                return Err(BlazeError::StorageError {
                    msg: format!(
                        "storage instances root {} changed while reopening a simulated daemon",
                        expected_root.display()
                    ),
                });
            }
        }
    }

    /// Create a provider with no separate image directory.
    ///
    /// This constructor is kept for focused tests. Daemon startup uses
    /// [`Self::with_images`] so immutable images and runtime slots cannot mix.
    #[cfg(test)]
    pub fn new(instances_dir: PathBuf) -> Self {
        let owner = Some(Arc::new(
            open(
                &instances_dir,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .expect("test storage instances root"),
        ));
        Self {
            images_dir: instances_dir.clone(),
            instances_dir,
            // Focused tests use this constructor to exercise missing-root
            // probing as well as normal slot operations. Production startup
            // always goes through `prepare` and retains the exclusive owner.
            instances_owner: owner,
            artifact_sync_open_hook: None,
            acquire_blocking_hook: None,
        }
    }

    #[cfg(test)]
    fn unavailable_for_test(instances_dir: PathBuf) -> Self {
        Self {
            images_dir: instances_dir.clone(),
            instances_dir,
            instances_owner: None,
            artifact_sync_open_hook: None,
            acquire_blocking_hook: None,
        }
    }

    /// Create a provider with distinct immutable image and runtime roots.
    #[cfg(test)]
    pub fn with_images(images_dir: PathBuf, instances_dir: PathBuf) -> Self {
        Self::try_with_images(images_dir, instances_dir).expect("storage instances root owner")
    }

    #[cfg(test)]
    fn try_with_images(images_dir: PathBuf, instances_dir: PathBuf) -> Result<Self> {
        let planned = plan_instances_owner(&instances_dir)?;
        let resolved = planned.resolved_instances_dir.clone();
        let owner = materialize_instances_owner(planned.parent, &planned.missing, &resolved)?;
        let owner = Arc::new(owner);
        Ok(Self {
            images_dir,
            instances_dir: resolved,
            instances_owner: Some(owner),
            #[cfg(test)]
            artifact_sync_open_hook: None,
            #[cfg(test)]
            acquire_blocking_hook: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn with_artifact_sync_open_hook(
        images_dir: PathBuf,
        instances_dir: PathBuf,
        hook: std::sync::Arc<ArtifactSyncOpenHook>,
    ) -> Self {
        let planned = plan_instances_owner(&instances_dir).expect("plan test storage root");
        let resolved = planned.resolved_instances_dir.clone();
        let owner = materialize_instances_owner(planned.parent, &planned.missing, &resolved)
            .expect("test storage root owner");
        let owner = Arc::new(owner);
        Self {
            images_dir,
            instances_dir: resolved,
            instances_owner: Some(owner),
            artifact_sync_open_hook: Some(hook),
            acquire_blocking_hook: None,
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

fn resolve_relative_instances_path(path: &Path, current_dir: &Path) -> PathBuf {
    current_dir.join(path)
}

fn plan_instances_owner(path: &Path) -> Result<PlannedFileStorageProvider> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let current_dir = std::env::current_dir().map_err(|error| BlazeError::StorageError {
            msg: format!("resolve storage instances root {}: {error}", path.display()),
        })?;
        resolve_relative_instances_path(path, &current_dir)
    };
    if absolute.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::Prefix(_)
        )
    }) {
        return Err(BlazeError::StorageError {
            msg: format!(
                "storage instances root {} contains an unsupported path component",
                path.display()
            ),
        });
    }
    let mut current = open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| BlazeError::StorageError {
        msg: format!("open storage root directory: {error}"),
    })?;
    let components = absolute
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(name) => Some(name.to_os_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut missing = Vec::new();
    for (index, name) in components.iter().enumerate() {
        match openat(
            &current,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(next) => {
                ensure_trusted_existing_path_component(&current, name, &next).map_err(|error| {
                    BlazeError::StorageError {
                        msg: format!("inspect storage instances root {}: {error}", path.display()),
                    }
                })?;
                current = next;
            }
            Err(Errno::NOENT) => {
                missing.extend(components[index..].iter().cloned());
                break;
            }
            Err(error) => {
                return Err(BlazeError::StorageError {
                    msg: format!("inspect storage instances root {}: {error}", path.display()),
                });
            }
        }
    }
    Ok(PlannedFileStorageProvider {
        resolved_instances_dir: absolute,
        parent: current,
        parent_path: components[..components.len() - missing.len()]
            .iter()
            .fold(PathBuf::from("/"), |path, component| path.join(component)),
        missing,
    })
}

fn revalidate_instances_owner_plan(planned: &PlannedFileStorageProvider) -> Result<()> {
    let current = open(
        &planned.parent_path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| BlazeError::StorageError {
        msg: format!(
            "revalidate storage instances ancestor {}: {error}",
            planned.parent_path.display()
        ),
    })?;
    ensure_same_object(
        &fstat(&planned.parent).map_err(std::io::Error::from)?,
        &current,
        "planned storage instances ancestor",
    )?;
    if let Some(component) = planned.missing.first() {
        match statat(&current, component, AtFlags::SYMLINK_NOFOLLOW) {
            Err(Errno::NOENT) => {}
            Ok(_) => {
                return Err(BlazeError::StorageError {
                    msg: format!(
                        "planned missing storage instances component {component:?} now exists"
                    ),
                });
            }
            Err(error) => return Err(std::io::Error::from(error).into()),
        }
    }
    Ok(())
}

enum DirectoryPublicationError {
    TargetExists,
    Clean(String),
    UnaddressableResidual(String),
    PublishedOwnedResidual(String),
}

struct RetainedDirectoryIdentity {
    metadata: rustix::fs::Stat,
    mount_id: u64,
}

struct PublishedDirectory {
    directory: std::os::fd::OwnedFd,
    identity: RetainedDirectoryIdentity,
}

fn verify_linked_directory_identity(
    parent: &std::os::fd::OwnedFd,
    name: &OsStr,
    expected: &RetainedDirectoryIdentity,
    label: &str,
) -> std::result::Result<(), String> {
    let linked = openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| format!("open {label}: {error}"))?;
    let metadata = fstat(&linked).map_err(|error| format!("inspect {label}: {error}"))?;
    if metadata.st_dev != expected.metadata.st_dev || metadata.st_ino != expected.metadata.st_ino {
        return Err(format!("{label} changed filesystem identity"));
    }
    let mount_id = opened_mount_id_for_owned_fd(&linked)
        .map_err(|error| format!("inspect {label} mount identity: {error}"))?;
    if mount_id != expected.mount_id {
        return Err(format!(
            "{label} changed mount identity {}->{mount_id}",
            expected.mount_id
        ));
    }
    Ok(())
}

fn slot_cleanup_is_retryable_by_id(
    parent: &std::os::fd::OwnedFd,
    name: &str,
    expected: &RetainedDirectoryIdentity,
) -> bool {
    if matches!(
        statat(parent, name, AtFlags::SYMLINK_NOFOLLOW),
        Err(Errno::NOENT)
    ) {
        // The directory was removed but its parent fsync may have failed.
        // release_by_id can safely retry that durability step.
        return true;
    }
    verify_linked_directory_identity(
        parent,
        OsStr::new(name),
        expected,
        "provider-owned slot directory",
    )
    .is_ok()
}

fn ensure_trusted_existing_path_component(
    parent: &std::os::fd::OwnedFd,
    name: &OsStr,
    child: &std::os::fd::OwnedFd,
) -> std::result::Result<(), String> {
    let parent_metadata =
        fstat(parent).map_err(|error| format!("inspect path-component parent: {error}"))?;
    let effective_uid = unsafe { libc::geteuid() };
    let trusted_parent_owner =
        parent_metadata.st_uid == 0 || parent_metadata.st_uid == effective_uid;
    let shared_write = parent_metadata.st_mode & (libc::S_IWGRP | libc::S_IWOTH) != 0;
    let sticky = parent_metadata.st_mode & libc::S_ISVTX != 0;
    if !trusted_parent_owner || (shared_write && !sticky) {
        return Err(format!(
            "path-component parent has unsafe owner or permissions (uid {}, mode {:04o})",
            parent_metadata.st_uid,
            parent_metadata.st_mode & 0o7777
        ));
    }

    let child_metadata = fstat(child)
        .map_err(|error| format!("inspect existing path component {name:?}: {error}"))?;
    if shared_write && child_metadata.st_uid != 0 && child_metadata.st_uid != effective_uid {
        return Err(format!(
            "existing path component {name:?} in a shared sticky parent is owned by untrusted uid {}",
            child_metadata.st_uid
        ));
    }
    let linked = statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|error| format!("revalidate existing path component {name:?}: {error}"))?;
    if linked.st_dev != child_metadata.st_dev || linked.st_ino != child_metadata.st_ino {
        return Err(format!(
            "existing path component {name:?} changed identity while it was opened"
        ));
    }
    Ok(())
}

fn ensure_trusted_publication_parent(
    parent: &std::os::fd::OwnedFd,
) -> std::result::Result<(), String> {
    let metadata = fstat(parent).map_err(|error| format!("inspect publication parent: {error}"))?;
    if FileType::from_raw_mode(metadata.st_mode) != FileType::Directory {
        return Err("publication parent is not a directory".to_string());
    }

    let effective_uid = unsafe { libc::geteuid() };
    let trusted_owner = metadata.st_uid == 0 || metadata.st_uid == effective_uid;
    let shared_write = metadata.st_mode & (libc::S_IWGRP | libc::S_IWOTH) != 0;
    // A sticky directory protects existing entries, but another user can still
    // reserve a not-yet-published sandbox name. Reject shared writers so later
    // recovery can never mistake such a reservation for provider-owned storage.
    if !trusted_owner || shared_write {
        return Err(format!(
            "publication parent has unsafe owner or permissions (uid {}, mode {:04o}); it must be owned by root or uid {} and must not be writable by group or other users",
            metadata.st_uid,
            metadata.st_mode & 0o7777,
            effective_uid
        ));
    }
    Ok(())
}

fn cleanup_owned_empty_directory_link(
    parent: &std::os::fd::OwnedFd,
    name: &OsStr,
    expected: &rustix::fs::Stat,
) -> std::result::Result<(), String> {
    let observed = match statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(observed) => observed,
        Err(Errno::NOENT) => return Err(format!("{name:?} disappeared before cleanup")),
        Err(error) => return Err(format!("inspect {name:?} before cleanup: {error}")),
    };
    if observed.st_dev != expected.st_dev || observed.st_ino != expected.st_ino {
        return Err(format!("{name:?} changed identity before cleanup"));
    }
    unlinkat(parent, name, AtFlags::REMOVEDIR)
        .map_err(|error| format!("remove {name:?}: {error}"))?;
    fsync(parent)
        .map_err(|error| format!("synchronize parent after removing {name:?}: {error}"))?;
    Ok(())
}

fn staging_publication_failure(
    parent: &std::os::fd::OwnedFd,
    staging_name: &OsStr,
    staging_identity: &rustix::fs::Stat,
    error: String,
) -> DirectoryPublicationError {
    match cleanup_owned_empty_directory_link(parent, staging_name, staging_identity) {
        Ok(()) => DirectoryPublicationError::Clean(error),
        Err(cleanup_error) => DirectoryPublicationError::UnaddressableResidual(format!(
            "{error}; private staging cleanup could not be confirmed: {cleanup_error}"
        )),
    }
}

enum PublishedDirectoryCleanupError {
    Unaddressable(String),
    Addressable(String),
}

fn cleanup_published_empty_directory_link(
    parent: &std::os::fd::OwnedFd,
    name: &OsStr,
    expected: &RetainedDirectoryIdentity,
) -> std::result::Result<(), PublishedDirectoryCleanupError> {
    verify_linked_directory_identity(parent, name, expected, "published directory")
        .map_err(PublishedDirectoryCleanupError::Unaddressable)?;
    let linked = statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|error| {
        PublishedDirectoryCleanupError::Unaddressable(format!(
            "inspect {name:?} before unlink: {error}"
        ))
    })?;
    if linked.st_dev != expected.metadata.st_dev || linked.st_ino != expected.metadata.st_ino {
        return Err(PublishedDirectoryCleanupError::Unaddressable(format!(
            "{name:?} changed identity before unlink"
        )));
    }
    unlinkat(parent, name, AtFlags::REMOVEDIR).map_err(|error| {
        PublishedDirectoryCleanupError::Addressable(format!("remove {name:?}: {error}"))
    })?;
    fsync(parent).map_err(|error| {
        PublishedDirectoryCleanupError::Addressable(format!(
            "synchronize parent after removing {name:?}: {error}"
        ))
    })?;
    Ok(())
}

fn published_directory_failure(
    parent: &std::os::fd::OwnedFd,
    target_name: &OsStr,
    directory_identity: &RetainedDirectoryIdentity,
    error: String,
) -> DirectoryPublicationError {
    match cleanup_published_empty_directory_link(parent, target_name, directory_identity) {
        Ok(()) => DirectoryPublicationError::Clean(error),
        Err(PublishedDirectoryCleanupError::Unaddressable(cleanup_error)) => {
            DirectoryPublicationError::UnaddressableResidual(format!(
                "{error}; published directory cleanup was unsafe: {cleanup_error}"
            ))
        }
        Err(PublishedDirectoryCleanupError::Addressable(cleanup_error)) => {
            DirectoryPublicationError::PublishedOwnedResidual(format!(
                "{error}; published directory cleanup could not be confirmed: {cleanup_error}"
            ))
        }
    }
}

fn publish_new_directory_at<F>(
    parent: &std::os::fd::OwnedFd,
    target_name: &OsStr,
    final_mode: Mode,
    after_publish: F,
) -> std::result::Result<PublishedDirectory, DirectoryPublicationError>
where
    F: FnOnce(),
{
    ensure_trusted_publication_parent(parent).map_err(DirectoryPublicationError::Clean)?;

    let staging_name = loop {
        let candidate = OsString::from(format!(".blaze-dir-{}.tmp", Uuid::new_v4()));
        if candidate == target_name {
            continue;
        }
        match mkdirat(parent, &candidate, Mode::from_bits_truncate(0o700)) {
            Ok(()) => break candidate,
            Err(Errno::EXIST) => continue,
            Err(error) => {
                return Err(DirectoryPublicationError::Clean(format!(
                    "create private staging directory: {error}"
                )));
            }
        }
    };
    let directory = match openat(
        parent,
        &staging_name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(directory) => directory,
        Err(error) => {
            let staging_identity = match statat(parent, &staging_name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(identity) => identity,
                Err(_) => {
                    return Err(DirectoryPublicationError::UnaddressableResidual(format!(
                        "retain private staging directory: {error}; private staging cleanup could not be confirmed"
                    )));
                }
            };
            return Err(staging_publication_failure(
                parent,
                &staging_name,
                &staging_identity,
                format!("retain private staging directory: {error}"),
            ));
        }
    };
    let directory_metadata = match fstat(&directory) {
        Ok(identity) => identity,
        Err(error) => {
            let fallback_identity = match statat(parent, &staging_name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(identity) => identity,
                Err(_) => {
                    return Err(DirectoryPublicationError::UnaddressableResidual(format!(
                        "inspect private staging directory: {error}; private staging cleanup could not be confirmed"
                    )));
                }
            };
            return Err(staging_publication_failure(
                parent,
                &staging_name,
                &fallback_identity,
                format!("inspect private staging directory: {error}"),
            ));
        }
    };
    let effective_uid = unsafe { libc::geteuid() };
    if FileType::from_raw_mode(directory_metadata.st_mode) != FileType::Directory
        || directory_metadata.st_uid != effective_uid
        || directory_metadata.st_mode & (libc::S_IWGRP | libc::S_IWOTH) != 0
    {
        return Err(staging_publication_failure(
            parent,
            &staging_name,
            &directory_metadata,
            format!(
                "private staging directory has unexpected owner or permissions (uid {}, mode {:04o})",
                directory_metadata.st_uid,
                directory_metadata.st_mode & 0o7777
            ),
        ));
    }
    let mount_id = match opened_mount_id_for_owned_fd(&directory) {
        Ok(mount_id) => mount_id,
        Err(error) => {
            return Err(staging_publication_failure(
                parent,
                &staging_name,
                &directory_metadata,
                format!("inspect private staging mount identity: {error}"),
            ));
        }
    };
    let directory_identity = RetainedDirectoryIdentity {
        metadata: directory_metadata,
        mount_id,
    };
    if let Err(error) = fchmod(&directory, final_mode) {
        return Err(staging_publication_failure(
            parent,
            &staging_name,
            &directory_identity.metadata,
            format!("set private staging directory permissions: {error}"),
        ));
    }
    if let Err(error) = fsync(&directory) {
        return Err(staging_publication_failure(
            parent,
            &staging_name,
            &directory_identity.metadata,
            format!("synchronize private staging directory: {error}"),
        ));
    }

    match renameat_with(
        parent,
        &staging_name,
        parent,
        target_name,
        RenameFlags::NOREPLACE,
    ) {
        Ok(()) => {}
        Err(Errno::EXIST) => {
            let cleanup = cleanup_owned_empty_directory_link(
                parent,
                &staging_name,
                &directory_identity.metadata,
            );
            return match cleanup {
                Ok(()) => Err(DirectoryPublicationError::TargetExists),
                Err(cleanup_error) => {
                    Err(DirectoryPublicationError::UnaddressableResidual(format!(
                        "target already exists and private staging cleanup could not be confirmed: {cleanup_error}"
                    )))
                }
            };
        }
        Err(error) => {
            return Err(staging_publication_failure(
                parent,
                &staging_name,
                &directory_identity.metadata,
                format!("publish directory without replacement: {error}"),
            ));
        }
    }

    if let Err(error) = fsync(parent) {
        return Err(published_directory_failure(
            parent,
            target_name,
            &directory_identity,
            format!("synchronize publication parent: {error}"),
        ));
    }
    after_publish();

    match verify_linked_directory_identity(
        parent,
        target_name,
        &directory_identity,
        "published directory",
    ) {
        Ok(()) => Ok(PublishedDirectory {
            directory,
            identity: directory_identity,
        }),
        Err(error) => Err(DirectoryPublicationError::UnaddressableResidual(format!(
            "published directory identity could not be verified: {error}"
        ))),
    }
}

fn materialize_instances_owner(
    mut current: std::os::fd::OwnedFd,
    missing: &[OsString],
    resolved: &Path,
) -> Result<std::os::fd::OwnedFd> {
    let mut walked = resolved.to_path_buf();
    for _ in missing {
        walked.pop();
    }
    for name in missing {
        walked.push(name);
        current = match publish_new_directory_at(
            &current,
            name,
            Mode::from_bits_truncate(0o750),
            || {},
        ) {
            Ok(published) => published.directory,
            Err(DirectoryPublicationError::TargetExists) => {
                return Err(BlazeError::StorageError {
                    msg: format!(
                        "storage instances path {} appeared after startup planning",
                        walked.display()
                    ),
                });
            }
            Err(
                DirectoryPublicationError::Clean(error)
                | DirectoryPublicationError::UnaddressableResidual(error)
                | DirectoryPublicationError::PublishedOwnedResidual(error),
            ) => {
                return Err(BlazeError::StorageError {
                    msg: format!(
                        "create storage instances path {}: {error}",
                        walked.display()
                    ),
                });
            }
        };
    }
    let directory = current;
    ensure_trusted_publication_parent(&directory).map_err(|error| BlazeError::StorageError {
        msg: format!(
            "storage instances root {} cannot safely publish slots: {error}",
            resolved.display()
        ),
    })?;
    if let Err(error) = flock(&directory, FlockOperation::NonBlockingLockExclusive) {
        return Err(BlazeError::StorageError {
            msg: if error == Errno::WOULDBLOCK {
                format!(
                    "storage instances root {} is already owned by another daemon",
                    resolved.display()
                )
            } else {
                format!(
                    "lock storage instances root {}: {error}",
                    resolved.display()
                )
            },
        });
    }
    Ok(directory)
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

#[async_trait]
impl StorageProvider for FileStorageProvider {
    async fn probe(&self) -> Result<bool> {
        Ok(self.images_dir.exists() && self.instances_owner.is_some())
    }

    async fn acquire(
        &self,
        opts: &AcquireOpts,
    ) -> std::result::Result<StorageSlot, StorageAcquireError> {
        crate::failpoint::storage("storage-acquire")?;
        let slot = self.slot_for_id(&opts.instance_id)?;

        let owner = self.instances_owner.as_ref().cloned().ok_or_else(|| {
            StorageAcquireError::clean(BlazeError::StorageError {
                msg: "storage provider does not retain an instances-root owner".to_string(),
            })
        })?;
        #[cfg(test)]
        let failpoint_context = crate::failpoint::capture_test_context();
        #[cfg(test)]
        let acquire_hook = self.acquire_blocking_hook.clone();
        #[cfg(test)]
        let finish_hook = self.acquire_blocking_hook.clone();
        let after_publish = move || {
            #[cfg(test)]
            if let Some(hook) = acquire_hook {
                hook.pause_after_publish();
            }
        };
        let instance_id = opts.instance_id.clone();
        let instances_dir = self.instances_dir.clone();
        let images_dir = self.images_dir.clone();
        let rootfs_size = opts.rootfs_size;
        let mem_size = opts.mem_size;
        let transaction_slot = slot.clone();
        let result = tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            let result = crate::failpoint::with_test_context(failpoint_context, || {
                acquire_slot_blocking(
                    owner,
                    transaction_slot,
                    instances_dir,
                    images_dir,
                    rootfs_size,
                    mem_size,
                    after_publish,
                )
            });
            #[cfg(not(test))]
            let result = acquire_slot_blocking(
                owner,
                transaction_slot,
                instances_dir,
                images_dir,
                rootfs_size,
                mem_size,
                after_publish,
            );
            #[cfg(test)]
            if let Some(hook) = finish_hook {
                hook.finish();
            }
            result
        })
        .await;
        match result {
            Ok(Ok(())) => Ok(slot),
            Ok(Err(error)) => Err(*error),
            Err(error) => Err(StorageAcquireError::with_manual_cleanup_required(
                BlazeError::StorageError {
                    msg: format!(
                        "acquire '{instance_id}': blocking transaction failed to join: {error}; outcome is unknown"
                    ),
                },
            )),
        }
    }

    async fn release(&self, slot: StorageSlot) -> Result<()> {
        crate::failpoint::storage("storage-release")?;
        // Re-derive the canonical path from instances_dir + slot.id. Do not
        // trust path strings carried in a persisted or externally built slot.
        validate_instance_id(&slot.id)?;
        let Some(owner) = self.instances_owner.as_ref().cloned() else {
            return Err(BlazeError::StorageError {
                msg: "storage provider does not retain an instances-root owner".to_string(),
            });
        };
        let id = slot.id.clone();
        tokio::task::spawn_blocking(move || remove_slot_tree(&owner, &id))
            .await
            .map_err(|error| BlazeError::StorageError {
                msg: format!("release '{}': join cleanup: {error}", slot.id),
            })??;
        Ok(())
    }

    async fn release_by_id(&self, instance_id: &str) -> Result<()> {
        let slot = self.slot_for_id(instance_id)?;
        self.release(slot).await
    }

    async fn reconstruct(&self, instance_id: &str) -> Result<StorageSlot> {
        let slot = self.slot_for_id(instance_id)?;
        let display_slot = slot.instance_dir.clone();
        let owner =
            self.instances_owner
                .as_ref()
                .cloned()
                .ok_or_else(|| BlazeError::StorageError {
                    msg: "storage provider does not retain an instances-root owner".to_string(),
                })?;
        let id = instance_id.to_string();
        let display_root = self.instances_dir.clone();
        tokio::task::spawn_blocking(move || {
            validate_complete_slot(&owner, &id, &display_root, &display_slot)
        })
        .await
        .map_err(|error| BlazeError::StorageError {
            msg: format!("reconstruct '{instance_id}': join validation: {error}"),
        })??;
        Ok(slot)
    }

    async fn sync_artifacts(&self, slot: &StorageSlot) -> Result<()> {
        crate::failpoint::storage("sync-artifacts")?;
        // Never trust paths carried by a runtime or persisted slot. Rebuild
        // the complete provider-owned artifact set from the validated ID.
        let canonical = self.slot_for_id(&slot.id)?;
        let owner =
            self.instances_owner
                .as_ref()
                .cloned()
                .ok_or_else(|| BlazeError::StorageError {
                    msg: "storage provider does not retain an instances-root owner".to_string(),
                })?;
        let slot_id = slot.id.clone();
        let directory_fd = open_required_slot_path(
            &slot.id,
            &canonical.instance_dir,
            RequiredPathType::Directory,
            move || {
                openat(
                    &owner,
                    slot_id.as_str(),
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

    async fn drain_pool(&self) -> Result<usize> {
        Ok(0)
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
            classify_required_slot_open_error(
                "sync artifacts",
                &task_instance_id,
                &task_path,
                required_type.description(),
                error,
            )
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

fn remove_slot_tree(root: &std::os::fd::OwnedFd, instance_id: &str) -> Result<()> {
    remove_slot_tree_with_hooks(
        root,
        instance_id,
        &mut || Ok(()),
        &mut || crate::failpoint::storage("storage-release-after-entry"),
        &mut || Ok(()),
        &mut || crate::failpoint::storage("storage-release-before-root-sync"),
    )
}

fn remove_slot_tree_matching(
    root: &std::os::fd::OwnedFd,
    instance_id: &str,
    expected: &RetainedDirectoryIdentity,
) -> Result<()> {
    remove_slot_tree_with_identity_and_hooks(
        root,
        instance_id,
        Some(expected),
        &mut || Ok(()),
        &mut || crate::failpoint::storage("storage-release-after-entry"),
        &mut || Ok(()),
        &mut || crate::failpoint::storage("storage-release-before-root-sync"),
    )
}

fn remove_slot_tree_with_hooks<I, E, U, S>(
    root: &std::os::fd::OwnedFd,
    instance_id: &str,
    after_inspect: &mut I,
    after_entry: &mut E,
    before_unlink: &mut U,
    before_root_sync: &mut S,
) -> Result<()>
where
    I: FnMut() -> Result<()>,
    E: FnMut() -> Result<()>,
    U: FnMut() -> Result<()>,
    S: FnMut() -> Result<()>,
{
    remove_slot_tree_with_identity_and_hooks(
        root,
        instance_id,
        None,
        after_inspect,
        after_entry,
        before_unlink,
        before_root_sync,
    )
}

fn remove_slot_tree_with_identity_and_hooks<I, E, U, S>(
    root: &std::os::fd::OwnedFd,
    instance_id: &str,
    expected: Option<&RetainedDirectoryIdentity>,
    after_inspect: &mut I,
    after_entry: &mut E,
    before_unlink: &mut U,
    before_root_sync: &mut S,
) -> Result<()>
where
    I: FnMut() -> Result<()>,
    E: FnMut() -> Result<()>,
    U: FnMut() -> Result<()>,
    S: FnMut() -> Result<()>,
{
    let root_mount_id =
        opened_mount_id_for_owned_fd(root).map_err(|error| BlazeError::StorageError {
            msg: format!("release '{instance_id}': inspect instances-root mount: {error}"),
        })?;
    let inspected = match statat(root, instance_id, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::Directory => stat,
        Ok(_) => {
            return Err(BlazeError::StorageError {
                msg: format!("release '{instance_id}': refusing non-directory slot"),
            });
        }
        Err(Errno::NOENT) => {
            sync_instances_root(root, instance_id, "missing slot", before_root_sync)?;
            return Ok(());
        }
        Err(error) => {
            return Err(BlazeError::StorageError {
                msg: format!("release '{instance_id}': inspect retained slot: {error}"),
            });
        }
    };
    if let Some(expected) = expected {
        ensure_same_metadata_object(&expected.metadata, &inspected, "provider-owned slot root")?;
    }
    after_inspect()?;
    let directory = match openat(
        root,
        instance_id,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(directory) => directory,
        Err(Errno::NOENT) => {
            return Err(BlazeError::StorageError {
                msg: format!("release '{instance_id}': slot disappeared after validation"),
            });
        }
        Err(error) => {
            return Err(BlazeError::StorageError {
                msg: format!("release '{instance_id}': open retained slot: {error}"),
            });
        }
    };
    ensure_same_object(&inspected, &directory, "provider slot root")?;
    if let Some(expected) = expected {
        ensure_same_object(&expected.metadata, &directory, "provider-owned slot root")?;
        let mount_id =
            opened_mount_id_for_owned_fd(&directory).map_err(|error| BlazeError::StorageError {
                msg: format!("release '{instance_id}': inspect owned slot mount: {error}"),
            })?;
        if mount_id != expected.mount_id {
            return Err(BlazeError::StorageError {
                msg: format!(
                    "release '{instance_id}': provider-owned slot changed mount identity {}->{mount_id}",
                    expected.mount_id
                ),
            });
        }
    }
    remove_directory_contents_on_mount(
        &directory,
        root_mount_id,
        DirEntry::file_type,
        opened_mount_id_for_owned_fd,
        after_entry,
    )
    .map_err(|error| BlazeError::StorageError {
        msg: format!("release '{instance_id}': remove contents: {error}"),
    })?;
    before_unlink()?;
    let linked =
        statat(root, instance_id, AtFlags::SYMLINK_NOFOLLOW).map_err(std::io::Error::from)?;
    ensure_same_metadata_object(&inspected, &linked, "provider slot root")?;
    ensure_same_object(&linked, &directory, "provider slot root")?;
    unlinkat(root, instance_id, AtFlags::REMOVEDIR).map_err(|error| BlazeError::StorageError {
        msg: format!("release '{instance_id}': unlink slot: {error}"),
    })?;
    sync_instances_root(root, instance_id, "removed slot", before_root_sync)?;
    Ok(())
}

fn sync_instances_root<S>(
    root: &std::os::fd::OwnedFd,
    instance_id: &str,
    state: &str,
    before_root_sync: &mut S,
) -> Result<()>
where
    S: FnMut() -> Result<()>,
{
    before_root_sync()?;
    fsync(root).map_err(|error| BlazeError::StorageError {
        msg: format!("release '{instance_id}': synchronize {state}: {error}"),
    })?;
    Ok(())
}

fn validate_complete_slot(
    root: &std::os::fd::OwnedFd,
    instance_id: &str,
    display_root: &Path,
    display_slot: &Path,
) -> Result<()> {
    let inspected = inspect_required_slot_entry(
        root,
        instance_id,
        display_slot,
        "directory",
        FileType::Directory,
    )?;
    let directory = openat(
        root,
        instance_id,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| BlazeError::StorageError {
        msg: format!(
            "reconstruct '{instance_id}': open {} after validation: {error}",
            display_slot.display()
        ),
    })?;
    ensure_same_object(&inspected, &directory, "provider slot root")?;
    for name in ["rootfs.ext4", "mem.bin", "mem.diff", "rootfs.diff"] {
        let display_path = display_slot.join(name);
        let inspected = inspect_required_slot_entry(
            &directory,
            instance_id,
            &display_path,
            "file",
            FileType::RegularFile,
        )?;
        let file = openat(
            &directory,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|error| BlazeError::StorageError {
            msg: format!(
                "reconstruct '{instance_id}': open {} after validation: {error}",
                display_path.display()
            ),
        })?;
        ensure_same_object(&inspected, &file, "provider slot file")?;
    }
    revalidate_backend_visible_root(root, display_root, "reconstruct")?;
    Ok(())
}

fn revalidate_backend_visible_root(
    retained: &std::os::fd::OwnedFd,
    configured: &Path,
    operation: &str,
) -> Result<()> {
    let visible = open(
        configured,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| BlazeError::StorageError {
        msg: format!(
            "{operation}: configured instances root {} no longer opens the retained root: {error}",
            configured.display()
        ),
    })?;
    let retained_metadata = fstat(retained).map_err(|error| BlazeError::StorageError {
        msg: format!(
            "{operation}: inspect retained instances root {}: {error}",
            configured.display()
        ),
    })?;
    ensure_same_object(
        &retained_metadata,
        &visible,
        "backend-visible storage instances root",
    )?;
    let retained_mount =
        opened_mount_id_for_owned_fd(retained).map_err(|error| BlazeError::StorageError {
            msg: format!(
                "{operation}: inspect retained instances-root mount for {}: {error}",
                configured.display()
            ),
        })?;
    let visible_mount =
        opened_mount_id_for_owned_fd(&visible).map_err(|error| BlazeError::StorageError {
            msg: format!(
                "{operation}: inspect configured instances-root mount for {}: {error}",
                configured.display()
            ),
        })?;
    if retained_mount != visible_mount {
        return Err(BlazeError::StorageError {
            msg: format!(
                "{operation}: configured instances root {} changed mount identity {retained_mount}->{visible_mount}",
                configured.display()
            ),
        });
    }
    Ok(())
}

fn inspect_required_slot_entry(
    directory: &std::os::fd::OwnedFd,
    instance_id: &str,
    display_path: &Path,
    expected: &'static str,
    expected_type: FileType,
) -> Result<rustix::fs::Stat> {
    let name = display_path
        .file_name()
        .ok_or_else(|| BlazeError::StorageError {
            msg: format!(
                "reconstruct '{instance_id}': {} has no final component",
                display_path.display()
            ),
        })?;
    let metadata = statat(directory, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|error| {
        classify_required_slot_open_error("reconstruct", instance_id, display_path, expected, error)
    })?;
    if FileType::from_raw_mode(metadata.st_mode) != expected_type {
        return Err(BlazeError::StorageIncomplete {
            instance_id: instance_id.to_string(),
            path: display_path.to_path_buf(),
            expected,
        });
    }
    Ok(metadata)
}

#[cfg(test)]
fn remove_directory_contents_with_type_hint<F>(
    directory: &std::os::fd::OwnedFd,
    type_hint: F,
) -> Result<()>
where
    F: Copy + Fn(&DirEntry) -> FileType,
{
    let mount_id =
        opened_mount_id_for_owned_fd(directory).map_err(|error| BlazeError::StorageError {
            msg: format!("inspect provider slot mount: {error}"),
        })?;
    remove_directory_contents_on_mount(
        directory,
        mount_id,
        type_hint,
        opened_mount_id_for_owned_fd,
        &mut || crate::failpoint::storage("storage-release-after-entry"),
    )
}

fn remove_directory_contents_on_mount<F, M, E>(
    directory: &std::os::fd::OwnedFd,
    expected_mount_id: u64,
    type_hint: F,
    mount_id: M,
    after_entry: &mut E,
) -> Result<()>
where
    F: Copy + Fn(&DirEntry) -> FileType,
    M: Copy + Fn(&std::os::fd::OwnedFd) -> std::io::Result<u64>,
    E: FnMut() -> Result<()>,
{
    let observed_mount_id = mount_id(directory).map_err(|error| BlazeError::StorageError {
        msg: format!("inspect provider slot mount: {error}"),
    })?;
    if observed_mount_id != expected_mount_id {
        return Err(BlazeError::StorageError {
            msg: format!(
                "refusing to remove provider slot across mount boundary \
                 {expected_mount_id}->{observed_mount_id}"
            ),
        });
    }
    for entry in Dir::read_from(directory).map_err(std::io::Error::from)? {
        let entry = entry.map_err(std::io::Error::from)?;
        let name = entry.file_name();
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        let metadata =
            statat(directory, name, AtFlags::SYMLINK_NOFOLLOW).map_err(std::io::Error::from)?;
        match file_type_from_metadata(type_hint(&entry), &metadata) {
            FileType::Directory => {
                let child = openat(
                    directory,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(std::io::Error::from)?;
                ensure_same_object(&metadata, &child, "provider slot directory")?;
                remove_directory_contents_on_mount(
                    &child,
                    expected_mount_id,
                    type_hint,
                    mount_id,
                    after_entry,
                )?;
                let linked = statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
                    .map_err(std::io::Error::from)?;
                ensure_same_metadata_object(&metadata, &linked, "provider slot directory")?;
                unlinkat(directory, name, AtFlags::REMOVEDIR).map_err(std::io::Error::from)?;
            }
            FileType::RegularFile => {
                if metadata.st_nlink != 1 {
                    return Err(BlazeError::StorageError {
                        msg: "refusing to remove provider slot file with multiple hard links"
                            .to_string(),
                    });
                }
                let file = openat(
                    directory,
                    name,
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                    Mode::empty(),
                )
                .map_err(std::io::Error::from)?;
                ensure_same_object(&metadata, &file, "provider slot file")?;
                let observed_mount_id =
                    mount_id(&file).map_err(|error| BlazeError::StorageError {
                        msg: format!("inspect provider slot file mount: {error}"),
                    })?;
                if observed_mount_id != expected_mount_id {
                    return Err(BlazeError::StorageError {
                        msg: format!(
                            "refusing to remove provider slot across mount boundary \
                             {expected_mount_id}->{observed_mount_id}"
                        ),
                    });
                }
                let linked = statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
                    .map_err(std::io::Error::from)?;
                ensure_same_metadata_object(&metadata, &linked, "provider slot file")?;
                ensure_same_object(&linked, &file, "provider slot file")?;
                unlinkat(directory, name, AtFlags::empty()).map_err(std::io::Error::from)?;
            }
            kind => {
                return Err(BlazeError::StorageError {
                    msg: format!(
                        "refusing to remove provider slot entry with unsupported type {kind:?}"
                    ),
                });
            }
        }
        after_entry()?;
    }
    fsync(directory).map_err(std::io::Error::from)?;
    Ok(())
}

fn file_type_from_metadata(
    _directory_entry_hint: FileType,
    metadata: &rustix::fs::Stat,
) -> FileType {
    FileType::from_raw_mode(metadata.st_mode)
}

fn ensure_same_object(
    expected: &rustix::fs::Stat,
    opened: &std::os::fd::OwnedFd,
    label: &str,
) -> Result<()> {
    let opened = fstat(opened).map_err(std::io::Error::from)?;
    ensure_same_metadata_object(expected, &opened, label)
}

fn ensure_same_metadata_object(
    expected: &rustix::fs::Stat,
    observed: &rustix::fs::Stat,
    label: &str,
) -> Result<()> {
    if expected.st_dev == observed.st_dev && expected.st_ino == observed.st_ino {
        Ok(())
    } else {
        Err(BlazeError::StorageError {
            msg: format!("{label} changed identity while it was retained"),
        })
    }
}

fn acquire_slot_blocking<F>(
    owner: Arc<std::os::fd::OwnedFd>,
    slot: StorageSlot,
    instances_dir: PathBuf,
    images_dir: PathBuf,
    rootfs_size: u64,
    mem_size: u64,
    after_publish: F,
) -> std::result::Result<(), Box<StorageAcquireError>>
where
    F: FnOnce(),
{
    let instance_id = slot.id.clone();
    crate::failpoint::pause_blocking("storage-acquire-before-slot-publish");
    let published = match publish_new_directory_at(
        &owner,
        OsStr::new(&instance_id),
        Mode::from_bits_truncate(0o750),
        || {},
    ) {
        Ok(published) => published,
        Err(DirectoryPublicationError::TargetExists) => {
            return Err(Box::new(StorageAcquireError::clean(
                BlazeError::StorageError {
                    msg: format!("acquire '{instance_id}': instance directory already exists"),
                },
            )));
        }
        Err(DirectoryPublicationError::Clean(error)) => {
            return Err(Box::new(StorageAcquireError::clean(
                BlazeError::StorageError {
                    msg: format!("acquire '{instance_id}': create dir: {error}"),
                },
            )));
        }
        Err(DirectoryPublicationError::PublishedOwnedResidual(error)) => {
            return Err(Box::new(StorageAcquireError::with_residual(
                BlazeError::StorageError {
                    msg: format!("acquire '{instance_id}': create dir: {error}"),
                },
                slot,
            )));
        }
        Err(DirectoryPublicationError::UnaddressableResidual(error)) => {
            return Err(Box::new(StorageAcquireError::with_manual_cleanup_required(
                BlazeError::StorageError {
                    msg: format!("acquire '{instance_id}': create dir: {error}"),
                },
            )));
        }
    };
    let PublishedDirectory {
        directory: slot_directory,
        identity: slot_identity,
    } = published;
    after_publish();
    crate::failpoint::pause_blocking("storage-acquire-after-slot-publish");

    let setup = (|| -> Result<()> {
        crate::failpoint::storage("storage-acquire-retain-slot")?;
        create_or_copy_at_blocking(
            &images_dir.join("rootfs.ext4"),
            &slot_directory,
            "rootfs.ext4",
            rootfs_size,
        )?;
        create_or_copy_at_blocking(
            &images_dir.join("mem.bin"),
            &slot_directory,
            "mem.bin",
            mem_size,
        )?;
        create_empty_at_blocking(&slot_directory, "mem.diff")?;
        create_empty_at_blocking(&slot_directory, "rootfs.diff")?;
        crate::failpoint::storage("storage-acquire-artifacts")?;
        crate::failpoint::storage("storage-acquire-before-root-sync")?;
        fsync(&owner).map_err(|error| BlazeError::StorageError {
            msg: format!("acquire '{instance_id}': synchronize instances root: {error}"),
        })?;
        // Backends still consume ordinary paths. Verify that those paths name
        // the retained root immediately before returning them; provider-local
        // cleanup remains descriptor-relative if this check fails.
        revalidate_backend_visible_root(&owner, &instances_dir, "acquire")?;
        verify_linked_directory_identity(
            &owner,
            OsStr::new(&instance_id),
            &slot_identity,
            "published slot directory",
        )
        .map_err(|error| BlazeError::StorageError {
            msg: format!("acquire '{instance_id}': {error}"),
        })?;
        Ok(())
    })();
    let Err(setup_error) = setup else {
        return Ok(());
    };

    let rollback_result = crate::failpoint::storage("storage-acquire-rollback")
        .and_then(|_| remove_slot_tree_matching(&owner, &instance_id, &slot_identity));
    match rollback_result {
        Ok(()) => Err(Box::new(StorageAcquireError::clean(
            BlazeError::StorageError {
                msg: format!(
                    "acquire '{instance_id}': slot setup failed, rolled back: {setup_error}"
                ),
            },
        ))),
        Err(cleanup_error)
            if slot_cleanup_is_retryable_by_id(&owner, &instance_id, &slot_identity) =>
        {
            Err(Box::new(StorageAcquireError::with_residual(
                BlazeError::StorageError {
                    msg: format!(
                        "acquire '{instance_id}': slot setup failed ({setup_error}); rollback failed for {}: {cleanup_error}",
                        slot.instance_dir.display()
                    ),
                },
                slot,
            )))
        }
        Err(cleanup_error) => Err(Box::new(StorageAcquireError::with_manual_cleanup_required(
            BlazeError::StorageError {
                msg: format!(
                    "acquire '{instance_id}': slot setup failed ({setup_error}); rollback could not safely address {}: {cleanup_error}",
                    slot.instance_dir.display()
                ),
            },
        ))),
    }
}

fn create_or_copy_at_blocking(
    source: &Path,
    directory: &std::os::fd::OwnedFd,
    target_name: &str,
    size: u64,
) -> std::io::Result<()> {
    let target = openat(
        directory,
        target_name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_bits_truncate(0o640),
    )
    .map_err(std::io::Error::from)?;
    let mut target = std::fs::File::from(target);
    if source.is_file() {
        let mut source = std::fs::File::open(source)?;
        std::io::copy(&mut source, &mut target)?;
    } else if size > 0 {
        target.set_len(size)?;
    }
    Ok(())
}

fn create_empty_at_blocking(
    directory: &std::os::fd::OwnedFd,
    target_name: &str,
) -> std::io::Result<()> {
    let file = openat(
        directory,
        target_name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_bits_truncate(0o640),
    )
    .map_err(std::io::Error::from)?;
    drop(std::fs::File::from(file));
    Ok(())
}

fn classify_required_slot_open_error(
    operation: &str,
    instance_id: &str,
    path: &Path,
    expected: &'static str,
    error: Errno,
) -> BlazeError {
    if matches!(error, Errno::NOENT | Errno::NOTDIR | Errno::LOOP) {
        BlazeError::StorageIncomplete {
            instance_id: instance_id.to_string(),
            path: path.to_path_buf(),
            expected,
        }
    } else {
        BlazeError::StorageError {
            msg: format!(
                "{operation} '{instance_id}': open {}: {error}",
                path.display()
            ),
        }
    }
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
    use blaze_core::storage::StorageAcquireDisposition;

    fn write_complete_replacement_slot(root: &Path, id: &str, marker: &[u8]) {
        let slot = root.join(id);
        std::fs::create_dir(&slot).expect("replacement slot");
        for name in ["rootfs.ext4", "mem.bin", "mem.diff", "rootfs.diff"] {
            std::fs::write(slot.join(name), marker).expect("replacement artifact");
        }
    }

    #[test]
    fn plan_materializes_multiple_missing_components_without_rewalking() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("one/two/instances");
        let planned = FileStorageProvider::plan(target.clone()).expect("read-only plan");
        assert!(!target.exists());

        let prepared = FileStorageProvider::prepare(planned).expect("materialize planned root");

        assert_eq!(prepared.resolved_instances_dir, target);
        assert!(target.is_dir());
    }

    #[test]
    fn relative_instances_path_resolves_against_current_directory() {
        let current = Path::new("/srv/blaze");
        let relative = Path::new("missing/instances");

        assert_eq!(
            resolve_relative_instances_path(relative, current),
            current.join(relative)
        );
    }

    #[test]
    fn directory_publication_never_replaces_an_existing_target() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("instances");
        std::fs::create_dir(&target).expect("existing target");
        std::fs::write(target.join("sentinel"), b"existing").expect("existing sentinel");
        let parent = open(
            temp.path(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .expect("publication parent");

        let error = match publish_new_directory_at(
            &parent,
            OsStr::new("instances"),
            Mode::from_bits_truncate(0o750),
            || {},
        ) {
            Ok(_) => panic!("publication must not replace an existing target"),
            Err(error) => error,
        };

        assert!(matches!(error, DirectoryPublicationError::TargetExists));
        assert_eq!(
            std::fs::read(target.join("sentinel")).expect("existing sentinel remains"),
            b"existing"
        );
        assert!(
            temp.path()
                .read_dir()
                .expect("publication parent")
                .all(|entry| !entry
                    .expect("directory entry")
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".blaze-dir-"))
        );
    }

    #[test]
    fn directory_publication_rejects_a_post_publish_replacement() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("instances");
        let detached = temp.path().join("instances-retained");
        let parent = open(
            temp.path(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .expect("publication parent");

        let error = match publish_new_directory_at(
            &parent,
            OsStr::new("instances"),
            Mode::from_bits_truncate(0o750),
            || {
                std::fs::rename(&target, &detached).expect("detach published directory");
                std::fs::create_dir(&target).expect("replacement directory");
                std::fs::write(target.join("sentinel"), b"replacement")
                    .expect("replacement sentinel");
            },
        ) {
            Ok(_) => panic!("publication must not retain a replacement directory"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            DirectoryPublicationError::UnaddressableResidual(ref message)
                if message.contains("changed filesystem identity")
        ));
        assert!(detached.is_dir());
        assert_eq!(
            std::fs::read(target.join("sentinel")).expect("replacement remains"),
            b"replacement"
        );
    }

    #[test]
    fn prepare_rejects_a_shared_writable_instances_root() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("instances");
        std::fs::create_dir(&target).expect("instances root");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o777))
            .expect("unsafe shared permissions");
        let planned = FileStorageProvider::plan(target.clone()).expect("plan instances root");

        let error = match FileStorageProvider::prepare(planned) {
            Ok(_) => panic!("unsafe shared parent must be rejected"),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("must not be writable by group or other users")
        );
    }

    #[test]
    fn prepare_rejects_a_sticky_shared_writable_instances_root() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("instances");
        std::fs::create_dir(&target).expect("instances root");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o1777))
            .expect("sticky shared permissions");
        let planned = FileStorageProvider::plan(target).expect("plan instances root");

        let error = match FileStorageProvider::prepare(planned) {
            Ok(_) => panic!("sticky shared parent must be rejected"),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("must not be writable by group or other users")
        );
    }

    #[test]
    fn plan_rejects_a_non_sticky_shared_writable_existing_ancestor() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let shared = temp.path().join("shared");
        let owned = shared.join("owned");
        std::fs::create_dir(&shared).expect("shared ancestor");
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777))
            .expect("unsafe shared permissions");
        std::fs::create_dir(&owned).expect("owned child");

        let error = match FileStorageProvider::plan(owned.join("instances")) {
            Ok(_) => panic!("unsafe existing ancestor must be rejected"),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("path-component parent has unsafe owner or permissions")
        );
    }

    #[test]
    fn plan_accepts_an_owned_component_below_a_sticky_system_parent() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let shared = temp.path().join("shared");
        let owned = shared.join("owned");
        std::fs::create_dir(&shared).expect("shared ancestor");
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o1777))
            .expect("sticky shared permissions");
        std::fs::create_dir(&owned).expect("owned child");

        let target = owned.join("instances");
        let planned = FileStorageProvider::plan(target.clone()).expect("safe existing chain");
        let prepared = FileStorageProvider::prepare(planned).expect("publish below owned child");

        assert_eq!(prepared.resolved_instances_dir, target);
    }

    #[test]
    fn prepare_rejects_replaced_planned_ancestor_without_materializing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ancestor = temp.path().join("ancestor");
        let detached = temp.path().join("ancestor-retained");
        let target = ancestor.join("one/two/instances");
        std::fs::create_dir(&ancestor).expect("ancestor");
        let planned = FileStorageProvider::plan(target.clone()).expect("read-only plan");
        std::fs::rename(&ancestor, &detached).expect("detach planned ancestor");
        std::fs::create_dir(&ancestor).expect("replacement ancestor");

        assert!(FileStorageProvider::prepare(planned).is_err());

        assert!(!ancestor.join("one").exists());
        assert!(!detached.join("one").exists());
    }

    #[test]
    fn prepare_rejects_a_planned_missing_component_that_appears() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ancestor = temp.path().join("ancestor");
        let target = ancestor.join("one/two/instances");
        std::fs::create_dir(&ancestor).expect("ancestor");
        let planned = FileStorageProvider::plan(target).expect("read-only plan");
        let appeared = ancestor.join("one");
        std::fs::create_dir(&appeared).expect("appeared component");
        std::fs::write(appeared.join("sentinel"), b"unrelated").expect("sentinel");

        assert!(FileStorageProvider::prepare(planned).is_err());

        assert_eq!(
            std::fs::read(appeared.join("sentinel")).expect("sentinel remains"),
            b"unrelated"
        );
        assert!(!appeared.join("two").exists());
    }

    #[test]
    fn plan_rejects_parent_components_before_creation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let downstream = temp.path().join("created");
        let target = temp.path().join("missing/../created");

        assert!(FileStorageProvider::plan(target).is_err());

        assert!(!downstream.exists());
    }

    #[tokio::test]
    async fn prepared_root_is_exclusive_until_provider_drop() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("instances");
        let first = FileStorageProvider::prepare(
            FileStorageProvider::plan(target.clone()).expect("first plan"),
        )
        .expect("first owner");
        let second_plan = FileStorageProvider::plan(target.clone()).expect("second plan");
        let error = match FileStorageProvider::prepare(second_plan) {
            Ok(_) => panic!("live provider owner must remain exclusive"),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            format!(
                "storage error: storage instances root {} is already owned by another daemon",
                target.display()
            )
        );

        drop(first);
        FileStorageProvider::reopen_after_simulated_restart(
            temp.path().join("images"),
            target,
            std::time::Duration::from_secs(1),
        )
        .await
        .expect("owner can be reacquired after drop");
    }

    #[test]
    fn plan_rejects_symlink_or_non_directory_components_without_creating_descendants() {
        use std::os::unix::fs::symlink;

        for kind in ["symlink", "file"] {
            let temp = tempfile::tempdir().expect("tempdir");
            let component = temp.path().join("blocked");
            if kind == "symlink" {
                let target = temp.path().join("target");
                std::fs::create_dir(&target).expect("target");
                symlink(&target, &component).expect("component link");
            } else {
                std::fs::write(&component, b"not a directory").expect("component file");
            }
            let descendant = component.join("one/two/instances");

            assert!(FileStorageProvider::plan(descendant.clone()).is_err());
            assert!(!descendant.exists());
        }
    }

    #[tokio::test]
    async fn plan_rejects_symlink_alias_without_disturbing_provider_owner() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("instances");
        std::fs::create_dir(&target).expect("instances");
        let alias = temp.path().join("instances-alias");
        symlink(&target, &alias).expect("alias");
        let first = FileStorageProvider::prepare(
            FileStorageProvider::plan(target.clone()).expect("target plan"),
        )
        .expect("first owner");

        assert!(FileStorageProvider::plan(alias).is_err());
        assert!(target.is_dir());
        assert!(
            target
                .read_dir()
                .expect("instances remains")
                .next()
                .is_none()
        );
        drop(first);
        FileStorageProvider::reopen_after_simulated_restart(
            temp.path().join("images"),
            target,
            std::time::Duration::from_secs(1),
        )
        .await
        .expect("owner reacquired after drop");
    }

    #[tokio::test]
    async fn backend_paths_fail_closed_after_prepared_root_replacement() {
        let temp = tempfile::tempdir().expect("tempdir");
        let images = temp.path().join("images");
        let configured = temp.path().join("instances");
        let detached = temp.path().join("instances-retained");
        std::fs::create_dir(&images).expect("images");
        std::fs::write(images.join("rootfs.ext4"), b"rootfs").expect("rootfs image");
        std::fs::write(images.join("mem.bin"), b"memory").expect("memory image");
        let prepared = FileStorageProvider::prepare(
            FileStorageProvider::plan(configured.clone()).expect("plan"),
        )
        .expect("prepare");
        let provider = FileStorageProvider::from_prepared(images, prepared);
        let id = Uuid::new_v4().to_string();
        let slot = provider
            .acquire(&AcquireOpts {
                instance_id: id.clone(),
                rootfs_size: 0,
                mem_size: 0,
            })
            .await
            .expect("acquire before root replacement");
        assert_eq!(slot.instance_dir, configured.join(&id));
        assert_eq!(slot.rootfs_path, configured.join(&id).join("rootfs.ext4"));
        assert_eq!(slot.mem_path, configured.join(&id).join("mem.bin"));
        assert_eq!(slot.mem_diff_path, configured.join(&id).join("mem.diff"));
        assert_eq!(
            slot.rootfs_diff_path,
            configured.join(&id).join("rootfs.diff")
        );

        std::fs::rename(&configured, &detached).expect("detach prepared root");
        std::fs::create_dir(&configured).expect("replacement root");
        write_complete_replacement_slot(&configured, &id, b"replacement-existing");
        let reconstruct_error = provider
            .reconstruct(&id)
            .await
            .expect_err("backend path must not escape the retained root");
        assert!(matches!(
            &reconstruct_error,
            BlazeError::StorageError { .. }
        ));
        assert!(
            reconstruct_error
                .to_string()
                .contains("backend-visible storage instances root")
        );
        provider
            .sync_artifacts(&slot)
            .await
            .expect("provider synchronization remains descriptor-relative");

        provider
            .release(slot)
            .await
            .expect("provider release remains descriptor-relative");

        assert!(!detached.join(&id).exists());
        assert_eq!(
            std::fs::read(configured.join(&id).join("rootfs.ext4"))
                .expect("replacement rootfs remains"),
            b"replacement-existing"
        );

        let later_id = Uuid::new_v4().to_string();
        write_complete_replacement_slot(&configured, &later_id, b"replacement-later");
        let acquire_error = provider
            .acquire(&AcquireOpts {
                instance_id: later_id.clone(),
                rootfs_size: 0,
                mem_size: 0,
            })
            .await
            .expect_err("acquire must not return a path through a replacement root");
        let (source, disposition) = acquire_error.into_parts();
        assert!(matches!(&source, BlazeError::StorageError { .. }));
        assert!(
            source
                .to_string()
                .contains("backend-visible storage instances root")
        );
        assert!(matches!(disposition, StorageAcquireDisposition::Clean));
        assert!(!detached.join(&later_id).exists());
        for name in ["rootfs.ext4", "mem.bin", "mem.diff", "rootfs.diff"] {
            assert_eq!(
                std::fs::read(configured.join(&later_id).join(name))
                    .expect("replacement artifact remains"),
                b"replacement-later"
            );
        }
    }

    #[tokio::test]
    async fn acquire_always_creates_empty_diff_artifacts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let images = temp.path().join("images");
        let instances = temp.path().join("instances");
        std::fs::create_dir(&images).expect("images");
        std::fs::create_dir(&instances).expect("instances");
        std::fs::write(images.join("rootfs.ext4"), b"rootfs").expect("rootfs image");
        std::fs::write(images.join("mem.bin"), b"memory").expect("memory image");
        std::fs::write(images.join("mem.diff"), b"stale memory state").expect("stale memory diff");
        std::fs::write(images.join("rootfs.diff"), b"stale disk state").expect("stale rootfs diff");
        let provider = FileStorageProvider::with_images(images, instances);

        let slot = provider
            .acquire(&AcquireOpts {
                instance_id: "empty-diffs".into(),
                rootfs_size: 0,
                mem_size: 0,
            })
            .await
            .expect("acquire slot");

        assert_eq!(std::fs::read(&slot.rootfs_path).expect("rootfs"), b"rootfs");
        assert_eq!(std::fs::read(&slot.mem_path).expect("memory"), b"memory");
        assert!(
            std::fs::read(&slot.mem_diff_path)
                .expect("memory diff")
                .is_empty()
        );
        assert!(
            std::fs::read(&slot.rootfs_diff_path)
                .expect("rootfs diff")
                .is_empty()
        );
    }

    #[test]
    fn unknown_directory_entry_hints_use_descriptor_relative_metadata() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let id = Uuid::new_v4().to_string();
        let slot = temp.path().join(&id);
        std::fs::create_dir_all(slot.join("nested")).expect("nested slot");
        std::fs::write(slot.join("nested/artifact"), b"artifact").expect("artifact");
        let root = open(
            temp.path(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .expect("root owner");

        let slot_owner = openat(
            &root,
            id.as_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .expect("slot owner");
        remove_directory_contents_with_type_hint(&slot_owner, |_| FileType::Unknown)
            .expect("recursive cleanup ignores unknown d_type");
        assert!(slot.read_dir().expect("empty slot").next().is_none());

        let target = temp.path().join("outside");
        std::fs::create_dir(&target).expect("outside");
        let link = slot.join("linked");
        symlink(&target, &link).expect("slot link");
        remove_directory_contents_with_type_hint(&slot_owner, |_| FileType::Unknown)
            .expect_err("symlink still fails closed when d_type is unknown");
        assert!(
            std::fs::symlink_metadata(&link)
                .expect("link remains")
                .file_type()
                .is_symlink()
        );
        assert!(target.is_dir());
    }

    #[test]
    fn recursive_cleanup_rejects_hard_linked_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let outside = temp.path().join("outside");
        let slot = temp.path().join("slot");
        std::fs::write(&outside, b"outside").expect("outside file");
        std::fs::create_dir(&slot).expect("slot");
        std::fs::hard_link(&outside, slot.join("linked")).expect("linked file");
        let slot_owner = open(
            &slot,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .expect("slot owner");

        let error = remove_directory_contents_with_type_hint(&slot_owner, DirEntry::file_type)
            .expect_err("hard-linked file must be retained");

        assert!(error.to_string().contains("multiple hard links"));
        assert_eq!(
            std::fs::read(&outside).expect("outside remains"),
            b"outside"
        );
        assert_eq!(
            std::fs::read(slot.join("linked")).expect("slot link remains"),
            b"outside"
        );
    }

    #[test]
    fn recursive_cleanup_rejects_file_mount_id_before_unlink() {
        let temp = tempfile::tempdir().expect("tempdir");
        let slot = temp.path().join("slot");
        let artifact = slot.join("artifact");
        std::fs::create_dir(&slot).expect("slot");
        std::fs::write(&artifact, b"artifact").expect("artifact");
        let slot_owner = open(
            &slot,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .expect("slot owner");
        let artifact_inode =
            statat(&slot_owner, "artifact", AtFlags::SYMLINK_NOFOLLOW).expect("artifact identity");

        let error = remove_directory_contents_on_mount(
            &slot_owner,
            41,
            DirEntry::file_type,
            |object| {
                let inode = fstat(object).map_err(std::io::Error::from)?.st_ino;
                Ok(if inode == artifact_inode.st_ino {
                    42
                } else {
                    41
                })
            },
            &mut || Ok(()),
        )
        .expect_err("file mount must be retained");

        assert!(error.to_string().contains("across mount boundary 41->42"));
        assert_eq!(
            std::fs::read(&artifact).expect("artifact remains"),
            b"artifact"
        );
    }

    #[test]
    fn recursive_cleanup_rejects_nested_mount_id_before_mutation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let slot = temp.path().join("slot");
        std::fs::create_dir_all(slot.join("nested")).expect("nested slot");
        let artifact = slot.join("nested/artifact");
        std::fs::write(&artifact, b"artifact").expect("artifact");
        let slot_owner = open(
            &slot,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .expect("slot owner");
        let nested =
            statat(&slot_owner, "nested", AtFlags::SYMLINK_NOFOLLOW).expect("nested identity");

        let error = remove_directory_contents_on_mount(
            &slot_owner,
            41,
            DirEntry::file_type,
            |directory| {
                let inode = fstat(directory).map_err(std::io::Error::from)?.st_ino;
                Ok(if inode == nested.st_ino { 42 } else { 41 })
            },
            &mut || Ok(()),
        )
        .expect_err("nested mount must be retained");

        assert!(error.to_string().contains("across mount boundary 41->42"));
        assert_eq!(
            std::fs::read(&artifact).expect("artifact remains"),
            b"artifact"
        );
        assert!(slot.join("nested").is_dir());
    }

    #[tokio::test]
    async fn probe_existing_dir_returns_true() {
        let tmp = tempfile::TempDir::new().unwrap();
        let provider = FileStorageProvider::new(tmp.path().to_path_buf());
        assert!(provider.probe().await.unwrap());
    }

    #[tokio::test]
    async fn probe_missing_dir_returns_false() {
        let provider = FileStorageProvider::unavailable_for_test(PathBuf::from(
            "/nonexistent/blaze-test-storage-probe",
        ));
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

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn acquire_rolls_back_when_slot_open_fails_after_mkdir() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let provider = FileStorageProvider::new(temporary.path().to_path_buf());
        let opts = AcquireOpts {
            instance_id: "retain-failure".into(),
            rootfs_size: 64,
            mem_size: 32,
        };
        let failpoint = crate::failpoint::TestFailpoint::new(&["storage-acquire-retain-slot"]);

        let error = failpoint
            .run(provider.acquire(&opts))
            .await
            .expect_err("slot retain failure");
        let (_source, disposition) = error.into_parts();

        assert!(matches!(disposition, StorageAcquireDisposition::Clean));
        assert!(!temporary.path().join(&opts.instance_id).exists());
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn acquire_returns_residual_when_open_failure_rollback_fails() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let provider = FileStorageProvider::new(temporary.path().to_path_buf());
        let opts = AcquireOpts {
            instance_id: "retain-residual".into(),
            rootfs_size: 64,
            mem_size: 32,
        };
        let failpoint = crate::failpoint::TestFailpoint::new(&[
            "storage-acquire-retain-slot",
            "storage-acquire-rollback",
        ]);

        let error = failpoint
            .run(provider.acquire(&opts))
            .await
            .expect_err("slot retain and rollback failure");
        let (_source, disposition) = error.into_parts();
        let StorageAcquireDisposition::Residual(residual) = disposition else {
            panic!("residual slot ownership must be transferred");
        };

        assert_eq!(residual.id, opts.instance_id);
        assert!(temporary.path().join(&residual.id).is_dir());
        provider
            .release(residual)
            .await
            .expect("release residual slot");
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn acquire_returns_a_retryable_residual_after_unlink_sync_failure() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let provider = FileStorageProvider::new(temporary.path().to_path_buf());
        let opts = AcquireOpts {
            instance_id: "removed-residual".into(),
            rootfs_size: 64,
            mem_size: 32,
        };
        let failpoint = crate::failpoint::TestFailpoint::new(&[
            "storage-acquire-artifacts",
            "storage-release-before-root-sync",
        ]);

        let error = failpoint
            .run(provider.acquire(&opts))
            .await
            .expect_err("root synchronization failure");
        let (_source, disposition) = error.into_parts();
        let StorageAcquireDisposition::Residual(residual) = disposition else {
            panic!("removed slot must retain a retryable cleanup owner");
        };

        assert!(!temporary.path().join(&opts.instance_id).exists());
        provider
            .release_by_id(&residual.id)
            .await
            .expect("retry missing-slot parent synchronization");
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn acquire_rolls_back_when_the_instances_root_sync_fails() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let provider = FileStorageProvider::new(temporary.path().to_path_buf());
        let opts = AcquireOpts {
            instance_id: "root-sync-failure".into(),
            rootfs_size: 64,
            mem_size: 32,
        };
        let failpoint = crate::failpoint::TestFailpoint::new(&["storage-acquire-before-root-sync"]);

        let error = failpoint
            .run(provider.acquire(&opts))
            .await
            .expect_err("instances-root synchronization failure");
        let (source, disposition) = error.into_parts();

        assert!(
            source
                .to_string()
                .contains("storage-acquire-before-root-sync")
        );
        assert!(matches!(disposition, StorageAcquireDisposition::Clean));
        assert!(!temporary.path().join(&opts.instance_id).exists());
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
    async fn release_by_id_recovers_missing_and_partial_slots() {
        let tmp = tempfile::TempDir::new().unwrap();
        let provider = FileStorageProvider::new(tmp.path().to_path_buf());
        let id = Uuid::new_v4().to_string();
        let missing_id = Uuid::new_v4().to_string();
        provider.release_by_id(&missing_id).await.unwrap();
        provider.release_by_id(&missing_id).await.unwrap();
        let partial = tmp.path().join(&id);
        tokio::fs::create_dir(&partial).await.unwrap();
        tokio::fs::write(partial.join("rootfs.ext4"), b"partial")
            .await
            .unwrap();

        provider.release_by_id(&id).await.unwrap();
        provider.release_by_id(&id).await.unwrap();

        assert!(!partial.exists());
    }

    #[tokio::test]
    async fn release_by_id_retries_after_a_partial_recursive_delete() {
        let tmp = tempfile::TempDir::new().unwrap();
        let provider = FileStorageProvider::new(tmp.path().to_path_buf());
        let id = Uuid::new_v4().to_string();
        let slot = tmp.path().join(&id);
        std::fs::create_dir_all(slot.join("nested")).expect("nested slot");
        std::fs::write(slot.join("00-first"), b"first").expect("first artifact");
        std::fs::write(slot.join("nested/second"), b"second").expect("second artifact");
        let owner = provider.instances_owner.as_ref().expect("instances owner");
        let mut fail_after_first_entry = true;

        remove_slot_tree_with_hooks(
            owner,
            &id,
            &mut || Ok(()),
            &mut || {
                if std::mem::take(&mut fail_after_first_entry) {
                    Err(BlazeError::StorageError {
                        msg: "injected recursive cleanup failure".into(),
                    })
                } else {
                    Ok(())
                }
            },
            &mut || Ok(()),
            &mut || Ok(()),
        )
        .expect_err("first recursive cleanup is interrupted");
        assert!(slot.exists());
        assert!(
            !slot.join("00-first").exists() || !slot.join("nested/second").exists(),
            "the injected failure must follow at least one unlink"
        );

        provider
            .release_by_id(&id)
            .await
            .expect("retry partial slot");
        provider.release_by_id(&id).await.expect("missing retry");
        assert!(!slot.exists());
    }

    #[tokio::test]
    async fn release_retries_the_instances_root_sync_after_unlink() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let provider = FileStorageProvider::new(temporary.path().to_path_buf());
        let id = Uuid::new_v4().to_string();
        let slot = temporary.path().join(&id);
        std::fs::create_dir(&slot).expect("slot");
        std::fs::write(slot.join("artifact"), b"artifact").expect("artifact");
        let owner = provider.instances_owner.as_ref().expect("instances owner");
        let mut fail_first_sync = true;

        remove_slot_tree_with_hooks(
            owner,
            &id,
            &mut || Ok(()),
            &mut || Ok(()),
            &mut || Ok(()),
            &mut || {
                if std::mem::take(&mut fail_first_sync) {
                    Err(BlazeError::StorageError {
                        msg: "injected instances-root sync failure".into(),
                    })
                } else {
                    Ok(())
                }
            },
        )
        .expect_err("root synchronization failure must be visible");
        assert!(!slot.exists(), "the failure follows the slot unlink");

        provider
            .release_by_id(&id)
            .await
            .expect("missing-slot retry crosses the root durability boundary");
        provider
            .release_by_id(&id)
            .await
            .expect("durable missing slot is idempotent");
    }

    #[test]
    fn release_rejects_slot_replacement_between_inspect_and_open() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let provider = FileStorageProvider::new(temporary.path().to_path_buf());
        let id = Uuid::new_v4().to_string();
        let slot = temporary.path().join(&id);
        let detached = temporary.path().join(format!("{id}-retained"));
        std::fs::create_dir(&slot).expect("slot");
        std::fs::write(slot.join("original"), b"original").expect("original sentinel");
        let owner = provider.instances_owner.as_ref().expect("instances owner");

        let error = remove_slot_tree_with_hooks(
            owner,
            &id,
            &mut || {
                std::fs::rename(&slot, &detached).expect("detach inspected slot");
                std::fs::create_dir(&slot).expect("replacement slot");
                std::fs::write(slot.join("replacement"), b"replacement")
                    .expect("replacement sentinel");
                Ok(())
            },
            &mut || Ok(()),
            &mut || Ok(()),
            &mut || Ok(()),
        )
        .expect_err("replacement must not match the inspected slot");

        assert!(error.to_string().contains("changed identity"));
        assert_eq!(
            std::fs::read(detached.join("original")).expect("original remains"),
            b"original"
        );
        assert_eq!(
            std::fs::read(slot.join("replacement")).expect("replacement remains"),
            b"replacement"
        );
    }

    #[test]
    fn release_rejects_slot_replacement_before_unlink() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let provider = FileStorageProvider::new(temporary.path().to_path_buf());
        let id = Uuid::new_v4().to_string();
        let slot = temporary.path().join(&id);
        let detached = temporary.path().join(format!("{id}-retained"));
        std::fs::create_dir(&slot).expect("slot");
        let owner = provider.instances_owner.as_ref().expect("instances owner");

        let error = remove_slot_tree_with_hooks(
            owner,
            &id,
            &mut || Ok(()),
            &mut || Ok(()),
            &mut || {
                std::fs::rename(&slot, &detached).expect("detach retained slot");
                std::fs::create_dir(&slot).expect("replacement slot");
                std::fs::write(slot.join("replacement"), b"replacement")
                    .expect("replacement sentinel");
                Ok(())
            },
            &mut || Ok(()),
        )
        .expect_err("replacement must not be unlinked");

        assert!(error.to_string().contains("changed identity"));
        assert!(detached.is_dir());
        assert_eq!(
            std::fs::read(slot.join("replacement")).expect("replacement remains"),
            b"replacement"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn release_by_id_rejects_non_directory_and_symlink_slots() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::TempDir::new().unwrap();
        let provider = FileStorageProvider::new(tmp.path().to_path_buf());
        let id = Uuid::new_v4().to_string();
        let slot_path = tmp.path().join(&id);
        tokio::fs::write(&slot_path, b"not a directory")
            .await
            .unwrap();

        let file_error = provider.release_by_id(&id).await.unwrap_err();
        assert!(file_error.to_string().contains("refusing non-directory"));
        assert!(slot_path.is_file());

        tokio::fs::remove_file(&slot_path).await.unwrap();
        let target = tempfile::TempDir::new().unwrap();
        symlink(target.path(), &slot_path).unwrap();

        let symlink_error = provider.release_by_id(&id).await.unwrap_err();
        assert!(symlink_error.to_string().contains("refusing non-directory"));
        assert!(std::fs::symlink_metadata(&slot_path).unwrap().is_symlink());
        assert!(target.path().is_dir());
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

    #[test]
    fn required_slot_open_errors_preserve_transient_failures() {
        let path = Path::new("/configured/instances/example/rootfs.ext4");
        let missing =
            classify_required_slot_open_error("reconstruct", "example", path, "file", Errno::NOENT);
        assert!(matches!(missing, BlazeError::StorageIncomplete { .. }));

        let transient =
            classify_required_slot_open_error("reconstruct", "example", path, "file", Errno::MFILE);
        assert!(matches!(transient, BlazeError::StorageError { .. }));
        assert!(transient.to_string().contains("Too many open files"));
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

    #[cfg(unix)]
    #[tokio::test]
    async fn reconstruct_classifies_a_socket_artifact_as_incomplete() {
        use std::os::unix::fs::FileTypeExt;
        use std::os::unix::net::UnixListener;

        let temp = tempfile::TempDir::new().unwrap();
        let provider = FileStorageProvider::new(temp.path().to_path_buf());
        let slot = provider
            .acquire(&AcquireOpts {
                instance_id: "socket-artifact".into(),
                rootfs_size: 64,
                mem_size: 32,
            })
            .await
            .unwrap();
        tokio::fs::remove_file(&slot.mem_diff_path).await.unwrap();
        let listener = UnixListener::bind(&slot.mem_diff_path).expect("socket artifact");

        let error = provider
            .reconstruct("socket-artifact")
            .await
            .expect_err("socket artifact must invalidate the slot");

        assert!(matches!(
            error,
            BlazeError::StorageIncomplete {
                ref instance_id,
                ref path,
                expected: "file",
            } if instance_id == "socket-artifact" && path == &slot.mem_diff_path
        ));
        drop(listener);
        assert!(
            std::fs::symlink_metadata(&slot.mem_diff_path)
                .unwrap()
                .file_type()
                .is_socket()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn acquire_rejects_a_slot_replaced_after_publication() -> Result<()> {
        use std::time::Duration;

        let temp = tempfile::tempdir()?;
        let mut provider = FileStorageProvider::new(temp.path().to_path_buf());
        let hook = Arc::new(AcquireBlockingHook::new());
        provider.acquire_blocking_hook = Some(Arc::clone(&hook));
        let provider = Arc::new(provider);
        let task_provider = Arc::clone(&provider);
        let task = tokio::spawn(async move {
            task_provider
                .acquire(&AcquireOpts {
                    instance_id: "publication-race".to_string(),
                    rootfs_size: 64,
                    mem_size: 32,
                })
                .await
        });

        tokio::time::timeout(Duration::from_secs(4), hook.wait_until_entered())
            .await
            .expect("slot publication reached deterministic boundary");
        let published = temp.path().join("publication-race");
        let detached = temp.path().join("publication-race-retained");
        std::fs::rename(&published, &detached).expect("detach published slot");
        std::fs::create_dir(&published).expect("replacement slot");
        std::fs::write(published.join("sentinel"), b"replacement").expect("replacement sentinel");
        hook.resume();

        let acquire_error = task
            .await
            .expect("acquire task completed")
            .expect_err("replacement must fail acquisition");
        let (source, disposition) = acquire_error.into_parts();
        assert!(source.to_string().contains("changed identity"));
        assert!(matches!(
            disposition,
            StorageAcquireDisposition::ManualCleanupRequired
        ));
        assert!(detached.is_dir());
        assert_eq!(
            std::fs::read(published.join("sentinel")).expect("replacement remains"),
            b"replacement"
        );
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn slot_creation_does_not_block_async_runtime() -> Result<()> {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc;
        use std::time::Duration;

        let temp = tempfile::tempdir()?;
        let mut provider = FileStorageProvider::new(temp.path().to_path_buf());
        let hook = Arc::new(AcquireBlockingHook::new());
        provider.acquire_blocking_hook = Some(Arc::clone(&hook));
        let (watchdog_cancel_tx, watchdog_cancel_rx) = mpsc::channel();
        let watchdog_fired = Arc::new(AtomicBool::new(false));
        let watchdog_state = Arc::clone(&watchdog_fired);
        let watchdog_hook = Arc::clone(&hook);
        let watchdog = std::thread::spawn(move || {
            if watchdog_cancel_rx
                .recv_timeout(Duration::from_secs(2))
                .is_err()
            {
                watchdog_state.store(true, Ordering::SeqCst);
                watchdog_hook.resume();
            }
        });

        let opts = AcquireOpts {
            instance_id: "runtime-progress".to_string(),
            rootfs_size: 64,
            mem_size: 32,
        };
        let acquire_future = provider.acquire(&opts);
        let progress_hook = Arc::clone(&hook);
        let runtime_progress = async {
            let entered =
                tokio::time::timeout(Duration::from_secs(4), progress_hook.wait_until_entered())
                    .await;
            assert!(
                entered.is_ok(),
                "blocking acquire transaction did not start"
            );
            tokio::task::yield_now().await;
            assert!(
                !watchdog_fired.load(Ordering::SeqCst),
                "blocking acquire transaction stalled the current-thread runtime"
            );
            progress_hook.resume();
            assert!(
                watchdog_cancel_tx.send(()).is_ok(),
                "cancel acquire watchdog"
            );
        };

        let (acquire_result, ()) = tokio::join!(acquire_future, runtime_progress);
        assert!(watchdog.join().is_ok(), "acquire watchdog panicked");
        assert!(
            !watchdog_fired.load(Ordering::SeqCst),
            "acquire watchdog released a blocked runtime"
        );
        let slot = acquire_result.map_err(|error| error.into_parts().0)?;
        assert_eq!(std::fs::metadata(&slot.rootfs_path)?.len(), 64);
        assert_eq!(std::fs::metadata(&slot.mem_path)?.len(), 32);
        assert!(slot.mem_diff_path.is_file());
        assert!(slot.rootfs_diff_path.is_file());
        provider.release(slot).await?;
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_acquire_finishes_without_a_partial_slot() -> Result<()> {
        use std::sync::mpsc;
        use std::time::Duration;

        let temp = tempfile::tempdir()?;
        let mut provider = FileStorageProvider::new(temp.path().to_path_buf());
        let hook = Arc::new(AcquireBlockingHook::new());
        provider.acquire_blocking_hook = Some(Arc::clone(&hook));
        let watchdog_hook = Arc::clone(&hook);
        let (watchdog_cancel_tx, watchdog_cancel_rx) = mpsc::channel();
        let watchdog = std::thread::spawn(move || {
            if watchdog_cancel_rx
                .recv_timeout(Duration::from_secs(2))
                .is_err()
            {
                watchdog_hook.resume();
            }
        });
        let provider = Arc::new(provider);
        let task_provider = Arc::clone(&provider);
        let task = tokio::spawn(async move {
            task_provider
                .acquire(&AcquireOpts {
                    instance_id: "cancelled-acquire".to_string(),
                    rootfs_size: 64,
                    mem_size: 32,
                })
                .await
        });

        tokio::time::timeout(Duration::from_secs(4), hook.wait_until_entered())
            .await
            .expect("blocking acquire transaction started");
        task.abort();
        hook.resume();
        tokio::time::timeout(Duration::from_secs(4), hook.wait_until_finished())
            .await
            .expect("blocking acquire transaction finished");
        assert!(
            watchdog_cancel_tx.send(()).is_ok(),
            "cancel acquire watchdog"
        );
        assert!(watchdog.join().is_ok(), "acquire watchdog panicked");
        assert!(
            task.await
                .expect_err("acquire task was cancelled")
                .is_cancelled(),
            "acquire task must report cancellation"
        );

        let slot = provider.reconstruct("cancelled-acquire").await?;
        assert_eq!(std::fs::metadata(&slot.rootfs_path)?.len(), 64);
        assert_eq!(std::fs::metadata(&slot.mem_path)?.len(), 32);
        assert!(slot.mem_diff_path.is_file());
        assert!(slot.rootfs_diff_path.is_file());
        provider.release(slot).await?;
        Ok(())
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
