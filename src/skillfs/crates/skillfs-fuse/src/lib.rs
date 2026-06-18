//! FUSE virtual filesystem layer for SkillFS.
//!
//! Exposes skills as a virtual filesystem. The default view (from
//! \`skillfs-views.toml\`) is shown directly under \`/skills/\`. Secondary
//! views are accessible via the always-visible \`skill-discover\` virtual
//! skill, which lists their real source paths so the AI can open them
//! directly.

use std::collections::HashMap;
use std::io::{Seek, SeekFrom, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuser::{
    FUSE_ROOT_ID, FileAttr, FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyOpen, Request,
};
use parking_lot::RwLock;
use skillfs_core::{
    SharedSkillStore, compiler, env::EnvironmentProfile, parser, views::ViewsConfig,
};
use thiserror::Error;
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Error Types
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum FuseError {
    #[error("mount failed: {0}")]
    MountFailed(String),
    #[error("unmount failed: {0}")]
    UnmountFailed(String),
    #[error("invalid mount point: {0}")]
    InvalidMountPoint(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("io error: {0}")]
    IoError(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// Mount Options
// ---------------------------------------------------------------------------

/// Mount options for the FUSE filesystem.
#[derive(Debug, Clone)]
pub struct MountOptions {
    /// Allow other users to access the mount (requires allow_other in fuse.conf)
    pub allow_other: bool,
    /// Run in foreground (don't daemonize)
    pub foreground: bool,
    /// Additional FUSE mount options
    pub fuse_options: Vec<String>,
}

impl Default for MountOptions {
    fn default() -> Self {
        Self {
            allow_other: false,
            foreground: false,
            fuse_options: vec!["noatime".to_string()],
        }
    }
}

// ---------------------------------------------------------------------------
// Mount Handle
// ---------------------------------------------------------------------------

/// Handle to a mounted FUSE filesystem.
pub struct MountHandle {
    /// The mount point path
    pub mountpoint: PathBuf,
    /// Background session (if mounted in background)
    session: Option<std::thread::JoinHandle<()>>,
}

impl MountHandle {
    /// Unmount the filesystem.
    pub fn unmount(self) -> Result<(), FuseError> {
        info!(mountpoint = %self.mountpoint.display(), "unmounting filesystem");

        if let Some(session) = self.session {
            drop(session);
        }

        #[cfg(target_os = "linux")]
        {
            let output = std::process::Command::new("fusermount3")
                .args(["-u", &self.mountpoint.to_string_lossy()])
                .output();

            match output {
                Ok(output) if output.status.success() => {
                    info!("unmount successful");
                    Ok(())
                }
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Err(FuseError::UnmountFailed(stderr.to_string()))
                }
                Err(e) => Err(FuseError::IoError(e)),
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            Ok(())
        }
    }

    /// Check if the mount is still active.
    pub fn is_mounted(&self) -> bool {
        std::fs::metadata(&self.mountpoint).is_ok()
    }
}

// ---------------------------------------------------------------------------
// Path Type
// ---------------------------------------------------------------------------

/// Types of paths in the SkillFS filesystem.
#[derive(Debug, Clone, PartialEq)]
enum PathType {
    /// Root directory (/)
    Root,
    /// Skills directory (/skills)
    SkillsDir,
    /// Skill directory (/skills/{skill_name})
    SkillDir { skill_name: String },
    /// SKILL.md file (/skills/{skill_name}/SKILL.md)
    SkillMd { skill_name: String },
    /// Passthrough file/directory (/skills/{skill_name}/{subdir}/...)
    Passthrough {
        skill_name: String,
        relative_path: PathBuf,
    },
    /// Unknown/invalid path
    Invalid,
}

/// Parse a path into its type.
///
/// When `in_place` is true the FUSE root IS the skills directory, so
/// paths have no `/skills/` prefix: `/{skill}`, `/{skill}/SKILL.md`, etc.
fn parse_path(path: &Path, in_place: bool) -> PathType {
    let components: Vec<_> = path.components().collect();

    if in_place {
        // In-place mode: root == skills dir, no /skills/ prefix.
        match components.as_slice() {
            [] => PathType::SkillsDir,
            [root] if root.as_os_str() == "/" => PathType::SkillsDir,
            [_, skill_name] => PathType::SkillDir {
                skill_name: skill_name.as_os_str().to_string_lossy().to_string(),
            },
            [_, skill_name, file] => {
                let skill_name = skill_name.as_os_str().to_string_lossy().to_string();
                let file_name = file.as_os_str().to_string_lossy();
                if file_name == "SKILL.md" {
                    PathType::SkillMd { skill_name }
                } else {
                    PathType::Passthrough {
                        skill_name,
                        relative_path: PathBuf::from(file.as_os_str()),
                    }
                }
            }
            [_, skill_name, rest @ ..] => {
                let skill_name = skill_name.as_os_str().to_string_lossy().to_string();
                let relative_path: PathBuf = rest.iter().map(|c| c.as_os_str()).collect();
                PathType::Passthrough {
                    skill_name,
                    relative_path,
                }
            }
            _ => PathType::Invalid,
        }
    } else {
        // Normal mode: skills live under /skills/
        match components.as_slice() {
            [] => PathType::Root,
            [root] if root.as_os_str() == "/" => PathType::Root,
            [_, skills] if skills.as_os_str() == "skills" => PathType::SkillsDir,
            [_, skills, skill_name] if skills.as_os_str() == "skills" => PathType::SkillDir {
                skill_name: skill_name.as_os_str().to_string_lossy().to_string(),
            },
            [_, skills, skill_name, file] if skills.as_os_str() == "skills" => {
                let skill_name = skill_name.as_os_str().to_string_lossy().to_string();
                let file_name = file.as_os_str().to_string_lossy();
                if file_name == "SKILL.md" {
                    PathType::SkillMd { skill_name }
                } else {
                    PathType::Passthrough {
                        skill_name,
                        relative_path: PathBuf::from(file.as_os_str()),
                    }
                }
            }
            [_, skills, skill_name, rest @ ..] if skills.as_os_str() == "skills" => {
                let skill_name = skill_name.as_os_str().to_string_lossy().to_string();
                let relative_path: PathBuf = rest.iter().map(|c| c.as_os_str()).collect();
                PathType::Passthrough {
                    skill_name,
                    relative_path,
                }
            }
            _ => PathType::Invalid,
        }
    }
}

// ---------------------------------------------------------------------------
// Inode Manager
// ---------------------------------------------------------------------------

/// Manages inode-to-path mappings for the FUSE filesystem.
struct InodeManager {
    next_ino: AtomicU64,
    inodes: RwLock<HashMap<u64, InodeEntry>>,
    paths: RwLock<HashMap<String, u64>>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct InodeEntry {
    ino: u64,
    path: String,
    kind: FileType,
    parent: u64,
}

impl InodeManager {
    fn new() -> Self {
        let mut inodes = HashMap::new();
        let mut paths = HashMap::new();

        inodes.insert(
            FUSE_ROOT_ID,
            InodeEntry {
                ino: FUSE_ROOT_ID,
                path: "/".to_string(),
                kind: FileType::Directory,
                parent: FUSE_ROOT_ID,
            },
        );
        paths.insert("/".to_string(), FUSE_ROOT_ID);

        Self {
            next_ino: AtomicU64::new(2),
            inodes: RwLock::new(inodes),
            paths: RwLock::new(paths),
        }
    }

    fn allocate(&self, path: &str, kind: FileType, parent: u64) -> u64 {
        let mut paths = self.paths.write();
        if let Some(&ino) = paths.get(path) {
            return ino;
        }
        let ino = self.next_ino.fetch_add(1, Ordering::SeqCst);
        let entry = InodeEntry {
            ino,
            path: path.to_string(),
            kind,
            parent,
        };
        self.inodes.write().insert(ino, entry);
        paths.insert(path.to_string(), ino);
        ino
    }

    fn get(&self, ino: u64) -> Option<InodeEntry> {
        self.inodes.read().get(&ino).cloned()
    }

    fn lookup_by_path(&self, path: &str) -> Option<u64> {
        self.paths.read().get(path).copied()
    }

    fn get_path(&self, ino: u64) -> Option<String> {
        self.inodes.read().get(&ino).map(|e| e.path.clone())
    }

    #[allow(dead_code)]
    fn remove(&self, ino: u64) {
        if let Some(entry) = self.inodes.write().remove(&ino) {
            self.paths.write().remove(&entry.path);
        }
    }

    /// Remove an inode and all children whose path starts with `path_prefix/`.
    fn remove_recursive(&self, path_prefix: &str) {
        let mut inodes = self.inodes.write();
        let mut paths = self.paths.write();
        let to_remove: Vec<u64> = inodes
            .iter()
            .filter(|(_, e)| {
                e.path == path_prefix || e.path.starts_with(&format!("{}/", path_prefix))
            })
            .map(|(&ino, _)| ino)
            .collect();
        for ino in to_remove {
            if let Some(entry) = inodes.remove(&ino) {
                paths.remove(&entry.path);
            }
        }
    }

    /// Rename an inode's path and all children paths that start with old_path.
    fn rename_path(&self, old_path: &str, new_path: &str) {
        let mut inodes = self.inodes.write();
        let mut paths = self.paths.write();
        let to_rename: Vec<(u64, String)> = inodes
            .iter()
            .filter(|(_, e)| e.path == old_path || e.path.starts_with(&format!("{}/", old_path)))
            .map(|(&ino, e)| (ino, e.path.clone()))
            .collect();
        for (ino, old) in to_rename {
            let new = old.replacen(old_path, new_path, 1);
            paths.remove(&old);
            paths.insert(new.clone(), ino);
            if let Some(entry) = inodes.get_mut(&ino) {
                entry.path = new;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Store Sync
// ---------------------------------------------------------------------------

/// Events sent from FUSE write callbacks to the background sync task.
#[derive(Debug)]
enum SyncEvent {
    /// Re-parse a skill's SKILL.md after write/create.
    Reparse { skill_name: String },
}

/// Spawn the background store-sync worker thread.
///
/// Collects events from the FUSE write path, batches them with a 50 ms
/// debounce window, then re-parses the affected SKILL.md files and updates
/// the shared store.
fn spawn_sync_worker(
    rx: std::sync::mpsc::Receiver<SyncEvent>,
    store: SharedSkillStore,
    source_base: PathBuf,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        // Block until the first event arrives; loop ends when the channel closes.
        while let Ok(first) = rx.recv() {
            // Collect more events within a 50 ms window (debounce).
            let mut pending: HashMap<String, SyncEvent> = HashMap::new();
            match &first {
                SyncEvent::Reparse { skill_name } => {
                    pending.insert(skill_name.clone(), first);
                }
            }
            while let Ok(ev) = rx.recv_timeout(std::time::Duration::from_millis(50)) {
                match &ev {
                    SyncEvent::Reparse { skill_name } => {
                        pending.insert(skill_name.clone(), ev);
                    }
                }
            }

            // Process the batch.
            for (_skill_name, event) in pending {
                match event {
                    SyncEvent::Reparse { ref skill_name } => {
                        let md_path = source_base.join(skill_name).join("SKILL.md");
                        match parser::parse_skill_file(&md_path) {
                            Ok(mut entry) => {
                                // The directory name is the authoritative store key.
                                // Override metadata.name so that a stale frontmatter
                                // `name:` field (e.g. after a rename) can never
                                // re-insert an entry under the old name.
                                entry.metadata.name = skill_name.clone();
                                info!(
                                    name = %skill_name,
                                    "sync: re-parsed SKILL.md"
                                );
                                store.write().upsert(entry);
                            }
                            Err(e) => {
                                warn!(
                                    name = %skill_name,
                                    error = %e,
                                    "sync: re-parse failed"
                                );
                            }
                        }
                    }
                }
            }
        }
        info!("sync worker exiting");
    })
}

// ---------------------------------------------------------------------------
// Filesystem Implementation
// ---------------------------------------------------------------------------

/// SkillFS FUSE filesystem implementation.
pub struct SkillFs {
    #[allow(dead_code)]
    mountpoint: PathBuf,
    /// Physical source directory (where skillfs-views.toml lives).
    source: PathBuf,
    store: SharedSkillStore,
    next_fh: RwLock<u64>,
    inodes: InodeManager,
    /// Runtime environment for SKILL.md conditional compilation.
    env_profile: EnvironmentProfile,
    /// View configuration loaded from skillfs-views.toml (if present).
    views_config: Option<ViewsConfig>,
    /// Pre-opened fd to source dir (in-place mode). Bypasses the FUSE mount
    /// layer so file reads still reach the real inode after over-mounting.
    source_dirfd: Option<std::fs::File>,
    /// Whether we are mounted in-place (source == mountpoint).
    in_place: bool,
    /// Channel to send sync events to the background sync worker.
    sync_tx: Option<std::sync::mpsc::Sender<SyncEvent>>,
}

impl SkillFs {
    /// Create a new SkillFS filesystem.
    ///
    /// `in_place`: the FUSE mount will be placed on `source` itself, so all
    /// physical reads must go through the pre-opened fd (`/proc/self/fd/{n}`)
    /// to bypass the FUSE layer.
    pub fn new(
        mountpoint: PathBuf,
        source: PathBuf,
        store: SharedSkillStore,
        in_place: bool,
    ) -> Self {
        let env_profile = EnvironmentProfile::detect();
        // Load views config from the source directory if present.
        let views_config = ViewsConfig::load(&source);
        if views_config.is_some() {
            info!("loaded skillfs-views.toml from {}", source.display());
        }

        // In in-place mode open the source dir before the mount so we hold an
        // fd that still points at the underlying directory after over-mounting.
        let source_dirfd = if in_place {
            match std::fs::File::open(&source) {
                Ok(f) => {
                    info!(
                        "opened source dirfd for in-place mount: {}",
                        source.display()
                    );
                    Some(f)
                }
                Err(e) => {
                    warn!("failed to open source dirfd ({}): {}", source.display(), e);
                    None
                }
            }
        } else {
            None
        };

        // Compute source_base for the sync worker before moving fields.
        let sync_source_base = if let Some(ref fd) = source_dirfd {
            use std::os::unix::io::AsRawFd;
            PathBuf::from(format!("/proc/self/fd/{}", fd.as_raw_fd()))
        } else {
            source.clone()
        };

        // Spawn the background sync worker.
        let (sync_tx, sync_rx) = std::sync::mpsc::channel();
        let sync_store = store.clone();
        spawn_sync_worker(sync_rx, sync_store, sync_source_base);

        let fs = Self {
            mountpoint,
            source,
            store,
            next_fh: RwLock::new(1),
            inodes: InodeManager::new(),
            env_profile,
            views_config,
            source_dirfd,
            in_place,
            sync_tx: Some(sync_tx),
        };

        // In normal mode pre-populate the /skills inode.
        // In in-place mode the root IS the skills dir — no sub-inode needed.
        if !in_place {
            fs.inodes
                .allocate("/skills", FileType::Directory, FUSE_ROOT_ID);
        }

        fs
    }

    /// Return the base path for physical file access.
    ///
    /// In in-place mode returns `/proc/self/fd/{n}` (the pre-opened dirfd)
    /// so that reads bypass the FUSE mount layer.  Otherwise returns the
    /// plain source directory path.
    fn source_base(&self) -> PathBuf {
        if let Some(fd) = &self.source_dirfd {
            use std::os::unix::io::AsRawFd;
            PathBuf::from(format!("/proc/self/fd/{}", fd.as_raw_fd()))
        } else {
            self.source.clone()
        }
    }

    /// FUSE inode path prefix for a skill dir.
    ///
    /// In normal mode → `/skills/{name}`; in in-place mode → `/{name}`.
    fn skill_inode_path(&self, skill_name: &str) -> String {
        if self.in_place {
            format!("/{}", skill_name)
        } else {
            format!("/skills/{}", skill_name)
        }
    }

    /// Inode for the skills directory (the parent of individual skill dirs).
    fn skills_dir_ino(&self) -> u64 {
        if self.in_place {
            FUSE_ROOT_ID
        } else {
            self.inodes.lookup_by_path("/skills").unwrap_or(2)
        }
    }

    /// Generate SKILL.md content for the virtual `skill-discover` skill.
    ///
    /// When views are configured, the body lists every secondary view as a
    /// section with a table of `name | description | source_path` rows.
    /// The `source_path` is the real physical path to each skill's SKILL.md,
    /// enabling the AI to open it directly via `read_file`.
    ///
    /// When no views config is present, falls back to a simple listing of all
    /// skills in the store.
    fn get_skill_discover_content(&self) -> String {
        let store = self.store.read();

        // ── Case 1: views config present ─────────────────────────────────
        if let Some(cfg) = &self.views_config {
            let secondary_views = cfg.secondary_views();
            if secondary_views.is_empty() {
                return self.simple_discover_md(&store);
            }

            // Collect all skill names in secondary views (for frontmatter description).
            let hidden_names: Vec<&str> = secondary_views
                .iter()
                .flat_map(|v| v.skills.iter().map(|s| s.as_str()))
                .filter(|name| store.get(name).is_some())
                .collect();

            // Collect all source paths to find a common prefix.
            let all_paths: Vec<std::path::PathBuf> = hidden_names
                .iter()
                .filter_map(|name| store.get(name).map(|e| e.source_path.clone()))
                .collect();
            let common_prefix = find_common_path_prefix(&all_paths);

            let frontmatter = format!(
                "---\nname: skill-discover\ndescription: 'Hidden skills: {}'\nversion: 0.1.0\ntags: [meta, discovery]\nenabled: true\n---\n",
                hidden_names.join(", ")
            );

            let mut body = String::from("\n# Secondary Skill Views\n\n");

            // Show base path hint once so individual paths stay short.
            if let Some(ref prefix) = common_prefix {
                body.push_str(&format!(
                    "Base path: `{}`\n\nPaths below are relative to the base path. \
Use `read_file` on any `source_path` to read the skill and learn how to use it.\n\n",
                    prefix.display()
                ));
            } else {
                body.push_str("Use `read_file` on any `source_path` to read the skill and learn how to use it.\n\n");
            }

            for view in &secondary_views {
                body.push_str(&format!("## {}\n", view.name));
                if !view.description.is_empty() {
                    body.push_str(&format!("{}\n\n", view.description));
                } else {
                    body.push('\n');
                }
                body.push_str("| name | description | source_path |\n");
                body.push_str("|------|-------------|-------------|\n");

                for skill_name in &view.skills {
                    if let Some(entry) = store.get(skill_name.as_str()) {
                        let desc = entry
                            .metadata
                            .description
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .replace('|', r"\|");
                        let display_path = match &common_prefix {
                            Some(prefix) => entry
                                .source_path
                                .strip_prefix(prefix)
                                .map(|p| p.display().to_string())
                                .unwrap_or_else(|_| entry.source_path.display().to_string()),
                            None => entry.source_path.display().to_string(),
                        };
                        body.push_str(&format!(
                            "| {} | {} | {} |\n",
                            skill_name, desc, display_path
                        ));
                    }
                }
                body.push('\n');
            }

            return format!("{}{}", frontmatter, body);
        }

        // ── Case 2: no views config — simple listing ──────────────────────
        self.simple_discover_md(&store)
    }

    /// Fallback skill-discover content when no views config is present.
    fn simple_discover_md(&self, store: &skillfs_core::store::SkillStore) -> String {
        let mut body = String::from(
            "| name | description |
|------|-------------|
",
        );
        let mut names: Vec<&str> = store.list();
        names.sort_unstable();
        for name in names {
            if let Some(entry) = store.get(name) {
                let desc = entry
                    .metadata
                    .description
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .replace('|', r"\|");
                body.push_str(&format!("| {} | {} |\n", name, desc));
            }
        }
        format!(
            "---
name: skill-discover
description: Lists all available skills.
version: 0.1.0
tags: [meta, discovery]
enabled: true
---

# Available Skills

{}
",
            body
        )
    }

    /// Resolve the physical directory containing a skill's files.
    ///
    /// In in-place mode uses `source_base()` (the pre-opened fd path) so
    /// reads bypass the FUSE mount layer.
    fn skill_physical_dir(&self, skill_name: &str) -> PathBuf {
        if self.in_place {
            // Always go through the fd to bypass the FUSE mount.
            self.source_base().join(skill_name)
        } else {
            self.skill_source_path(skill_name)
                .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                .unwrap_or_else(|| self.source.join(skill_name))
        }
    }

    /// Resolve the physical SKILL.md path for a skill via the store.
    fn skill_source_path(&self, skill_name: &str) -> Option<PathBuf> {
        let store = self.store.read();
        store.get(skill_name).map(|e| e.source_path.clone())
    }

    /// Read and compile a skill's SKILL.md content.
    ///
    /// In in-place mode reads via `/proc/self/fd/{n}` to bypass FUSE.
    fn compiled_skill_md(&self, skill_name: &str) -> Option<String> {
        if skill_name == "skill-discover" {
            return Some(self.get_skill_discover_content());
        }
        let physical_path = if self.in_place {
            // Bypass the FUSE layer via the pre-opened fd.
            self.source_base().join(skill_name).join("SKILL.md")
        } else {
            self.skill_source_path(skill_name)?
        };
        let raw = std::fs::read_to_string(&physical_path).ok()?;
        Some(compiler::compile(&raw, &self.env_profile))
    }

    /// Return the list of skill names to show in /skills (default view).
    ///
    /// If views config is present, returns the default view's skills
    /// (filtered to those actually in the store). Otherwise returns all skills.
    fn primary_skill_names(&self) -> Vec<String> {
        if let Some(cfg) = &self.views_config {
            let primary = cfg.default_skills();
            let store = self.store.read();
            let (primary, _) = store.split_primary(Some(&primary));
            primary
        } else {
            let store = self.store.read();
            store.list().iter().map(|s| s.to_string()).collect()
        }
    }

    fn allocate_fh(&self) -> u64 {
        let mut fh = self.next_fh.write();
        let result = *fh;
        *fh += 1;
        result
    }

    fn virtual_file_attr(&self, size: u64) -> FileAttr {
        let now = SystemTime::now();
        FileAttr {
            ino: 0,
            size,
            blocks: size.div_ceil(512),
            atime: now,
            mtime: now,
            ctime: now,
            crtime: now,
            kind: FileType::RegularFile,
            perm: 0o444,
            nlink: 1,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            flags: 0,
            blksize: 512,
        }
    }

    fn dir_attr(&self) -> FileAttr {
        let now = SystemTime::now();
        FileAttr {
            ino: 0,
            size: 0,
            blocks: 0,
            atime: now,
            mtime: now,
            ctime: now,
            crtime: now,
            kind: FileType::Directory,
            perm: 0o755,
            nlink: 2,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            flags: 0,
            blksize: 512,
        }
    }

    #[allow(dead_code)]
    fn skill_physical_path(&self, skill_name: &str) -> Option<PathBuf> {
        let store = self.store.read();
        let entry = store.get(skill_name)?;
        Some(entry.source_path.parent()?.to_path_buf())
    }

    /// Emit a WARN log when a write operation is rejected on the read-only mount.
    fn ro_warn(&self, op: &str, path_hint: &str) {
        let mountpoint = self.mountpoint.display().to_string();
        warn!(
            op,
            path = path_hint,
            mountpoint,
            "SkillFS is read-only while mounted — write op rejected. \
             To install or modify skills, unmount first:\n  \
             fusermount3 -u '{mountpoint}'\n  \
             or press Ctrl-C / send SIGTERM to the skillfs process."
        );
    }

    /// Build the canonical FUSE path from a parent inode and child name.
    fn build_fuse_path(&self, parent: u64, name: &std::ffi::OsStr) -> Option<String> {
        let parent_path = self.inodes.get_path(parent)?;
        let name_str = name.to_string_lossy();
        if parent_path == "/" {
            Some(format!("/{}", name_str))
        } else {
            Some(format!("{}/{}", parent_path, name_str))
        }
    }

    /// Resolve a FUSE virtual path to the underlying physical path.
    ///
    /// Uses `source_base()` (which goes through `/proc/self/fd/{n}` in
    /// in-place mode) so that all I/O bypasses the FUSE layer.
    fn resolve_physical_path(&self, fuse_path: &str) -> Option<PathBuf> {
        match parse_path(Path::new(fuse_path), self.in_place) {
            PathType::SkillDir { skill_name } => Some(self.source_base().join(&skill_name)),
            PathType::SkillMd { skill_name } => {
                Some(self.source_base().join(&skill_name).join("SKILL.md"))
            }
            PathType::Passthrough {
                skill_name,
                relative_path,
            } => Some(self.source_base().join(&skill_name).join(&relative_path)),
            _ => None,
        }
    }

    /// Send a sync event to the background worker (non-blocking).
    fn send_sync(&self, event: SyncEvent) {
        if let Some(ref tx) = self.sync_tx {
            let _ = tx.send(event);
        }
    }
}

impl Filesystem for SkillFs {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &std::ffi::OsStr, reply: ReplyEntry) {
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
                reply.entry(&Duration::from_secs(1), &attr, 0);
            }
            PathType::SkillDir { skill_name } => {
                let exists = skill_name == "skill-discover" || {
                    let store = self.store.read();
                    store.get(&skill_name).is_some()
                };
                if exists {
                    let ino = self.inodes.allocate(&path_str, FileType::Directory, parent);
                    let mut attr = self.dir_attr();
                    attr.ino = ino;
                    reply.entry(&Duration::from_secs(1), &attr, 0);
                } else {
                    reply.error(libc::ENOENT);
                }
            }
            PathType::SkillMd { skill_name } => {
                match self.compiled_skill_md(&skill_name) {
                    Some(compiled) => {
                        let ino = self
                            .inodes
                            .allocate(&path_str, FileType::RegularFile, parent);
                        // Fetch metadata via fd-safe path to avoid FUSE re-entry.
                        let mut attr = if skill_name == "skill-discover" {
                            self.virtual_file_attr(compiled.len() as u64)
                        } else {
                            let md_phys = self.source_base().join(&skill_name).join("SKILL.md");
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
                        reply.entry(&Duration::from_secs(1), &attr, 0);
                    }
                    None => reply.error(libc::ENOENT),
                }
            }
            PathType::Passthrough {
                skill_name,
                relative_path,
            } => {
                let physical_path = self.skill_physical_dir(&skill_name).join(&relative_path);
                if physical_path.exists() {
                    match std::fs::metadata(&physical_path) {
                        Ok(meta) => {
                            let kind = if meta.is_dir() {
                                FileType::Directory
                            } else {
                                FileType::RegularFile
                            };
                            let ino = self.inodes.allocate(&path_str, kind, parent);
                            let mut attr = file_attr_from_metadata(&meta);
                            attr.ino = ino;
                            reply.entry(&Duration::from_secs(1), &attr, 0);
                        }
                        Err(_) => reply.error(libc::EIO),
                    }
                } else {
                    reply.error(libc::ENOENT);
                }
            }
            PathType::Invalid => reply.error(libc::ENOENT),
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        debug!(ino, "getattr");

        if ino == FUSE_ROOT_ID {
            reply.attr(&Duration::from_secs(1), &self.dir_attr());
            return;
        }

        let path = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        match parse_path(Path::new(&path), self.in_place) {
            PathType::Root | PathType::SkillsDir | PathType::SkillDir { .. } => {
                reply.attr(&Duration::from_secs(1), &self.dir_attr());
            }
            PathType::SkillMd { skill_name } => {
                match self.compiled_skill_md(&skill_name) {
                    Some(compiled) => {
                        // Use fd-safe path to avoid FUSE re-entry in in-place mode.
                        let attr = if skill_name == "skill-discover" {
                            self.virtual_file_attr(compiled.len() as u64)
                        } else {
                            let md_phys = self.source_base().join(&skill_name).join("SKILL.md");
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
                let physical_path = self.skill_physical_dir(&skill_name).join(&relative_path);
                match std::fs::metadata(&physical_path) {
                    Ok(meta) => {
                        let attr = file_attr_from_metadata(&meta);
                        reply.attr(&Duration::from_secs(1), &attr);
                    }
                    Err(_) => reply.error(libc::ENOENT),
                }
            }
            PathType::Invalid => reply.error(libc::ENOENT),
        }
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
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
                reply.error(libc::ENOENT);
                return;
            }
        };

        let content = match parse_path(Path::new(&path), self.in_place) {
            PathType::SkillMd { skill_name } => match self.compiled_skill_md(&skill_name) {
                Some(c) => c,
                None => {
                    reply.error(libc::ENOENT);
                    return;
                }
            },
            PathType::Passthrough {
                skill_name,
                relative_path,
            } => {
                let physical_path = self.skill_physical_dir(&skill_name).join(&relative_path);
                match std::fs::read(&physical_path) {
                    Ok(bytes) => {
                        // For binary files, work directly with bytes
                        let offset = offset as usize;
                        if offset >= bytes.len() {
                            reply.data(&[]);
                            return;
                        }
                        let end = (offset + size as usize).min(bytes.len());
                        reply.data(&bytes[offset..end]);
                        return;
                    }
                    Err(_) => {
                        reply.error(libc::EIO);
                        return;
                    }
                }
            }
            _ => {
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

    fn open(&mut self, _req: &Request, ino: u64, flags: i32, reply: ReplyOpen) {
        debug!(ino, flags, "open");
        if self.inodes.get(ino).is_none() && ino != FUSE_ROOT_ID {
            reply.error(libc::ENOENT);
            return;
        }

        // Handle O_TRUNC for writable files.
        let is_trunc = (flags & libc::O_TRUNC) != 0;
        if is_trunc {
            let target = self.inodes.get_path(ino).and_then(|path| {
                self.resolve_physical_path(&path)
                    .map(|physical| (path, physical))
            });
            if let Some((path, physical)) = target {
                if let Err(e) = std::fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(&physical)
                {
                    warn!(op = "open+trunc", ?physical, error = %e, "truncate failed");
                    reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                    return;
                }

                // Keep store in sync when SKILL.md is truncated via O_TRUNC.
                if let PathType::SkillMd { skill_name } =
                    parse_path(Path::new(&path), self.in_place)
                {
                    self.send_sync(SyncEvent::Reparse { skill_name });
                }
            }
        }

        let fh = self.allocate_fh();
        reply.opened(fh, 0);
    }

    fn release(
        &mut self,
        _req: &Request,
        _ino: u64,
        _fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        reply.ok();
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        debug!(ino, offset, "readdir");

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
                    // Normal mode only: root shows the /skills sub-directory.
                    vec![
                        (FUSE_ROOT_ID, FileType::Directory, ".".to_string()),
                        (FUSE_ROOT_ID, FileType::Directory, "..".to_string()),
                        (
                            self.inodes.lookup_by_path("/skills").unwrap_or(2),
                            FileType::Directory,
                            "skills".to_string(),
                        ),
                    ]
                }
                PathType::SkillsDir => {
                    // Show only primary skills + always-visible skill-discover.
                    // In in-place mode this is the root; in normal mode it is /skills.
                    let skill_names = self.primary_skill_names();
                    let skills_dir_ino = self.skills_dir_ino();

                    let mut entries: Vec<(u64, FileType, String)> = vec![
                        (ino, FileType::Directory, ".".to_string()),
                        (FUSE_ROOT_ID, FileType::Directory, "..".to_string()),
                    ];

                    for name in &skill_names {
                        let skill_path = self.skill_inode_path(name);
                        let skill_ino =
                            self.inodes
                                .allocate(&skill_path, FileType::Directory, skills_dir_ino);
                        entries.push((skill_ino, FileType::Directory, name.clone()));
                    }

                    // Always include skill-discover.
                    if !skill_names.iter().any(|n| n == "skill-discover") {
                        let discover_path = self.skill_inode_path("skill-discover");
                        let discover_ino = self.inodes.allocate(
                            &discover_path,
                            FileType::Directory,
                            skills_dir_ino,
                        );
                        entries.push((
                            discover_ino,
                            FileType::Directory,
                            "skill-discover".to_string(),
                        ));
                    }

                    entries
                }
                PathType::SkillDir { skill_name } => {
                    let parent_ino = self.skills_dir_ino();
                    let mut entries: Vec<(u64, FileType, String)> = vec![
                        (ino, FileType::Directory, ".".to_string()),
                        (parent_ino, FileType::Directory, "..".to_string()),
                    ];

                    // SKILL.md (always present)
                    let md_path = format!("{}/SKILL.md", path);
                    let md_ino = self.inodes.allocate(&md_path, FileType::RegularFile, ino);
                    entries.push((md_ino, FileType::RegularFile, "SKILL.md".to_string()));

                    // Physical subdirectories / extra files (scripts/, references/, etc.)
                    if skill_name != "skill-discover" {
                        let phys_dir = self.skill_physical_dir(&skill_name);
                        if let Ok(dir_iter) = std::fs::read_dir(&phys_dir) {
                            for entry in dir_iter.flatten() {
                                let name = entry.file_name().to_string_lossy().to_string();
                                if name == "SKILL.md" {
                                    continue;
                                }
                                let kind = if entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
                                {
                                    FileType::Directory
                                } else {
                                    FileType::RegularFile
                                };
                                let entry_path = format!("{}/{}", path, name);
                                let entry_ino = self.inodes.allocate(&entry_path, kind, ino);
                                entries.push((entry_ino, kind, name));
                            }
                        }
                    }

                    entries
                }
                PathType::Passthrough {
                    skill_name,
                    relative_path,
                } => {
                    let phys_dir = self.skill_physical_dir(&skill_name).join(&relative_path);
                    let mut entries: Vec<(u64, FileType, String)> = vec![
                        (ino, FileType::Directory, ".".to_string()),
                        (ino, FileType::Directory, "..".to_string()),
                    ];
                    if let Ok(dir_iter) = std::fs::read_dir(&phys_dir) {
                        for entry in dir_iter.flatten() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            let kind = if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                                FileType::Directory
                            } else {
                                FileType::RegularFile
                            };
                            let entry_path = format!("{}/{}", path, name);
                            let entry_ino = self.inodes.allocate(&entry_path, kind, ino);
                            entries.push((entry_ino, kind, name));
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

    // -----------------------------------------------------------------------
    // Write operations — passthrough to physical filesystem.
    // Only readdir is virtualized; all other I/O goes to the underlying
    // directory via source_base() (which uses /proc/self/fd/{n} in in-place
    // mode to bypass the FUSE layer).
    // -----------------------------------------------------------------------

    fn write(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: fuser::ReplyWrite,
    ) {
        let path = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let physical = match self.resolve_physical_path(&path) {
            Some(p) => p,
            None => {
                self.ro_warn("write", &path);
                reply.error(libc::EROFS);
                return;
            }
        };

        debug!(ino, offset, len = data.len(), ?physical, "write");

        match std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&physical)
        {
            Ok(mut f) => {
                if let Err(e) = f.seek(SeekFrom::Start(offset as u64)) {
                    reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                    return;
                }
                match f.write(data) {
                    Ok(written) => {
                        // Trigger async re-parse if this is a SKILL.md.
                        if let PathType::SkillMd { skill_name } =
                            parse_path(Path::new(&path), self.in_place)
                        {
                            self.send_sync(SyncEvent::Reparse { skill_name });
                        }
                        reply.written(written as u32);
                    }
                    Err(e) => reply.error(e.raw_os_error().unwrap_or(libc::EIO)),
                }
            }
            Err(e) => reply.error(e.raw_os_error().unwrap_or(libc::EIO)),
        }
    }

    fn create(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &std::ffi::OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: fuser::ReplyCreate,
    ) {
        let path_str = match self.build_fuse_path(parent, name) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let physical = match self.resolve_physical_path(&path_str) {
            Some(p) => p,
            None => {
                self.ro_warn("create", &path_str);
                reply.error(libc::EROFS);
                return;
            }
        };

        debug!(parent, name = %name.to_string_lossy(), ?physical, "create");

        match std::fs::File::create(&physical) {
            Ok(_) => {
                let ino = self
                    .inodes
                    .allocate(&path_str, FileType::RegularFile, parent);
                let fh = self.allocate_fh();
                let attr = match std::fs::metadata(&physical) {
                    Ok(meta) => {
                        let mut a = file_attr_from_metadata(&meta);
                        a.ino = ino;
                        a
                    }
                    Err(_) => {
                        let mut a = self.virtual_file_attr(0);
                        a.ino = ino;
                        a
                    }
                };

                // Trigger re-parse if creating a SKILL.md.
                if let PathType::SkillMd { skill_name } =
                    parse_path(Path::new(&path_str), self.in_place)
                {
                    self.send_sync(SyncEvent::Reparse { skill_name });
                }

                reply.created(&Duration::from_secs(1), &attr, 0, fh, 0);
            }
            Err(e) => {
                warn!(op = "create", path = %path_str, error = %e, "create failed");
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    fn mkdir(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &std::ffi::OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let path_str = match self.build_fuse_path(parent, name) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let physical = match self.resolve_physical_path(&path_str) {
            Some(p) => p,
            None => {
                self.ro_warn("mkdir", &path_str);
                reply.error(libc::EROFS);
                return;
            }
        };

        debug!(parent, name = %name.to_string_lossy(), ?physical, "mkdir");

        match std::fs::create_dir(&physical) {
            Ok(()) => {
                let ino = self.inodes.allocate(&path_str, FileType::Directory, parent);
                let mut attr = self.dir_attr();
                attr.ino = ino;

                // If this is a skill-level directory, immediately add a placeholder
                // entry so the new skill appears in readdir/lookup right away.
                // The async Reparse (triggered when SKILL.md is later written) will
                // replace the placeholder with the real parsed entry.
                if let PathType::SkillDir { ref skill_name } =
                    parse_path(Path::new(&path_str), self.in_place)
                {
                    use skillfs_core::{ParseStatus, SkillEntry, SkillMetadata};
                    let placeholder = SkillEntry {
                        metadata: SkillMetadata {
                            name: skill_name.clone(),
                            ..SkillMetadata::default()
                        },
                        parameters: vec![],
                        returns: vec![],
                        body: String::new(),
                        parse_status: ParseStatus::Degraded(
                            "directory created, awaiting SKILL.md".to_string(),
                        ),
                        source_path: physical.join("SKILL.md"),
                        last_modified: std::time::SystemTime::now(),
                    };
                    self.store.write().upsert(placeholder);
                    debug!(name = %skill_name, "mkdir: inserted placeholder into store");
                }

                reply.entry(&Duration::from_secs(1), &attr, 0);
            }
            Err(e) => {
                warn!(op = "mkdir", path = %path_str, error = %e, "mkdir failed");
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    fn mknod(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &std::ffi::OsStr,
        _mode: u32,
        _umask: u32,
        _rdev: u32,
        reply: ReplyEntry,
    ) {
        let parent_path = self
            .inodes
            .get_path(parent)
            .unwrap_or_else(|| "<unknown>".into());
        let hint = format!("{}/{}", parent_path, name.to_string_lossy());
        self.ro_warn("mknod", &hint);
        reply.error(libc::EROFS);
    }

    fn unlink(&mut self, _req: &Request, parent: u64, name: &std::ffi::OsStr, reply: ReplyEmpty) {
        let path_str = match self.build_fuse_path(parent, name) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let physical = match self.resolve_physical_path(&path_str) {
            Some(p) => p,
            None => {
                self.ro_warn("unlink", &path_str);
                reply.error(libc::EROFS);
                return;
            }
        };

        debug!(parent, name = %name.to_string_lossy(), ?physical, "unlink");

        match std::fs::remove_file(&physical) {
            Ok(()) => {
                // Remove inode mapping.
                if let Some(ino) = self.inodes.lookup_by_path(&path_str) {
                    self.inodes.remove(ino);
                }
                // Fast-path store sync: if deleting SKILL.md, remove from store.
                if let PathType::SkillMd { skill_name } =
                    parse_path(Path::new(&path_str), self.in_place)
                {
                    self.store.write().remove(&skill_name);
                    info!(name = %skill_name, "sync: removed skill (SKILL.md deleted)");
                }
                reply.ok();
            }
            Err(e) => {
                warn!(op = "unlink", path = %path_str, error = %e, "unlink failed");
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    fn rmdir(&mut self, _req: &Request, parent: u64, name: &std::ffi::OsStr, reply: ReplyEmpty) {
        let path_str = match self.build_fuse_path(parent, name) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let physical = match self.resolve_physical_path(&path_str) {
            Some(p) => p,
            None => {
                self.ro_warn("rmdir", &path_str);
                reply.error(libc::EROFS);
                return;
            }
        };

        debug!(parent, name = %name.to_string_lossy(), ?physical, "rmdir");

        match std::fs::remove_dir(&physical) {
            Ok(()) => {
                // Remove inode and all children.
                self.inodes.remove_recursive(&path_str);
                // Fast-path store sync: if removing a skill directory.
                if let PathType::SkillDir { skill_name } =
                    parse_path(Path::new(&path_str), self.in_place)
                {
                    self.store.write().remove(&skill_name);
                    info!(name = %skill_name, "sync: removed skill (directory deleted)");
                }
                reply.ok();
            }
            Err(e) => {
                warn!(op = "rmdir", path = %path_str, error = %e, "rmdir failed");
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    fn rename(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &std::ffi::OsStr,
        newparent: u64,
        newname: &std::ffi::OsStr,
        _flags: u32,
        reply: ReplyEmpty,
    ) {
        let old_path = match self.build_fuse_path(parent, name) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let new_path = match self.build_fuse_path(newparent, newname) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let old_physical = match self.resolve_physical_path(&old_path) {
            Some(p) => p,
            None => {
                self.ro_warn("rename", &old_path);
                reply.error(libc::EROFS);
                return;
            }
        };
        let new_physical = match self.resolve_physical_path(&new_path) {
            Some(p) => p,
            None => {
                self.ro_warn("rename", &new_path);
                reply.error(libc::EROFS);
                return;
            }
        };

        debug!(
            old = %old_path, new = %new_path,
            ?old_physical, ?new_physical,
            "rename"
        );

        match std::fs::rename(&old_physical, &new_physical) {
            Ok(()) => {
                // Update inode mappings.
                self.inodes.rename_path(&old_path, &new_path);

                // Store sync for skill-level renames.
                let old_type = parse_path(Path::new(&old_path), self.in_place);
                let new_type = parse_path(Path::new(&new_path), self.in_place);
                match (&old_type, &new_type) {
                    (
                        PathType::SkillDir {
                            skill_name: old_name,
                        },
                        PathType::SkillDir {
                            skill_name: new_name,
                        },
                    ) => {
                        self.store.write().remove(old_name);
                        // Synchronously update the store under the new directory name.
                        // We must use the *directory* name as the store key regardless
                        // of what SKILL.md frontmatter says (the user may not have
                        // updated the `name:` field yet).
                        let md_path = self.source_base().join(new_name).join("SKILL.md");
                        let new_entry = match parser::parse_skill_file(&md_path) {
                            Ok(mut entry) => {
                                // Ensure the store key matches the directory name.
                                entry.metadata.name = new_name.clone();
                                entry
                            }
                            Err(_) => {
                                // SKILL.md not readable yet — insert a placeholder so
                                // the directory appears in readdir immediately.
                                use skillfs_core::{ParseStatus, SkillEntry, SkillMetadata};
                                SkillEntry {
                                    metadata: SkillMetadata {
                                        name: new_name.clone(),
                                        ..SkillMetadata::default()
                                    },
                                    parameters: vec![],
                                    returns: vec![],
                                    body: String::new(),
                                    parse_status: ParseStatus::Degraded(
                                        "renamed, awaiting SKILL.md update".to_string(),
                                    ),
                                    source_path: md_path,
                                    last_modified: std::time::SystemTime::now(),
                                }
                            }
                        };
                        self.store.write().upsert(new_entry);
                        info!(
                            old = %old_name, new = %new_name,
                            "sync: skill renamed (immediate store update)"
                        );
                    }
                    _ => {
                        // File-level rename inside a skill — trigger re-parse
                        // if SKILL.md is involved.
                        if let PathType::SkillMd { skill_name } = &new_type {
                            self.send_sync(SyncEvent::Reparse {
                                skill_name: skill_name.clone(),
                            });
                        }
                        if let PathType::SkillMd { skill_name } = &old_type {
                            self.store.write().remove(skill_name);
                        }
                    }
                }

                reply.ok();
            }
            Err(e) => {
                warn!(
                    op = "rename", old = %old_path, new = %new_path,
                    error = %e, "rename failed"
                );
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }

    fn setattr(
        &mut self,
        _req: &Request,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<fuser::TimeOrNow>,
        _mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<std::time::SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<std::time::SystemTime>,
        _chgtime: Option<std::time::SystemTime>,
        _bkuptime: Option<std::time::SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        let path = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let physical = match self.resolve_physical_path(&path) {
            Some(p) => p,
            None => {
                // For non-writable paths, return current attr if no mutation
                // is requested, otherwise EROFS.
                if size.is_none() {
                    // Read-only getattr-like call — just return attrs.
                    let pt = parse_path(Path::new(&path), self.in_place);
                    match pt {
                        PathType::Root | PathType::SkillsDir | PathType::SkillDir { .. } => {
                            reply.attr(&Duration::from_secs(1), &self.dir_attr());
                        }
                        _ => reply.error(libc::EROFS),
                    }
                } else {
                    self.ro_warn("setattr", &path);
                    reply.error(libc::EROFS);
                }
                return;
            }
        };

        debug!(ino, ?size, ?physical, "setattr");

        // Handle truncate.
        if let Some(new_size) = size {
            match std::fs::OpenOptions::new().write(true).open(&physical) {
                Ok(f) => {
                    if let Err(e) = f.set_len(new_size) {
                        reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                        return;
                    }

                    // Keep store in sync when SKILL.md is truncated via setattr.
                    if let PathType::SkillMd { skill_name } =
                        parse_path(Path::new(&path), self.in_place)
                    {
                        self.send_sync(SyncEvent::Reparse { skill_name });
                    }
                }
                Err(e) => {
                    reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                    return;
                }
            }
        }

        // Return updated attributes.
        match std::fs::metadata(&physical) {
            Ok(meta) => {
                let mut attr = file_attr_from_metadata(&meta);
                attr.ino = ino;
                reply.attr(&Duration::from_secs(1), &attr);
            }
            Err(e) => reply.error(e.raw_os_error().unwrap_or(libc::EIO)),
        }
    }

    fn symlink(
        &mut self,
        _req: &Request,
        parent: u64,
        link_name: &std::ffi::OsStr,
        _target: &std::path::Path,
        reply: ReplyEntry,
    ) {
        let parent_path = self
            .inodes
            .get_path(parent)
            .unwrap_or_else(|| "<unknown>".into());
        let hint = format!("{}/{}", parent_path, link_name.to_string_lossy());
        self.ro_warn("symlink", &hint);
        reply.error(libc::EROFS);
    }

    fn link(
        &mut self,
        _req: &Request,
        _ino: u64,
        newparent: u64,
        newname: &std::ffi::OsStr,
        reply: ReplyEntry,
    ) {
        let parent_path = self
            .inodes
            .get_path(newparent)
            .unwrap_or_else(|| "<unknown>".into());
        let hint = format!("{}/{}", parent_path, newname.to_string_lossy());
        self.ro_warn("link", &hint);
        reply.error(libc::EROFS);
    }
}

/// Find the longest common directory prefix shared by all given paths.
///
/// For example, given paths:
///   `/home/user/skills/apple-notes/SKILL.md`
///   `/home/user/skills/discord/SKILL.md`
/// Returns `Some("/home/user/skills")`.
fn find_common_path_prefix(paths: &[std::path::PathBuf]) -> Option<std::path::PathBuf> {
    if paths.is_empty() {
        return None;
    }
    // Work with parent dirs (strip filename component)
    let dirs: Vec<std::path::PathBuf> = paths
        .iter()
        .map(|p| p.parent().map(|d| d.to_path_buf()).unwrap_or_default())
        .collect();

    let first_components: Vec<_> = dirs[0].components().collect();
    let mut common_len = first_components.len();

    for dir in &dirs[1..] {
        let comps: Vec<_> = dir.components().collect();
        let match_len = first_components
            .iter()
            .zip(comps.iter())
            .take_while(|(a, b)| a == b)
            .count();
        common_len = common_len.min(match_len);
    }

    if common_len == 0 {
        return None;
    }

    let prefix: std::path::PathBuf = first_components[..common_len]
        .iter()
        .map(|c| c.as_os_str())
        .collect();
    Some(prefix)
}

/// Convert std::fs::Metadata to FUSE FileAttr.
fn file_attr_from_metadata(meta: &std::fs::Metadata) -> FileAttr {
    FileAttr {
        ino: 0,
        size: meta.len(),
        blocks: meta.len().div_ceil(512),
        atime: meta.accessed().unwrap_or(UNIX_EPOCH),
        mtime: meta.modified().unwrap_or(UNIX_EPOCH),
        ctime: UNIX_EPOCH,
        crtime: UNIX_EPOCH,
        kind: if meta.is_dir() {
            FileType::Directory
        } else {
            FileType::RegularFile
        },
        perm: if meta.is_dir() { 0o755 } else { 0o644 },
        nlink: 1,
        uid: unsafe { libc::getuid() },
        gid: unsafe { libc::getgid() },
        rdev: 0,
        flags: 0,
        blksize: 512,
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Mount the SkillFS FUSE filesystem (blocking).
pub fn mount(
    mountpoint: &Path,
    source: &Path,
    store: SharedSkillStore,
    options: MountOptions,
    in_place: bool,
) -> Result<(), FuseError> {
    info!(mountpoint = %mountpoint.display(), source = %source.display(), in_place, "mounting SkillFS");

    if !mountpoint.exists() {
        return Err(FuseError::InvalidMountPoint(
            "mount point does not exist".to_string(),
        ));
    }
    if !mountpoint.is_dir() {
        return Err(FuseError::InvalidMountPoint(
            "mount point is not a directory".to_string(),
        ));
    }

    #[cfg(target_os = "linux")]
    {
        let mountinfo = std::fs::read_to_string("/proc/mounts").ok();
        if let Some(info) = mountinfo {
            let mount_str = mountpoint.to_string_lossy();
            if info
                .lines()
                .any(|line| line.split_whitespace().nth(1) == Some(&*mount_str))
            {
                warn!(mountpoint = %mountpoint.display(), "mount point already mounted, attempting cleanup");
                let _ = std::process::Command::new("fusermount3")
                    .args(["-u", &mountpoint.to_string_lossy()])
                    .output();
                // Give the kernel time to process the unmount
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
        }
    }

    let mut fuse_opts: Vec<fuser::MountOption> = vec![];
    fuse_opts.push(fuser::MountOption::NoAtime);
    if options.allow_other {
        fuse_opts.push(fuser::MountOption::AllowOther);
    }

    let fs = SkillFs::new(
        mountpoint.to_path_buf(),
        source.to_path_buf(),
        store,
        in_place,
    );
    info!("starting FUSE filesystem");

    match fuser::mount2(fs, mountpoint, &fuse_opts) {
        Ok(()) => {
            info!("filesystem unmounted");
            Ok(())
        }
        Err(e) => Err(FuseError::MountFailed(e.to_string())),
    }
}

/// Mount in background (non-blocking).
pub fn mount_background(
    mountpoint: &Path,
    source: &Path,
    store: SharedSkillStore,
    options: MountOptions,
    in_place: bool,
) -> Result<MountHandle, FuseError> {
    let mountpoint_path = mountpoint.to_path_buf();
    let source_path = source.to_path_buf();

    let handle = std::thread::spawn(move || {
        let mut opts = options;
        opts.foreground = true;
        if let Err(e) = mount(&mountpoint_path, &source_path, store, opts, in_place) {
            error!(error = %e, "background mount failed");
        }
    });

    std::thread::sleep(Duration::from_millis(100));

    Ok(MountHandle {
        mountpoint: mountpoint.to_path_buf(),
        session: Some(handle),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_path_root() {
        assert_eq!(parse_path(Path::new("/"), false), PathType::Root);
    }

    #[test]
    fn test_parse_path_skills_dir() {
        assert_eq!(parse_path(Path::new("/skills"), false), PathType::SkillsDir);
    }

    #[test]
    fn test_parse_path_skill_dir() {
        assert_eq!(
            parse_path(Path::new("/skills/web-search"), false),
            PathType::SkillDir {
                skill_name: "web-search".to_string()
            }
        );
    }

    #[test]
    fn test_parse_path_skill_md() {
        assert_eq!(
            parse_path(Path::new("/skills/web-search/SKILL.md"), false),
            PathType::SkillMd {
                skill_name: "web-search".to_string()
            }
        );
    }

    #[test]
    fn test_parse_path_passthrough() {
        assert_eq!(
            parse_path(Path::new("/skills/web-search/scripts/run.sh"), false),
            PathType::Passthrough {
                skill_name: "web-search".to_string(),
                relative_path: PathBuf::from("scripts/run.sh"),
            }
        );
    }

    #[test]
    fn test_parse_path_invalid() {
        assert_eq!(
            parse_path(Path::new("/unknown-file"), false),
            PathType::Invalid
        );
    }

    #[test]
    fn test_mount_options_default() {
        let opts = MountOptions::default();
        assert!(!opts.allow_other);
        assert!(!opts.foreground);
        assert!(opts.fuse_options.contains(&"noatime".to_string()));
    }

    #[test]
    fn test_inode_manager_allocate() {
        let manager = InodeManager::new();
        assert!(manager.get(FUSE_ROOT_ID).is_some());
        assert_eq!(manager.get_path(FUSE_ROOT_ID), Some("/".to_string()));

        let ino = manager.allocate("/test", FileType::RegularFile, FUSE_ROOT_ID);
        assert!(ino > FUSE_ROOT_ID);
        assert_eq!(manager.get_path(ino), Some("/test".to_string()));

        let ino2 = manager.allocate("/test", FileType::RegularFile, FUSE_ROOT_ID);
        assert_eq!(ino, ino2);
    }

    #[test]
    fn test_inode_manager_lookup_by_path() {
        let manager = InodeManager::new();
        assert_eq!(manager.lookup_by_path("/"), Some(FUSE_ROOT_ID));
        assert_eq!(manager.lookup_by_path("/unknown"), None);

        let ino = manager.allocate("/new_file", FileType::RegularFile, FUSE_ROOT_ID);
        assert_eq!(manager.lookup_by_path("/new_file"), Some(ino));
    }

    #[test]
    fn test_parse_path_edge_cases() {
        assert_eq!(
            parse_path(Path::new("/unknown-file"), false),
            PathType::Invalid
        );
        assert_eq!(
            parse_path(Path::new("/skills/web-search/a/b/c/d.txt"), false),
            PathType::Passthrough {
                skill_name: "web-search".to_string(),
                relative_path: PathBuf::from("a/b/c/d.txt"),
            }
        );
    }

    #[test]
    fn test_parse_path_in_place() {
        assert_eq!(parse_path(Path::new("/"), true), PathType::SkillsDir);
        assert_eq!(
            parse_path(Path::new("/github"), true),
            PathType::SkillDir {
                skill_name: "github".to_string()
            }
        );
        assert_eq!(
            parse_path(Path::new("/github/SKILL.md"), true),
            PathType::SkillMd {
                skill_name: "github".to_string()
            }
        );
        assert_eq!(
            parse_path(Path::new("/github/scripts/run.sh"), true),
            PathType::Passthrough {
                skill_name: "github".to_string(),
                relative_path: PathBuf::from("scripts/run.sh"),
            }
        );
    }
}
