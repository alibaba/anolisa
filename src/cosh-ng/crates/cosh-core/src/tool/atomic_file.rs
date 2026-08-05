//! Durable atomic replacement for general user files written by tools.

use std::fmt;
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

const TEMP_NAME_ATTEMPTS: usize = 16;
const SYMLINK_LIMIT: usize = 40;

#[derive(Debug)]
pub(super) enum ReplaceError {
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Conflict {
        path: PathBuf,
    },
    Committed {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for ReplaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "failed to {operation} {}: {source}",
                path.display()
            ),
            Self::Conflict { path } => write!(
                formatter,
                "Edit conflict: {} changed after it was read; no changes were written",
                path.display()
            ),
            Self::Committed { path, source } => write!(
                formatter,
                "replacement of {} was committed, but syncing its parent directory failed: \
                 {source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ReplaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } | Self::Committed { source, .. } => Some(source),
            Self::Conflict { .. } => None,
        }
    }
}

#[derive(Debug)]
pub(super) struct FileSnapshot {
    bytes: Vec<u8>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl FileSnapshot {
    fn read(path: &Path) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let metadata = file.metadata()?;
        let mut bytes = Vec::with_capacity(metadata.len().try_into().unwrap_or(0));
        file.read_to_end(&mut bytes)?;

        Ok(Self {
            bytes,
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        })
    }

    pub(super) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn still_matches(&self, current: &Self) -> bool {
        self.bytes == current.bytes && {
            #[cfg(unix)]
            {
                self.device == current.device && self.inode == current.inode
            }
            #[cfg(not(unix))]
            {
                true
            }
        }
    }
}

pub(super) async fn read_snapshot(path: PathBuf) -> Result<FileSnapshot, ReplaceError> {
    let error_path = path.clone();
    tokio::task::spawn_blocking(move || FileSnapshot::read(&path))
        .await
        .map_err(|error| ReplaceError::Io {
            operation: "join snapshot read for",
            path: error_path.clone(),
            source: io::Error::other(error),
        })?
        .map_err(|source| ReplaceError::Io {
            operation: "read",
            path: error_path,
            source,
        })
}

pub(super) async fn replace(
    path: PathBuf,
    bytes: Vec<u8>,
    expected: Option<FileSnapshot>,
) -> Result<(), ReplaceError> {
    let error_path = path.clone();
    tokio::task::spawn_blocking(move || {
        replace_with_hooks(
            &path,
            &bytes,
            expected.as_ref(),
            |_, _| Ok(()),
            |_, _| Ok(()),
            |_| Ok(()),
        )
    })
    .await
    .map_err(|error| ReplaceError::Io {
        operation: "join atomic replacement for",
        path: error_path,
        source: io::Error::other(error),
    })?
}

fn replace_with_hooks<BeforeWrite, BeforeCommit, BeforeDirectorySync>(
    requested_path: &Path,
    bytes: &[u8],
    expected: Option<&FileSnapshot>,
    before_write: BeforeWrite,
    before_commit: BeforeCommit,
    before_directory_sync: BeforeDirectorySync,
) -> Result<(), ReplaceError>
where
    BeforeWrite: FnOnce(&Path, &Path) -> io::Result<()>,
    BeforeCommit: FnOnce(&Path, &Path) -> io::Result<()>,
    BeforeDirectorySync: FnOnce(&Path) -> io::Result<()>,
{
    let target = resolve_target(requested_path).map_err(|source| ReplaceError::Io {
        operation: "resolve",
        path: requested_path.to_path_buf(),
        source,
    })?;
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let permissions = target_permissions(&target)?;
    let (mut temporary, mut guard) = create_temporary(parent)?;

    if let Some(permissions) = permissions {
        temporary
            .set_permissions(permissions)
            .map_err(|source| io_error("preserve permissions on", guard.path(), source))?;
    }
    before_write(&target, guard.path())
        .map_err(|source| io_error("prepare temporary file for", &target, source))?;
    temporary
        .write_all(bytes)
        .map_err(|source| io_error("write temporary file", guard.path(), source))?;
    temporary
        .flush()
        .map_err(|source| io_error("flush temporary file", guard.path(), source))?;
    temporary
        .sync_all()
        .map_err(|source| io_error("sync temporary file", guard.path(), source))?;
    drop(temporary);

    // The directory inode stays stable across rename, so every helper user
    // serializes the snapshot check and commit on the same advisory lock.
    let directory =
        File::open(parent).map_err(|source| io_error("open parent directory", parent, source))?;
    directory
        .lock_exclusive()
        .map_err(|source| io_error("lock parent directory", parent, source))?;
    before_commit(&target, guard.path())
        .map_err(|source| io_error("prepare replacement of", &target, source))?;
    if let Some(expected) = expected {
        let current = match FileSnapshot::read(&target) {
            Ok(current) => current,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Err(ReplaceError::Conflict { path: target });
            }
            Err(source) => return Err(io_error("verify", &target, source)),
        };
        if !expected.still_matches(&current) {
            return Err(ReplaceError::Conflict { path: target });
        }
    }

    fs::rename(guard.path(), &target)
        .map_err(|source| io_error("atomically replace", &target, source))?;
    guard.disarm();

    before_directory_sync(parent).map_err(|source| ReplaceError::Committed {
        path: target.clone(),
        source,
    })?;
    directory
        .sync_all()
        .map_err(|source| ReplaceError::Committed {
            path: target,
            source,
        })?;
    Ok(())
}

fn resolve_target(path: &Path) -> io::Result<PathBuf> {
    let mut target = path.to_path_buf();
    let mut followed = 0;
    loop {
        match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                if followed == SYMLINK_LIMIT {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "too many symbolic links while resolving target",
                    ));
                }
                let referent = fs::read_link(&target)?;
                target = if referent.is_absolute() {
                    referent
                } else {
                    target
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join(referent)
                };
                followed += 1;
            }
            Ok(_) => return Ok(target),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(target),
            Err(error) => return Err(error),
        }
    }
}

fn target_permissions(path: &Path) -> Result<Option<Permissions>, ReplaceError> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata.permissions())),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(io_error("inspect permissions for", path, source)),
    }
}

fn create_temporary(parent: &Path) -> Result<(File, TemporaryGuard), ReplaceError> {
    for _ in 0..TEMP_NAME_ATTEMPTS {
        let path = parent.join(format!(".cosh-tmp-{}", uuid::Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o666);

        match options.open(&path) {
            Ok(file) => return Ok((file, TemporaryGuard::new(path))),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => return Err(io_error("create temporary file", &path, source)),
        }
    }

    Err(io_error(
        "create unique temporary file in",
        parent,
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "temporary name collision limit reached",
        ),
    ))
}

fn io_error(operation: &'static str, path: &Path, source: io::Error) -> ReplaceError {
    ReplaceError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

struct TemporaryGuard {
    path: PathBuf,
    armed: bool,
}

impl TemporaryGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TemporaryGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
pub(super) fn replace_with_before_commit_for_test<BeforeCommit>(
    path: &Path,
    bytes: &[u8],
    expected: &FileSnapshot,
    before_commit: BeforeCommit,
) -> Result<(), ReplaceError>
where
    BeforeCommit: FnOnce(&Path, &Path) -> io::Result<()>,
{
    replace_with_hooks(
        path,
        bytes,
        Some(expected),
        |_, _| Ok(()),
        before_commit,
        |_| Ok(()),
    )
}

#[cfg(test)]
mod tests {
    use std::io::{Seek, SeekFrom};

    #[cfg(unix)]
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    use super::*;

    fn temporary_entries(parent: &Path) -> Vec<PathBuf> {
        fs::read_dir(parent)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".cosh-tmp-"))
            })
            .collect()
    }

    #[test]
    #[cfg(unix)]
    fn replacement_uses_rename_and_preserves_permissions() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("source.rs");
        fs::write(&target, b"old content").unwrap();
        fs::set_permissions(&target, Permissions::from_mode(0o640)).unwrap();
        let original_inode = fs::metadata(&target).unwrap().ino();
        let mut original_handle = File::open(&target).unwrap();

        replace_with_hooks(
            &target,
            b"new content",
            None,
            |_, _| Ok(()),
            |_, _| Ok(()),
            |_| Ok(()),
        )
        .unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"new content");
        assert_ne!(fs::metadata(&target).unwrap().ino(), original_inode);
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o640
        );
        original_handle.seek(SeekFrom::Start(0)).unwrap();
        let mut original_content = String::new();
        original_handle
            .read_to_string(&mut original_content)
            .unwrap();
        assert_eq!(original_content, "old content");
        assert!(temporary_entries(directory.path()).is_empty());
    }

    #[test]
    fn pre_commit_failure_keeps_target_and_cleans_unique_same_directory_temp() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("config.toml");
        fs::write(&target, b"stable").unwrap();
        let mut temporary_path = None;

        let error = replace_with_hooks(
            &target,
            b"candidate",
            None,
            |_, _| Ok(()),
            |_, temporary| {
                temporary_path = Some(temporary.to_path_buf());
                Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "injected interruption",
                ))
            },
            |_| Ok(()),
        )
        .unwrap_err();

        let temporary_path = temporary_path.unwrap();
        assert_eq!(temporary_path.parent(), Some(directory.path()));
        assert!(temporary_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(".cosh-tmp-"));
        assert!(error.to_string().contains("injected interruption"));
        assert_eq!(fs::read(&target).unwrap(), b"stable");
        assert!(!temporary_path.exists());
    }

    #[test]
    fn rename_failure_keeps_target_and_cleans_temp() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("existing-directory");
        fs::create_dir(&target).unwrap();

        let error = replace_with_hooks(
            &target,
            b"candidate",
            None,
            |_, _| Ok(()),
            |_, _| Ok(()),
            |_| Ok(()),
        )
        .unwrap_err();

        assert!(error.to_string().contains("atomically replace"));
        assert!(target.is_dir());
        assert!(temporary_entries(directory.path()).is_empty());
    }

    #[test]
    fn concurrent_content_change_is_rejected_without_overwrite() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("source.rs");
        fs::write(&target, b"original").unwrap();
        let snapshot = FileSnapshot::read(&target).unwrap();

        let error = replace_with_hooks(
            &target,
            b"edited",
            Some(&snapshot),
            |_, _| Ok(()),
            |target, _| fs::write(target, b"intervening change"),
            |_| Ok(()),
        )
        .unwrap_err();

        assert!(matches!(error, ReplaceError::Conflict { .. }));
        assert_eq!(fs::read(&target).unwrap(), b"intervening change");
        assert!(temporary_entries(directory.path()).is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn same_content_replacement_is_detected_by_file_identity() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("source.rs");
        fs::write(&target, b"original").unwrap();
        let snapshot = FileSnapshot::read(&target).unwrap();

        let error = replace_with_hooks(
            &target,
            b"edited",
            Some(&snapshot),
            |_, _| Ok(()),
            |target, _| {
                let replacement = target.with_extension("replacement");
                fs::write(&replacement, b"original")?;
                fs::rename(replacement, target)
            },
            |_| Ok(()),
        )
        .unwrap_err();

        assert!(matches!(error, ReplaceError::Conflict { .. }));
        assert_eq!(fs::read(&target).unwrap(), b"original");
    }

    #[test]
    #[cfg(unix)]
    fn existing_permissions_are_applied_before_any_content_is_written() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("credentials.txt");
        fs::write(&target, b"old secret").unwrap();
        fs::set_permissions(&target, Permissions::from_mode(0o600)).unwrap();
        let mut observed_mode = None;
        let mut observed_length = None;

        let error = replace_with_hooks(
            &target,
            b"new secret",
            None,
            |_, temporary| {
                let metadata = fs::metadata(temporary)?;
                observed_mode = Some(metadata.permissions().mode() & 0o777);
                observed_length = Some(metadata.len());
                Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "stop before write",
                ))
            },
            |_, _| Ok(()),
            |_| Ok(()),
        )
        .unwrap_err();

        assert_eq!(observed_mode, Some(0o600));
        assert_eq!(observed_length, Some(0));
        assert!(error.to_string().contains("stop before write"));
        assert_eq!(fs::read(&target).unwrap(), b"old secret");
        assert!(temporary_entries(directory.path()).is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_snapshot_commits_allow_only_one_writer() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("source.rs");
        fs::write(&target, b"original").unwrap();
        let first_snapshot = read_snapshot(target.clone()).await.unwrap();
        let second_snapshot = read_snapshot(target.clone()).await.unwrap();

        let (first, second) = tokio::join!(
            replace(target.clone(), b"first".to_vec(), Some(first_snapshot)),
            replace(target.clone(), b"second".to_vec(), Some(second_snapshot)),
        );

        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        assert_eq!(
            usize::from(matches!(first, Err(ReplaceError::Conflict { .. })))
                + usize::from(matches!(second, Err(ReplaceError::Conflict { .. }))),
            1
        );
        assert!(matches!(
            fs::read(&target).unwrap().as_slice(),
            b"first" | b"second"
        ));
    }

    #[test]
    fn parent_sync_failure_reports_that_replacement_was_committed() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("source.rs");
        fs::write(&target, b"old").unwrap();

        let error = replace_with_hooks(
            &target,
            b"new",
            None,
            |_, _| Ok(()),
            |_, _| Ok(()),
            |_| Err(io::Error::other("injected sync failure")),
        )
        .unwrap_err();

        assert!(matches!(error, ReplaceError::Committed { .. }));
        assert!(error.to_string().contains("was committed"));
        assert!(error.to_string().contains("injected sync failure"));
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(temporary_entries(directory.path()).is_empty());
    }
}
