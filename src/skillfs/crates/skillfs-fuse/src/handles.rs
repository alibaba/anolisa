//! File and directory handle bookkeeping for the FUSE layer.
//!
//! Tracks the per-open state (`HandleEntry`) and per-opendir snapshot
//! (`DirHandleEntry`) referenced by FUSE callbacks via `fh` values, and
//! provides the `O_*` -> `OpenOptions` helper used at `open`/`create`
//! time.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use fuser::FileType;
use parking_lot::RwLock;

use crate::security::ActiveTarget;

pub(crate) struct HandleEntry {
    #[allow(dead_code)]
    pub(crate) ino: u64,
    pub(crate) flags: i32,
    pub(crate) file: Option<std::fs::File>,
    pub(crate) append_mode: bool,
    pub(crate) pinned_target: Option<ActiveTarget>,
}

/// Directory handle entry with snapshot of entries at opendir time
pub(crate) struct DirHandleEntry {
    #[allow(dead_code)]
    pub(crate) ino: u64,
    /// Ordered snapshot of directory entries, frozen at opendir time.
    /// Each entry: (inode, file_type, name)
    pub(crate) entries: Vec<(u64, FileType, String)>,
    /// Physical directory fd for fsyncdir. None for virtual directories.
    pub(crate) dir_file: Option<std::fs::File>,
}

pub(crate) struct HandleManager {
    next_fh: AtomicU64,
    handles: RwLock<HashMap<u64, HandleEntry>>,
    dir_handles: RwLock<HashMap<u64, DirHandleEntry>>,
}

impl HandleManager {
    pub(crate) fn new() -> Self {
        Self {
            next_fh: AtomicU64::new(1),
            handles: RwLock::new(HashMap::new()),
            dir_handles: RwLock::new(HashMap::new()),
        }
    }

    pub(crate) fn allocate(
        &self,
        ino: u64,
        flags: i32,
        file: Option<std::fs::File>,
        pinned_target: Option<ActiveTarget>,
    ) -> u64 {
        let fh = self.next_fh.fetch_add(1, Ordering::Relaxed);
        let append_mode = (flags & libc::O_APPEND) != 0;
        self.handles.write().insert(
            fh,
            HandleEntry {
                ino,
                flags,
                file,
                append_mode,
                pinned_target,
            },
        );
        fh
    }

    /// Allocate a directory handle with a frozen snapshot
    pub(crate) fn allocate_dir(
        &self,
        ino: u64,
        entries: Vec<(u64, FileType, String)>,
        dir_file: Option<std::fs::File>,
    ) -> u64 {
        let fh = self.next_fh.fetch_add(1, Ordering::Relaxed);
        self.dir_handles.write().insert(
            fh,
            DirHandleEntry {
                ino,
                entries,
                dir_file,
            },
        );
        fh
    }

    /// Perform sync on the directory handle's physical fd.
    /// Returns Some(Ok(())) for virtual dirs or successful sync,
    /// Some(Err(e)) for sync failure, None if fh not found.
    pub(crate) fn sync_dir(&self, fh: u64, datasync: bool) -> Option<std::io::Result<()>> {
        let handles = self.dir_handles.read();
        handles.get(&fh).map(|entry| {
            match &entry.dir_file {
                Some(file) => {
                    if datasync {
                        file.sync_data()
                    } else {
                        file.sync_all()
                    }
                }
                None => Ok(()), // Virtual directory: no-op success
            }
        })
    }

    /// Get a clone of the directory snapshot entries
    pub(crate) fn get_dir_entries(&self, fh: u64) -> Option<Vec<(u64, FileType, String)>> {
        self.dir_handles.read().get(&fh).map(|e| e.entries.clone())
    }

    /// Remove a directory handle, returns true if it existed
    pub(crate) fn remove_dir(&self, fh: u64) -> bool {
        self.dir_handles.write().remove(&fh).is_some()
    }

    /// Run `f` on the first handle (in arbitrary order) whose `ino` matches
    /// the given inode AND whose entry carries a real fd. Used by `getattr`
    /// to satisfy POSIX `fstat`-after-`unlink`: the kernel's
    /// `vfs_fstat` path forwards to FUSE getattr WITHOUT setting
    /// `FUSE_GETATTR_FH`, so we cannot just consult the `fh` argument.
    /// Returns `None` if no such handle exists (caller falls back to
    /// path-based stat or ENOENT).
    pub(crate) fn with_handle_for_ino<R>(
        &self,
        ino: u64,
        f: impl FnOnce(&std::fs::File) -> R,
    ) -> Option<R> {
        let handles = self.handles.read();
        for entry in handles.values() {
            if entry.ino == ino {
                if let Some(ref file) = entry.file {
                    return Some(f(file));
                }
            }
        }
        None
    }

    pub(crate) fn with_handle<R>(&self, fh: u64, f: impl FnOnce(&HandleEntry) -> R) -> Option<R> {
        let handles = self.handles.read();
        handles.get(&fh).map(f)
    }

    pub(crate) fn with_handle_mut<R>(
        &self,
        fh: u64,
        f: impl FnOnce(&mut HandleEntry) -> R,
    ) -> Option<R> {
        let mut handles = self.handles.write();
        handles.get_mut(&fh).map(f)
    }

    pub(crate) fn remove(&self, fh: u64) -> Option<HandleEntry> {
        self.handles.write().remove(&fh)
    }
}

pub(crate) fn open_options_from_flags(flags: i32) -> std::fs::OpenOptions {
    let mut opts = std::fs::OpenOptions::new();
    let access = flags & libc::O_ACCMODE;
    match access {
        libc::O_RDONLY => {
            opts.read(true);
        }
        libc::O_WRONLY => {
            opts.write(true);
        }
        libc::O_RDWR => {
            opts.read(true).write(true);
        }
        _ => {
            opts.read(true);
        }
    }
    // O_APPEND only takes effect when the file is opened for writing
    if (flags & libc::O_APPEND) != 0 && access != libc::O_RDONLY {
        opts.append(true);
    }
    // O_TRUNC only takes effect when the file is opened for writing
    if (flags & libc::O_TRUNC) != 0 && access != libc::O_RDONLY {
        opts.truncate(true);
    }
    opts
}
