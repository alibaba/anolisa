//! FUSE metadata callbacks: `lookup`, `getattr`, `access`, `statfs`.

use std::path::Path;
use std::time::Duration;

use fuser::{FUSE_ROOT_ID, FileType, ReplyAttr, ReplyEmpty, ReplyEntry, ReplyStatfs, Request};
use tracing::debug;

use super::super::SkillFs;
use super::super::read_resolution::ReadResolution;
use crate::attr::{file_attr_from_metadata, file_attr_from_stat};
use crate::path::{PathType, is_skill_discover_path, parse_path};
use crate::security::{SkillEventKind, lifecycle::is_reserved_lifecycle_name};
use crate::sys::{errno, fstatat_leaf};

impl SkillFs {
    pub(in crate::fs) fn lookup_impl(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &std::ffi::OsStr,
        reply: ReplyEntry,
    ) {
        let name_str = name.to_string_lossy();
        debug!(parent, name = %name_str, "lookup");

        let parent_path = match self.inodes.get_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let path_str = if parent_path == "/" {
            format!("/{}", name_str)
        } else {
            format!("{}/{}", parent_path, name_str)
        };
        let path = Path::new(&path_str);

        match parse_path(path, self.in_place) {
            PathType::Root => {
                let attr = self.dir_attr();
                self.inodes.remember(FUSE_ROOT_ID);
                reply.entry(&Duration::from_secs(1), &attr, 0);
            }
            PathType::SkillsDir => {
                // In-place mode: root acts as skills dir — return root attrs.
                let ino = if self.in_place {
                    FUSE_ROOT_ID
                } else {
                    self.inodes
                        .lookup_by_path(&path_str)
                        .unwrap_or(FUSE_ROOT_ID)
                };
                let mut attr = self.dir_attr();
                attr.ino = ino;
                self.inodes.remember(ino);
                reply.entry(&Duration::from_secs(1), &attr, 0);
            }
            PathType::SkillDir { skill_name } => {
                // S3: lifecycle namespaces are hidden from ordinary lookup.
                if is_reserved_lifecycle_name(&skill_name) {
                    reply.error(libc::ENOENT);
                    return;
                }
                // I2: staging roots are installer-private workspaces.
                // They are hidden from /skills readdir but must remain
                // accessible for lookup so installers can write inside
                // them through the FUSE mount. Check the physical dir
                // directly instead of the store.
                if let Some(ref matcher) = self.staging_matcher {
                    if matcher.is_staging_root(&skill_name) {
                        let physical = self.source_base().join(&skill_name);
                        if physical.is_dir() {
                            let ino = self.inodes.allocate(&path_str, FileType::Directory, parent);
                            self.inodes.remember(ino);
                            let mut attr = self.dir_attr();
                            attr.ino = ino;
                            reply.entry(&Duration::from_secs(1), &attr, 0);
                        } else {
                            reply.error(libc::ENOENT);
                        }
                        return;
                    }
                }
                // Pending installs bypass the active resolver so
                // installers can continue writing via exact path.
                if self.is_pending_install(&skill_name) {
                    let physical = self.source_base().join(&skill_name);
                    if physical.is_dir() {
                        let ino = self.inodes.allocate(&path_str, FileType::Directory, parent);
                        self.inodes.remember(ino);
                        let mut attr = self.dir_attr();
                        attr.ino = ino;
                        reply.entry(&Duration::from_secs(1), &attr, 0);
                    } else {
                        reply.error(libc::ENOENT);
                    }
                    return;
                }
                // D1.1: ledger-hidden skills surface as ENOENT on direct
                // probes, exactly as if the skill did not exist.
                // I4: post-publish grace bypasses the hidden gate so
                // installers can traverse the skill directory to reach
                // whitelisted files within the grace window.
                if matches!(self.resolve_skill_read(&skill_name), ReadResolution::Hidden)
                    && !self.is_post_publish_grace_allowed(&skill_name, None)
                {
                    reply.error(libc::ENOENT);
                    return;
                }
                let exists = skill_name == "skill-discover" || {
                    let store = self.store.read();
                    store.get(&skill_name).is_some()
                };
                if exists {
                    let ino = self.inodes.allocate(&path_str, FileType::Directory, parent);
                    self.inodes.remember(ino);
                    let mut attr = self.dir_attr();
                    attr.ino = ino;
                    reply.entry(&Duration::from_secs(1), &attr, 0);
                } else {
                    reply.error(libc::ENOENT);
                }
            }
            PathType::SkillMd { skill_name } => {
                // S3: lifecycle namespaces are hidden, including their
                // virtual `SKILL.md`. The boundary holds even if a caller
                // bypasses readdir and probes the path directly.
                if is_reserved_lifecycle_name(&skill_name) {
                    reply.error(libc::ENOENT);
                    return;
                }
                // I2: staging roots serve SKILL.md as a raw physical
                // file, not through the compiler/resolver.
                // Pending installs use the same raw physical path.
                if self.is_staging_skill_root(&skill_name) || self.is_pending_install(&skill_name) {
                    let physical = self.source_base().join(&skill_name).join("SKILL.md");
                    match std::fs::symlink_metadata(&physical) {
                        Ok(meta) => {
                            let ino =
                                self.inodes
                                    .allocate(&path_str, FileType::RegularFile, parent);
                            self.inodes.remember(ino);
                            let mut attr = file_attr_from_metadata(&meta);
                            attr.ino = ino;
                            reply.entry(&Duration::from_secs(1), &attr, 0);
                        }
                        Err(e) => reply.error(errno(&e)),
                    }
                    return;
                }
                // D1.1: hidden skills also hide their virtual SKILL.md.
                // `compiled_skill_md` already returns `None` for
                // `ReadResolution::Hidden`, but short-circuiting here
                // keeps the lookup path uniform with the SkillDir branch
                // and avoids an extra read attempt.
                if matches!(self.resolve_skill_read(&skill_name), ReadResolution::Hidden) {
                    reply.error(libc::ENOENT);
                    return;
                }
                match self.compiled_skill_md(&skill_name) {
                    Some(compiled) => {
                        let ino = self
                            .inodes
                            .allocate(&path_str, FileType::RegularFile, parent);
                        // Fetch metadata via fd-safe path to avoid FUSE re-entry.
                        // For snapshots the size projection is read from the
                        // snapshot SKILL.md so attr.size matches the compiled
                        // payload the kernel will see on the next `read`.
                        let mut attr = if skill_name == "skill-discover" {
                            self.virtual_file_attr(compiled.len() as u64)
                        } else {
                            let md_phys = self
                                .skill_read_dir(&skill_name)
                                .map(|d| d.join("SKILL.md"))
                                .unwrap_or_else(|| {
                                    self.source_base().join(&skill_name).join("SKILL.md")
                                });
                            match std::fs::metadata(&md_phys) {
                                Ok(meta) => {
                                    let mut a = file_attr_from_metadata(&meta);
                                    a.size = compiled.len() as u64;
                                    a
                                }
                                Err(_) => self.virtual_file_attr(compiled.len() as u64),
                            }
                        };
                        attr.ino = ino;
                        self.inodes.remember(ino);
                        reply.entry(&Duration::from_secs(1), &attr, 0);
                    }
                    None => reply.error(libc::ENOENT),
                }
            }
            PathType::Passthrough {
                skill_name,
                relative_path,
            } => {
                // S3: lifecycle namespaces are hidden — every descendant
                // of a reserved root is treated as if it does not exist,
                // even if it is physically present in the source tree.
                if is_reserved_lifecycle_name(&skill_name) {
                    reply.error(libc::ENOENT);
                    return;
                }
                // I2: staging roots use the physical source dir
                // directly, bypassing the active resolver.
                // Pending installs use the same bypass.
                // D1.1: hidden skills also hide their passthrough
                // descendants. For fallback skills we resolve against
                // the snapshot dir so the kernel sees the snapshot's
                // tree (sizes, types, file presence) — direct lookups
                // of files that exist only in the live source surface
                // as ENOENT, matching the contract that snapshots are
                // the read-side ground truth.
                let read_dir = if self.is_staging_skill_root(&skill_name)
                    || self.is_pending_install(&skill_name)
                {
                    self.source_base().join(&skill_name)
                } else {
                    match self.skill_read_dir(&skill_name) {
                        Some(d) => d,
                        None => {
                            // I4: grace bypass — if the skill is hidden but
                            // the path matches the post-publish grace
                            // whitelist, resolve against physical source.
                            if self.is_post_publish_grace_allowed(&skill_name, Some(&relative_path))
                            {
                                self.source_base().join(&skill_name)
                            } else {
                                reply.error(libc::ENOENT);
                                return;
                            }
                        }
                    }
                };
                let physical_path = read_dir.join(&relative_path);
                // Use symlink_metadata so a passthrough symlink is reported
                // with FileType::Symlink rather than collapsed onto its
                // target. The kernel issues readlink() afterwards if it
                // wants to follow the link.
                match std::fs::symlink_metadata(&physical_path) {
                    Ok(meta) => {
                        let mut attr = file_attr_from_metadata(&meta);
                        let ino = self.inodes.allocate(&path_str, attr.kind, parent);
                        self.inodes.remember(ino);
                        attr.ino = ino;
                        reply.entry(&Duration::from_secs(1), &attr, 0);
                    }
                    Err(e) if e.raw_os_error() == Some(libc::ENAMETOOLONG) => {
                        // Long-path fallback: the leaf's absolute path
                        // exceeds PATH_MAX so `symlink_metadata` can't
                        // resolve it, but the parent fits and `fstatat`
                        // with just the leaf component succeeds (or
                        // returns the real errno such as ENOENT/ENOTDIR).
                        match self.open_parent_dir_for(&path_str) {
                            Ok((parent_fd, leaf)) => match fstatat_leaf(&parent_fd, &leaf, false) {
                                Ok(st) => {
                                    let mut attr = file_attr_from_stat(&st);
                                    let ino = self.inodes.allocate(&path_str, attr.kind, parent);
                                    self.inodes.remember(ino);
                                    attr.ino = ino;
                                    reply.entry(&Duration::from_secs(1), &attr, 0);
                                }
                                Err(e2) => reply.error(errno(&e2)),
                            },
                            Err(_) => reply.error(errno(&e)),
                        }
                    }
                    Err(e) => reply.error(errno(&e)),
                }
            }
            PathType::InboxDir => {
                // Virtual inbox root. Always present, regardless of
                // active resolver state, so installers can repair
                // hidden / new skills even when `/skills` does not
                // expose them.
                let ino = self.inodes.lookup_by_path(&path_str).unwrap_or_else(|| {
                    self.inodes.allocate(&path_str, FileType::Directory, parent)
                });
                self.inodes.remember(ino);
                let mut attr = self.dir_attr();
                attr.ino = ino;
                reply.entry(&Duration::from_secs(1), &attr, 0);
            }
            PathType::InboxSkillDir { skill_name } => {
                if !Self::is_inbox_skill_name_allowed(&skill_name) {
                    reply.error(libc::ENOENT);
                    return;
                }
                let physical = self.inbox_skill_dir(&skill_name);
                match std::fs::symlink_metadata(&physical) {
                    Ok(meta) if meta.is_dir() => {
                        let ino = self.inodes.allocate(&path_str, FileType::Directory, parent);
                        self.inodes.remember(ino);
                        let mut attr = file_attr_from_metadata(&meta);
                        attr.ino = ino;
                        reply.entry(&Duration::from_secs(1), &attr, 0);
                    }
                    Ok(_) => reply.error(libc::ENOTDIR),
                    Err(e) => reply.error(errno(&e)),
                }
            }
            PathType::InboxPassthrough {
                skill_name,
                relative_path,
            } => {
                if !Self::is_inbox_skill_name_allowed(&skill_name) {
                    reply.error(libc::ENOENT);
                    return;
                }
                let physical = self.inbox_skill_dir(&skill_name).join(&relative_path);
                match std::fs::symlink_metadata(&physical) {
                    Ok(meta) => {
                        let mut attr = file_attr_from_metadata(&meta);
                        let ino = self.inodes.allocate(&path_str, attr.kind, parent);
                        self.inodes.remember(ino);
                        attr.ino = ino;
                        reply.entry(&Duration::from_secs(1), &attr, 0);
                    }
                    Err(e) => reply.error(errno(&e)),
                }
            }
            PathType::Invalid => reply.error(libc::ENOENT),
        }
    }
    pub(in crate::fs) fn getattr_impl(
        &mut self,
        _req: &Request,
        ino: u64,
        fh: Option<u64>,
        reply: ReplyAttr,
    ) {
        debug!(ino, ?fh, "getattr");

        if ino == FUSE_ROOT_ID {
            reply.attr(&Duration::from_secs(1), &self.dir_attr());
            return;
        }

        let path = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => {
                // Open-after-unlink fast path. The path mapping was torn
                // down by `unlink()` but the FUSE handle still references
                // a valid open fd. The kernel's `vfs_fstat` path does NOT
                // set `FUSE_GETATTR_FH`, so the `fh` argument is `None`
                // here even when the caller invoked `fstat` on an open
                // descriptor; we therefore scan handles by ino as well.
                // `file.metadata()` is `fstat(fd)` and works post-unlink,
                // so SKILL.md virtual-size handling is preserved by the
                // path-based branches below for any caller that still has
                // a live path mapping.
                let by_fh = fh.and_then(|h| {
                    self.handles
                        .with_handle(h, |entry| entry.file.as_ref().map(|f| f.metadata()))
                });
                let meta_result = match by_fh {
                    Some(Some(r)) => Some(r),
                    _ => self.handles.with_handle_for_ino(ino, |f| f.metadata()),
                };
                if let Some(meta_result) = meta_result {
                    match meta_result {
                        Ok(meta) => {
                            let mut attr = file_attr_from_metadata(&meta);
                            attr.ino = ino;
                            reply.attr(&Duration::from_secs(1), &attr);
                            return;
                        }
                        Err(e) => {
                            reply.error(errno(&e));
                            return;
                        }
                    }
                }
                reply.error(libc::ENOENT);
                return;
            }
        };

        match parse_path(Path::new(&path), self.in_place) {
            PathType::Root | PathType::SkillsDir => {
                reply.attr(&Duration::from_secs(1), &self.dir_attr());
            }
            PathType::SkillDir { skill_name } => {
                // I2: staging roots are accessible for getattr so
                // installers can stat the directory during writes.
                // Pending installs use the same bypass.
                if let Some(ref matcher) = self.staging_matcher {
                    if matcher.is_staging_root(&skill_name) {
                        reply.attr(&Duration::from_secs(1), &self.dir_attr());
                        return;
                    }
                }
                if self.is_pending_install(&skill_name) {
                    reply.attr(&Duration::from_secs(1), &self.dir_attr());
                    return;
                }
                // I4: grace bypass for getattr on skill dir.
                if matches!(self.resolve_skill_read(&skill_name), ReadResolution::Hidden)
                    && !self.is_post_publish_grace_allowed(&skill_name, None)
                {
                    reply.error(libc::ENOENT);
                    return;
                }
                reply.attr(&Duration::from_secs(1), &self.dir_attr());
            }
            PathType::SkillMd { skill_name } => {
                // I2: staging roots serve SKILL.md as a raw physical file.
                // Pending installs use the same bypass.
                if self.is_staging_skill_root(&skill_name) || self.is_pending_install(&skill_name) {
                    let physical = self.source_base().join(&skill_name).join("SKILL.md");
                    match std::fs::metadata(&physical) {
                        Ok(meta) => {
                            let mut attr = file_attr_from_metadata(&meta);
                            attr.ino = ino;
                            reply.attr(&Duration::from_secs(1), &attr);
                        }
                        Err(e) => reply.error(errno(&e)),
                    }
                    return;
                }
                if matches!(self.resolve_skill_read(&skill_name), ReadResolution::Hidden) {
                    reply.error(libc::ENOENT);
                    return;
                }
                match self.compiled_skill_md(&skill_name) {
                    Some(compiled) => {
                        // Use fd-safe path to avoid FUSE re-entry in in-place mode.
                        // Snapshot mode reads metadata from the snapshot's
                        // SKILL.md so attr.size is consistent with the
                        // compiled payload the kernel will see on `read`.
                        let attr = if skill_name == "skill-discover" {
                            self.virtual_file_attr(compiled.len() as u64)
                        } else {
                            let md_phys = self
                                .skill_read_dir(&skill_name)
                                .map(|d| d.join("SKILL.md"))
                                .unwrap_or_else(|| {
                                    self.source_base().join(&skill_name).join("SKILL.md")
                                });
                            match std::fs::metadata(&md_phys) {
                                Ok(meta) => {
                                    let mut a = file_attr_from_metadata(&meta);
                                    a.size = compiled.len() as u64;
                                    a
                                }
                                Err(_) => self.virtual_file_attr(compiled.len() as u64),
                            }
                        };
                        reply.attr(&Duration::from_secs(1), &attr);
                    }
                    None => reply.error(libc::ENOENT),
                }
            }
            PathType::Passthrough {
                skill_name,
                relative_path,
            } => {
                // I2: staging roots use the physical source dir
                // directly, bypassing the active resolver.
                // Pending installs use the same bypass.
                // D1.1: hidden / snapshot resolution. Hidden surfaces
                // ENOENT; snapshot redirects attr resolution to the
                // snapshot tree so size/type/mtime match the bytes the
                // kernel will receive on the next read.
                let read_dir = if self.is_staging_skill_root(&skill_name)
                    || self.is_pending_install(&skill_name)
                {
                    self.source_base().join(&skill_name)
                } else {
                    match self.skill_read_dir(&skill_name) {
                        Some(d) => d,
                        None => {
                            // I4: grace bypass for getattr on passthrough path.
                            if self.is_post_publish_grace_allowed(&skill_name, Some(&relative_path))
                            {
                                self.source_base().join(&skill_name)
                            } else {
                                reply.error(libc::ENOENT);
                                return;
                            }
                        }
                    }
                };
                let physical_path = read_dir.join(&relative_path);
                // symlink_metadata preserves FileType::Symlink for passthrough
                // links; regular file/directory attrs remain unchanged.
                match std::fs::symlink_metadata(&physical_path) {
                    Ok(meta) => {
                        // file_attr_from_metadata sets ino=0; the kernel
                        // matches getattr to the inode it cached from
                        // lookup, so we must restore the SkillFS-allocated
                        // inode here to avoid (dev, ino) collisions that
                        // confuse tools like `rm -r` ("Circular directory
                        // structure" warnings).
                        let mut attr = file_attr_from_metadata(&meta);
                        attr.ino = ino;
                        reply.attr(&Duration::from_secs(1), &attr);
                    }
                    Err(e) if e.raw_os_error() == Some(libc::ENAMETOOLONG) => {
                        // Long-path fallback: see `lookup` for the same
                        // pattern. We fstatat against the parent dir fd
                        // so the leaf is the only string the syscall
                        // sees.
                        match self.open_parent_dir_for(&path) {
                            Ok((parent_fd, leaf)) => match fstatat_leaf(&parent_fd, &leaf, false) {
                                Ok(st) => {
                                    let mut attr = file_attr_from_stat(&st);
                                    attr.ino = ino;
                                    reply.attr(&Duration::from_secs(1), &attr);
                                }
                                Err(e2) => reply.error(errno(&e2)),
                            },
                            Err(_) => reply.error(errno(&e)),
                        }
                    }
                    Err(e) => reply.error(errno(&e)),
                }
            }
            PathType::InboxDir => {
                reply.attr(&Duration::from_secs(1), &self.dir_attr());
            }
            PathType::InboxSkillDir { skill_name } => {
                if !Self::is_inbox_skill_name_allowed(&skill_name) {
                    reply.error(libc::ENOENT);
                    return;
                }
                let physical = self.inbox_skill_dir(&skill_name);
                match std::fs::symlink_metadata(&physical) {
                    Ok(meta) => {
                        let mut attr = file_attr_from_metadata(&meta);
                        attr.ino = ino;
                        reply.attr(&Duration::from_secs(1), &attr);
                    }
                    Err(e) => reply.error(errno(&e)),
                }
            }
            PathType::InboxPassthrough {
                skill_name,
                relative_path,
            } => {
                if !Self::is_inbox_skill_name_allowed(&skill_name) {
                    reply.error(libc::ENOENT);
                    return;
                }
                let physical = self.inbox_skill_dir(&skill_name).join(&relative_path);
                match std::fs::symlink_metadata(&physical) {
                    Ok(meta) => {
                        let mut attr = file_attr_from_metadata(&meta);
                        attr.ino = ino;
                        reply.attr(&Duration::from_secs(1), &attr);
                    }
                    Err(e) => reply.error(errno(&e)),
                }
            }
            PathType::Invalid => reply.error(libc::ENOENT),
        }
    }
    pub(in crate::fs) fn statfs_impl(&mut self, _req: &Request, _ino: u64, reply: ReplyStatfs) {
        let source = self.source_base();
        let c_path = match std::ffi::CString::new(source.to_string_lossy().into_owned()) {
            Ok(p) => p,
            Err(_) => return reply.error(libc::EINVAL),
        };

        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        let ret = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };

        if ret != 0 {
            let e = std::io::Error::last_os_error();
            return reply.error(errno(&e));
        }

        reply.statfs(
            stat.f_blocks,
            stat.f_bfree,
            stat.f_bavail,
            stat.f_files,
            stat.f_ffree,
            stat.f_bsize as u32,
            stat.f_namemax as u32,
            stat.f_frsize as u32,
        );
    }
    pub(in crate::fs) fn access_impl(
        &mut self,
        req: &Request,
        ino: u64,
        mask: i32,
        reply: ReplyEmpty,
    ) {
        let valid_bits = libc::F_OK | libc::R_OK | libc::W_OK | libc::X_OK;
        if mask & !valid_bits != 0 {
            return reply.error(libc::EINVAL);
        }

        let path = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => return reply.error(libc::ENOENT),
        };

        let path_type = parse_path(Path::new(&path), self.in_place);

        match path_type {
            PathType::Root | PathType::SkillsDir | PathType::InboxDir => {
                if (mask & libc::W_OK) != 0 {
                    reply.error(libc::EACCES);
                } else {
                    reply.ok();
                }
            }
            PathType::SkillDir { .. } => {
                // All visible skill directories: virtual semantics
                // F_OK/R_OK/X_OK succeed, W_OK denied
                if (mask & libc::W_OK) != 0 {
                    reply.error(libc::EACCES);
                } else {
                    reply.ok();
                }
            }
            PathType::InboxSkillDir { ref skill_name } => {
                if !Self::is_inbox_skill_name_allowed(skill_name) {
                    reply.error(libc::ENOENT);
                    return;
                }
                let dir_path = self.inbox_skill_dir(skill_name);
                let result = self.check_physical_access_result(&dir_path, mask, req);
                if result == 0 {
                    reply.ok();
                } else {
                    reply.error(result);
                }
            }
            PathType::InboxPassthrough {
                ref skill_name,
                ref relative_path,
            } => {
                if !Self::is_inbox_skill_name_allowed(skill_name) {
                    reply.error(libc::ENOENT);
                    return;
                }
                if (mask & libc::W_OK) != 0 {
                    let pt = PathType::InboxPassthrough {
                        skill_name: skill_name.clone(),
                        relative_path: relative_path.clone(),
                    };
                    if let Some(errno) = self.enforce_skill_meta(
                        &pt,
                        SkillEventKind::Metadata,
                        req,
                        Some(format!("access mask=0x{:x}", mask)),
                    ) {
                        reply.error(errno);
                        return;
                    }
                }
                let file_path = self.inbox_skill_dir(skill_name).join(relative_path);
                let result = self.check_physical_access_result(&file_path, mask, req);
                if result == 0 {
                    reply.ok();
                } else {
                    reply.error(result);
                }
            }
            PathType::SkillMd { ref skill_name } => {
                if is_skill_discover_path(skill_name) {
                    if (mask & (libc::W_OK | libc::X_OK)) != 0 {
                        reply.error(libc::EACCES);
                    } else {
                        reply.ok();
                    }
                } else {
                    let file_path = self.source_base().join(skill_name).join("SKILL.md");
                    let result = self.check_physical_access_result(&file_path, mask, req);
                    if result == 0 {
                        reply.ok();
                    } else {
                        reply.error(result);
                    }
                }
            }
            PathType::Passthrough {
                ref skill_name,
                ref relative_path,
            } => {
                if is_skill_discover_path(skill_name) {
                    if (mask & (libc::W_OK | libc::X_OK)) != 0 {
                        reply.error(libc::EACCES);
                    } else {
                        reply.ok();
                    }
                } else {
                    // S1: deny W_OK on `.skill-meta/**`. R_OK/X_OK/F_OK
                    // still defer to the underlying physical permissions.
                    if (mask & libc::W_OK) != 0 {
                        let pt = PathType::Passthrough {
                            skill_name: skill_name.clone(),
                            relative_path: relative_path.clone(),
                        };
                        if let Some(errno) = self.enforce_skill_meta(
                            &pt,
                            SkillEventKind::Metadata,
                            req,
                            Some(format!("access mask=0x{:x}", mask)),
                        ) {
                            reply.error(errno);
                            return;
                        }
                    }
                    let file_path = self.source_base().join(skill_name).join(relative_path);
                    let result = self.check_physical_access_result(&file_path, mask, req);
                    if result == 0 {
                        reply.ok();
                    } else {
                        reply.error(result);
                    }
                }
            }
            PathType::Invalid => {
                reply.error(libc::ENOENT);
            }
        }
    }
}
