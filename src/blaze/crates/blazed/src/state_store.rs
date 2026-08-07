// SPDX-License-Identifier: Apache-2.0
//! Daemon-owned access to persisted sandbox state and runtime directories.

use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::{Read, Write};
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use blaze_core::lifecycle::SandboxInstance;
use rustix::fs::{
    AtFlags, Dir, FlockOperation, Mode, OFlags, RenameFlags, flock, fstat, fsync, mkdirat, open,
    openat, renameat, renameat_with, unlinkat,
};
use rustix::io::Errno;
use uuid::Uuid;

use crate::error::{BlazeDaemonError, Result};

const STATE_FILE: &str = "state.json";
const TEMP_STATE_FILE: &str = "state.json.tmp";

/// Central access point for the daemon state directory.
///
/// The store holds the opened state-root object for its complete lifetime.
/// Record I/O and runtime-directory paths are derived from that object rather
/// than reopening the configured pathname.
#[derive(Clone)]
pub struct StateStore {
    inner: Arc<StateStoreInner>,
}

struct StateStoreInner {
    configured_root: PathBuf,
    root: OwnedFd,
    run_dirs: Mutex<HashMap<Uuid, RunDirEntry>>,
}

enum RunDirEntry {
    Owned(OwnedRunDir),
    Uncertain(OwnedRunDir),
    Released,
}

/// A cloneable owner of one sandbox runtime directory.
///
/// The handle keeps the opened directory object alive while lifecycle and
/// backend work use it. Its stable path resolves through that descriptor on
/// Linux, so replacing the configured pathname cannot redirect later work.
#[derive(Clone)]
pub(crate) struct OwnedRunDir {
    inner: Arc<OwnedRunDirInner>,
}

struct OwnedRunDirInner {
    instance_id: Uuid,
    configured_path: PathBuf,
    stable_path: PathBuf,
    directory: OwnedFd,
    writer: Mutex<()>,
}

impl fmt::Debug for StateStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StateStore")
            .field("configured_root", &self.inner.configured_root)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for OwnedRunDir {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OwnedRunDir")
            .field("instance_id", &self.inner.instance_id)
            .field("configured_path", &self.inner.configured_path)
            .finish_non_exhaustive()
    }
}

impl StateStore {
    /// Open and exclusively own the configured state directory.
    pub fn open(root: PathBuf) -> Result<Self> {
        Self::open_with_lock(root, true)
    }

    #[cfg(test)]
    pub(crate) fn new(root: PathBuf) -> Self {
        Self::open_with_lock(root, false).expect("open test state store")
    }

    fn open_with_lock(root: PathBuf, lock: bool) -> Result<Self> {
        let directory = open(
            &root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        if lock && let Err(error) = flock(&directory, FlockOperation::NonBlockingLockExclusive) {
            return if error == Errno::WOULDBLOCK {
                Err(BlazeDaemonError::Conflict(format!(
                    "state directory {} is already owned by another daemon",
                    root.display()
                )))
            } else {
                Err(std::io::Error::from(error).into())
            };
        }
        Ok(Self {
            inner: Arc::new(StateStoreInner {
                configured_root: root,
                root: directory,
                run_dirs: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// Return the retained owner for one known sandbox directory.
    pub(crate) fn run_dir(&self, id: Uuid) -> Result<OwnedRunDir> {
        self.cached_run_dir(id)?.ok_or_else(|| {
            BlazeDaemonError::NotFound(format!(
                "owned runtime directory for instance {id} is unavailable"
            ))
        })
    }

    /// Report whether a failed first publication left an object that must be
    /// completed or released through lifecycle recovery.
    pub(crate) fn has_run_dir_residual(&self, id: Uuid) -> Result<bool> {
        Ok(matches!(
            self.inner
                .run_dirs
                .lock()
                .map_err(|_| BlazeDaemonError::Internal(
                    "state run-directory lock poisoned".into()
                ))?
                .get(&id),
            Some(RunDirEntry::Owned(_) | RunDirEntry::Uncertain(_))
        ))
    }

    /// Persist one lifecycle record below the owned state root.
    pub fn persist(&self, instance: &SandboxInstance) -> Result<()> {
        let json = serde_json::to_vec_pretty(instance)?;
        let run_dir = {
            let mut run_dirs = self.inner.run_dirs.lock().map_err(|_| {
                BlazeDaemonError::Internal("state run-directory lock poisoned".into())
            })?;
            match run_dirs.get(&instance.id) {
                Some(RunDirEntry::Owned(run_dir)) => run_dir.clone(),
                Some(RunDirEntry::Uncertain(run_dir)) => {
                    let run_dir = run_dir.clone();
                    self.revalidate_uncertain(instance.id, &run_dir)?;
                    run_dirs.insert(instance.id, RunDirEntry::Owned(run_dir.clone()));
                    run_dir
                }
                Some(RunDirEntry::Released) => {
                    return Err(BlazeDaemonError::Conflict(format!(
                        "terminal lifecycle record for instance {} cannot be rewritten",
                        instance.id
                    )));
                }
                None => {
                    return self.publish_new_record(&mut run_dirs, instance, &json);
                }
            }
        };
        let _writer =
            run_dir.inner.writer.lock().map_err(|_| {
                BlazeDaemonError::Internal("state record writer lock poisoned".into())
            })?;
        {
            let run_dirs = self.inner.run_dirs.lock().map_err(|_| {
                BlazeDaemonError::Internal("state run-directory lock poisoned".into())
            })?;
            match run_dirs.get(&instance.id) {
                Some(RunDirEntry::Owned(retained)) if retained.same_object(&run_dir) => {}
                Some(RunDirEntry::Uncertain(_)) => {
                    return Err(BlazeDaemonError::RecoveryRequired(format!(
                        "runtime-directory publication for instance {} is unconfirmed",
                        instance.id
                    )));
                }
                Some(RunDirEntry::Released) => {
                    return Err(BlazeDaemonError::Conflict(format!(
                        "terminal lifecycle record for instance {} cannot be rewritten",
                        instance.id
                    )));
                }
                Some(RunDirEntry::Owned(_)) => {
                    return Err(BlazeDaemonError::Conflict(format!(
                        "runtime-directory ownership changed for instance {}",
                        instance.id
                    )));
                }
                None => {
                    return Err(BlazeDaemonError::Internal(format!(
                        "runtime-directory ownership disappeared for instance {}",
                        instance.id
                    )));
                }
            }
        }
        self.write_record_locked(&run_dir, &json)?;
        // Retrying a publication whose parent-directory sync previously
        // failed must cross that durability boundary before the owner can be
        // released. Syncing the root on every commit keeps that retry path
        // explicit without maintaining a second publication journal.
        fsync(&self.inner.root).map_err(std::io::Error::from)?;
        self.update_retention(instance, &run_dir)?;
        Ok(())
    }

    /// Load one lifecycle record from the owned sandbox directory.
    #[cfg(test)]
    pub fn load(&self, id: Uuid) -> Result<SandboxInstance> {
        let run_dir = match self.cached_run_dir(id)? {
            Some(run_dir) => run_dir,
            None => self.open_run_dir_object(id)?,
        };
        Self::load_from(&run_dir)
    }

    /// Best-effort startup scan of persisted lifecycle records.
    ///
    /// The scan owns the run-directory map for its complete duration and must
    /// run before request handling starts. This prevents a stale scan result
    /// from restoring ownership after a concurrent terminal commit.
    pub fn scan(&self) -> Result<HashMap<Uuid, SandboxInstance>> {
        let mut instances = HashMap::new();
        let mut run_dirs =
            self.inner.run_dirs.lock().map_err(|_| {
                BlazeDaemonError::Internal("state run-directory lock poisoned".into())
            })?;
        if !run_dirs.is_empty() {
            return Err(BlazeDaemonError::Internal(
                "state scan must complete before lifecycle persistence starts".into(),
            ));
        }
        let entries = Dir::read_from(&self.inner.root).map_err(std::io::Error::from)?;
        for entry in entries {
            let entry = entry.map_err(std::io::Error::from)?;
            let Ok(name) = entry.file_name().to_str() else {
                continue;
            };
            if is_state_staging_name(name) {
                if let Err(error) = self.remove_stale_staging(name) {
                    tracing::warn!(entry = name, %error, "failed to remove stale state staging");
                }
                continue;
            }
            let Ok(id) = Uuid::parse_str(name) else {
                continue;
            };
            let run_dir = match self.open_run_dir_object(id) {
                Ok(run_dir) => run_dir,
                Err(error) => {
                    tracing::warn!(instance = %id, %error, "skipping invalid instance directory");
                    continue;
                }
            };
            if let Err(error) = remove_file_if_exists(&run_dir.inner.directory, TEMP_STATE_FILE) {
                tracing::warn!(
                    instance = %id,
                    %error,
                    "failed to remove stale state record temporary file"
                );
            }
            match Self::load_from(&run_dir) {
                Ok(instance) => {
                    let ownership =
                        if instance.state == blaze_core::lifecycle::SandboxState::Destroyed {
                            RunDirEntry::Released
                        } else {
                            RunDirEntry::Owned(run_dir)
                        };
                    run_dirs.insert(id, ownership);
                    instances.insert(id, instance);
                }
                Err(error) => {
                    tracing::warn!(instance = %id, %error, "skipping corrupt instance state");
                }
            }
        }
        tracing::info!(
            instances = instances.len(),
            "rehydrated instances from state_dir"
        );
        Ok(instances)
    }

    fn cached_run_dir(&self, id: Uuid) -> Result<Option<OwnedRunDir>> {
        let run_dirs =
            self.inner.run_dirs.lock().map_err(|_| {
                BlazeDaemonError::Internal("state run-directory lock poisoned".into())
            })?;
        match run_dirs.get(&id) {
            Some(RunDirEntry::Owned(run_dir)) => Ok(Some(run_dir.clone())),
            Some(RunDirEntry::Uncertain(_)) => Err(BlazeDaemonError::RecoveryRequired(format!(
                "runtime-directory publication for instance {id} is unconfirmed"
            ))),
            Some(RunDirEntry::Released) | None => Ok(None),
        }
    }

    fn open_run_dir_object(&self, id: Uuid) -> Result<OwnedRunDir> {
        let name = id.to_string();
        let directory = openat(
            &self.inner.root,
            name.as_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        Ok(OwnedRunDir::new(
            id,
            self.inner.configured_root.join(&name),
            directory,
        ))
    }

    fn publish_new_record(
        &self,
        run_dirs: &mut HashMap<Uuid, RunDirEntry>,
        instance: &SandboxInstance,
        json: &[u8],
    ) -> Result<()> {
        crate::failpoint::state("state-before-first-publication")?;
        let id = instance.id;
        let final_name = id.to_string();
        let staging_name = loop {
            let candidate = format!(".state-{id}-{}.tmp", Uuid::new_v4());
            match mkdirat(
                &self.inner.root,
                candidate.as_str(),
                Mode::from_bits_truncate(0o777),
            ) {
                Ok(()) => break candidate,
                Err(Errno::EXIST) => continue,
                Err(error) => return Err(std::io::Error::from(error).into()),
            }
        };
        let directory = match openat(
            &self.inner.root,
            staging_name.as_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(directory) => directory,
            Err(error) => {
                let original = BlazeDaemonError::from(std::io::Error::from(error));
                let cleanup = remove_directory_if_exists(&self.inner.root, &staging_name);
                return Err(combine_publication_cleanup(original, cleanup));
            }
        };
        let run_dir = OwnedRunDir::new(id, self.inner.configured_root.join(&final_name), directory);
        let writer =
            run_dir.inner.writer.lock().map_err(|_| {
                BlazeDaemonError::Internal("state record writer lock poisoned".into())
            })?;
        if let Err(error) = self.write_record_locked(&run_dir, json) {
            let cleanup = self.discard_staging(&run_dir, &staging_name);
            return Err(retain_failed_publication(
                run_dirs, id, &run_dir, error, cleanup,
            ));
        }
        if let Err(error) = renameat_with(
            &self.inner.root,
            staging_name.as_str(),
            &self.inner.root,
            final_name.as_str(),
            RenameFlags::NOREPLACE,
        ) {
            let original = if error == Errno::EXIST {
                BlazeDaemonError::Conflict(format!(
                    "runtime directory for new instance {id} already exists"
                ))
            } else {
                BlazeDaemonError::from(std::io::Error::from(error))
            };
            let cleanup = self.discard_staging(&run_dir, &staging_name);
            return Err(retain_failed_publication(
                run_dirs, id, &run_dir, original, cleanup,
            ));
        }
        let linkage = crate::failpoint::state("state-post-publication-identity")
            .and_then(|_| self.linked_directory_matches(&final_name, &run_dir));
        match linkage {
            Ok(Some(true)) => {}
            Ok(Some(false)) => {
                run_dirs.insert(id, RunDirEntry::Uncertain(run_dir.clone()));
                return Err(BlazeDaemonError::Conflict(format!(
                    "published runtime directory for new instance {id} changed identity"
                )));
            }
            Ok(None) => {
                run_dirs.insert(id, RunDirEntry::Uncertain(run_dir.clone()));
                return Err(BlazeDaemonError::Conflict(format!(
                    "published runtime directory for new instance {id} disappeared"
                )));
            }
            Err(error) => {
                run_dirs.insert(id, RunDirEntry::Uncertain(run_dir.clone()));
                return Err(BlazeDaemonError::RecoveryRequired(format!(
                    "published runtime directory for new instance {id} could not be verified: \
                     {error}"
                )));
            }
        }
        run_dirs.insert(id, RunDirEntry::Owned(run_dir.clone()));
        crate::failpoint::state("state-first-publication-root-sync")?;
        if let Err(error) = fsync(&self.inner.root) {
            return Err(std::io::Error::from(error).into());
        }
        if instance.state == blaze_core::lifecycle::SandboxState::Destroyed {
            run_dirs.insert(id, RunDirEntry::Released);
        }
        drop(writer);
        Ok(())
    }

    fn write_record_locked(&self, run_dir: &OwnedRunDir, json: &[u8]) -> Result<()> {
        match unlinkat(&run_dir.inner.directory, TEMP_STATE_FILE, AtFlags::empty()) {
            Ok(()) | Err(Errno::NOENT) => {}
            Err(error) => return Err(std::io::Error::from(error).into()),
        }
        let temporary = openat(
            &run_dir.inner.directory,
            TEMP_STATE_FILE,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(0o666),
        )
        .map_err(std::io::Error::from)?;
        let result = (|| -> Result<()> {
            let mut temporary = File::from(temporary);
            temporary.write_all(json)?;
            temporary.write_all(b"\n")?;
            temporary.sync_all()?;
            renameat(
                &run_dir.inner.directory,
                TEMP_STATE_FILE,
                &run_dir.inner.directory,
                STATE_FILE,
            )
            .map_err(std::io::Error::from)?;
            fsync(&run_dir.inner.directory).map_err(std::io::Error::from)?;
            Ok(())
        })();
        match result {
            Ok(()) => Ok(()),
            Err(original) => {
                let cleanup = remove_file_if_exists(&run_dir.inner.directory, TEMP_STATE_FILE);
                Err(combine_record_cleanup(original, cleanup))
            }
        }
    }

    fn update_retention(&self, instance: &SandboxInstance, run_dir: &OwnedRunDir) -> Result<()> {
        let mut run_dirs =
            self.inner.run_dirs.lock().map_err(|_| {
                BlazeDaemonError::Internal("state run-directory lock poisoned".into())
            })?;
        if instance.state == blaze_core::lifecycle::SandboxState::Destroyed {
            if matches!(
                run_dirs.get(&instance.id),
                Some(RunDirEntry::Owned(retained)) if retained.same_object(run_dir)
            ) {
                run_dirs.insert(instance.id, RunDirEntry::Released);
            }
        } else {
            run_dirs.insert(instance.id, RunDirEntry::Owned(run_dir.clone()));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn retained_run_dir_count(&self) -> usize {
        self.inner
            .run_dirs
            .lock()
            .expect("state run-directory lock")
            .values()
            .filter(|entry| matches!(entry, RunDirEntry::Owned(_) | RunDirEntry::Uncertain(_)))
            .count()
    }

    fn discard_staging(&self, run_dir: &OwnedRunDir, staging_name: &str) -> Result<()> {
        crate::failpoint::state("state-before-staging-discard")?;
        match self.linked_directory_matches(staging_name, run_dir)? {
            Some(true) => {}
            Some(false) => {
                return Err(BlazeDaemonError::Conflict(format!(
                    "state staging entry {staging_name} changed identity before cleanup"
                )));
            }
            None => {
                return Err(BlazeDaemonError::RecoveryRequired(format!(
                    "state staging entry {staging_name} disappeared before cleanup"
                )));
            }
        }
        let mut errors = Vec::new();
        for name in [STATE_FILE, TEMP_STATE_FILE] {
            if let Err(error) = remove_file_if_exists(&run_dir.inner.directory, name) {
                errors.push(format!("remove {name}: {error}"));
            }
        }
        if let Err(error) = remove_directory_if_exists(&self.inner.root, staging_name) {
            errors.push(format!("remove {staging_name}: {error}"));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(BlazeDaemonError::Internal(errors.join("; ")))
        }
    }

    fn linked_directory_matches(&self, name: &str, run_dir: &OwnedRunDir) -> Result<Option<bool>> {
        let linked = match openat(
            &self.inner.root,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(linked) => linked,
            Err(Errno::NOENT) => return Ok(None),
            Err(error) => return Err(std::io::Error::from(error).into()),
        };
        Ok(Some(same_opened_object(&linked, &run_dir.inner.directory)?))
    }

    fn revalidate_uncertain(&self, id: Uuid, run_dir: &OwnedRunDir) -> Result<()> {
        let name = id.to_string();
        let linkage = crate::failpoint::state("state-post-publication-identity")
            .and_then(|_| self.linked_directory_matches(&name, run_dir));
        match linkage {
            Ok(Some(true)) => Ok(()),
            Ok(Some(false)) => Err(BlazeDaemonError::RecoveryRequired(format!(
                "published runtime directory for instance {id} has a different identity"
            ))),
            Ok(None) => Err(BlazeDaemonError::RecoveryRequired(format!(
                "published runtime directory for instance {id} is missing"
            ))),
            Err(error) => Err(BlazeDaemonError::RecoveryRequired(format!(
                "published runtime directory for instance {id} still cannot be verified: {error}"
            ))),
        }
    }

    fn remove_stale_staging(&self, staging_name: &str) -> Result<()> {
        let directory = openat(
            &self.inner.root,
            staging_name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        let run_dir = OwnedRunDir::new(
            Uuid::nil(),
            self.inner.configured_root.join(staging_name),
            directory,
        );
        self.discard_staging(&run_dir, staging_name)
    }

    fn load_from(run_dir: &OwnedRunDir) -> Result<SandboxInstance> {
        let state = openat(
            &run_dir.inner.directory,
            STATE_FILE,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        let mut state = File::from(state);
        if !state.metadata()?.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "{} must be a regular file",
                    run_dir.inner.configured_path.join(STATE_FILE).display()
                ),
            )
            .into());
        }
        let mut raw = Vec::new();
        state.read_to_end(&mut raw)?;
        Ok(serde_json::from_slice(&raw)?)
    }
}

fn remove_file_if_exists(directory: &OwnedFd, name: &str) -> Result<()> {
    match unlinkat(directory, name, AtFlags::empty()) {
        Ok(()) | Err(Errno::NOENT) => Ok(()),
        Err(error) => Err(std::io::Error::from(error).into()),
    }
}

fn same_opened_object(left: &OwnedFd, right: &OwnedFd) -> Result<bool> {
    let left = fstat(left).map_err(std::io::Error::from)?;
    let right = fstat(right).map_err(std::io::Error::from)?;
    Ok(left.st_dev == right.st_dev && left.st_ino == right.st_ino)
}

fn remove_directory_if_exists(directory: &OwnedFd, name: &str) -> Result<()> {
    match unlinkat(directory, name, AtFlags::REMOVEDIR) {
        Ok(()) | Err(Errno::NOENT) => Ok(()),
        Err(error) => Err(std::io::Error::from(error).into()),
    }
}

fn combine_publication_cleanup(
    original: BlazeDaemonError,
    cleanup: Result<()>,
) -> BlazeDaemonError {
    match cleanup {
        Ok(()) => original,
        Err(cleanup) => BlazeDaemonError::Internal(format!(
            "{original}; unpublished state staging cleanup failed: {cleanup}"
        )),
    }
}

fn retain_failed_publication(
    run_dirs: &mut HashMap<Uuid, RunDirEntry>,
    id: Uuid,
    run_dir: &OwnedRunDir,
    original: BlazeDaemonError,
    cleanup: Result<()>,
) -> BlazeDaemonError {
    match cleanup {
        Ok(()) => original,
        Err(cleanup) => {
            run_dirs.insert(id, RunDirEntry::Uncertain(run_dir.clone()));
            BlazeDaemonError::RecoveryRequired(format!(
                "{original}; unpublished state staging cleanup failed: {cleanup}; \
                 runtime-directory owner retained for recovery"
            ))
        }
    }
}

fn combine_record_cleanup(original: BlazeDaemonError, cleanup: Result<()>) -> BlazeDaemonError {
    match cleanup {
        Ok(()) => original,
        Err(cleanup) => BlazeDaemonError::Internal(format!(
            "{original}; state record temporary-file cleanup failed: {cleanup}"
        )),
    }
}

fn is_state_staging_name(name: &str) -> bool {
    let Some(body) = name
        .strip_prefix(".state-")
        .and_then(|body| body.strip_suffix(".tmp"))
    else {
        return false;
    };
    if body.len() != 73 || body.as_bytes().get(36) != Some(&b'-') {
        return false;
    }
    let Some(instance_id) = body.get(..36) else {
        return false;
    };
    let Some(nonce) = body.get(37..) else {
        return false;
    };
    Uuid::parse_str(instance_id).is_ok() && Uuid::parse_str(nonce).is_ok()
}

impl OwnedRunDir {
    fn new(instance_id: Uuid, configured_path: PathBuf, directory: OwnedFd) -> Self {
        #[cfg(target_os = "linux")]
        let stable_path = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
        #[cfg(not(target_os = "linux"))]
        let stable_path = configured_path.clone();
        Self {
            inner: Arc::new(OwnedRunDirInner {
                instance_id,
                configured_path,
                stable_path,
                directory,
                writer: Mutex::new(()),
            }),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.inner.stable_path
    }

    pub(crate) fn instance_id(&self) -> Uuid {
        self.inner.instance_id
    }

    fn same_object(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn inherit_into(&self, command: &mut tokio::process::Command) {
        use std::os::unix::process::CommandExt;

        let owner = self.clone();
        // SAFETY: `fcntl` is async-signal-safe. The closure only changes the
        // child-side copy of a descriptor retained through the spawn call.
        unsafe {
            command.as_std_mut().pre_exec(move || {
                let descriptor = owner.inner.directory.as_raw_fd();
                if libc::fcntl(descriptor, libc::F_SETFD, 0) == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn inherit_into(&self, _command: &mut tokio::process::Command) {}

    #[cfg(test)]
    pub(crate) fn for_test(instance_id: Uuid, path: PathBuf) -> Self {
        std::fs::create_dir_all(&path).expect("test runtime directory");
        let directory = open(
            &path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .expect("open test runtime directory");
        Self::new(instance_id, path, directory)
    }
}

#[cfg(test)]
mod tests {
    use blaze_core::backend::BackendKind;
    use blaze_core::lifecycle::StartPath;
    use blaze_core::policy::WorkloadClass;

    use super::*;

    fn instance() -> SandboxInstance {
        SandboxInstance::new(
            BackendKind::Mock,
            WorkloadClass::AgentTool,
            "sha256:test".into(),
            StartPath::Cold,
            "default".into(),
        )
    }

    #[test]
    fn store_centralizes_record_io_scan_and_run_directory_derivation() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("state");
        std::fs::create_dir(&root).expect("state directory");
        let store = StateStore::new(root.clone());
        let instance = instance();

        store.persist(&instance).expect("persist instance");

        let loaded = store.load(instance.id).expect("load instance");
        assert_eq!(loaded.id, instance.id);
        let run_dir = store.run_dir(instance.id).expect("owned run directory");
        assert_eq!(
            std::fs::canonicalize(run_dir.path()).expect("resolve owned run directory"),
            std::fs::canonicalize(root.join(instance.id.to_string())).expect("resolve configured")
        );
        let scanned = StateStore::new(root).scan().expect("scan state store");
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[&instance.id].id, instance.id);
    }

    #[test]
    fn configured_root_replacement_does_not_redirect_record_io() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let configured = temporary.path().join("state");
        let owned = temporary.path().join("owned");
        std::fs::create_dir(&configured).expect("state directory");
        let store = StateStore::new(configured.clone());

        std::fs::rename(&configured, &owned).expect("move owned root");
        std::fs::create_dir(&configured).expect("replacement root");
        let instance = instance();
        store
            .persist(&instance)
            .expect("persist through owned root");

        assert!(
            owned
                .join(instance.id.to_string())
                .join(STATE_FILE)
                .is_file()
        );
        assert!(!configured.join(instance.id.to_string()).exists());
        assert_eq!(
            store.load(instance.id).expect("load owned record").id,
            instance.id
        );
    }

    #[test]
    fn opened_run_directory_replacement_does_not_redirect_record_io() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("state");
        std::fs::create_dir(&root).expect("state directory");
        let store = StateStore::new(root.clone());
        let mut instance = instance();
        store.persist(&instance).expect("initial persist");

        let configured_run_dir = root.join(instance.id.to_string());
        let owned_run_dir = root.join("owned-run-dir");
        std::fs::rename(&configured_run_dir, &owned_run_dir).expect("move owned run directory");
        std::fs::create_dir(&configured_run_dir).expect("replacement run directory");
        instance.policy_name = "updated".into();
        store
            .persist(&instance)
            .expect("persist through owned run directory");

        let owned: SandboxInstance = serde_json::from_slice(
            &std::fs::read(owned_run_dir.join(STATE_FILE)).expect("owned record"),
        )
        .expect("decode owned record");
        assert_eq!(owned.policy_name, "updated");
        assert!(!configured_run_dir.join(STATE_FILE).exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn runtime_path_remains_attached_to_the_opened_run_directory() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("state");
        std::fs::create_dir(&root).expect("state directory");
        let store = StateStore::new(root.clone());
        let instance = instance();
        store.persist(&instance).expect("initial persist");
        let runtime_owner = store.run_dir(instance.id).expect("runtime owner");

        let configured_run_dir = root.join(instance.id.to_string());
        let owned_run_dir = root.join("owned-run-dir");
        std::fs::rename(&configured_run_dir, &owned_run_dir).expect("move owned run directory");
        std::fs::create_dir(&configured_run_dir).expect("replacement run directory");
        std::fs::write(runtime_owner.path().join("backend.pid"), b"42\n")
            .expect("write through owned runtime path");

        assert_eq!(
            std::fs::read(owned_run_dir.join("backend.pid")).expect("owned backend marker"),
            b"42\n"
        );
        assert!(!configured_run_dir.join("backend.pid").exists());
    }

    #[test]
    fn first_publication_does_not_adopt_a_preexisting_directory() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("state");
        std::fs::create_dir(&root).expect("state directory");
        let store = StateStore::new(root.clone());
        let instance = instance();
        let preexisting = root.join(instance.id.to_string());
        std::fs::create_dir(&preexisting).expect("preexisting directory");
        std::fs::write(preexisting.join("owner-marker"), b"external\n")
            .expect("preexisting marker");

        let error = store
            .persist(&instance)
            .expect_err("preexisting directory must not be adopted");

        assert!(matches!(error, BlazeDaemonError::Conflict(_)));
        assert_eq!(
            std::fs::read(preexisting.join("owner-marker")).expect("unchanged marker"),
            b"external\n"
        );
        assert!(!preexisting.join(STATE_FILE).exists());
        assert!(
            std::fs::read_dir(&root)
                .expect("state entries")
                .all(|entry| !entry
                    .expect("state entry")
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".state-"))
        );
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn failed_staging_cleanup_retains_an_uncertain_owner() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("state");
        std::fs::create_dir(&root).expect("state directory");
        let store = StateStore::new(root.clone());
        let instance = instance();
        let preexisting = root.join(instance.id.to_string());
        std::fs::create_dir(&preexisting).expect("preexisting directory");
        std::fs::write(preexisting.join("owner-marker"), b"external\n")
            .expect("preexisting marker");
        let hook = crate::failpoint::TestFailpoint::new(&["state-before-staging-discard"]);

        let error = hook
            .run(async { store.persist(&instance) })
            .await
            .expect_err("failed staging cleanup must retain recovery ownership");

        assert!(matches!(error, BlazeDaemonError::RecoveryRequired(_)));
        assert!(
            error
                .to_string()
                .contains("runtime-directory owner retained for recovery")
        );
        assert!(
            store
                .has_run_dir_residual(instance.id)
                .expect("publication residual")
        );
        assert_eq!(store.retained_run_dir_count(), 1);
        assert!(matches!(
            store.run_dir(instance.id),
            Err(BlazeDaemonError::RecoveryRequired(_))
        ));
        assert_eq!(
            std::fs::read(preexisting.join("owner-marker")).expect("unchanged marker"),
            b"external\n"
        );
        assert!(!preexisting.join(STATE_FILE).exists());

        let staging = std::fs::read_dir(&root)
            .expect("state entries")
            .filter_map(|entry| entry.ok())
            .find(|entry| is_state_staging_name(&entry.file_name().to_string_lossy()))
            .expect("retained staging entry");
        assert!(staging.path().join(STATE_FILE).is_file());
    }

    #[test]
    fn linked_directory_identity_check_detects_replacement() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("state");
        std::fs::create_dir(&root).expect("state directory");
        let store = StateStore::new(root.clone());
        let id = Uuid::new_v4();
        let name = format!(".state-{id}-{}.tmp", Uuid::new_v4());
        let configured = root.join(&name);
        std::fs::create_dir(&configured).expect("staging directory");
        let directory = open(
            &configured,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .expect("open staging directory");
        let owner = OwnedRunDir::new(id, configured.clone(), directory);

        assert_eq!(
            store
                .linked_directory_matches(&name, &owner)
                .expect("identity check"),
            Some(true)
        );

        std::fs::rename(&configured, root.join("moved-staging")).expect("move staging directory");
        std::fs::create_dir(&configured).expect("replacement staging directory");

        assert_eq!(
            store
                .linked_directory_matches(&name, &owner)
                .expect("replacement identity check"),
            Some(false)
        );
    }

    #[test]
    fn destroyed_commit_releases_cache_but_preserves_external_owner() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("state");
        std::fs::create_dir(&root).expect("state directory");
        let store = StateStore::new(root.clone());
        let mut instance = instance();
        store.persist(&instance).expect("initial persist");
        let runtime_owner = store.run_dir(instance.id).expect("runtime owner");

        instance
            .transition(blaze_core::lifecycle::SandboxState::Destroyed)
            .expect("destroy transition");
        store.persist(&instance).expect("terminal persist");

        assert_eq!(store.retained_run_dir_count(), 0);
        assert!(matches!(
            store.run_dir(instance.id),
            Err(BlazeDaemonError::NotFound(_))
        ));
        assert_eq!(
            store.load(instance.id).expect("load terminal record").state,
            blaze_core::lifecycle::SandboxState::Destroyed
        );
        assert_eq!(store.retained_run_dir_count(), 0);

        assert!(matches!(
            store.persist(&instance),
            Err(BlazeDaemonError::Conflict(_))
        ));
        assert_eq!(store.retained_run_dir_count(), 0);

        let configured_run_dir = root.join(instance.id.to_string());
        let owned_run_dir = root.join("owned-terminal-run-dir");
        std::fs::rename(&configured_run_dir, &owned_run_dir).expect("move terminal directory");
        std::fs::create_dir(&configured_run_dir).expect("replacement directory");
        std::fs::write(runtime_owner.path().join("runtime-marker"), b"owned\n")
            .expect("write through external owner");

        assert_eq!(
            std::fs::read(owned_run_dir.join("runtime-marker")).expect("owned marker"),
            b"owned\n"
        );
        assert!(!configured_run_dir.join("runtime-marker").exists());
    }

    #[test]
    fn scan_does_not_retain_terminal_run_directories() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("state");
        std::fs::create_dir(&root).expect("state directory");
        let writer = StateStore::new(root.clone());
        let mut instance = instance();
        instance
            .transition(blaze_core::lifecycle::SandboxState::Destroyed)
            .expect("destroy transition");
        writer.persist(&instance).expect("persist terminal record");

        let reader = StateStore::new(root);
        let scanned = reader.scan().expect("scan state store");

        assert_eq!(
            scanned[&instance.id].state,
            blaze_core::lifecycle::SandboxState::Destroyed
        );
        assert_eq!(reader.retained_run_dir_count(), 0);
        assert!(matches!(
            reader.persist(&instance),
            Err(BlazeDaemonError::Conflict(_))
        ));
        assert_eq!(reader.retained_run_dir_count(), 0);
        assert_eq!(
            reader
                .load(instance.id)
                .expect("reload repeated terminal record")
                .state,
            blaze_core::lifecycle::SandboxState::Destroyed
        );
    }

    #[test]
    fn scan_retains_nonterminal_run_directories() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("state");
        std::fs::create_dir(&root).expect("state directory");
        let writer = StateStore::new(root.clone());
        let mut instance = instance();
        instance
            .transition(blaze_core::lifecycle::SandboxState::RecoveryRequired)
            .expect("recovery transition");
        writer.persist(&instance).expect("persist recovery record");
        drop(writer);

        let reader = StateStore::new(root);
        let scanned = reader.scan().expect("scan state store");

        assert_eq!(
            scanned[&instance.id].state,
            blaze_core::lifecycle::SandboxState::RecoveryRequired
        );
        assert_eq!(reader.retained_run_dir_count(), 1);
        assert!(reader.run_dir(instance.id).is_ok());
    }

    #[test]
    fn recovery_required_record_retains_its_runtime_directory() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("state");
        std::fs::create_dir(&root).expect("state directory");
        let store = StateStore::new(root);
        let mut instance = instance();
        instance
            .transition(blaze_core::lifecycle::SandboxState::RecoveryRequired)
            .expect("recovery transition");

        store.persist(&instance).expect("persist recovery record");

        assert_eq!(store.retained_run_dir_count(), 1);
        assert_eq!(
            store
                .run_dir(instance.id)
                .expect("recovery runtime owner")
                .instance_id(),
            instance.id
        );
    }

    #[test]
    fn startup_scan_removes_known_stale_state_temporaries() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("state");
        std::fs::create_dir(&root).expect("state directory");
        let instance = instance();
        instance.persist(&root).expect("write lifecycle fixture");
        let record_temp = root.join(instance.id.to_string()).join(TEMP_STATE_FILE);
        std::fs::write(&record_temp, b"stale\n").expect("stale record temp");

        let staging_name = format!(".state-{}-{}.tmp", instance.id, Uuid::new_v4());
        let staging = root.join(&staging_name);
        std::fs::create_dir(&staging).expect("stale staging directory");
        std::fs::write(staging.join(STATE_FILE), b"stale\n").expect("stale staged state");
        std::fs::write(staging.join(TEMP_STATE_FILE), b"stale\n").expect("stale staged temp");

        let store = StateStore::new(root);
        let scanned = store.scan().expect("startup scan");

        assert_eq!(scanned[&instance.id].id, instance.id);
        assert!(!record_temp.exists());
        assert!(!staging.exists());
    }

    #[test]
    fn concurrent_commits_keep_record_and_retention_consistent() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("state");
        std::fs::create_dir(&root).expect("state directory");
        let store = StateStore::new(root);

        for iteration in 0..16 {
            let mut active = instance();
            active.policy_name = format!("active-{iteration}");
            store.persist(&active).expect("initial active record");
            let id = active.id;

            let mut terminal = active.clone();
            terminal
                .transition(blaze_core::lifecycle::SandboxState::Destroyed)
                .expect("destroy transition");
            terminal.policy_name = format!("terminal-{iteration}");

            let barrier = Arc::new(std::sync::Barrier::new(3));
            let active_store = store.clone();
            let active_barrier = Arc::clone(&barrier);
            let active_thread = std::thread::spawn(move || {
                active_barrier.wait();
                active_store.persist(&active)
            });
            let terminal_store = store.clone();
            let terminal_barrier = Arc::clone(&barrier);
            let terminal_thread = std::thread::spawn(move || {
                terminal_barrier.wait();
                terminal_store.persist(&terminal)
            });
            barrier.wait();

            let active_result = active_thread.join().expect("active writer thread");
            terminal_thread
                .join()
                .expect("terminal writer thread")
                .expect("terminal persist");

            if let Err(error) = active_result {
                assert!(matches!(error, BlazeDaemonError::Conflict(_)));
            }
            assert_eq!(
                store.load(id).expect("load final record").state,
                blaze_core::lifecycle::SandboxState::Destroyed
            );
            assert!(matches!(
                store.run_dir(id),
                Err(BlazeDaemonError::NotFound(_))
            ));
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn child_inherits_the_owned_runtime_directory() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("state");
        std::fs::create_dir(&root).expect("state directory");
        let store = StateStore::new(root.clone());
        let instance = instance();
        store.persist(&instance).expect("initial persist");
        let runtime_owner = store.run_dir(instance.id).expect("runtime owner");

        let configured_run_dir = root.join(instance.id.to_string());
        let owned_run_dir = root.join("owned-child-run-dir");
        std::fs::rename(&configured_run_dir, &owned_run_dir).expect("move owned run directory");
        std::fs::create_dir(&configured_run_dir).expect("replacement run directory");

        let marker = runtime_owner.path().join("child-marker");
        let mut command = tokio::process::Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("printf child > \"$1\"")
            .arg("sh")
            .arg(marker);
        runtime_owner.inherit_into(&mut command);
        drop(runtime_owner);
        drop(store);
        let status = command.status().await.expect("run child");

        assert!(status.success());
        assert_eq!(
            std::fs::read(owned_run_dir.join("child-marker")).expect("owned child marker"),
            b"child"
        );
        assert!(!configured_run_dir.join("child-marker").exists());
    }

    #[test]
    fn state_root_allows_only_one_production_owner() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("state");
        std::fs::create_dir(&root).expect("state directory");
        let owner = StateStore::open(root.clone()).expect("first owner");

        let error = StateStore::open(root).expect_err("second owner must fail");

        assert!(matches!(error, BlazeDaemonError::Conflict(_)));
        drop(owner);
    }
}
