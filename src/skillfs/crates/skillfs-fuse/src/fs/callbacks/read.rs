//! FUSE read-path callbacks: `open`, `read`, `release`, `flush`, `fsync`.

use std::os::unix::fs::FileExt;
use std::path::Path;

use fuser::{FUSE_ROOT_ID, ReplyData, ReplyEmpty, ReplyOpen, Request};
use tracing::{debug, warn};

use super::super::SkillFs;
use super::super::read_resolution::ReadResolution;
use crate::handles::open_options_from_flags;
use crate::path::{PathType, is_skill_discover_path, parse_path};
use crate::security::{SkillEventAction, SkillEventKind};
use crate::sync::SyncEvent;
use crate::sys::{errno, openat_leaf};

impl SkillFs {
    pub(in crate::fs) fn read_impl(
        &mut self,
        req: &Request,
        ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        debug!(ino, offset, size, "read");

        let path = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => {
                // Open-after-unlink fast path. The inode → path mapping was
                // torn down by `unlink`, but POSIX guarantees the open fd
                // remains usable until last close. If the handle still owns
                // a real file, serve the read directly from it. Reads of
                // virtual SKILL.md content reach this branch only if the
                // inode was forcibly evicted (it cannot be `unlink`ed
                // through FUSE because `unlink` removes the path mapping
                // synchronously) so the handle's `file = None` correctly
                // returns ENOENT here.
                let handle_read = self.handles.with_handle(fh, |entry| {
                    entry.file.as_ref().map(|file| {
                        let mut buf = vec![0u8; size as usize];
                        file.read_at(&mut buf, offset as u64)
                            .map(|n| buf[..n].to_vec())
                    })
                });
                match handle_read {
                    Some(Some(Ok(data))) => reply.data(&data),
                    Some(Some(Err(e))) => reply.error(errno(&e)),
                    _ => reply.error(libc::ENOENT),
                }
                return;
            }
        };

        let path_type = parse_path(Path::new(&path), self.in_place);

        // Read events are high-volume so we emit only on failure to keep the
        // audit stream useful without flooding it with per-syscall successes.
        // The byte-count signal still exists on the Write side.
        let pinned = self
            .handles
            .with_handle(fh, |entry| entry.pinned_target.clone())
            .flatten();
        let content = match path_type.clone() {
            PathType::SkillMd { skill_name }
                if self.is_staging_skill_root(&skill_name)
                    || self.is_pending_install(&skill_name) =>
            {
                // I2/pending: staging and pending install SKILL.md reads
                // from the physical handle.
                let result = self.handles.with_handle(fh, |entry| {
                    if let Some(ref file) = entry.file {
                        let mut buf = vec![0u8; size as usize];
                        match file.read_at(&mut buf, offset as u64) {
                            Ok(n) => Ok(buf[..n].to_vec()),
                            Err(e) => Err(errno(&e)),
                        }
                    } else {
                        Err(libc::EBADF)
                    }
                });
                match result {
                    Some(Ok(data)) => {
                        reply.data(&data);
                        return;
                    }
                    Some(Err(e)) => {
                        reply.error(e);
                        return;
                    }
                    None => {
                        reply.error(libc::EBADF);
                        return;
                    }
                }
            }
            PathType::SkillMd { skill_name } => {
                match self.compiled_skill_md_pinned(&skill_name, pinned.as_ref()) {
                    Some(c) => c,
                    None => {
                        self.emit_op_event(
                            req,
                            &path_type,
                            SkillEventKind::Read,
                            SkillEventAction::Failed,
                            Some(libc::ENOENT),
                            None,
                        );
                        reply.error(libc::ENOENT);
                        return;
                    }
                }
            }
            PathType::Passthrough { .. } | PathType::InboxPassthrough { .. } => {
                // Use fd-backed read via handle
                let result = self.handles.with_handle(fh, |entry| {
                    if let Some(ref file) = entry.file {
                        let mut buf = vec![0u8; size as usize];
                        match file.read_at(&mut buf, offset as u64) {
                            Ok(n) => Ok(buf[..n].to_vec()),
                            Err(e) => Err(errno(&e)),
                        }
                    } else {
                        Err(libc::EBADF)
                    }
                });
                match result {
                    Some(Ok(data)) => {
                        reply.data(&data);
                        return;
                    }
                    Some(Err(e)) => {
                        self.emit_op_event(
                            req,
                            &path_type,
                            SkillEventKind::Read,
                            SkillEventAction::Failed,
                            Some(e),
                            None,
                        );
                        reply.error(e);
                        return;
                    }
                    None => {
                        self.emit_op_event(
                            req,
                            &path_type,
                            SkillEventKind::Read,
                            SkillEventAction::Failed,
                            Some(libc::EBADF),
                            None,
                        );
                        reply.error(libc::EBADF);
                        return;
                    }
                }
            }
            _ => {
                self.emit_op_event(
                    req,
                    &path_type,
                    SkillEventKind::Read,
                    SkillEventAction::Failed,
                    Some(libc::EISDIR),
                    None,
                );
                reply.error(libc::EISDIR);
                return;
            }
        };

        let offset = offset as usize;
        if offset >= content.len() {
            reply.data(&[]);
            return;
        }
        let end = (offset + size as usize).min(content.len());
        reply.data(&content.as_bytes()[offset..end]);
    }
    pub(in crate::fs) fn open_impl(
        &mut self,
        req: &Request,
        ino: u64,
        flags: i32,
        reply: ReplyOpen,
    ) {
        debug!(ino, flags, "open");
        if self.inodes.get(ino).is_none() && ino != FUSE_ROOT_ID {
            reply.error(libc::ENOENT);
            return;
        }

        let path = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let path_type = parse_path(Path::new(&path), self.in_place);
        let access_mode = flags & libc::O_ACCMODE;
        let is_write = access_mode == libc::O_WRONLY || access_mode == libc::O_RDWR;

        // Virtual directory types: return EISDIR for file open
        match &path_type {
            PathType::Root | PathType::SkillsDir | PathType::InboxDir => {
                reply.error(libc::EISDIR);
                return;
            }
            PathType::SkillDir { skill_name } => {
                // skill-discover dir opened for write → EROFS
                if skill_name == "skill-discover" && is_write {
                    reply.error(libc::EROFS);
                    return;
                }
                reply.error(libc::EISDIR);
                return;
            }
            PathType::InboxSkillDir { skill_name } => {
                if !Self::is_inbox_skill_name_allowed(skill_name) {
                    reply.error(libc::ENOENT);
                    return;
                }
                reply.error(libc::EISDIR);
                return;
            }
            PathType::InboxPassthrough { skill_name, .. } => {
                if !Self::is_inbox_skill_name_allowed(skill_name) {
                    reply.error(libc::ENOENT);
                    return;
                }
            }
            _ => {}
        }

        // skill-discover/SKILL.md is always read-only
        if let PathType::SkillMd { ref skill_name } = path_type {
            if skill_name == "skill-discover" {
                if is_write {
                    reply.error(libc::EROFS);
                    return;
                }
                // Read-only open for virtual skill-discover SKILL.md
                let fh = self.handles.allocate(ino, flags, None, None);
                reply.opened(fh, 0);
                return;
            }
        }

        // S1: deny mutating opens (write modes, O_APPEND with write, or
        // O_TRUNC even with O_RDONLY) on `.skill-meta/**` before any I/O.
        // O_RDONLY without O_TRUNC stays allowed so directory traversal
        // and read-only manifest inspection still work.
        let is_mutating_open = is_write || (flags & libc::O_TRUNC) != 0;
        if is_mutating_open {
            // S3: deny mutating opens on a reserved lifecycle namespace.
            // Read-only opens are blocked earlier by `lookup` returning
            // ENOENT, so this gate only matters for callers that already
            // hold an inode for a lifecycle path (defense in depth).
            if let Some(errno) = self.enforce_lifecycle_reservation(
                &path_type,
                SkillEventKind::Write,
                req,
                Some(format!("flags=0x{:x}", flags)),
            ) {
                reply.error(errno);
                return;
            }
            if let Some(errno) = self.enforce_skill_meta(
                &path_type,
                SkillEventKind::Write,
                req,
                Some(format!("flags=0x{:x}", flags)),
            ) {
                reply.error(errno);
                return;
            }
        }

        // For non-virtual paths, resolve physical path
        let mut physical = match self.resolve_physical_path(&path) {
            Some(p) => p,
            None => {
                reply.error(libc::EROFS);
                return;
            }
        };

        // D1.1: ledger-aware read redirection. Hidden skills refuse to
        // open even on a stale inode; snapshot skills serve passthrough
        // reads from the snapshot directory.
        //
        // Crucially, the redirect only kicks in for **non-mutating**
        // opens. `O_RDONLY | O_TRUNC` is mutating from the kernel's
        // perspective (the access mode is read-only but the open also
        // truncates the file), so `is_write == false` is *not* a safe
        // signal — we must use `is_mutating_open`. If we redirected on
        // `!is_write` alone, the truncate branch further down would
        // truncate the snapshot file, violating the D1.1 invariant
        // "snapshots are read-only; writes/create/rename/unlink/setattr
        // still target live source". Mutating opens stay pointed at the
        // live source physical path (write to live, read from snapshot
        // is enforced separately via lookup/getattr/read).
        let mut pinned_target: Option<crate::security::ActiveTarget> = None;
        match &path_type {
            PathType::SkillMd { skill_name } | PathType::Passthrough { skill_name, .. } => {
                // I2: staging roots bypass the active resolver entirely.
                // Pending installs use the same bypass.
                if !self.is_staging_skill_root(skill_name) && !self.is_pending_install(skill_name) {
                    if let Some(ref resolver) = self.active_resolver {
                        pinned_target = resolver.get(skill_name);
                    }
                    let grace_rel = match &path_type {
                        PathType::Passthrough { relative_path, .. } => {
                            Some(relative_path.as_path())
                        }
                        // SkillMd is never grace-allowed: use the literal
                        // "SKILL.md" path so the whitelist check rejects it.
                        PathType::SkillMd { .. } => Some(Path::new("SKILL.md")),
                        _ => None,
                    };
                    match self.resolve_skill_read(skill_name) {
                        ReadResolution::Hidden
                            if !self.is_post_publish_grace_allowed(skill_name, grace_rel) =>
                        {
                            reply.error(libc::ENOENT);
                            return;
                        }
                        ReadResolution::Hidden => {
                            // I4: grace bypass — let the open proceed against source.
                        }
                        ReadResolution::Snapshot { dir, .. } if !is_mutating_open => {
                            if let PathType::Passthrough { relative_path, .. } = &path_type {
                                physical = dir.join(relative_path);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }

        // O_NOFOLLOW check first (higher priority than O_DIRECTORY):
        // If path is a symlink and O_NOFOLLOW is set, block the open.
        // When O_DIRECTORY is also set, kernel sees symlink as non-directory → ENOTDIR.
        // Otherwise, O_NOFOLLOW alone → ELOOP.
        if (flags & libc::O_NOFOLLOW) != 0 {
            if let Ok(m) = std::fs::symlink_metadata(&physical) {
                if m.file_type().is_symlink() {
                    if (flags & libc::O_DIRECTORY) != 0 {
                        // O_NOFOLLOW|O_DIRECTORY on symlink: kernel sees symlink as non-directory
                        reply.error(libc::ENOTDIR);
                    } else {
                        reply.error(libc::ELOOP);
                    }
                    return;
                }
            }
        }

        if (flags & libc::O_DIRECTORY) != 0 {
            if let Ok(meta) = std::fs::metadata(&physical) {
                if !meta.is_dir() {
                    reply.error(libc::ENOTDIR);
                    return;
                }
            }
        }

        // Directory file open: O_DIRECTORY + read-only is allowed (allocate empty handle);
        // write modes on directories return EISDIR (matching Linux semantics).
        if let PathType::Passthrough { .. } = &path_type {
            if let Ok(meta) = std::fs::metadata(&physical) {
                if meta.is_dir() {
                    if (flags & libc::O_DIRECTORY) != 0 {
                        // O_DIRECTORY on actual directory: only O_RDONLY is permitted
                        let access_mode = flags & libc::O_ACCMODE;
                        if access_mode == libc::O_RDONLY {
                            let fh = self.handles.allocate(ino, flags, None, None);
                            reply.opened(fh, 0);
                        } else {
                            // Write mode on directory -> EISDIR
                            reply.error(libc::EISDIR);
                        }
                        return;
                    } else {
                        reply.error(libc::EISDIR);
                        return;
                    }
                }
            }
        }

        // skill-discover passthrough paths are read-only
        if let PathType::Passthrough { ref skill_name, .. } = path_type {
            if is_skill_discover_path(skill_name) && is_write {
                reply.error(libc::EROFS);
                return;
            }
        }

        // SKILL.md: virtual read, physical write
        if let PathType::SkillMd { ref skill_name } = path_type {
            let is_trunc = (flags & libc::O_TRUNC) != 0;

            // O_TRUNC always truncates source, regardless of access mode
            if is_trunc {
                if let Err(e) = std::fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(&physical)
                {
                    warn!(op = "open", ?physical, error = %e, "SKILL.md O_TRUNC failed");
                    reply.error(errno(&e));
                    return;
                }
                self.send_sync(SyncEvent::Reparse {
                    skill_name: skill_name.clone(),
                });
                self.observe_mutation(
                    skill_name,
                    Some(Path::new("SKILL.md")),
                    crate::security::MutationKind::SetattrTruncate,
                );
            }

            if is_write {
                // Open physical file for writing
                match open_options_from_flags(flags).open(&physical) {
                    Ok(file) => {
                        let fh =
                            self.handles
                                .allocate(ino, flags, Some(file), pinned_target.clone());
                        self.emit_op_event(
                            req,
                            &path_type,
                            SkillEventKind::Open,
                            SkillEventAction::Allowed,
                            None,
                            None,
                        );
                        reply.opened(fh, 0);
                    }
                    Err(e) => {
                        warn!(op = "open", ?physical, error = %e, "open failed");
                        let err = errno(&e);
                        self.emit_op_event(
                            req,
                            &path_type,
                            SkillEventKind::Open,
                            SkillEventAction::Failed,
                            Some(err),
                            None,
                        );
                        reply.error(err);
                    }
                }
            } else if self.is_staging_skill_root(skill_name) || self.is_pending_install(skill_name)
            {
                // I2/pending: staging and pending install SKILL.md is
                // served as raw physical file.
                match open_options_from_flags(flags).open(&physical) {
                    Ok(file) => {
                        let fh = self.handles.allocate(ino, flags, Some(file), None);
                        reply.opened(fh, 0);
                    }
                    Err(e) => reply.error(errno(&e)),
                }
            } else {
                // Read-only open for SKILL.md: virtual content, no physical fd needed
                let fh = self
                    .handles
                    .allocate(ino, flags, None, pinned_target.clone());
                self.emit_op_event(
                    req,
                    &path_type,
                    SkillEventKind::Open,
                    SkillEventAction::Allowed,
                    None,
                    None,
                );
                reply.opened(fh, 0);
            }
            return;
        }

        // Passthrough file: open with real fd
        // O_RDONLY|O_TRUNC: truncate first, then open read-only
        let is_trunc = (flags & libc::O_TRUNC) != 0;
        if is_trunc && access_mode == libc::O_RDONLY {
            // Perform truncation as a separate operation (Linux allows O_RDONLY|O_TRUNC to truncate)
            let trunc_result = match std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&physical)
            {
                Ok(f) => Ok(f),
                Err(e) if e.raw_os_error() == Some(libc::ENAMETOOLONG) => {
                    match self.open_parent_dir_for(&path) {
                        Ok((parent_fd, leaf)) => {
                            openat_leaf(&parent_fd, &leaf, libc::O_WRONLY | libc::O_TRUNC, 0)
                        }
                        Err(_) => Err(e),
                    }
                }
                Err(e) => Err(e),
            };
            if let Err(e) = trunc_result {
                let err = errno(&e);
                self.emit_op_event(
                    req,
                    &path_type,
                    SkillEventKind::Open,
                    SkillEventAction::Failed,
                    Some(err),
                    None,
                );
                reply.error(err);
                return;
            }
            match &path_type {
                PathType::Passthrough {
                    skill_name,
                    relative_path,
                } => self.observe_mutation(
                    skill_name,
                    Some(relative_path.as_path()),
                    crate::security::MutationKind::SetattrTruncate,
                ),
                PathType::InboxPassthrough {
                    skill_name,
                    relative_path,
                } => self.inbox_observe_install_complete(
                    skill_name,
                    relative_path.as_path(),
                    crate::security::MutationKind::SetattrTruncate,
                ),
                _ => {}
            }
        }

        // Then open with the requested access mode (open_options_from_flags handles non-RDONLY truncate)
        let opts = open_options_from_flags(flags);
        let primary_open = opts.open(&physical);
        let final_open = match primary_open {
            Ok(f) => Ok(f),
            Err(e) if e.raw_os_error() == Some(libc::ENAMETOOLONG) => {
                match self.open_parent_dir_for(&path) {
                    Ok((parent_fd, leaf)) => openat_leaf(&parent_fd, &leaf, flags, 0),
                    Err(_) => Err(e),
                }
            }
            Err(e) => Err(e),
        };
        match final_open {
            Ok(file) => {
                let fh = self.handles.allocate(ino, flags, Some(file), pinned_target);
                if is_trunc && access_mode != libc::O_RDONLY {
                    match &path_type {
                        PathType::Passthrough {
                            skill_name,
                            relative_path,
                        } => self.observe_mutation(
                            skill_name,
                            Some(relative_path.as_path()),
                            crate::security::MutationKind::SetattrTruncate,
                        ),
                        PathType::InboxPassthrough {
                            skill_name,
                            relative_path,
                        } => self.inbox_observe_install_complete(
                            skill_name,
                            relative_path.as_path(),
                            crate::security::MutationKind::SetattrTruncate,
                        ),
                        _ => {}
                    }
                }
                self.emit_op_event(
                    req,
                    &path_type,
                    SkillEventKind::Open,
                    SkillEventAction::Allowed,
                    None,
                    None,
                );
                reply.opened(fh, 0);
            }
            Err(e) => {
                warn!(op = "open", ?physical, error = %e, "open failed");
                let err = errno(&e);
                self.emit_op_event(
                    req,
                    &path_type,
                    SkillEventKind::Open,
                    SkillEventAction::Failed,
                    Some(err),
                    None,
                );
                reply.error(err);
            }
        }
    }
    pub(in crate::fs) fn release_impl(
        &mut self,
        _req: &Request,
        _ino: u64,
        fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        self.handles.remove(fh);
        reply.ok();
    }
    pub(in crate::fs) fn flush_impl(
        &mut self,
        _req: &Request,
        _ino: u64,
        fh: u64,
        _lock_owner: u64,
        reply: ReplyEmpty,
    ) {
        let exists = self.handles.with_handle(fh, |_| ()).is_some();
        if exists {
            reply.ok();
        } else {
            reply.error(libc::EBADF);
        }
    }
    pub(in crate::fs) fn fsync_impl(
        &mut self,
        _req: &Request,
        _ino: u64,
        fh: u64,
        datasync: bool,
        reply: ReplyEmpty,
    ) {
        let result = self.handles.with_handle(fh, |entry| {
            if let Some(ref file) = entry.file {
                if datasync {
                    file.sync_data()
                } else {
                    file.sync_all()
                }
            } else {
                Ok(()) // virtual path
            }
        });
        match result {
            Some(Ok(())) => reply.ok(),
            Some(Err(e)) => reply.error(errno(&e)),
            None => reply.error(libc::EBADF),
        }
    }
}
