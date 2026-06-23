//! FUSE directory callbacks: `opendir`, `readdir`, `releasedir`, `fsyncdir`, plus the `readdir_dynamic` helper.

use std::path::Path;

use fuser::{FUSE_ROOT_ID, FileType, ReplyDirectory, ReplyEmpty, ReplyOpen, Request};
use tracing::{debug, warn};

use super::super::SkillFs;
use super::super::read_resolution::ReadResolution;
use crate::attr::dir_entry_file_type;
use crate::path::{PathType, is_skill_discover_path, parse_path};
use crate::security::{inbox::INBOX_DIR_NAME, lifecycle::is_reserved_lifecycle_name};
use crate::sys::errno;

impl SkillFs {
    pub(in crate::fs) fn readdir_dynamic(
        &mut self,
        ino: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let path = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let entries: Vec<(u64, FileType, String)> =
            match parse_path(Path::new(&path), self.in_place) {
                PathType::Root => {
                    let inbox_ino = self
                        .inodes
                        .lookup_by_path("/.skillfs-inbox")
                        .unwrap_or_else(|| {
                            self.inodes.allocate(
                                "/.skillfs-inbox",
                                FileType::Directory,
                                FUSE_ROOT_ID,
                            )
                        });
                    vec![
                        (FUSE_ROOT_ID, FileType::Directory, ".".to_string()),
                        (FUSE_ROOT_ID, FileType::Directory, "..".to_string()),
                        (
                            self.inodes.lookup_by_path("/skills").unwrap_or(2),
                            FileType::Directory,
                            "skills".to_string(),
                        ),
                        (inbox_ino, FileType::Directory, INBOX_DIR_NAME.to_string()),
                    ]
                }
                PathType::SkillsDir => {
                    let mut skill_names = self.primary_skill_names();
                    // S3: lifecycle namespaces never appear in ordinary
                    // `/skills` listings. The store loader already skips
                    // hidden top-level directories, but defend in depth here
                    // in case a placeholder lands in the store via mkdir
                    // before the S3 mkdir gate fires.
                    skill_names.retain(|n| !is_reserved_lifecycle_name(n));
                    // I2: staging roots are installer-private workspaces,
                    // not skills. Hide them from the /skills listing.
                    if let Some(ref matcher) = self.staging_matcher {
                        skill_names.retain(|n| !matcher.is_staging_root(n));
                    }
                    // Pending installs are hidden from discovery until
                    // completeness check passes and activation exists.
                    if self.pending_install_controller.is_some() {
                        skill_names.retain(|n| !self.is_pending_install(n));
                    }
                    // D1.1: mirror the opendir filter so the dynamic
                    // fallback path also hides ledger-hidden skills.
                    if self.active_resolver.is_some() {
                        skill_names.retain(|n| {
                            n == "skill-discover"
                                || !matches!(self.resolve_skill_read(n), ReadResolution::Hidden)
                        });
                    }
                    let skills_dir_ino = self.skills_dir_ino();

                    let mut entries: Vec<(u64, FileType, String)> = vec![
                        (ino, FileType::Directory, ".".to_string()),
                        (FUSE_ROOT_ID, FileType::Directory, "..".to_string()),
                    ];

                    for name in &skill_names {
                        let skill_path = self.skill_inode_path(name);
                        let skill_ino = self.inodes.readdir_ino(&skill_path);
                        entries.push((skill_ino, FileType::Directory, name.clone()));
                    }

                    if !skill_names.iter().any(|n| n == "skill-discover") {
                        let discover_path = self.skill_inode_path("skill-discover");
                        let discover_ino = self.inodes.readdir_ino(&discover_path);
                        entries.push((
                            discover_ino,
                            FileType::Directory,
                            "skill-discover".to_string(),
                        ));
                    }

                    // L1: in in-place mode, the FUSE root IS the skills
                    // directory, so the inbox appears here. In normal
                    // mode the inbox lives under `/` and is exposed by
                    // the `Root` branch above; this gate avoids
                    // double-listing it when readdir descends into
                    // `/skills`.
                    if self.in_place {
                        let inbox_ino = self
                            .inodes
                            .lookup_by_path("/.skillfs-inbox")
                            .unwrap_or_else(|| {
                                self.inodes.allocate(
                                    "/.skillfs-inbox",
                                    FileType::Directory,
                                    skills_dir_ino,
                                )
                            });
                        entries.push((inbox_ino, FileType::Directory, INBOX_DIR_NAME.to_string()));
                    }

                    entries
                }
                PathType::SkillDir { skill_name } => {
                    // I2: staging roots bypass the active resolver and
                    // enumerate physical entries directly — no compiled
                    // SKILL.md virtual entry, no skill-discover virtual
                    // entries.  Pending installs use the same bypass.
                    if self.is_staging_skill_root(&skill_name)
                        || self.is_pending_install(&skill_name)
                    {
                        let parent_ino = self.skills_dir_ino();
                        let mut entries: Vec<(u64, FileType, String)> = vec![
                            (ino, FileType::Directory, ".".to_string()),
                            (parent_ino, FileType::Directory, "..".to_string()),
                        ];
                        let phys_dir = self.source_base().join(&skill_name);
                        if let Ok(dir_iter) = std::fs::read_dir(&phys_dir) {
                            for entry in dir_iter.flatten() {
                                let name = entry.file_name().to_string_lossy().to_string();
                                let kind = dir_entry_file_type(&entry);
                                let entry_path = format!("{}/{}", path, name);
                                let entry_ino = self.inodes.readdir_ino(&entry_path);
                                entries.push((entry_ino, kind, name));
                            }
                        }
                        entries
                    } else {
                        // D1.1: ledger-hidden skills do not list anything.
                        // I4: grace does NOT bypass readdir — the skill stays
                        // hidden from directory listing. Grace only allows
                        // exact-path traversal (lookup/getattr) so the
                        // installer can reach whitelisted files.
                        if matches!(self.resolve_skill_read(&skill_name), ReadResolution::Hidden) {
                            reply.error(libc::ENOENT);
                            return;
                        }
                        let parent_ino = self.skills_dir_ino();
                        let mut entries: Vec<(u64, FileType, String)> = vec![
                            (ino, FileType::Directory, ".".to_string()),
                            (parent_ino, FileType::Directory, "..".to_string()),
                        ];

                        let md_path = format!("{}/SKILL.md", path);
                        let md_ino = self.inodes.readdir_ino(&md_path);
                        entries.push((md_ino, FileType::RegularFile, "SKILL.md".to_string()));

                        if skill_name != "skill-discover" {
                            // D1.1: fallback skills enumerate from the
                            // snapshot tree; current and no-resolver skills
                            // enumerate from the live source.
                            let phys_dir = self
                                .skill_read_dir(&skill_name)
                                .unwrap_or_else(|| self.skill_physical_dir(&skill_name));
                            if let Ok(dir_iter) = std::fs::read_dir(&phys_dir) {
                                for entry in dir_iter.flatten() {
                                    let name = entry.file_name().to_string_lossy().to_string();
                                    if name == "SKILL.md" {
                                        continue;
                                    }
                                    let kind = dir_entry_file_type(&entry);
                                    let entry_path = format!("{}/{}", path, name);
                                    let entry_ino = self.inodes.readdir_ino(&entry_path);
                                    entries.push((entry_ino, kind, name));
                                }
                            }
                        }

                        entries
                    }
                }
                PathType::Passthrough {
                    skill_name,
                    relative_path,
                } => {
                    // I2: staging roots use the physical source dir
                    // directly, bypassing the active resolver.
                    // Pending installs use the same bypass.
                    let skill_dir = if self.is_staging_skill_root(&skill_name)
                        || self.is_pending_install(&skill_name)
                    {
                        self.source_base().join(&skill_name)
                    } else {
                        match self.skill_read_dir(&skill_name) {
                            Some(d) => d,
                            None => {
                                reply.error(libc::ENOENT);
                                return;
                            }
                        }
                    };
                    let parent_ino = {
                        let parent_path = Path::new(&path)
                            .parent()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default();
                        self.inodes.lookup_by_path(&parent_path).unwrap_or(ino)
                    };
                    let phys_dir = skill_dir.join(&relative_path);
                    let mut entries: Vec<(u64, FileType, String)> = vec![
                        (ino, FileType::Directory, ".".to_string()),
                        (parent_ino, FileType::Directory, "..".to_string()),
                    ];
                    if let Ok(dir_iter) = std::fs::read_dir(&phys_dir) {
                        for entry in dir_iter.flatten() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            let kind = dir_entry_file_type(&entry);
                            let entry_path = format!("{}/{}", path, name);
                            let entry_ino = self.inodes.readdir_ino(&entry_path);
                            entries.push((entry_ino, kind, name));
                        }
                    }
                    entries
                }
                PathType::InboxDir => {
                    let parent_ino = FUSE_ROOT_ID;
                    let mut entries: Vec<(u64, FileType, String)> = vec![
                        (ino, FileType::Directory, ".".to_string()),
                        (parent_ino, FileType::Directory, "..".to_string()),
                    ];
                    if let Ok(dir_iter) = std::fs::read_dir(self.source_base()) {
                        for entry in dir_iter.flatten() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            if !Self::is_inbox_skill_name_allowed(&name) {
                                continue;
                            }
                            let meta = match entry.metadata() {
                                Ok(m) => m,
                                Err(_) => continue,
                            };
                            if !meta.is_dir() {
                                continue;
                            }
                            let entry_path = format!("/.skillfs-inbox/{}", name);
                            let entry_ino = self.inodes.readdir_ino(&entry_path);
                            entries.push((entry_ino, FileType::Directory, name));
                        }
                    }
                    entries
                }
                PathType::InboxSkillDir { skill_name } => {
                    if !Self::is_inbox_skill_name_allowed(&skill_name) {
                        reply.error(libc::ENOENT);
                        return;
                    }
                    let parent_ino = self
                        .inodes
                        .lookup_by_path("/.skillfs-inbox")
                        .unwrap_or(FUSE_ROOT_ID);
                    let phys_dir = self.inbox_skill_dir(&skill_name);
                    let mut entries: Vec<(u64, FileType, String)> = vec![
                        (ino, FileType::Directory, ".".to_string()),
                        (parent_ino, FileType::Directory, "..".to_string()),
                    ];
                    match std::fs::read_dir(&phys_dir) {
                        Ok(dir_iter) => {
                            for entry in dir_iter.flatten() {
                                let name = entry.file_name().to_string_lossy().to_string();
                                let kind = dir_entry_file_type(&entry);
                                let entry_path = format!("{}/{}", path, name);
                                let entry_ino = self.inodes.readdir_ino(&entry_path);
                                entries.push((entry_ino, kind, name));
                            }
                        }
                        Err(e) => {
                            reply.error(errno(&e));
                            return;
                        }
                    }
                    entries
                }
                PathType::InboxPassthrough {
                    skill_name,
                    relative_path,
                } => {
                    if !Self::is_inbox_skill_name_allowed(&skill_name) {
                        reply.error(libc::ENOENT);
                        return;
                    }
                    let phys_dir = self.inbox_skill_dir(&skill_name).join(&relative_path);
                    let parent_ino = {
                        let parent_path = Path::new(&path)
                            .parent()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default();
                        self.inodes.lookup_by_path(&parent_path).unwrap_or(ino)
                    };
                    let mut entries: Vec<(u64, FileType, String)> = vec![
                        (ino, FileType::Directory, ".".to_string()),
                        (parent_ino, FileType::Directory, "..".to_string()),
                    ];
                    match std::fs::read_dir(&phys_dir) {
                        Ok(dir_iter) => {
                            for entry in dir_iter.flatten() {
                                let name = entry.file_name().to_string_lossy().to_string();
                                let kind = dir_entry_file_type(&entry);
                                let entry_path = format!("{}/{}", path, name);
                                let entry_ino = self.inodes.readdir_ino(&entry_path);
                                entries.push((entry_ino, kind, name));
                            }
                        }
                        Err(e) => {
                            reply.error(errno(&e));
                            return;
                        }
                    }
                    entries
                }
                _ => {
                    reply.error(libc::ENOTDIR);
                    return;
                }
            };

        for (i, (entry_ino, kind, name)) in entries.iter().enumerate().skip(offset as usize) {
            if reply.add(*entry_ino, (i + 1) as i64, *kind, name.as_str()) {
                break;
            }
        }

        reply.ok();
    }
    pub(in crate::fs) fn opendir_impl(
        &mut self,
        _req: &Request,
        ino: u64,
        _flags: i32,
        reply: ReplyOpen,
    ) {
        let path = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => return reply.error(libc::ENOENT),
        };

        let path_type = parse_path(Path::new(&path), self.in_place);

        let (entries, dir_file) = match path_type {
            PathType::Root => {
                let skills_ino = self.inodes.lookup_by_path("/skills").unwrap_or_else(|| {
                    self.inodes
                        .allocate("/skills", FileType::Directory, FUSE_ROOT_ID)
                });
                let inbox_ino = self
                    .inodes
                    .lookup_by_path("/.skillfs-inbox")
                    .unwrap_or_else(|| {
                        self.inodes
                            .allocate("/.skillfs-inbox", FileType::Directory, FUSE_ROOT_ID)
                    });
                (
                    vec![
                        (FUSE_ROOT_ID, FileType::Directory, ".".to_string()),
                        (FUSE_ROOT_ID, FileType::Directory, "..".to_string()),
                        (skills_ino, FileType::Directory, "skills".to_string()),
                        (inbox_ino, FileType::Directory, INBOX_DIR_NAME.to_string()),
                    ],
                    None,
                )
            }
            PathType::SkillsDir => {
                let mut skill_names = self.primary_skill_names();
                // S3: lifecycle namespaces are hidden from ordinary
                // `/skills` listings; mirror the `readdir_dynamic` filter
                // so the snapshot taken at `opendir` cannot leak them
                // even if a placeholder lands in the store later.
                skill_names.retain(|n| !is_reserved_lifecycle_name(n));
                // I2: staging roots are hidden from the opendir snapshot.
                if let Some(ref matcher) = self.staging_matcher {
                    skill_names.retain(|n| !matcher.is_staging_root(n));
                }
                // Pending installs are hidden from opendir snapshot.
                if self.pending_install_controller.is_some() {
                    skill_names.retain(|n| !self.is_pending_install(n));
                }
                // D1.1: ledger-hidden skills are also dropped from the
                // opendir snapshot so readdir cannot leak them.
                // skill-discover is exempt — it is always virtual and
                // visible.
                if self.active_resolver.is_some() {
                    skill_names.retain(|n| {
                        n == "skill-discover"
                            || !matches!(self.resolve_skill_read(n), ReadResolution::Hidden)
                    });
                }
                let skills_dir_ino = if self.in_place {
                    FUSE_ROOT_ID
                } else {
                    self.inodes.lookup_by_path("/skills").unwrap_or_else(|| {
                        self.inodes
                            .allocate("/skills", FileType::Directory, FUSE_ROOT_ID)
                    })
                };

                let mut entries = vec![
                    (skills_dir_ino, FileType::Directory, ".".to_string()),
                    (FUSE_ROOT_ID, FileType::Directory, "..".to_string()),
                ];

                let mut sorted_names = skill_names;
                sorted_names.sort();

                for name in &sorted_names {
                    let skill_path = self.skill_inode_path(name);
                    let skill_ino = self.inodes.readdir_ino(&skill_path);
                    entries.push((skill_ino, FileType::Directory, name.clone()));
                }

                // Always include skill-discover
                if !sorted_names.iter().any(|n| n == "skill-discover") {
                    let discover_path = self.skill_inode_path("skill-discover");
                    let discover_ino = self.inodes.readdir_ino(&discover_path);
                    entries.push((
                        discover_ino,
                        FileType::Directory,
                        "skill-discover".to_string(),
                    ));
                }

                // L1: in in-place mode, the inbox lives next to the
                // listed skills under the FUSE root. Mirror the
                // dynamic-readdir branch here so opendir snapshots stay
                // consistent.
                if self.in_place {
                    let inbox_ino = self
                        .inodes
                        .lookup_by_path("/.skillfs-inbox")
                        .unwrap_or_else(|| {
                            self.inodes.allocate(
                                "/.skillfs-inbox",
                                FileType::Directory,
                                skills_dir_ino,
                            )
                        });
                    entries.push((inbox_ino, FileType::Directory, INBOX_DIR_NAME.to_string()));
                }

                (entries, None)
            }
            PathType::SkillDir { ref skill_name } => {
                // I2: staging roots bypass the active resolver and
                // enumerate physical entries directly.
                // Pending installs use the same bypass.
                if self.is_staging_skill_root(skill_name) || self.is_pending_install(skill_name) {
                    let skills_dir_ino = self.skills_dir_ino();
                    let mut entries = vec![
                        (ino, FileType::Directory, ".".to_string()),
                        (skills_dir_ino, FileType::Directory, "..".to_string()),
                    ];
                    let phys_dir = self.source_base().join(skill_name);
                    if let Ok(dir_iter) = std::fs::read_dir(&phys_dir) {
                        let mut phys_entries: Vec<_> = dir_iter.flatten().collect();
                        phys_entries.sort_by_key(|e| e.file_name());
                        for entry in phys_entries {
                            let name = entry.file_name().to_string_lossy().to_string();
                            let kind = dir_entry_file_type(&entry);
                            let entry_path = format!("{}/{}", path, name);
                            let entry_ino = self.inodes.readdir_ino(&entry_path);
                            entries.push((entry_ino, kind, name));
                        }
                    }
                    let dir_file = std::fs::File::open(&phys_dir).ok();
                    (entries, dir_file)
                } else {
                    // D1.1: hidden skills should not even open as a
                    // directory. Defense in depth — lookup already returns
                    // ENOENT, but a stale ino must not leak a listing.
                    // I4: grace does NOT bypass opendir — same rationale
                    // as readdir above.
                    if matches!(self.resolve_skill_read(skill_name), ReadResolution::Hidden) {
                        return reply.error(libc::ENOENT);
                    }
                    let skills_dir_ino = self.skills_dir_ino();
                    let mut entries = vec![
                        (ino, FileType::Directory, ".".to_string()),
                        (skills_dir_ino, FileType::Directory, "..".to_string()),
                    ];

                    // Virtual SKILL.md always present
                    let md_path = format!("{}/SKILL.md", path);
                    let md_ino = self.inodes.readdir_ino(&md_path);
                    entries.push((md_ino, FileType::RegularFile, "SKILL.md".to_string()));

                    // Physical files (non skill-discover). For ledger
                    // fallback skills the snapshot directory drives the
                    // listing instead of the live source — the demo
                    // promise is that `/skills/<skill>` is read-served from
                    // the trusted snapshot tree, files and all.
                    let dir_file = if !is_skill_discover_path(skill_name) {
                        // `skill_read_dir` returns the snapshot dir for
                        // fallback and the live dir otherwise. Hidden is
                        // handled by the early-return above so the
                        // `unwrap_or` is just defensive.
                        let phys_dir = self
                            .skill_read_dir(skill_name)
                            .unwrap_or_else(|| self.skill_physical_dir(skill_name));
                        if let Ok(dir_iter) = std::fs::read_dir(&phys_dir) {
                            let mut phys_entries: Vec<_> = dir_iter.flatten().collect();
                            phys_entries.sort_by_key(|e| e.file_name());

                            for entry in phys_entries {
                                let name = entry.file_name().to_string_lossy().to_string();
                                if name == "SKILL.md" {
                                    continue;
                                }
                                let kind = dir_entry_file_type(&entry);
                                let entry_path = format!("{}/{}", path, name);
                                let entry_ino = self.inodes.readdir_ino(&entry_path);
                                entries.push((entry_ino, kind, name));
                            }
                        }
                        std::fs::File::open(&phys_dir).ok()
                    } else {
                        None
                    };

                    (entries, dir_file)
                }
            }
            PathType::Passthrough {
                ref skill_name,
                ref relative_path,
            } => {
                // I2: staging roots use the physical source dir
                // directly, bypassing the active resolver.
                // Pending installs use the same bypass.
                // D1.1: hidden / snapshot resolution. Hidden surfaces
                // ENOENT; snapshot drives the listing from the trusted
                // snapshot tree so subdirectory enumerations stay
                // consistent with what lookup/getattr/read reports.
                let skill_dir = if self.is_staging_skill_root(skill_name)
                    || self.is_pending_install(skill_name)
                {
                    self.source_base().join(skill_name)
                } else {
                    match self.skill_read_dir(skill_name) {
                        Some(d) => d,
                        None => return reply.error(libc::ENOENT),
                    }
                };
                let parent_ino = {
                    let parent_path = Path::new(&path)
                        .parent()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default();
                    self.inodes.lookup_by_path(&parent_path).unwrap_or(ino)
                };

                let mut entries = vec![
                    (ino, FileType::Directory, ".".to_string()),
                    (parent_ino, FileType::Directory, "..".to_string()),
                ];

                let phys_dir = skill_dir.join(relative_path);
                if let Ok(dir_iter) = std::fs::read_dir(&phys_dir) {
                    let mut phys_entries: Vec<_> = dir_iter.flatten().collect();
                    phys_entries.sort_by_key(|e| e.file_name());

                    for entry in phys_entries {
                        let name = entry.file_name().to_string_lossy().to_string();
                        let kind = dir_entry_file_type(&entry);
                        let entry_path = format!("{}/{}", path, name);
                        let entry_ino = self.inodes.readdir_ino(&entry_path);
                        entries.push((entry_ino, kind, name));
                    }
                }
                let dir_file = std::fs::File::open(&phys_dir).ok();

                (entries, dir_file)
            }
            PathType::InboxDir => {
                let mut entries = vec![
                    (ino, FileType::Directory, ".".to_string()),
                    (FUSE_ROOT_ID, FileType::Directory, "..".to_string()),
                ];
                if let Ok(dir_iter) = std::fs::read_dir(self.source_base()) {
                    let mut phys_entries: Vec<_> = dir_iter.flatten().collect();
                    phys_entries.sort_by_key(|e| e.file_name());
                    for entry in phys_entries {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if !Self::is_inbox_skill_name_allowed(&name) {
                            continue;
                        }
                        let meta = match entry.metadata() {
                            Ok(m) => m,
                            Err(_) => continue,
                        };
                        if !meta.is_dir() {
                            continue;
                        }
                        let entry_path = format!("/.skillfs-inbox/{}", name);
                        let entry_ino = self.inodes.readdir_ino(&entry_path);
                        entries.push((entry_ino, FileType::Directory, name));
                    }
                }
                (entries, None)
            }
            PathType::InboxSkillDir { ref skill_name } => {
                if !Self::is_inbox_skill_name_allowed(skill_name) {
                    return reply.error(libc::ENOENT);
                }
                let parent_ino = self
                    .inodes
                    .lookup_by_path("/.skillfs-inbox")
                    .unwrap_or(FUSE_ROOT_ID);
                let phys_dir = self.inbox_skill_dir(skill_name);
                let mut entries = vec![
                    (ino, FileType::Directory, ".".to_string()),
                    (parent_ino, FileType::Directory, "..".to_string()),
                ];
                let dir_file = match std::fs::read_dir(&phys_dir) {
                    Ok(dir_iter) => {
                        let mut phys_entries: Vec<_> = dir_iter.flatten().collect();
                        phys_entries.sort_by_key(|e| e.file_name());
                        for entry in phys_entries {
                            let name = entry.file_name().to_string_lossy().to_string();
                            let kind = dir_entry_file_type(&entry);
                            let entry_path = format!("{}/{}", path, name);
                            let entry_ino = self.inodes.readdir_ino(&entry_path);
                            entries.push((entry_ino, kind, name));
                        }
                        std::fs::File::open(&phys_dir).ok()
                    }
                    Err(e) => return reply.error(errno(&e)),
                };
                (entries, dir_file)
            }
            PathType::InboxPassthrough {
                ref skill_name,
                ref relative_path,
            } => {
                if !Self::is_inbox_skill_name_allowed(skill_name) {
                    return reply.error(libc::ENOENT);
                }
                let parent_ino = {
                    let parent_path = Path::new(&path)
                        .parent()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default();
                    self.inodes.lookup_by_path(&parent_path).unwrap_or(ino)
                };
                let phys_dir = self.inbox_skill_dir(skill_name).join(relative_path);
                let mut entries = vec![
                    (ino, FileType::Directory, ".".to_string()),
                    (parent_ino, FileType::Directory, "..".to_string()),
                ];
                let dir_file = match std::fs::read_dir(&phys_dir) {
                    Ok(dir_iter) => {
                        let mut phys_entries: Vec<_> = dir_iter.flatten().collect();
                        phys_entries.sort_by_key(|e| e.file_name());
                        for entry in phys_entries {
                            let name = entry.file_name().to_string_lossy().to_string();
                            let kind = dir_entry_file_type(&entry);
                            let entry_path = format!("{}/{}", path, name);
                            let entry_ino = self.inodes.readdir_ino(&entry_path);
                            entries.push((entry_ino, kind, name));
                        }
                        std::fs::File::open(&phys_dir).ok()
                    }
                    Err(e) => return reply.error(errno(&e)),
                };
                (entries, dir_file)
            }
            _ => {
                // SkillMd, Invalid — not a directory
                return reply.error(libc::ENOTDIR);
            }
        };

        let fh = self.handles.allocate_dir(ino, entries, dir_file);
        reply.opened(fh, 0);
    }
    pub(in crate::fs) fn readdir_impl(
        &mut self,
        _req: &Request,
        ino: u64,
        fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        debug!(ino, fh, offset, "readdir");

        // Use snapshot from opendir if available
        if let Some(entries) = self.handles.get_dir_entries(fh) {
            for (i, (entry_ino, kind, name)) in entries.iter().enumerate().skip(offset as usize) {
                if reply.add(*entry_ino, (i + 1) as i64, *kind, name.as_str()) {
                    break;
                }
            }
            reply.ok();
            return;
        }

        // Fallback: dynamic listing (for compatibility when opendir was not called)
        warn!(
            ino,
            fh, "readdir: no directory handle found, falling back to dynamic listing"
        );
        self.readdir_dynamic(ino, offset, reply);
    }
    pub(in crate::fs) fn releasedir_impl(
        &mut self,
        _req: &Request,
        ino: u64,
        fh: u64,
        _flags: i32,
        reply: ReplyEmpty,
    ) {
        debug!(ino, fh, "releasedir");
        self.handles.remove_dir(fh);
        reply.ok();
    }
    pub(in crate::fs) fn fsyncdir_impl(
        &mut self,
        _req: &Request,
        ino: u64,
        fh: u64,
        datasync: bool,
        reply: ReplyEmpty,
    ) {
        // Prefer using the directory handle's physical fd
        if let Some(result) = self.handles.sync_dir(fh, datasync) {
            match result {
                Ok(()) => reply.ok(),
                Err(e) => reply.error(errno(&e)),
            }
            return;
        }

        // Fallback: no directory handle found, use ino-based path resolution
        let path = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => return reply.error(libc::ENOENT),
        };

        let path_type = parse_path(Path::new(&path), self.in_place);

        match path_type {
            PathType::Root | PathType::SkillsDir | PathType::InboxDir => {
                reply.ok();
            }
            PathType::SkillDir { ref skill_name } | PathType::InboxSkillDir { ref skill_name } => {
                if is_skill_discover_path(skill_name) {
                    reply.ok();
                } else {
                    let dir_path = self.source_base().join(skill_name);
                    match std::fs::metadata(&dir_path) {
                        Ok(m) if m.is_dir() => match std::fs::File::open(&dir_path) {
                            Ok(dir_file) => {
                                let result = if datasync {
                                    dir_file.sync_data()
                                } else {
                                    dir_file.sync_all()
                                };
                                match result {
                                    Ok(()) => reply.ok(),
                                    Err(e) => reply.error(errno(&e)),
                                }
                            }
                            Err(e) => reply.error(errno(&e)),
                        },
                        Ok(_) => reply.error(libc::ENOTDIR),
                        Err(e) => reply.error(errno(&e)),
                    }
                }
            }
            PathType::Passthrough {
                ref skill_name,
                ref relative_path,
            }
            | PathType::InboxPassthrough {
                ref skill_name,
                ref relative_path,
            } => {
                let dir_path = self.source_base().join(skill_name).join(relative_path);
                match std::fs::metadata(&dir_path) {
                    Ok(m) if m.is_dir() => match std::fs::File::open(&dir_path) {
                        Ok(dir_file) => {
                            let result = if datasync {
                                dir_file.sync_data()
                            } else {
                                dir_file.sync_all()
                            };
                            match result {
                                Ok(()) => reply.ok(),
                                Err(e) => reply.error(errno(&e)),
                            }
                        }
                        Err(e) => reply.error(errno(&e)),
                    },
                    Ok(_) => reply.error(libc::ENOTDIR),
                    Err(e) => reply.error(errno(&e)),
                }
            }
            PathType::SkillMd { .. } => {
                reply.error(libc::ENOTDIR);
            }
            PathType::Invalid => {
                reply.error(libc::ENOENT);
            }
        }
    }
}
