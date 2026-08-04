// SPDX-License-Identifier: Apache-2.0
//! Exclusive daemon state ownership and Unix-domain socket binding.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::net::{UnixListener, UnixStream};
use tokio::time::timeout;

use crate::error::{BlazeDaemonError, Result};

const LOCK_MODE: u32 = 0o600;
const LOCK_DIRECTORY_MODE: u32 = 0o700;
const SOCKET_LOCK_DIRECTORY: &str = ".blaze-socket-locks";
const SOCKET_PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// A bound API socket whose state ownership remains held for the same lifetime.
pub(super) struct DaemonSocket {
    listener: Option<UnixListener>,
    _lock: DaemonLock,
}

impl DaemonSocket {
    /// Examines and replaces the socket only under exclusive ownership.
    pub(super) async fn bind(lock: DaemonLock) -> Result<Self> {
        let socket_path = &lock.socket_path;
        prepare_socket_path(socket_path).await?;
        let listener =
            UnixListener::bind(socket_path).map_err(|source| BlazeDaemonError::DaemonSocketIo {
                path: socket_path.to_path_buf(),
                source,
            })?;
        Ok(Self {
            listener: Some(listener),
            _lock: lock,
        })
    }

    /// Accepts the next client while retaining exclusive daemon ownership.
    pub(super) async fn accept(&self) -> io::Result<(UnixStream, tokio::net::unix::SocketAddr)> {
        let listener = self.listener.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "daemon socket is no longer accepting connections",
            )
        })?;
        listener.accept().await
    }

    /// Closes the listener while retaining daemon ownership for cleanup.
    pub(super) fn stop_accepting(&mut self) {
        self.listener.take();
    }
}

/// Exclusive daemon ownership tied to one state directory and API socket.
pub(super) struct DaemonLock {
    state_file: File,
    socket_file: Option<File>,
    socket_path: PathBuf,
}

impl DaemonLock {
    /// Acquires ownership before daemon subsystems begin startup.
    pub(super) fn acquire(state_dir: &Path, socket_path: &Path) -> Result<Self> {
        let canonical_state_dir =
            fs::canonicalize(state_dir).map_err(|source| BlazeDaemonError::DaemonLockIo {
                path: state_dir.to_path_buf(),
                source,
            })?;
        let state_lock_path = state_lock_path_for(&canonical_state_dir);
        let (state_file, state_metadata) = open_validated_lock_file(&state_lock_path)?;
        lock_opened_file(&state_file, &state_lock_path, &state_metadata, || {
            BlazeDaemonError::DaemonStateAlreadyOwned {
                state_dir: canonical_state_dir,
            }
        })?;

        let canonical_socket_path = canonicalize_socket_path(socket_path)?;
        let socket_lock_path = socket_lock_path_for(&canonical_socket_path)?;
        let (socket_file, socket_metadata) = open_validated_lock_file(&socket_lock_path)?;
        let shared_lock_object = same_file_object(&state_metadata, &socket_metadata);

        // A state directory may alias the endpoint lock namespace through a
        // separate mount. Compare the validated objects rather than their path
        // spellings so this process never contends with its own state lock.
        let socket_file = if shared_lock_object {
            validate_locked_path(&socket_lock_path, &socket_metadata).map_err(|reason| {
                BlazeDaemonError::InvalidDaemonLock {
                    path: socket_lock_path,
                    reason,
                }
            })?;
            None
        } else {
            lock_opened_file(&socket_file, &socket_lock_path, &socket_metadata, || {
                BlazeDaemonError::DaemonSocketAlreadyOwned {
                    socket: canonical_socket_path.clone(),
                }
            })?;
            Some(socket_file)
        };

        // Every lock file is deliberately left on disk. Removing an advisory
        // lock file would let a new process lock a different inode during
        // teardown.
        Ok(Self {
            state_file,
            socket_file,
            socket_path: canonical_socket_path,
        })
    }
}

impl Drop for DaemonLock {
    fn drop(&mut self) {
        // SAFETY: every acquired file remains open while its advisory lock is
        // released. Process termination still closes the descriptors and
        // releases the locks if this destructor cannot run.
        unsafe {
            if let Some(socket_file) = &self.socket_file {
                libc::flock(socket_file.as_raw_fd(), libc::LOCK_UN);
            }
            libc::flock(self.state_file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn state_lock_path_for(state_dir: &Path) -> PathBuf {
    state_dir.join("daemon.lock")
}

fn canonicalize_socket_path(socket_path: &Path) -> Result<PathBuf> {
    let file_name =
        socket_path
            .file_name()
            .ok_or_else(|| BlazeDaemonError::InvalidDaemonSocket {
                path: socket_path.to_path_buf(),
                reason: "socket path must name an endpoint".to_string(),
            })?;
    let parent = socket_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let canonical_parent =
        fs::canonicalize(parent).map_err(|source| BlazeDaemonError::DaemonSocketIo {
            path: parent.to_path_buf(),
            source,
        })?;
    Ok(canonical_parent.join(file_name))
}

fn socket_lock_path_for(socket_path: &Path) -> Result<PathBuf> {
    let parent = socket_path
        .parent()
        .ok_or_else(|| BlazeDaemonError::InvalidDaemonSocket {
            path: socket_path.to_path_buf(),
            reason: "canonical socket path must have a parent directory".to_string(),
        })?;
    let file_name =
        socket_path
            .file_name()
            .ok_or_else(|| BlazeDaemonError::InvalidDaemonSocket {
                path: socket_path.to_path_buf(),
                reason: "socket path must name an endpoint".to_string(),
            })?;
    let lock_directory = parent.join(SOCKET_LOCK_DIRECTORY);
    ensure_lock_directory(&lock_directory)?;

    let mut lock_name = OsString::from(file_name);
    lock_name.push(".lock");
    Ok(lock_directory.join(lock_name))
}

fn ensure_lock_directory(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.mode(LOCK_DIRECTORY_MODE);
    let created = match builder.create(path) {
        Ok(()) => true,
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => false,
        Err(source) => {
            return Err(BlazeDaemonError::DaemonLockIo {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if created {
        fs::set_permissions(path, fs::Permissions::from_mode(LOCK_DIRECTORY_MODE)).map_err(
            |source| BlazeDaemonError::DaemonLockIo {
                path: path.to_path_buf(),
                source,
            },
        )?;
    }

    let metadata = fs::symlink_metadata(path).map_err(|source| BlazeDaemonError::DaemonLockIo {
        path: path.to_path_buf(),
        source,
    })?;
    let reason = if !metadata.file_type().is_dir() {
        Some("lock namespace is not a directory".to_string())
    } else if metadata.mode() & 0o7777 != LOCK_DIRECTORY_MODE {
        Some(format!(
            "lock namespace mode must be {LOCK_DIRECTORY_MODE:#o}, found {:#o}",
            metadata.mode() & 0o7777
        ))
    } else {
        // SAFETY: `geteuid` has no preconditions and does not modify memory.
        let effective_uid = unsafe { libc::geteuid() };
        (metadata.uid() != effective_uid).then(|| {
            format!(
                "lock namespace owner uid {} does not match effective uid {effective_uid}",
                metadata.uid()
            )
        })
    };
    if let Some(reason) = reason {
        return Err(BlazeDaemonError::InvalidDaemonLock {
            path: path.to_path_buf(),
            reason,
        });
    }
    Ok(())
}

fn open_validated_lock_file(path: &Path) -> Result<(File, fs::Metadata)> {
    let (file, created) = open_lock_file(path)?;
    if created {
        file.set_permissions(fs::Permissions::from_mode(LOCK_MODE))
            .map_err(|source| BlazeDaemonError::DaemonLockIo {
                path: path.to_path_buf(),
                source,
            })?;
    }

    let opened_metadata = validate_opened_lock(&file, path).map_err(|reason| {
        BlazeDaemonError::InvalidDaemonLock {
            path: path.to_path_buf(),
            reason,
        }
    })?;

    Ok((file, opened_metadata))
}

fn same_file_object(first: &fs::Metadata, second: &fs::Metadata) -> bool {
    first.dev() == second.dev() && first.ino() == second.ino()
}

fn lock_opened_file<F>(
    file: &File,
    path: &Path,
    opened_metadata: &fs::Metadata,
    already_owned: F,
) -> Result<()>
where
    F: FnOnce() -> BlazeDaemonError,
{
    // SAFETY: `file` owns a valid descriptor for the entire call. `flock`
    // changes only the advisory lock associated with that open file.
    let lock_result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if lock_result != 0 {
        let source = io::Error::last_os_error();
        if source.kind() == io::ErrorKind::WouldBlock {
            return Err(already_owned());
        }
        return Err(BlazeDaemonError::DaemonLockIo {
            path: path.to_path_buf(),
            source,
        });
    }

    validate_locked_path(path, opened_metadata).map_err(|reason| {
        BlazeDaemonError::InvalidDaemonLock {
            path: path.to_path_buf(),
            reason,
        }
    })?;
    Ok(())
}

fn open_lock_file(path: &Path) -> Result<(File, bool)> {
    match lock_options(true).open(path) {
        Ok(file) => Ok((file, true)),
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            let file = lock_options(false).open(path).map_err(|source| {
                BlazeDaemonError::DaemonLockIo {
                    path: path.to_path_buf(),
                    source,
                }
            })?;
            Ok((file, false))
        }
        Err(source) => Err(BlazeDaemonError::DaemonLockIo {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn lock_options(create_new: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(create_new)
        .mode(LOCK_MODE)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    options
}

fn validate_opened_lock(file: &File, path: &Path) -> std::result::Result<fs::Metadata, String> {
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect opened file: {error}"))?;
    if !metadata.file_type().is_file() {
        return Err("lock target is not a regular file".to_string());
    }
    if metadata.mode() & 0o7777 != LOCK_MODE {
        return Err(format!(
            "mode must be {LOCK_MODE:#o}, found {:#o}",
            metadata.mode() & 0o7777
        ));
    }
    // SAFETY: `geteuid` has no preconditions and does not modify memory.
    let effective_uid = unsafe { libc::geteuid() };
    if metadata.uid() != effective_uid {
        return Err(format!(
            "owner uid {} does not match effective uid {effective_uid}",
            metadata.uid()
        ));
    }
    if metadata.nlink() != 1 {
        return Err(format!(
            "lock file must have one link, found {}",
            metadata.nlink()
        ));
    }

    let path_metadata =
        fs::symlink_metadata(path).map_err(|error| format!("cannot inspect path: {error}"))?;
    if path_metadata.file_type().is_symlink() {
        return Err("lock path is a symbolic link".to_string());
    }
    if path_metadata.dev() != metadata.dev() || path_metadata.ino() != metadata.ino() {
        return Err("lock path changed while it was opened".to_string());
    }
    Ok(metadata)
}

fn validate_locked_path(
    path: &Path,
    opened_metadata: &fs::Metadata,
) -> std::result::Result<(), String> {
    let path_metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect locked path: {error}"))?;
    if path_metadata.file_type().is_symlink() {
        return Err("lock path became a symbolic link".to_string());
    }
    if path_metadata.dev() != opened_metadata.dev() || path_metadata.ino() != opened_metadata.ino()
    {
        return Err("lock path changed while ownership was acquired".to_string());
    }
    if path_metadata.mode() & 0o7777 != LOCK_MODE {
        return Err(format!(
            "mode changed while ownership was acquired: found {:#o}",
            path_metadata.mode() & 0o7777
        ));
    }
    Ok(())
}

async fn prepare_socket_path(socket_path: &Path) -> Result<()> {
    match fs::symlink_metadata(socket_path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            match timeout(SOCKET_PROBE_TIMEOUT, UnixStream::connect(socket_path)).await {
                Ok(Ok(_stream)) => Err(BlazeDaemonError::DaemonSocketAlreadyOwned {
                    socket: socket_path.to_path_buf(),
                }),
                Ok(Err(source)) if source.kind() == io::ErrorKind::ConnectionRefused => {
                    fs::remove_file(socket_path).map_err(|source| {
                        BlazeDaemonError::DaemonSocketIo {
                            path: socket_path.to_path_buf(),
                            source,
                        }
                    })
                }
                Ok(Err(source)) if source.kind() == io::ErrorKind::NotFound => Ok(()),
                Ok(Err(source)) => Err(BlazeDaemonError::DaemonSocketIo {
                    path: socket_path.to_path_buf(),
                    source,
                }),
                Err(_) => Err(BlazeDaemonError::InvalidDaemonSocket {
                    path: socket_path.to_path_buf(),
                    reason: "existing socket did not complete the ownership probe".to_string(),
                }),
            }
        }
        Ok(metadata) => {
            let kind = if metadata.file_type().is_symlink() {
                "symbolic link"
            } else if metadata.is_dir() {
                "directory"
            } else {
                "non-socket file"
            };
            Err(BlazeDaemonError::InvalidDaemonSocket {
                path: socket_path.to_path_buf(),
                reason: format!("existing path is a {kind}"),
            })
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(BlazeDaemonError::DaemonSocketIo {
            path: socket_path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::os::unix::net::UnixListener as StdUnixListener;
    use std::process::{Command, Stdio};

    const ABRUPT_EXIT_SOCKET_ENV: &str = "BLAZE_TEST_ABRUPT_EXIT_SOCKET";
    const ABRUPT_EXIT_READY_ENV: &str = "BLAZE_TEST_ABRUPT_EXIT_READY";
    static SOCKET_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn serialize_socket_test() -> tokio::sync::MutexGuard<'static, ()> {
        // The abrupt-exit test starts a helper process. Between fork and exec,
        // that helper can inherit another concurrent test's listener and keep
        // its endpoint reachable after the parent test drops the listener.
        SOCKET_TEST_LOCK.lock().await
    }

    fn socket_inode(path: &Path) -> u64 {
        fs::symlink_metadata(path).expect("socket metadata").ino()
    }

    fn acquire_for_socket(path: &Path) -> Result<DaemonLock> {
        DaemonLock::acquire(path.parent().expect("socket parent"), path)
    }

    async fn claim_and_bind(path: &Path) -> Result<DaemonSocket> {
        DaemonSocket::bind(acquire_for_socket(path)?).await
    }

    #[tokio::test]
    async fn second_daemon_cannot_replace_owned_socket() {
        let _test_guard = serialize_socket_test().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("api.sock");
        let first = claim_and_bind(&socket_path)
            .await
            .expect("first daemon binds");
        let first_inode = socket_inode(&socket_path);

        let error = acquire_for_socket(&socket_path)
            .err()
            .expect("second daemon must be rejected");

        assert!(matches!(
            error,
            BlazeDaemonError::DaemonStateAlreadyOwned { .. }
        ));
        assert_eq!(socket_inode(&socket_path), first_inode);
        drop(first);
    }

    #[tokio::test]
    async fn same_state_directory_rejects_a_different_socket() {
        let _test_guard = serialize_socket_test().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let first_socket = temp.path().join("first.sock");
        let second_socket = temp.path().join("second.sock");
        let first = claim_and_bind(&first_socket)
            .await
            .expect("first daemon binds");

        let error = acquire_for_socket(&second_socket)
            .err()
            .expect("the shared state directory must reject a second daemon");

        assert!(matches!(
            error,
            BlazeDaemonError::DaemonStateAlreadyOwned { .. }
        ));
        assert!(!second_socket.exists());
        drop(first);
    }

    #[tokio::test]
    async fn same_socket_rejects_a_different_state_directory() {
        let _test_guard = serialize_socket_test().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let first_state = temp.path().join("first-state");
        let second_state = temp.path().join("second-state");
        fs::create_dir(&first_state).expect("create first state directory");
        fs::create_dir(&second_state).expect("create second state directory");
        let socket_path = temp.path().join("api.sock");
        let stale = StdUnixListener::bind(&socket_path).expect("bind stale socket");
        drop(stale);
        let first = DaemonSocket::bind(
            DaemonLock::acquire(&first_state, &socket_path).expect("claim first daemon"),
        )
        .await
        .expect("first daemon replaces stale socket");
        let first_inode = socket_inode(&socket_path);

        let error = DaemonLock::acquire(&second_state, &socket_path)
            .err()
            .expect("the shared socket must reject a second daemon");

        assert!(matches!(
            error,
            BlazeDaemonError::DaemonSocketAlreadyOwned { .. }
        ));
        assert_eq!(socket_inode(&socket_path), first_inode);
        drop(first);
    }

    #[tokio::test]
    async fn socket_lock_does_not_conflict_with_state_lock_name() {
        let _test_guard = serialize_socket_test().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("daemon");

        let daemon = claim_and_bind(&socket_path)
            .await
            .expect("state and socket ownership use separate lock namespaces");

        assert!(
            fs::symlink_metadata(&socket_path)
                .expect("socket metadata")
                .file_type()
                .is_socket()
        );
        assert!(state_lock_path_for(temp.path()).is_file());
        drop(daemon);
    }

    #[test]
    fn shared_state_and_socket_lock_identity_is_acquired_once() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state_dir = temp.path().join(SOCKET_LOCK_DIRECTORY);
        let state_alias = temp.path().join("state-alias");
        fs::DirBuilder::new()
            .mode(LOCK_DIRECTORY_MODE)
            .create(&state_dir)
            .expect("create shared lock directory");
        symlink(&state_dir, &state_alias).expect("create state directory alias");
        let socket_path = temp.path().join("daemon");

        let daemon = DaemonLock::acquire(&state_alias, &socket_path)
            .expect("one daemon may share its state and socket lock identity");

        let state_error = DaemonLock::acquire(&state_dir, &temp.path().join("other.sock"))
            .err()
            .expect("the shared state identity remains exclusive");
        assert!(matches!(
            state_error,
            BlazeDaemonError::DaemonStateAlreadyOwned { .. }
        ));

        let other_state = temp.path().join("other-state");
        fs::create_dir(&other_state).expect("create other state directory");
        let socket_error = DaemonLock::acquire(&other_state, &socket_path)
            .err()
            .expect("the shared socket identity remains exclusive");
        assert!(matches!(
            socket_error,
            BlazeDaemonError::DaemonSocketAlreadyOwned { .. }
        ));

        drop(daemon);
    }

    #[test]
    fn same_file_object_uses_opened_fd_identity() {
        let temp = tempfile::tempdir().expect("tempdir");
        let lock_dir = temp.path().join("lock-dir");
        let lock_alias = temp.path().join("lock-alias");
        fs::create_dir(&lock_dir).expect("create lock directory");
        symlink(&lock_dir, &lock_alias).expect("create lock directory alias");
        let first_path = lock_dir.join("shared.lock");
        let second_path = lock_alias.join("shared.lock");

        let (first_file, first_metadata) =
            open_validated_lock_file(&first_path).expect("open first lock path");
        let (second_file, second_metadata) =
            open_validated_lock_file(&second_path).expect("open aliased lock path");
        let repeated_metadata = first_file.metadata().expect("inspect first lock again");

        assert!(same_file_object(&first_metadata, &repeated_metadata));
        assert_ne!(first_path, second_path);
        assert_ne!(first_file.as_raw_fd(), second_file.as_raw_fd());
        assert!(same_file_object(&first_metadata, &second_metadata));
    }

    #[tokio::test]
    async fn socket_lock_name_remains_available_as_an_endpoint() {
        let _test_guard = serialize_socket_test().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let first_state = temp.path().join("first-state");
        let second_state = temp.path().join("second-state");
        fs::create_dir(&first_state).expect("create first state directory");
        fs::create_dir(&second_state).expect("create second state directory");
        let first_socket = temp.path().join("api.sock");
        let second_socket = temp.path().join("api.sock.lock");
        let first = DaemonSocket::bind(
            DaemonLock::acquire(&first_state, &first_socket).expect("claim first daemon"),
        )
        .await
        .expect("first daemon binds");

        let second = DaemonSocket::bind(
            DaemonLock::acquire(&second_state, &second_socket).expect("claim second daemon"),
        )
        .await
        .expect("lock suffix remains a valid endpoint name");

        assert!(
            fs::symlink_metadata(&second_socket)
                .expect("second socket metadata")
                .file_type()
                .is_socket()
        );
        drop(second);
        drop(first);
    }

    #[tokio::test]
    async fn canonical_state_directory_aliases_share_one_lock() {
        let _test_guard = serialize_socket_test().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let state_dir = temp.path().join("state");
        let state_alias = temp.path().join("state-alias");
        fs::create_dir(&state_dir).expect("create state directory");
        symlink(&state_dir, &state_alias).expect("create state directory alias");
        let first_socket = temp.path().join("first.sock");
        let second_socket = temp.path().join("second.sock");
        let first = DaemonLock::acquire(&state_dir, &first_socket).expect("first daemon locks");

        let error = DaemonLock::acquire(&state_alias, &second_socket)
            .err()
            .expect("canonical aliases must contend on one lock");

        assert!(matches!(
            error,
            BlazeDaemonError::DaemonStateAlreadyOwned { .. }
        ));
        drop(first);
    }

    #[tokio::test]
    async fn canonical_socket_directory_aliases_share_one_lock() {
        let _test_guard = serialize_socket_test().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let first_state = temp.path().join("first-state");
        let second_state = temp.path().join("second-state");
        let socket_dir = temp.path().join("socket-dir");
        let socket_alias = temp.path().join("socket-alias");
        fs::create_dir(&first_state).expect("create first state directory");
        fs::create_dir(&second_state).expect("create second state directory");
        fs::create_dir(&socket_dir).expect("create socket directory");
        symlink(&socket_dir, &socket_alias).expect("create socket directory alias");
        let first_socket = socket_dir.join("api.sock");
        let second_socket = socket_alias.join("api.sock");
        let first =
            DaemonLock::acquire(&first_state, &first_socket).expect("first daemon locks socket");

        let error = DaemonLock::acquire(&second_state, &second_socket)
            .err()
            .expect("canonical socket aliases must contend on one lock");

        assert!(matches!(
            error,
            BlazeDaemonError::DaemonSocketAlreadyOwned { .. }
        ));
        drop(first);
    }

    #[tokio::test]
    async fn released_lock_can_be_reacquired_and_stale_socket_replaced() {
        let _test_guard = serialize_socket_test().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("api.sock");
        let first = claim_and_bind(&socket_path)
            .await
            .expect("first daemon binds");
        drop(first);

        let second = claim_and_bind(&socket_path)
            .await
            .expect("released lock is reusable");

        assert!(
            fs::symlink_metadata(&socket_path)
                .expect("replacement socket metadata")
                .file_type()
                .is_socket()
        );
        drop(second);
    }

    #[tokio::test]
    async fn closing_listener_retains_daemon_ownership() {
        let _test_guard = serialize_socket_test().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("api.sock");
        let mut daemon = claim_and_bind(&socket_path).await.expect("daemon binds");

        daemon.stop_accepting();
        let error = acquire_for_socket(&socket_path)
            .err()
            .expect("closed listener must retain daemon ownership");
        assert!(matches!(
            error,
            BlazeDaemonError::DaemonStateAlreadyOwned { .. }
        ));

        drop(daemon);
        let recovered = acquire_for_socket(&socket_path).expect("ownership releases with daemon");
        drop(recovered);
    }

    #[tokio::test]
    async fn lock_is_released_after_owner_process_exits_abruptly() {
        let _test_guard = serialize_socket_test().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("api.sock");
        let ready_path = temp.path().join("lock-ready");
        let status = Command::new(std::env::current_exe().expect("current test binary"))
            .arg("--exact")
            .arg("daemon_socket::tests::abrupt_exit_lock_helper")
            .arg("--nocapture")
            .env(ABRUPT_EXIT_SOCKET_ENV, &socket_path)
            .env(ABRUPT_EXIT_READY_ENV, &ready_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("run abrupt-exit helper");

        assert_eq!(status.code(), Some(73));
        assert_eq!(
            fs::read(&ready_path).expect("helper acquired lock"),
            b"locked"
        );

        let recovered = acquire_for_socket(&socket_path).expect("kernel released process lock");
        drop(recovered);
    }

    #[test]
    fn abrupt_exit_lock_helper() {
        let Some(socket_path) = std::env::var_os(ABRUPT_EXIT_SOCKET_ENV) else {
            return;
        };
        let Some(ready_path) = std::env::var_os(ABRUPT_EXIT_READY_ENV) else {
            return;
        };
        let _lock =
            acquire_for_socket(Path::new(&socket_path)).expect("helper acquires daemon lock");
        fs::write(ready_path, b"locked").expect("publish helper readiness");

        // SAFETY: `_exit` terminates only this dedicated helper process. It
        // intentionally skips Rust destructors to exercise kernel lock release.
        unsafe {
            libc::_exit(73);
        }
    }

    #[tokio::test]
    async fn stale_socket_is_untouched_when_lock_is_held() {
        let _test_guard = serialize_socket_test().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("api.sock");
        let stale = StdUnixListener::bind(&socket_path).expect("bind stale socket");
        drop(stale);
        let stale_inode = socket_inode(&socket_path);
        let lock = acquire_for_socket(&socket_path).expect("hold daemon lock");

        let error = acquire_for_socket(&socket_path)
            .err()
            .expect("competing daemon must be rejected");

        assert!(matches!(
            error,
            BlazeDaemonError::DaemonStateAlreadyOwned { .. }
        ));
        assert_eq!(socket_inode(&socket_path), stale_inode);
        drop(lock);

        let daemon = claim_and_bind(&socket_path)
            .await
            .expect("owner may replace stale socket");
        assert!(
            fs::symlink_metadata(&socket_path)
                .expect("replacement socket metadata")
                .file_type()
                .is_socket()
        );
        drop(daemon);
    }

    #[tokio::test]
    async fn symlinked_lock_is_rejected_without_touching_socket() {
        let _test_guard = serialize_socket_test().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("api.sock");
        let stale = StdUnixListener::bind(&socket_path).expect("bind stale socket");
        drop(stale);
        let stale_inode = socket_inode(&socket_path);
        let target = temp.path().join("lock-target");
        File::create(&target).expect("create target");
        symlink(
            &target,
            state_lock_path_for(socket_path.parent().expect("socket parent")),
        )
        .expect("create lock symlink");

        assert!(acquire_for_socket(&socket_path).is_err());
        assert_eq!(socket_inode(&socket_path), stale_inode);
    }

    #[tokio::test]
    async fn live_socket_without_lock_is_preserved() {
        let _test_guard = serialize_socket_test().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("api.sock");
        let legacy_listener =
            StdUnixListener::bind(&socket_path).expect("bind legacy daemon socket");
        let original_inode = socket_inode(&socket_path);
        let lock = acquire_for_socket(&socket_path).expect("acquire new daemon lock");

        let error = DaemonSocket::bind(lock)
            .await
            .err()
            .expect("live socket must be rejected");

        assert!(matches!(
            error,
            BlazeDaemonError::DaemonSocketAlreadyOwned { .. }
        ));
        assert_eq!(socket_inode(&socket_path), original_inode);
        drop(legacy_listener);
    }

    #[tokio::test]
    async fn insecure_lock_mode_is_rejected_without_touching_socket() {
        let _test_guard = serialize_socket_test().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("api.sock");
        let stale = StdUnixListener::bind(&socket_path).expect("bind stale socket");
        drop(stale);
        let stale_inode = socket_inode(&socket_path);
        let lock_path = state_lock_path_for(socket_path.parent().expect("socket parent"));
        let lock = File::create(&lock_path).expect("create lock");
        lock.set_permissions(fs::Permissions::from_mode(0o644))
            .expect("set insecure mode");

        let error = acquire_for_socket(&socket_path)
            .err()
            .expect("insecure lock must be rejected");

        assert!(matches!(error, BlazeDaemonError::InvalidDaemonLock { .. }));
        assert_eq!(socket_inode(&socket_path), stale_inode);
    }

    #[tokio::test]
    async fn non_socket_endpoint_is_rejected_without_removal() {
        let _test_guard = serialize_socket_test().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("api.sock");
        fs::write(&socket_path, b"do not remove").expect("write endpoint sentinel");

        let lock = acquire_for_socket(&socket_path).expect("acquire daemon lock");
        let error = DaemonSocket::bind(lock)
            .await
            .err()
            .expect("non-socket endpoint must be rejected");

        assert!(matches!(
            error,
            BlazeDaemonError::InvalidDaemonSocket { .. }
        ));
        assert_eq!(
            fs::read(&socket_path).expect("sentinel remains"),
            b"do not remove"
        );
    }
}
