//! FUSE virtual filesystem layer for SkillFS.
//!
//! Exposes skills as a virtual filesystem. The default view (from
//! \`skillfs-views.toml\`) is shown directly under \`/skills/\`. Secondary
//! views are accessible via the always-visible \`skill-discover\` virtual
//! skill, which lists their real source paths so the AI can open them
//! directly.

use std::collections::HashMap;
use std::os::unix::fs::{DirBuilderExt, FileExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuser::{
    FUSE_ROOT_ID, FileAttr, FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyOpen, ReplyStatfs, ReplyXattr, Request,
};
use parking_lot::RwLock;
use skillfs_core::{
    SharedSkillStore, compiler, env::EnvironmentProfile, parser, views::ViewsConfig,
};
use thiserror::Error;
use tracing::{debug, error, info, warn};

pub mod security;

use security::{
    NoopEventSink, PathPolicy, PolicyDecision, SecurityPolicy, SkillEvent, SkillEventAction,
    SkillEventKind, SkillEventSink, SkillMetaProtectionPolicy,
    lifecycle::{
        LifecycleNameClass, classify_skill_name as classify_lifecycle_name,
        is_reserved_lifecycle_name,
    },
};

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
        // Block until the first event arrives; exit when the channel closes.
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
// Handle Manager
// ---------------------------------------------------------------------------

struct HandleEntry {
    #[allow(dead_code)]
    ino: u64,
    flags: i32,
    file: Option<std::fs::File>,
    append_mode: bool,
}

/// Directory handle entry with snapshot of entries at opendir time
struct DirHandleEntry {
    #[allow(dead_code)]
    ino: u64,
    /// Ordered snapshot of directory entries, frozen at opendir time.
    /// Each entry: (inode, file_type, name)
    entries: Vec<(u64, FileType, String)>,
    /// Physical directory fd for fsyncdir. None for virtual directories.
    dir_file: Option<std::fs::File>,
}

struct HandleManager {
    next_fh: AtomicU64,
    handles: RwLock<HashMap<u64, HandleEntry>>,
    dir_handles: RwLock<HashMap<u64, DirHandleEntry>>,
}

impl HandleManager {
    fn new() -> Self {
        Self {
            next_fh: AtomicU64::new(1),
            handles: RwLock::new(HashMap::new()),
            dir_handles: RwLock::new(HashMap::new()),
        }
    }

    fn allocate(&self, ino: u64, flags: i32, file: Option<std::fs::File>) -> u64 {
        let fh = self.next_fh.fetch_add(1, Ordering::Relaxed);
        let append_mode = (flags & libc::O_APPEND) != 0;
        self.handles.write().insert(
            fh,
            HandleEntry {
                ino,
                flags,
                file,
                append_mode,
            },
        );
        fh
    }

    /// Allocate a directory handle with a frozen snapshot
    fn allocate_dir(
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
    fn sync_dir(&self, fh: u64, datasync: bool) -> Option<std::io::Result<()>> {
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
    fn get_dir_entries(&self, fh: u64) -> Option<Vec<(u64, FileType, String)>> {
        self.dir_handles.read().get(&fh).map(|e| e.entries.clone())
    }

    /// Remove a directory handle, returns true if it existed
    fn remove_dir(&self, fh: u64) -> bool {
        self.dir_handles.write().remove(&fh).is_some()
    }

    /// Run `f` on the first handle (in arbitrary order) whose `ino` matches
    /// the given inode AND whose entry carries a real fd. Used by `getattr`
    /// to satisfy POSIX `fstat`-after-`unlink`: the kernel's
    /// `vfs_fstat` path forwards to FUSE getattr WITHOUT setting
    /// `FUSE_GETATTR_FH`, so we cannot just consult the `fh` argument.
    /// Returns `None` if no such handle exists (caller falls back to
    /// path-based stat or ENOENT).
    fn with_handle_for_ino<R>(&self, ino: u64, f: impl FnOnce(&std::fs::File) -> R) -> Option<R> {
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

    fn with_handle<R>(&self, fh: u64, f: impl FnOnce(&HandleEntry) -> R) -> Option<R> {
        let handles = self.handles.read();
        handles.get(&fh).map(f)
    }

    fn with_handle_mut<R>(&self, fh: u64, f: impl FnOnce(&mut HandleEntry) -> R) -> Option<R> {
        let mut handles = self.handles.write();
        handles.get_mut(&fh).map(f)
    }

    fn remove(&self, fh: u64) -> Option<HandleEntry> {
        self.handles.write().remove(&fh)
    }
}

fn open_options_from_flags(flags: i32) -> std::fs::OpenOptions {
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

/// Check whether a relative path belongs to the skill-discover namespace.
fn is_skill_discover_path(skill_name: &str) -> bool {
    skill_name == "skill-discover"
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
    handles: HandleManager,
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
    /// Skill Security policy. The S1 default is
    /// [`SkillMetaProtectionPolicy`], which denies mutating operations
    /// under `.skill-meta/**`. Embedders/tests can swap it for
    /// [`security::PermissivePolicy`] via [`SkillFs::with_policy`].
    policy: Arc<dyn SecurityPolicy>,
    /// Skill Security event sink (Package S0 seam). Default drops events.
    event_sink: Arc<dyn SkillEventSink>,
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
            handles: HandleManager::new(),
            inodes: InodeManager::new(),
            env_profile,
            views_config,
            source_dirfd,
            in_place,
            sync_tx: Some(sync_tx),
            policy: Arc::new(SkillMetaProtectionPolicy),
            event_sink: Arc::new(NoopEventSink),
        };

        // In normal mode pre-populate the /skills inode.
        // In in-place mode the root IS the skills dir — no sub-inode needed.
        if !in_place {
            fs.inodes
                .allocate("/skills", FileType::Directory, FUSE_ROOT_ID);
        }

        fs
    }

    /// Override the Skill Security policy. The S1 default is
    /// [`SkillMetaProtectionPolicy`]; tests/embedders that need fully
    /// permissive behaviour can plug in
    /// [`security::PermissivePolicy`] here.
    ///
    /// Builder-style; preserves backward compatibility with the existing
    /// `SkillFs::new` callers that do not configure security.
    pub fn with_policy(mut self, policy: Arc<dyn SecurityPolicy>) -> Self {
        self.policy = policy;
        self
    }

    /// Override the Skill Security event sink. Default is [`NoopEventSink`].
    pub fn with_event_sink(mut self, sink: Arc<dyn SkillEventSink>) -> Self {
        self.event_sink = sink;
        self
    }

    /// Best-effort event emission. The result is intentionally discarded —
    /// sinks are expected to be non-blocking, but SkillFS does not retry or
    /// surface a misbehaving sink either way (see [`SkillEventSink`]).
    fn emit_event(&self, event: SkillEvent) {
        self.event_sink.emit(&event);
    }

    /// Build and emit a normalized event for a FUSE operation given the
    /// parsed `path_type` and a known outcome. Centralized so individual
    /// callbacks add a single line at the success/failure branches rather
    /// than reconstructing skill name + relative path inline each time.
    ///
    /// This helper always allocates the [`SkillEvent`] (including any
    /// `PathBuf` clone) before invoking the sink — the default
    /// [`NoopEventSink`] then drops it cheaply. A future optimization could
    /// add an `events_enabled` fast path; doing so today would require
    /// reaching into the trait object and is not worth the complexity while
    /// FUSE callback rates are dominated by I/O work.
    fn emit_op_event(
        &self,
        req: &Request,
        path_type: &PathType,
        kind: SkillEventKind,
        action: SkillEventAction,
        errno_value: Option<i32>,
        bytes: Option<u64>,
    ) {
        let (skill_name, relative_path) = match path_type {
            PathType::Passthrough {
                skill_name,
                relative_path,
            } => (Some(skill_name.clone()), Some(relative_path.clone())),
            PathType::SkillMd { skill_name } => {
                (Some(skill_name.clone()), Some(PathBuf::from("SKILL.md")))
            }
            PathType::SkillDir { skill_name } => (Some(skill_name.clone()), None),
            _ => (None, None),
        };
        let mut event = SkillEvent::new(kind)
            .with_optional_skill_name(skill_name)
            .with_optional_relative_path(relative_path)
            .with_action(action)
            .with_caller(req.uid(), req.gid());
        if let Some(e) = errno_value {
            event = event.with_errno(e);
        }
        if let Some(b) = bytes {
            event = event.with_bytes(b);
        }
        self.emit_event(event);
    }

    /// Run the configured Skill Security policy against `ctx`.
    fn policy_check(&self, ctx: &PathPolicy<'_>) -> PolicyDecision {
        self.policy.check_path(ctx)
    }

    /// Emit a normalized `Metadata` event for an xattr mutation (`setxattr`
    /// or `removexattr`). The xattr verb (`set` / `remove`) and the xattr
    /// name are folded into the existing `detail` string so the JSONL audit
    /// shape stays backwards compatible.
    ///
    /// `class` is an optional snake_case label appended to the audit
    /// `detail` string when present. It lets rejection branches name *why*
    /// the request was refused (e.g. `virtual_xattr_path`,
    /// `unsupported_xattr_namespace`) without inventing new JSONL fields.
    #[allow(clippy::too_many_arguments)]
    fn emit_xattr_event(
        &self,
        req: &Request,
        path_type: &PathType,
        verb: &str,
        name: &std::ffi::OsStr,
        action: SkillEventAction,
        errno_value: Option<i32>,
        class: Option<&str>,
    ) {
        let (skill_name, relative_path) = match path_type {
            PathType::Passthrough {
                skill_name,
                relative_path,
            } => (Some(skill_name.clone()), Some(relative_path.clone())),
            PathType::SkillMd { skill_name } => {
                (Some(skill_name.clone()), Some(PathBuf::from("SKILL.md")))
            }
            PathType::SkillDir { skill_name } => (Some(skill_name.clone()), None),
            _ => (None, None),
        };
        let display_name = name.to_string_lossy();
        let detail = match class {
            Some(c) => format!("xattr={} name={} class={}", verb, display_name, c),
            None => format!("xattr={} name={}", verb, display_name),
        };
        let mut event = SkillEvent::new(SkillEventKind::Metadata)
            .with_optional_skill_name(skill_name)
            .with_optional_relative_path(relative_path)
            .with_action(action)
            .with_caller(req.uid(), req.gid())
            .with_detail(detail);
        if let Some(e) = errno_value {
            event = event.with_errno(e);
        }
        self.emit_event(event);
    }

    /// Centralized `.skill-meta` mutation gate.
    ///
    /// Looks at the parsed path and the operation kind, asks the configured
    /// policy whether the mutation is allowed, and on `Deny` emits a
    /// `PolicyDenied` event and returns `Some(errno)` so the caller can
    /// short-circuit. `Allow` (or paths the policy cannot reason about)
    /// returns `None`.
    fn enforce_skill_meta(
        &self,
        path_type: &PathType,
        operation: SkillEventKind,
        req: &Request,
        detail: Option<String>,
    ) -> Option<i32> {
        let (skill_name, relative_path) = match path_type {
            PathType::Passthrough {
                skill_name,
                relative_path,
            } => (skill_name.as_str(), relative_path.as_path()),
            // Only Passthrough paths can land inside `.skill-meta`. Other
            // path classes (Root, SkillsDir, SkillDir, SkillMd, Invalid)
            // are excluded by construction.
            _ => return None,
        };

        let ctx = PathPolicy::new(operation)
            .with_skill_name(Some(skill_name))
            .with_relative_path(Some(relative_path));
        match self.policy_check(&ctx) {
            PolicyDecision::Allow => None,
            PolicyDecision::Deny { errno, reason } => {
                let mut event = SkillEvent::new(SkillEventKind::PolicyDenied)
                    .with_skill_name(skill_name)
                    .with_relative_path(relative_path)
                    .with_action(SkillEventAction::Rejected)
                    .with_errno(errno)
                    .with_caller(req.uid(), req.gid())
                    .with_detail(format!("op={:?} reason={}", operation, reason));
                if let Some(extra) = detail {
                    event = event
                        .with_detail(format!("op={:?} reason={} {}", operation, reason, extra));
                }
                self.emit_event(event);
                Some(errno)
            }
        }
    }

    /// Return the lifecycle namespace name reserved by Package S3 when the
    /// parsed path's top-level skill-name segment matches one of
    /// `.staging`, `.certified`, `.quarantine`, or `.archive`. Otherwise
    /// returns `None`.
    ///
    /// The check is purely lexical and applies to `SkillDir`, `SkillMd`,
    /// and `Passthrough` paths — i.e. any FUSE path that lives below the
    /// reserved top-level segment. Root, SkillsDir, and Invalid never have
    /// a skill-name component and always return `None`.
    fn lifecycle_reservation(path_type: &PathType) -> Option<&'static str> {
        let skill_name = match path_type {
            PathType::SkillDir { skill_name }
            | PathType::SkillMd { skill_name }
            | PathType::Passthrough { skill_name, .. } => skill_name.as_str(),
            _ => return None,
        };
        match classify_lifecycle_name(skill_name) {
            LifecycleNameClass::Reserved(canonical) => Some(canonical),
            LifecycleNameClass::Ordinary => None,
        }
    }

    /// Centralized lifecycle namespace reservation gate (Package S3).
    ///
    /// When the parsed path resolves to a reserved lifecycle namespace
    /// (`.staging`, `.certified`, `.quarantine`, `.archive`), emits a
    /// `PolicyDenied` audit event with `EACCES` and returns
    /// `Some(libc::EACCES)` so callers can short-circuit before touching
    /// the underlying filesystem. Returns `None` for ordinary paths.
    ///
    /// The reservation is enforced for **mutating** operations only
    /// (`Create`, `Delete`, `Rename`, `Write`, `Metadata`,
    /// `SymlinkAttempt`, `HardlinkAttempt`); non-mutating operations
    /// observe the boundary through hidden lookup/readdir at virtual-view
    /// layers. Phase 1 errno semantics for ordinary paths are preserved.
    fn enforce_lifecycle_reservation(
        &self,
        path_type: &PathType,
        operation: SkillEventKind,
        req: &Request,
        detail: Option<String>,
    ) -> Option<i32> {
        let canonical = Self::lifecycle_reservation(path_type)?;
        let errno = libc::EACCES;
        let reason = "lifecycle namespace is reserved";
        let (skill_name, relative_path) = match path_type {
            PathType::Passthrough {
                skill_name,
                relative_path,
            } => (skill_name.clone(), Some(relative_path.clone())),
            PathType::SkillMd { skill_name } => {
                (skill_name.clone(), Some(PathBuf::from("SKILL.md")))
            }
            PathType::SkillDir { skill_name } => (skill_name.clone(), None),
            _ => return None,
        };
        let mut event = SkillEvent::new(SkillEventKind::PolicyDenied)
            .with_skill_name(skill_name)
            .with_action(SkillEventAction::Rejected)
            .with_errno(errno)
            .with_caller(req.uid(), req.gid());
        if let Some(rel) = relative_path {
            event = event.with_relative_path(rel);
        }
        let base_detail = format!(
            "op={:?} reason={} lifecycle={}",
            operation, reason, canonical
        );
        let final_detail = match detail {
            Some(extra) => format!("{} {}", base_detail, extra),
            None => base_detail,
        };
        event = event.with_detail(final_detail);
        self.emit_event(event);
        Some(errno)
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

    /// Open the physical parent directory of `fuse_path` and return both the
    /// open fd and the leaf name suitable for an `*at` syscall. Used to
    /// sidestep `PATH_MAX` on absolute physical paths whose total length
    /// exceeds the kernel's userspace path-name limit: the parent itself
    /// stays within `PATH_MAX` for any reachable leaf (because the leaf
    /// component is at least one byte), so opening the parent succeeds and
    /// the `*at` syscall only needs the short leaf component.
    ///
    /// Returns the FUSE-side errno on failure (parent unresolvable, parent
    /// open failed, or leaf missing).
    fn open_parent_dir_for(
        &self,
        fuse_path: &str,
    ) -> Result<(std::fs::File, std::ffi::OsString), i32> {
        let path = Path::new(fuse_path);
        let leaf = path
            .file_name()
            .map(|n| n.to_os_string())
            .ok_or(libc::EINVAL)?;
        let parent_fuse = path.parent().ok_or(libc::EINVAL)?;
        let parent_fuse_str = match parent_fuse.to_str() {
            Some(s) => s.to_string(),
            None => return Err(libc::EINVAL),
        };
        let parent_physical = match parse_path(parent_fuse, self.in_place) {
            PathType::SkillDir { skill_name } => self.source_base().join(&skill_name),
            PathType::SkillMd { skill_name } => {
                self.source_base().join(&skill_name).join("SKILL.md")
            }
            PathType::Passthrough {
                skill_name,
                relative_path,
            } => self.source_base().join(&skill_name).join(&relative_path),
            PathType::SkillsDir | PathType::Root => self.source_base(),
            PathType::Invalid => return Err(libc::ENOTDIR),
        };
        let _ = parent_fuse_str; // suppress unused-binding warning when tracing is off
        open_dir_path(&parent_physical)
            .map(|f| (f, leaf))
            .map_err(|e| errno(&e))
    }

    /// Send a sync event to the background worker (non-blocking).
    fn send_sync(&self, event: SyncEvent) {
        if let Some(ref tx) = self.sync_tx {
            let _ = tx.send(event);
        }
    }

    /// Check physical file access permissions.
    ///
    /// Returns 0 on success, or an errno value on failure.
    fn check_physical_access_result(&self, path: &Path, mask: i32, req: &Request) -> i32 {
        if mask == libc::F_OK {
            return match std::fs::metadata(path) {
                Ok(_) => 0,
                Err(e) => errno(&e),
            };
        }
        let metadata = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => return errno(&e),
        };
        let mode = metadata.mode();
        let file_uid = metadata.uid();
        let file_gid = metadata.gid();
        let caller_uid = req.uid();
        let caller_gid = req.gid();

        if caller_uid == 0 {
            if (mask & libc::X_OK) != 0 && (mode & 0o111) == 0 {
                return libc::EACCES;
            }
            return 0;
        }

        // NOTE: FUSE protocol only provides the caller's primary gid via req.gid().
        // Supplementary group membership is not available in the FUSE request,
        // which may cause false negatives when access is granted via supplementary groups.
        // This is a known limitation of the FUSE protocol.
        let perm_bits = if caller_uid == file_uid {
            (mode >> 6) & 0o7
        } else if caller_gid == file_gid {
            (mode >> 3) & 0o7
        } else {
            mode & 0o7
        };

        if (mask & libc::R_OK) != 0 && (perm_bits & 0o4) == 0 {
            return libc::EACCES;
        }
        if (mask & libc::W_OK) != 0 && (perm_bits & 0o2) == 0 {
            return libc::EACCES;
        }
        if (mask & libc::X_OK) != 0 && (perm_bits & 0o1) == 0 {
            return libc::EACCES;
        }
        0
    }

    /// Dynamic readdir fallback — generates entries on the fly without a
    /// pre-opened directory handle snapshot.
    fn readdir_dynamic(&mut self, ino: u64, offset: i64, mut reply: ReplyDirectory) {
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
                    let mut skill_names = self.primary_skill_names();
                    // S3: lifecycle namespaces never appear in ordinary
                    // `/skills` listings. The store loader already skips
                    // hidden top-level directories, but defend in depth here
                    // in case a placeholder lands in the store via mkdir
                    // before the S3 mkdir gate fires.
                    skill_names.retain(|n| !is_reserved_lifecycle_name(n));
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

                    let md_path = format!("{}/SKILL.md", path);
                    let md_ino = self.inodes.allocate(&md_path, FileType::RegularFile, ino);
                    entries.push((md_ino, FileType::RegularFile, "SKILL.md".to_string()));

                    if skill_name != "skill-discover" {
                        let phys_dir = self.skill_physical_dir(&skill_name);
                        if let Ok(dir_iter) = std::fs::read_dir(&phys_dir) {
                            for entry in dir_iter.flatten() {
                                let name = entry.file_name().to_string_lossy().to_string();
                                if name == "SKILL.md" {
                                    continue;
                                }
                                let kind = dir_entry_file_type(&entry);
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
                    let parent_ino = {
                        let parent_path = Path::new(&path)
                            .parent()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default();
                        self.inodes.lookup_by_path(&parent_path).unwrap_or(ino)
                    };
                    let phys_dir = self.skill_physical_dir(&skill_name).join(&relative_path);
                    let mut entries: Vec<(u64, FileType, String)> = vec![
                        (ino, FileType::Directory, ".".to_string()),
                        (parent_ino, FileType::Directory, "..".to_string()),
                    ];
                    if let Ok(dir_iter) = std::fs::read_dir(&phys_dir) {
                        for entry in dir_iter.flatten() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            let kind = dir_entry_file_type(&entry);
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
                // S3: lifecycle namespaces are hidden from ordinary lookup.
                // A future management view may expose them, but no such view
                // exists yet, so any caller that probes directly must see
                // the same `ENOENT` they would see for a non-existent skill.
                if is_reserved_lifecycle_name(&skill_name) {
                    reply.error(libc::ENOENT);
                    return;
                }
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
                // S3: lifecycle namespaces are hidden, including their
                // virtual `SKILL.md`. The boundary holds even if a caller
                // bypasses readdir and probes the path directly.
                if is_reserved_lifecycle_name(&skill_name) {
                    reply.error(libc::ENOENT);
                    return;
                }
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
                // S3: lifecycle namespaces are hidden — every descendant
                // of a reserved root is treated as if it does not exist,
                // even if it is physically present in the source tree.
                if is_reserved_lifecycle_name(&skill_name) {
                    reply.error(libc::ENOENT);
                    return;
                }
                let physical_path = self.skill_physical_dir(&skill_name).join(&relative_path);
                // Use symlink_metadata so a passthrough symlink is reported
                // with FileType::Symlink rather than collapsed onto its
                // target. The kernel issues readlink() afterwards if it
                // wants to follow the link.
                match std::fs::symlink_metadata(&physical_path) {
                    Ok(meta) => {
                        let ft = meta.file_type();
                        let kind = if ft.is_symlink() {
                            FileType::Symlink
                        } else if ft.is_dir() {
                            FileType::Directory
                        } else {
                            FileType::RegularFile
                        };
                        let ino = self.inodes.allocate(&path_str, kind, parent);
                        let mut attr = file_attr_from_metadata(&meta);
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
                                    let kind = match st.st_mode & libc::S_IFMT {
                                        libc::S_IFLNK => FileType::Symlink,
                                        libc::S_IFDIR => FileType::Directory,
                                        _ => FileType::RegularFile,
                                    };
                                    let ino = self.inodes.allocate(&path_str, kind, parent);
                                    let mut attr = file_attr_from_stat(&st);
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
            PathType::Invalid => reply.error(libc::ENOENT),
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, fh: Option<u64>, reply: ReplyAttr) {
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
            PathType::Invalid => reply.error(libc::ENOENT),
        }
    }

    fn read(
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
        let content = match path_type.clone() {
            PathType::SkillMd { skill_name } => match self.compiled_skill_md(&skill_name) {
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
            },
            PathType::Passthrough { .. } => {
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

    fn open(&mut self, req: &Request, ino: u64, flags: i32, reply: ReplyOpen) {
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
            PathType::Root | PathType::SkillsDir => {
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
                let fh = self.handles.allocate(ino, flags, None);
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
        let physical = match self.resolve_physical_path(&path) {
            Some(p) => p,
            None => {
                reply.error(libc::EROFS);
                return;
            }
        };

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
                            let fh = self.handles.allocate(ino, flags, None);
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
            }

            if is_write {
                // Open physical file for writing
                match open_options_from_flags(flags).open(&physical) {
                    Ok(file) => {
                        let fh = self.handles.allocate(ino, flags, Some(file));
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
            } else {
                // Read-only open for SKILL.md: virtual content, no physical fd needed
                let fh = self.handles.allocate(ino, flags, None);
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
                let fh = self.handles.allocate(ino, flags, Some(file));
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

    fn release(
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

    fn flush(&mut self, _req: &Request, _ino: u64, fh: u64, _lock_owner: u64, reply: ReplyEmpty) {
        let exists = self.handles.with_handle(fh, |_| ()).is_some();
        if exists {
            reply.ok();
        } else {
            reply.error(libc::EBADF);
        }
    }

    fn fsync(&mut self, _req: &Request, _ino: u64, fh: u64, datasync: bool, reply: ReplyEmpty) {
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

    fn opendir(&mut self, _req: &Request, ino: u64, _flags: i32, reply: ReplyOpen) {
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
                (
                    vec![
                        (FUSE_ROOT_ID, FileType::Directory, ".".to_string()),
                        (FUSE_ROOT_ID, FileType::Directory, "..".to_string()),
                        (skills_ino, FileType::Directory, "skills".to_string()),
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
                    let skill_ino =
                        self.inodes
                            .allocate(&skill_path, FileType::Directory, skills_dir_ino);
                    entries.push((skill_ino, FileType::Directory, name.clone()));
                }

                // Always include skill-discover
                if !sorted_names.iter().any(|n| n == "skill-discover") {
                    let discover_path = self.skill_inode_path("skill-discover");
                    let discover_ino =
                        self.inodes
                            .allocate(&discover_path, FileType::Directory, skills_dir_ino);
                    entries.push((
                        discover_ino,
                        FileType::Directory,
                        "skill-discover".to_string(),
                    ));
                }

                (entries, None)
            }
            PathType::SkillDir { ref skill_name } => {
                let skills_dir_ino = self.skills_dir_ino();
                let mut entries = vec![
                    (ino, FileType::Directory, ".".to_string()),
                    (skills_dir_ino, FileType::Directory, "..".to_string()),
                ];

                // Virtual SKILL.md always present
                let md_path = format!("{}/SKILL.md", path);
                let md_ino = self.inodes.allocate(&md_path, FileType::RegularFile, ino);
                entries.push((md_ino, FileType::RegularFile, "SKILL.md".to_string()));

                // Physical files (non skill-discover)
                let dir_file = if !is_skill_discover_path(skill_name) {
                    let phys_dir = self.skill_physical_dir(skill_name);
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
                            let entry_ino = self.inodes.allocate(&entry_path, kind, ino);
                            entries.push((entry_ino, kind, name));
                        }
                    }
                    std::fs::File::open(self.skill_physical_dir(skill_name)).ok()
                } else {
                    None
                };

                (entries, dir_file)
            }
            PathType::Passthrough {
                ref skill_name,
                ref relative_path,
            } => {
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

                let phys_dir = self.skill_physical_dir(skill_name).join(relative_path);
                if let Ok(dir_iter) = std::fs::read_dir(&phys_dir) {
                    let mut phys_entries: Vec<_> = dir_iter.flatten().collect();
                    phys_entries.sort_by_key(|e| e.file_name());

                    for entry in phys_entries {
                        let name = entry.file_name().to_string_lossy().to_string();
                        let kind = dir_entry_file_type(&entry);
                        let entry_path = format!("{}/{}", path, name);
                        let entry_ino = self.inodes.allocate(&entry_path, kind, ino);
                        entries.push((entry_ino, kind, name));
                    }
                }
                let dir_file = std::fs::File::open(&phys_dir).ok();

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

    fn readdir(
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

    fn releasedir(&mut self, _req: &Request, ino: u64, fh: u64, _flags: i32, reply: ReplyEmpty) {
        debug!(ino, fh, "releasedir");
        self.handles.remove_dir(fh);
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
        req: &Request,
        ino: u64,
        fh: u64,
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
                // Open-after-unlink: the path mapping is gone but a write
                // arriving through the same fh must still land on the open
                // file descriptor (POSIX `unlink` leaves an open fd usable
                // until last close). S1/S3 defense-in-depth re-checks are
                // skipped because the path is no longer in any protected
                // zone — protection at unlink time already gated the move.
                let result = self.handles.with_handle_mut(fh, |entry| {
                    let access = entry.flags & libc::O_ACCMODE;
                    if access == libc::O_RDONLY {
                        return Err(libc::EBADF);
                    }
                    if let Some(ref file) = entry.file {
                        if entry.append_mode {
                            use std::io::Write;
                            let mut file = file;
                            file.write(data).map_err(|e| errno(&e))
                        } else {
                            file.write_at(data, offset as u64).map_err(|e| errno(&e))
                        }
                    } else {
                        Err(libc::EBADF)
                    }
                });
                match result {
                    Some(Ok(n)) => {
                        reply.written(n as u32);
                    }
                    Some(Err(e)) => {
                        reply.error(e);
                    }
                    None => {
                        reply.error(libc::ENOENT);
                    }
                }
                return;
            }
        };

        let path_type = parse_path(Path::new(&path), self.in_place);

        // skill-discover namespace is always read-only
        match &path_type {
            PathType::SkillMd { skill_name }
            | PathType::SkillDir { skill_name }
            | PathType::Passthrough { skill_name, .. }
                if is_skill_discover_path(skill_name) =>
            {
                reply.error(libc::EROFS);
                return;
            }
            _ => {}
        }

        // S3 defense-in-depth: refuse writes against a reserved lifecycle
        // namespace even if a handle for it predates the boundary.
        if let Some(errno) =
            self.enforce_lifecycle_reservation(&path_type, SkillEventKind::Write, req, None)
        {
            reply.error(errno);
            return;
        }

        // S1 defense-in-depth: even if a handle for `.skill-meta` slipped
        // past the open gate, refuse the write.
        if let Some(errno) = self.enforce_skill_meta(&path_type, SkillEventKind::Write, req, None) {
            reply.error(errno);
            return;
        }

        debug!(ino, offset, len = data.len(), "write");

        // Must go through fh lookup
        let result = self.handles.with_handle_mut(fh, |entry| {
            // Check writable
            let access = entry.flags & libc::O_ACCMODE;
            if access == libc::O_RDONLY {
                return Err(libc::EBADF);
            }
            if let Some(ref file) = entry.file {
                if entry.append_mode {
                    // O_APPEND: use write() (kernel guarantees seek-to-end)
                    use std::io::Write;
                    let mut f = file;
                    // We need a mutable reference workaround via Write on &File
                    match f.write(data) {
                        Ok(n) => Ok(n),
                        Err(e) => Err(errno(&e)),
                    }
                } else {
                    match file.write_at(data, offset as u64) {
                        Ok(n) => Ok(n),
                        Err(e) => Err(errno(&e)),
                    }
                }
            } else {
                Err(libc::EBADF)
            }
        });

        match result {
            Some(Ok(written)) => {
                // Trigger async re-parse if this is a SKILL.md.
                if let PathType::SkillMd { skill_name } = &path_type {
                    self.send_sync(SyncEvent::Reparse {
                        skill_name: skill_name.clone(),
                    });
                }
                self.emit_op_event(
                    req,
                    &path_type,
                    SkillEventKind::Write,
                    SkillEventAction::Allowed,
                    None,
                    Some(written as u64),
                );
                reply.written(written as u32);
            }
            Some(Err(e)) => {
                self.emit_op_event(
                    req,
                    &path_type,
                    SkillEventKind::Write,
                    SkillEventAction::Failed,
                    Some(e),
                    None,
                );
                reply.error(e);
            }
            None => {
                self.emit_op_event(
                    req,
                    &path_type,
                    SkillEventKind::Write,
                    SkillEventAction::Failed,
                    Some(libc::EBADF),
                    None,
                );
                reply.error(libc::EBADF);
            }
        }
    }

    fn create(
        &mut self,
        req: &Request,
        parent: u64,
        name: &std::ffi::OsStr,
        mode: u32,
        umask: u32,
        flags: i32,
        reply: fuser::ReplyCreate,
    ) {
        let path_str = match self.build_fuse_path(parent, name) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let path_type = parse_path(Path::new(&path_str), self.in_place);

        // S3: refuse to create entries beneath a reserved lifecycle
        // namespace before any physical I/O.
        if let Some(errno) =
            self.enforce_lifecycle_reservation(&path_type, SkillEventKind::Create, req, None)
        {
            reply.error(errno);
            return;
        }

        // S1: `.skill-meta/**` is mutation-protected. Reject before touching
        // the underlying filesystem so no partial state is left behind.
        if let Some(errno) = self.enforce_skill_meta(&path_type, SkillEventKind::Create, req, None)
        {
            reply.error(errno);
            return;
        }

        let physical = match self.resolve_physical_path(&path_str) {
            Some(p) => p,
            None => {
                self.ro_warn("create", &path_str);
                reply.error(libc::EROFS);
                return;
            }
        };

        debug!(parent, name = %name.to_string_lossy(), ?physical, "create");

        // skill-discover namespace is read-only
        if let PathType::Passthrough { ref skill_name, .. } = path_type {
            if is_skill_discover_path(skill_name) {
                reply.error(libc::EROFS);
                return;
            }
        }

        // Build open options: reuse open_options_from_flags and add create semantics
        let mut opts = open_options_from_flags(flags);
        if (flags & libc::O_EXCL) != 0 {
            opts.create_new(true);
        } else {
            opts.create(true);
        }
        // Physical create requires write capability on the fd; however the handle's
        // flags preserve the original access mode requested by the caller (O_RDONLY).
        let access = flags & libc::O_ACCMODE;
        if access == libc::O_RDONLY {
            opts.write(true);
        }
        // POSIX: file permission bits of the new file shall be initialized
        // from mode and then masked by the process file-mode creation mask.
        // The FUSE protocol passes both the requested mode and the caller's
        // umask, so we apply them here rather than letting the FUSE daemon's
        // own umask (typically 0o022) shadow the caller's intent.
        let effective_mode = mode & !umask & 0o7777;
        opts.mode(effective_mode);
        let file_result = match opts.open(&physical) {
            Ok(f) => Ok(f),
            Err(e) if e.raw_os_error() == Some(libc::ENAMETOOLONG) => {
                // Long-path fallback (see mkdir for the same pattern). We
                // re-derive the open flags from the requested access so the
                // *at syscall behaves identically to the OpenOptions path.
                match self.open_parent_dir_for(&path_str) {
                    Ok((parent_fd, leaf)) => {
                        let mut creat_flags = flags;
                        if (creat_flags & libc::O_EXCL) != 0 {
                            // openat respects O_EXCL natively when O_CREAT is set
                        }
                        creat_flags |= libc::O_CREAT;
                        // Mirror the OpenOptions tweak above: read-only opens
                        // still need write capability to create the file.
                        let access = creat_flags & libc::O_ACCMODE;
                        if access == libc::O_RDONLY {
                            creat_flags = (creat_flags & !libc::O_ACCMODE) | libc::O_RDWR;
                        }
                        openat_leaf(&parent_fd, &leaf, creat_flags, effective_mode)
                    }
                    Err(_) => Err(e),
                }
            }
            Err(e) => Err(e),
        };

        match file_result {
            Ok(file) => {
                let ino = self
                    .inodes
                    .allocate(&path_str, FileType::RegularFile, parent);
                // Pull metadata directly off the freshly opened fd so this
                // path works even when the absolute physical path exceeds
                // PATH_MAX (where `std::fs::metadata(&physical)` would fail
                // with ENAMETOOLONG even though the create itself just
                // succeeded via openat).
                let attr = match file.metadata() {
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
                let fh = self.handles.allocate(ino, flags, Some(file));

                // Trigger re-parse if creating a SKILL.md.
                if let PathType::SkillMd { skill_name } = &path_type {
                    self.send_sync(SyncEvent::Reparse {
                        skill_name: skill_name.clone(),
                    });
                }

                self.emit_op_event(
                    req,
                    &path_type,
                    SkillEventKind::Create,
                    SkillEventAction::Allowed,
                    None,
                    None,
                );
                reply.created(&Duration::from_secs(1), &attr, 0, fh, 0);
            }
            Err(e) => {
                warn!(op = "create", path = %path_str, error = %e, "create failed");
                let err = errno(&e);
                self.emit_op_event(
                    req,
                    &path_type,
                    SkillEventKind::Create,
                    SkillEventAction::Failed,
                    Some(err),
                    None,
                );
                reply.error(err);
            }
        }
    }

    fn mkdir(
        &mut self,
        req: &Request,
        parent: u64,
        name: &std::ffi::OsStr,
        mode: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
        let path_str = match self.build_fuse_path(parent, name) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let path_type = parse_path(Path::new(&path_str), self.in_place);

        // S3: refuse to mkdir on a reserved lifecycle namespace name. The
        // gate runs before `.skill-meta` enforcement so the lifecycle
        // boundary cannot be sidestepped by also matching `.skill-meta`.
        if let Some(errno) =
            self.enforce_lifecycle_reservation(&path_type, SkillEventKind::Create, req, None)
        {
            reply.error(errno);
            return;
        }

        // S1: refuse to create directories under `.skill-meta/**`.
        if let Some(errno) = self.enforce_skill_meta(&path_type, SkillEventKind::Create, req, None)
        {
            reply.error(errno);
            return;
        }

        let physical = match self.resolve_physical_path(&path_str) {
            Some(p) => p,
            None => {
                self.ro_warn("mkdir", &path_str);
                reply.error(libc::EROFS);
                return;
            }
        };

        debug!(parent, name = %name.to_string_lossy(), ?physical, "mkdir");

        // POSIX: directory permission bits shall be initialized from mode
        // and then masked by the process file-mode creation mask. The FUSE
        // protocol delivers both, so we honor them explicitly instead of
        // inheriting the FUSE daemon's own umask.
        let effective_mode = mode & !umask & 0o7777;
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(effective_mode);
        let mkdir_result = match builder.create(&physical) {
            Ok(()) => Ok(()),
            Err(e) if e.raw_os_error() == Some(libc::ENAMETOOLONG) => {
                // Long-path fallback: the absolute physical path exceeds
                // PATH_MAX, but `mkdir -p`'s component-by-component walk
                // through the kernel only required NAME_MAX per component.
                // Open the parent dir and use `mkdirat` so the leaf name is
                // the only string the syscall sees.
                match self.open_parent_dir_for(&path_str) {
                    Ok((parent_fd, leaf)) => mkdirat_leaf(&parent_fd, &leaf, effective_mode),
                    Err(_) => Err(e),
                }
            }
            Err(e) => Err(e),
        };
        match mkdir_result {
            Ok(()) => {
                let ino = self.inodes.allocate(&path_str, FileType::Directory, parent);
                let mut attr = self.dir_attr();
                attr.ino = ino;

                // If this is a skill-level directory, immediately add a placeholder
                // entry so the new skill appears in readdir/lookup right away.
                // The async Reparse (triggered when SKILL.md is later written) will
                // replace the placeholder with the real parsed entry.
                if let PathType::SkillDir { ref skill_name } = path_type {
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
                reply.error(errno(&e));
            }
        }
    }

    fn mknod(
        &mut self,
        req: &Request,
        parent: u64,
        name: &std::ffi::OsStr,
        mode: u32,
        umask: u32,
        _rdev: u32,
        reply: ReplyEntry,
    ) {
        let path_str = match self.build_fuse_path(parent, name) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let path_type = parse_path(Path::new(&path_str), self.in_place);

        // T2 mknod policy: FIFO is the only special file SkillFS creates.
        // Sockets, block/char devices, and any other S_IFMT bit are
        // rejected with `EPERM` — matching the deterministic Linux errno
        // an unprivileged caller would see, and giving auditors a clear
        // signal that the request was a policy denial rather than an
        // unimplemented surface (`ENOSYS`) or a real `EROFS`. Regular
        // files come through `create()` in normal Linux FUSE clients;
        // an `S_IFREG` mknod here is therefore unexpected and refused
        // through the same `EPERM` path.
        let file_type_bits = mode & libc::S_IFMT;
        if file_type_bits != libc::S_IFIFO {
            warn!(
                op = "mknod",
                path = %path_str,
                file_type = format!("0o{:o}", file_type_bits),
                "non-FIFO mknod rejected by policy"
            );
            self.emit_op_event(
                req,
                &path_type,
                SkillEventKind::Create,
                SkillEventAction::Rejected,
                Some(libc::EPERM),
                None,
            );
            reply.error(libc::EPERM);
            return;
        }

        // Only Passthrough leaves under an ordinary skill can host a
        // freshly created FIFO. Virtual paths (Root, SkillsDir, SkillDir,
        // SkillMd, Invalid) are rejected before any physical I/O.
        let (skill_name, _relative_path) = match &path_type {
            PathType::Passthrough {
                skill_name,
                relative_path,
            } => (skill_name.clone(), relative_path.clone()),
            _ => {
                self.ro_warn("mknod", &path_str);
                self.emit_op_event(
                    req,
                    &path_type,
                    SkillEventKind::Create,
                    SkillEventAction::Rejected,
                    Some(libc::EROFS),
                    None,
                );
                reply.error(libc::EROFS);
                return;
            }
        };

        if is_skill_discover_path(&skill_name) {
            self.emit_op_event(
                req,
                &path_type,
                SkillEventKind::Create,
                SkillEventAction::Rejected,
                Some(libc::EROFS),
                None,
            );
            reply.error(libc::EROFS);
            return;
        }

        if let Some(errno) =
            self.enforce_lifecycle_reservation(&path_type, SkillEventKind::Create, req, None)
        {
            reply.error(errno);
            return;
        }
        if let Some(errno) = self.enforce_skill_meta(&path_type, SkillEventKind::Create, req, None)
        {
            reply.error(errno);
            return;
        }

        let physical = match self.resolve_physical_path(&path_str) {
            Some(p) => p,
            None => {
                self.ro_warn("mknod", &path_str);
                reply.error(libc::EROFS);
                return;
            }
        };

        let effective_mode = mode & !umask & 0o7777;
        use std::os::unix::ffi::OsStrExt as _;
        let c_path = match std::ffi::CString::new(physical.as_os_str().as_bytes()) {
            Ok(p) => p,
            Err(_) => {
                reply.error(libc::EINVAL);
                return;
            }
        };
        let rc = unsafe { libc::mkfifo(c_path.as_ptr(), effective_mode as libc::mode_t) };
        if rc != 0 {
            let e = std::io::Error::last_os_error();
            let err = errno(&e);
            warn!(op = "mknod", path = %path_str, error = %e, "mkfifo failed");
            self.emit_op_event(
                req,
                &path_type,
                SkillEventKind::Create,
                SkillEventAction::Failed,
                Some(err),
                None,
            );
            reply.error(err);
            return;
        }

        let ino = self.inodes.allocate(&path_str, FileType::NamedPipe, parent);
        let attr = match std::fs::symlink_metadata(&physical) {
            Ok(meta) => {
                let mut a = file_attr_from_metadata(&meta);
                a.ino = ino;
                a
            }
            Err(_) => {
                let mut a = self.virtual_file_attr(0);
                a.kind = FileType::NamedPipe;
                a.ino = ino;
                a
            }
        };
        self.emit_op_event(
            req,
            &path_type,
            SkillEventKind::Create,
            SkillEventAction::Allowed,
            None,
            None,
        );
        reply.entry(&Duration::from_secs(1), &attr, 0);
    }

    fn unlink(&mut self, req: &Request, parent: u64, name: &std::ffi::OsStr, reply: ReplyEmpty) {
        let path_str = match self.build_fuse_path(parent, name) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let path_type = parse_path(Path::new(&path_str), self.in_place);
        let (skill_name_for_event, relative_for_event) = match &path_type {
            PathType::Passthrough {
                skill_name,
                relative_path,
            } => (Some(skill_name.clone()), Some(relative_path.clone())),
            PathType::SkillMd { skill_name } => {
                (Some(skill_name.clone()), Some(PathBuf::from("SKILL.md")))
            }
            PathType::SkillDir { skill_name } => (Some(skill_name.clone()), None),
            _ => (None, None),
        };

        // S3: refuse to unlink under a reserved lifecycle namespace.
        if let Some(errno) =
            self.enforce_lifecycle_reservation(&path_type, SkillEventKind::Delete, req, None)
        {
            reply.error(errno);
            return;
        }

        // S1: refuse to unlink anything under `.skill-meta/**`.
        if let Some(errno) = self.enforce_skill_meta(&path_type, SkillEventKind::Delete, req, None)
        {
            reply.error(errno);
            return;
        }

        let physical = match self.resolve_physical_path(&path_str) {
            Some(p) => p,
            None => {
                self.ro_warn("unlink", &path_str);
                self.emit_event(
                    SkillEvent::new(SkillEventKind::Delete)
                        .with_optional_skill_name(skill_name_for_event)
                        .with_optional_relative_path(relative_for_event)
                        .with_action(SkillEventAction::Rejected)
                        .with_errno(libc::EROFS)
                        .with_caller(req.uid(), req.gid()),
                );
                reply.error(libc::EROFS);
                return;
            }
        };

        debug!(parent, name = %name.to_string_lossy(), ?physical, "unlink");

        let unlink_result = match std::fs::remove_file(&physical) {
            Ok(()) => Ok(()),
            Err(e) if e.raw_os_error() == Some(libc::ENAMETOOLONG) => {
                match self.open_parent_dir_for(&path_str) {
                    Ok((parent_fd, leaf)) => unlinkat_leaf(&parent_fd, &leaf, 0),
                    Err(_) => Err(e),
                }
            }
            Err(e) => Err(e),
        };
        match unlink_result {
            Ok(()) => {
                // Remove inode mapping.
                if let Some(ino) = self.inodes.lookup_by_path(&path_str) {
                    self.inodes.remove(ino);
                }
                // Fast-path store sync: if deleting SKILL.md, remove from store.
                if let PathType::SkillMd { skill_name } = &path_type {
                    self.store.write().remove(skill_name);
                    info!(name = %skill_name, "sync: removed skill (SKILL.md deleted)");
                }
                self.emit_event(
                    SkillEvent::new(SkillEventKind::Delete)
                        .with_optional_skill_name(skill_name_for_event)
                        .with_optional_relative_path(relative_for_event)
                        .with_action(SkillEventAction::Allowed)
                        .with_caller(req.uid(), req.gid()),
                );
                reply.ok();
            }
            Err(e) => {
                warn!(op = "unlink", path = %path_str, error = %e, "unlink failed");
                let err = errno(&e);
                self.emit_event(
                    SkillEvent::new(SkillEventKind::Delete)
                        .with_optional_skill_name(skill_name_for_event)
                        .with_optional_relative_path(relative_for_event)
                        .with_action(SkillEventAction::Failed)
                        .with_errno(err)
                        .with_caller(req.uid(), req.gid()),
                );
                reply.error(err);
            }
        }
    }

    fn rmdir(&mut self, req: &Request, parent: u64, name: &std::ffi::OsStr, reply: ReplyEmpty) {
        let path_str = match self.build_fuse_path(parent, name) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let path_type = parse_path(Path::new(&path_str), self.in_place);

        // S3: refuse to rmdir a reserved lifecycle namespace or any
        // directory beneath one. The gate fires before any physical
        // resolution so the source tree is untouched.
        if let Some(errno) =
            self.enforce_lifecycle_reservation(&path_type, SkillEventKind::Delete, req, None)
        {
            reply.error(errno);
            return;
        }

        // S1: refuse to remove `.skill-meta/**` directories.
        if let Some(errno) = self.enforce_skill_meta(&path_type, SkillEventKind::Delete, req, None)
        {
            reply.error(errno);
            return;
        }

        let physical = match self.resolve_physical_path(&path_str) {
            Some(p) => p,
            None => {
                self.ro_warn("rmdir", &path_str);
                reply.error(libc::EROFS);
                return;
            }
        };

        debug!(parent, name = %name.to_string_lossy(), ?physical, "rmdir");

        let rmdir_result = match std::fs::remove_dir(&physical) {
            Ok(()) => Ok(()),
            Err(e) if e.raw_os_error() == Some(libc::ENAMETOOLONG) => {
                match self.open_parent_dir_for(&path_str) {
                    Ok((parent_fd, leaf)) => unlinkat_leaf(&parent_fd, &leaf, libc::AT_REMOVEDIR),
                    Err(_) => Err(e),
                }
            }
            Err(e) => Err(e),
        };
        match rmdir_result {
            Ok(()) => {
                // Remove inode and all children.
                self.inodes.remove_recursive(&path_str);
                // Fast-path store sync: if removing a skill directory.
                if let PathType::SkillDir { skill_name } = path_type {
                    self.store.write().remove(&skill_name);
                    info!(name = %skill_name, "sync: removed skill (directory deleted)");
                }
                reply.ok();
            }
            Err(e) => {
                warn!(op = "rmdir", path = %path_str, error = %e, "rmdir failed");
                reply.error(errno(&e));
            }
        }
    }

    fn rename(
        &mut self,
        req: &Request,
        parent: u64,
        name: &std::ffi::OsStr,
        newparent: u64,
        newname: &std::ffi::OsStr,
        flags: u32,
        reply: ReplyEmpty,
    ) {
        // Phase 1 rename flag policy: only plain rename and `RENAME_NOREPLACE`
        // are supported. Any other bit (including `RENAME_EXCHANGE`,
        // `RENAME_WHITEOUT`, or unknown bits) must be rejected with `EINVAL`
        // so callers don't get a silent fall-through to plain rename.
        #[cfg(target_os = "linux")]
        const SUPPORTED_RENAME_FLAGS: u32 = libc::RENAME_NOREPLACE;
        #[cfg(not(target_os = "linux"))]
        const SUPPORTED_RENAME_FLAGS: u32 = 0;

        if flags & !SUPPORTED_RENAME_FLAGS != 0 {
            warn!(flags, "rename: rejecting unsupported flags");
            self.emit_event(
                SkillEvent::new(SkillEventKind::Rename)
                    .with_action(SkillEventAction::Failed)
                    .with_errno(libc::EINVAL)
                    .with_caller(req.uid(), req.gid())
                    .with_detail(format!("flags=0x{:x}", flags)),
            );
            reply.error(libc::EINVAL);
            return;
        }
        let no_replace = flags & SUPPORTED_RENAME_FLAGS != 0;

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
        let old_path_type = parse_path(Path::new(&old_path), self.in_place);
        let new_path_type = parse_path(Path::new(&new_path), self.in_place);
        let (event_skill, event_relative) = match &old_path_type {
            PathType::Passthrough {
                skill_name,
                relative_path,
            } => (Some(skill_name.clone()), Some(relative_path.clone())),
            PathType::SkillMd { skill_name } => {
                (Some(skill_name.clone()), Some(PathBuf::from("SKILL.md")))
            }
            PathType::SkillDir { skill_name } => (Some(skill_name.clone()), None),
            _ => (None, None),
        };

        // S3: reject renames that source from or target a reserved
        // lifecycle namespace. Both sides are checked before physical
        // resolution so the source remains untouched on rejection.
        if let Some(errno) = self.enforce_lifecycle_reservation(
            &old_path_type,
            SkillEventKind::Rename,
            req,
            Some(new_path.clone()),
        ) {
            reply.error(errno);
            return;
        }
        if let Some(errno) = self.enforce_lifecycle_reservation(
            &new_path_type,
            SkillEventKind::Rename,
            req,
            Some(new_path.clone()),
        ) {
            reply.error(errno);
            return;
        }

        // S1: refuse renames that move out of `.skill-meta/**` (mutates the
        // protected metadata directory) or into `.skill-meta/**` (creates a
        // new entry inside it). The from-side check fires before any
        // physical resolution so the source remains untouched.
        if let Some(errno) = self.enforce_skill_meta(
            &old_path_type,
            SkillEventKind::Rename,
            req,
            Some(new_path.clone()),
        ) {
            reply.error(errno);
            return;
        }
        if let Some(errno) = self.enforce_skill_meta(
            &new_path_type,
            SkillEventKind::Rename,
            req,
            Some(new_path.clone()),
        ) {
            reply.error(errno);
            return;
        }

        let old_physical = match self.resolve_physical_path(&old_path) {
            Some(p) => p,
            None => {
                self.ro_warn("rename", &old_path);
                self.emit_event(
                    SkillEvent::new(SkillEventKind::Rename)
                        .with_optional_skill_name(event_skill.clone())
                        .with_optional_relative_path(event_relative.clone())
                        .with_action(SkillEventAction::Rejected)
                        .with_errno(libc::EROFS)
                        .with_caller(req.uid(), req.gid())
                        .with_detail(new_path.clone()),
                );
                reply.error(libc::EROFS);
                return;
            }
        };
        let new_physical = match self.resolve_physical_path(&new_path) {
            Some(p) => p,
            None => {
                self.ro_warn("rename", &new_path);
                self.emit_event(
                    SkillEvent::new(SkillEventKind::Rename)
                        .with_optional_skill_name(event_skill.clone())
                        .with_optional_relative_path(event_relative.clone())
                        .with_action(SkillEventAction::Rejected)
                        .with_errno(libc::EROFS)
                        .with_caller(req.uid(), req.gid())
                        .with_detail(new_path.clone()),
                );
                reply.error(libc::EROFS);
                return;
            }
        };

        debug!(
            old = %old_path, new = %new_path,
            ?old_physical, ?new_physical,
            no_replace,
            "rename"
        );

        let rename_result = if no_replace {
            rename_noreplace(&old_physical, &new_physical)
        } else {
            std::fs::rename(&old_physical, &new_physical)
        };
        let rename_result = match rename_result {
            Ok(()) => Ok(()),
            Err(e) if e.raw_os_error() == Some(libc::ENAMETOOLONG) => {
                // Long-path fallback: rename via two parent dir fds + leafs.
                // Both sides may exceed PATH_MAX on the absolute physical
                // path even when each parent dir individually fits.
                let old_parent = self.open_parent_dir_for(&old_path);
                let new_parent = self.open_parent_dir_for(&new_path);
                match (old_parent, new_parent) {
                    (Ok((old_fd, old_leaf)), Ok((new_fd, new_leaf))) => {
                        let flags: u32 = if no_replace {
                            SUPPORTED_RENAME_FLAGS
                        } else {
                            0
                        };
                        renameat2_leaf(&old_fd, &old_leaf, &new_fd, &new_leaf, flags)
                    }
                    _ => Err(e),
                }
            }
            Err(e) => Err(e),
        };

        match rename_result {
            Ok(()) => {
                // Update inode mappings.
                self.inodes.rename_path(&old_path, &new_path);

                // Store sync for skill-level renames.
                let old_type = old_path_type.clone();
                let new_type = new_path_type.clone();
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

                self.emit_event(
                    SkillEvent::new(SkillEventKind::Rename)
                        .with_optional_skill_name(event_skill)
                        .with_optional_relative_path(event_relative)
                        .with_action(SkillEventAction::Allowed)
                        .with_caller(req.uid(), req.gid())
                        .with_detail(new_path.clone()),
                );
                reply.ok();
            }
            Err(e) => {
                warn!(
                    op = "rename", old = %old_path, new = %new_path,
                    error = %e, "rename failed"
                );
                let err = errno(&e);
                self.emit_event(
                    SkillEvent::new(SkillEventKind::Rename)
                        .with_optional_skill_name(event_skill)
                        .with_optional_relative_path(event_relative)
                        .with_action(SkillEventAction::Failed)
                        .with_errno(err)
                        .with_caller(req.uid(), req.gid())
                        .with_detail(new_path.clone()),
                );
                reply.error(err);
            }
        }
    }

    fn setattr(
        &mut self,
        req: &Request,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<fuser::TimeOrNow>,
        mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<std::time::SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<std::time::SystemTime>,
        _chgtime: Option<std::time::SystemTime>,
        _bkuptime: Option<std::time::SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        // NOTE: Permission enforcement for setattr mutations relies on the underlying
        // filesystem (kernel) rather than checking req.uid()/req.gid() in userspace.
        // This is acceptable for single-user FUSE mounts but may deviate from caller's
        // POSIX permission expectations under allow_other or privileged daemon scenarios.
        // Full per-caller permission emulation would require reimplementing the kernel's
        // permission model, which is deferred to a future hardening pass. The S1
        // `.skill-meta` policy still uses `req` for caller attribution in
        // `PolicyDenied` events.

        let path = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let path_type = parse_path(Path::new(&path), self.in_place);

        // Determine whether any mutation is requested.
        let has_mutation = size.is_some()
            || mode.is_some()
            || uid.is_some()
            || gid.is_some()
            || atime.is_some()
            || mtime.is_some();

        // Virtual paths: Root, SkillsDir, SkillDir, skill-discover
        match &path_type {
            PathType::Root | PathType::SkillsDir => {
                if has_mutation {
                    reply.error(libc::EROFS);
                } else {
                    reply.attr(&Duration::from_secs(1), &self.dir_attr());
                }
                return;
            }
            PathType::SkillDir { .. } => {
                // All skill directories treated as virtual read-only for metadata mutations
                if has_mutation {
                    reply.error(libc::EROFS);
                } else {
                    reply.attr(&Duration::from_secs(1), &self.dir_attr());
                }
                return;
            }
            PathType::SkillMd { skill_name } | PathType::Passthrough { skill_name, .. } => {
                if is_skill_discover_path(skill_name) {
                    if has_mutation {
                        reply.error(libc::EROFS);
                    } else {
                        // Return virtual file attr for skill-discover
                        match self.compiled_skill_md(skill_name) {
                            Some(compiled) => {
                                let attr = self.virtual_file_attr(compiled.len() as u64);
                                reply.attr(&Duration::from_secs(1), &attr);
                            }
                            None => reply.error(libc::ENOENT),
                        }
                    }
                    return;
                }
                // Non skill-discover: fall through to physical mutation
            }
            PathType::Invalid => {
                reply.error(libc::ENOENT);
                return;
            }
        }

        // S3: deny metadata mutations on a reserved lifecycle namespace.
        // SkillDir is already rejected with EROFS above; this gate covers
        // SkillMd and Passthrough paths whose top-level segment matches a
        // reserved name.
        if has_mutation {
            if let Some(errno) =
                self.enforce_lifecycle_reservation(&path_type, SkillEventKind::Metadata, req, None)
            {
                reply.error(errno);
                return;
            }
        }

        // S1: deny chmod/chown/utimens/truncate-size on `.skill-meta/**`.
        // Pure stat (no mutation requested) still succeeds via the physical
        // metadata fall-through below.
        if has_mutation {
            if let Some(errno) =
                self.enforce_skill_meta(&path_type, SkillEventKind::Metadata, req, None)
            {
                reply.error(errno);
                return;
            }
        }

        // Physical path handling
        let physical = match self.resolve_physical_path(&path) {
            Some(p) => p,
            None => {
                reply.error(libc::EROFS);
                return;
            }
        };

        debug!(ino, ?size, ?mode, ?uid, ?gid, ?physical, "setattr");

        // 1. Handle size (truncate) — preserve existing logic
        if let Some(new_size) = size {
            let open_result = match std::fs::OpenOptions::new().write(true).open(&physical) {
                Ok(f) => Ok(f),
                Err(e) if e.raw_os_error() == Some(libc::ENAMETOOLONG) => {
                    match self.open_parent_dir_for(&path) {
                        Ok((parent_fd, leaf)) => openat_leaf(&parent_fd, &leaf, libc::O_WRONLY, 0),
                        Err(_) => Err(e),
                    }
                }
                Err(e) => Err(e),
            };
            match open_result {
                Ok(f) => {
                    if let Err(e) = f.set_len(new_size) {
                        reply.error(errno(&e));
                        return;
                    }
                    // SKILL.md truncate triggers store reparse
                    if let PathType::SkillMd { ref skill_name } = path_type {
                        self.send_sync(SyncEvent::Reparse {
                            skill_name: skill_name.clone(),
                        });
                    }
                }
                Err(e) => {
                    reply.error(errno(&e));
                    return;
                }
            }
        }

        // 2. Handle mode (chmod)
        if let Some(new_mode) = mode {
            let perms = std::fs::Permissions::from_mode(new_mode);
            if let Err(e) = std::fs::set_permissions(&physical, perms) {
                reply.error(errno(&e));
                return;
            }
        }

        // 3. Handle uid/gid (chown)
        if uid.is_some() || gid.is_some() {
            let c_path = match std::ffi::CString::new(physical.to_string_lossy().into_owned()) {
                Ok(p) => p,
                Err(_) => {
                    reply.error(libc::EINVAL);
                    return;
                }
            };
            // -1 means "don't change" — on Linux (uid_t)-1 == u32::MAX
            let new_uid = uid.map(|u| u as libc::uid_t).unwrap_or(u32::MAX);
            let new_gid = gid.map(|g| g as libc::gid_t).unwrap_or(u32::MAX);
            let ret = unsafe { libc::chown(c_path.as_ptr(), new_uid, new_gid) };
            if ret != 0 {
                let e = std::io::Error::last_os_error();
                reply.error(errno(&e));
                return;
            }
        }

        // 4. Handle atime/mtime (utimensat)
        if atime.is_some() || mtime.is_some() {
            let c_path = match std::ffi::CString::new(physical.to_string_lossy().into_owned()) {
                Ok(p) => p,
                Err(_) => {
                    reply.error(libc::EINVAL);
                    return;
                }
            };

            let atime_spec = match atime {
                Some(fuser::TimeOrNow::Now) => libc::timespec {
                    tv_sec: 0,
                    tv_nsec: libc::UTIME_NOW,
                },
                Some(fuser::TimeOrNow::SpecificTime(t)) => {
                    match t.duration_since(UNIX_EPOCH) {
                        Ok(d) => libc::timespec {
                            tv_sec: d.as_secs() as i64,
                            tv_nsec: d.subsec_nanos() as i64,
                        },
                        Err(e) => {
                            // Pre-epoch time: negative seconds
                            let d = e.duration();
                            let mut sec = -(d.as_secs() as i64);
                            let mut nsec = -(d.subsec_nanos() as i64);
                            // Normalize: nsec should be non-negative for timespec
                            if nsec < 0 {
                                sec -= 1;
                                nsec += 1_000_000_000;
                            }
                            libc::timespec {
                                tv_sec: sec,
                                tv_nsec: nsec,
                            }
                        }
                    }
                }
                None => libc::timespec {
                    tv_sec: 0,
                    tv_nsec: libc::UTIME_OMIT,
                },
            };

            let mtime_spec = match mtime {
                Some(fuser::TimeOrNow::Now) => libc::timespec {
                    tv_sec: 0,
                    tv_nsec: libc::UTIME_NOW,
                },
                Some(fuser::TimeOrNow::SpecificTime(t)) => {
                    match t.duration_since(UNIX_EPOCH) {
                        Ok(d) => libc::timespec {
                            tv_sec: d.as_secs() as i64,
                            tv_nsec: d.subsec_nanos() as i64,
                        },
                        Err(e) => {
                            // Pre-epoch time: negative seconds
                            let d = e.duration();
                            let mut sec = -(d.as_secs() as i64);
                            let mut nsec = -(d.subsec_nanos() as i64);
                            // Normalize: nsec should be non-negative for timespec
                            if nsec < 0 {
                                sec -= 1;
                                nsec += 1_000_000_000;
                            }
                            libc::timespec {
                                tv_sec: sec,
                                tv_nsec: nsec,
                            }
                        }
                    }
                }
                None => libc::timespec {
                    tv_sec: 0,
                    tv_nsec: libc::UTIME_OMIT,
                },
            };

            let times = [atime_spec, mtime_spec];
            let ret =
                unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
            if ret != 0 {
                let e = std::io::Error::last_os_error();
                reply.error(errno(&e));
                return;
            }
        }

        // 5. Return updated attributes. Long-path fallback mirrors the
        // truncate branch above: if the absolute physical path exceeds
        // `PATH_MAX`, refetch via `fstatat` against the parent fd.
        // Without this, a successful truncate (which already changed the
        // on-disk size via openat fallback) would still reply with
        // `ENAMETOOLONG` here, and the kernel would surface that errno to
        // the caller while keeping the stale attr cache (`stat` after the
        // failed reply would still report the pre-truncate size).
        let final_attr: std::io::Result<FileAttr> = match std::fs::metadata(&physical) {
            Ok(meta) => Ok(file_attr_from_metadata(&meta)),
            Err(e) if e.raw_os_error() == Some(libc::ENAMETOOLONG) => {
                match self.open_parent_dir_for(&path) {
                    Ok((parent_fd, leaf)) => match fstatat_leaf(&parent_fd, &leaf, true) {
                        Ok(st) => Ok(file_attr_from_stat(&st)),
                        Err(e2) => Err(e2),
                    },
                    Err(_) => Err(e),
                }
            }
            Err(e) => Err(e),
        };
        match final_attr {
            Ok(mut attr) => {
                attr.ino = ino;
                // For SKILL.md, override size with compiled content length (consistent with getattr)
                if let PathType::SkillMd { ref skill_name } = path_type {
                    if let Some(compiled) = self.compiled_skill_md(skill_name) {
                        attr.size = compiled.len() as u64;
                    }
                }
                if has_mutation {
                    self.emit_op_event(
                        req,
                        &path_type,
                        SkillEventKind::Metadata,
                        SkillEventAction::Allowed,
                        None,
                        size,
                    );
                }
                reply.attr(&Duration::from_secs(1), &attr);
            }
            Err(e) => {
                let err = errno(&e);
                if has_mutation {
                    self.emit_op_event(
                        req,
                        &path_type,
                        SkillEventKind::Metadata,
                        SkillEventAction::Failed,
                        Some(err),
                        None,
                    );
                }
                reply.error(err);
            }
        }
    }

    fn readlink(&mut self, req: &Request, ino: u64, reply: ReplyData) {
        debug!(ino, "readlink");
        let path = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => return reply.error(libc::ENOENT),
        };

        match parse_path(Path::new(&path), self.in_place) {
            // Virtual directories are never symlinks; readlink on a
            // non-symlink returns EINVAL on Linux.
            PathType::Root | PathType::SkillsDir | PathType::SkillDir { .. } => {
                self.emit_event(
                    SkillEvent::new(SkillEventKind::Readlink)
                        .with_action(SkillEventAction::Failed)
                        .with_errno(libc::EINVAL)
                        .with_caller(req.uid(), req.gid()),
                );
                reply.error(libc::EINVAL);
            }
            // Compiled SKILL.md is a virtual regular file, not a symlink.
            PathType::SkillMd { skill_name } => {
                self.emit_event(
                    SkillEvent::new(SkillEventKind::Readlink)
                        .with_skill_name(skill_name)
                        .with_relative_path("SKILL.md")
                        .with_action(SkillEventAction::Failed)
                        .with_errno(libc::EINVAL)
                        .with_caller(req.uid(), req.gid()),
                );
                reply.error(libc::EINVAL);
            }
            PathType::Passthrough {
                skill_name,
                relative_path,
            } => {
                if is_skill_discover_path(&skill_name) {
                    // skill-discover virtual namespace contains no symlinks.
                    self.emit_event(
                        SkillEvent::new(SkillEventKind::Readlink)
                            .with_skill_name(&skill_name)
                            .with_relative_path(&relative_path)
                            .with_action(SkillEventAction::Failed)
                            .with_errno(libc::EINVAL)
                            .with_caller(req.uid(), req.gid()),
                    );
                    reply.error(libc::EINVAL);
                    return;
                }
                let physical = self.skill_physical_dir(&skill_name).join(&relative_path);
                match std::fs::read_link(&physical) {
                    Ok(target) => {
                        use std::os::unix::ffi::OsStrExt;
                        let bytes = target.as_os_str().as_bytes();
                        self.emit_event(
                            SkillEvent::new(SkillEventKind::Readlink)
                                .with_skill_name(&skill_name)
                                .with_relative_path(&relative_path)
                                .with_action(SkillEventAction::Allowed)
                                .with_bytes(bytes.len() as u64)
                                .with_caller(req.uid(), req.gid()),
                        );
                        reply.data(bytes);
                    }
                    Err(e) => {
                        let err = errno(&e);
                        self.emit_event(
                            SkillEvent::new(SkillEventKind::Readlink)
                                .with_skill_name(&skill_name)
                                .with_relative_path(&relative_path)
                                .with_action(SkillEventAction::Failed)
                                .with_errno(err)
                                .with_caller(req.uid(), req.gid()),
                        );
                        reply.error(err);
                    }
                }
            }
            PathType::Invalid => {
                self.emit_event(
                    SkillEvent::new(SkillEventKind::Readlink)
                        .with_action(SkillEventAction::Failed)
                        .with_errno(libc::ENOENT)
                        .with_caller(req.uid(), req.gid()),
                );
                reply.error(libc::ENOENT);
            }
        }
    }

    fn symlink(
        &mut self,
        req: &Request,
        parent: u64,
        link_name: &std::ffi::OsStr,
        target: &std::path::Path,
        reply: ReplyEntry,
    ) {
        let path_str = match self.build_fuse_path(parent, link_name) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let path_type = parse_path(Path::new(&path_str), self.in_place);
        let target_str = target.display().to_string();

        // Only Passthrough leaves under an ordinary skill may host a new
        // symlink. Virtual paths keep their existing virtual semantics,
        // which means SymlinkDir / SymlinkMd / Root / SkillsDir / Invalid
        // remain EROFS as in S0.
        let (skill_name, relative_path) = match &path_type {
            PathType::Passthrough {
                skill_name,
                relative_path,
            } => (skill_name.clone(), relative_path.clone()),
            _ => {
                self.ro_warn("symlink", &path_str);
                self.emit_event(
                    SkillEvent::new(SkillEventKind::SymlinkAttempt)
                        .with_action(SkillEventAction::Rejected)
                        .with_errno(libc::EROFS)
                        .with_caller(req.uid(), req.gid())
                        .with_detail(format!("class=virtual_link target={}", target_str)),
                );
                reply.error(libc::EROFS);
                return;
            }
        };

        // skill-discover is virtual and read-only regardless of the
        // physical layout, so refuse before any classifier work.
        if is_skill_discover_path(&skill_name) {
            self.emit_event(
                SkillEvent::new(SkillEventKind::SymlinkAttempt)
                    .with_skill_name(&skill_name)
                    .with_relative_path(&relative_path)
                    .with_action(SkillEventAction::Rejected)
                    .with_errno(libc::EROFS)
                    .with_caller(req.uid(), req.gid())
                    .with_detail(format!("class=skill_discover target={}", target_str)),
            );
            reply.error(libc::EROFS);
            return;
        }

        // S3 lifecycle namespace and S1 `.skill-meta` gates apply to the
        // link path itself before any physical resolution.
        if let Some(errno) = self.enforce_lifecycle_reservation(
            &path_type,
            SkillEventKind::SymlinkAttempt,
            req,
            Some(format!("target={}", target_str)),
        ) {
            reply.error(errno);
            return;
        }
        if let Some(errno) = self.enforce_skill_meta(
            &path_type,
            SkillEventKind::SymlinkAttempt,
            req,
            Some(format!("target={}", target_str)),
        ) {
            reply.error(errno);
            return;
        }

        // T2 default policy: only **relative** same-skill symlink targets
        // are accepted. Absolute targets are rejected even when they land
        // inside the same skill — in non-in-place mounts an absolute
        // `<source>/<skill>/...` target points at the *physical* source
        // path, so following the link from userspace bypasses the FUSE
        // layer and any audit/policy enforcement attached to it. A
        // future package may relax this for `--security-mode` /
        // in-place-only mounts where the resolved path still flows
        // through SkillFS. Until then, refuse with `EACCES` so callers
        // get a deterministic, audited rejection rather than a silent
        // bypass.
        if target.is_absolute() {
            warn!(
                op = "symlink",
                link = %path_str,
                target = %target_str,
                "absolute symlink target rejected (T2 default policy)"
            );
            self.emit_event(
                SkillEvent::new(SkillEventKind::SymlinkAttempt)
                    .with_skill_name(&skill_name)
                    .with_relative_path(&relative_path)
                    .with_action(SkillEventAction::Rejected)
                    .with_errno(libc::EACCES)
                    .with_caller(req.uid(), req.gid())
                    .with_detail(format!(
                        "class=absolute_target_disallowed target={}",
                        target_str
                    )),
            );
            reply.error(libc::EACCES);
            return;
        }

        // Lexical target boundary classification (Package I helper). The
        // classifier needs an absolute source root and absolute link
        // parent in the same coordinate system; `self.source` (the real
        // source path, not the `/proc/self/fd/{n}` proxy) is what user
        // space sees when it constructs an absolute target, so we use it
        // for both. Relative targets are resolved against the parent of
        // the link path.
        let source_root = self
            .source
            .canonicalize()
            .unwrap_or_else(|_| self.source.clone());
        let link_parent_for_classifier = source_root
            .join(&skill_name)
            .join(relative_path.parent().unwrap_or(Path::new("")));
        let store_guard = self.store.read();
        let known_skill_names: Vec<&str> = store_guard.list();
        let class = symlink_policy::classify_symlink_target(
            &source_root,
            &skill_name,
            &known_skill_names,
            &link_parent_for_classifier,
            target,
        );
        drop(store_guard);

        let class_label = symlink_class_label(&class);
        if !matches!(class, symlink_policy::SymlinkTargetClass::SameSkill) {
            warn!(
                op = "symlink",
                link = %path_str,
                target = %target_str,
                class = class_label,
                "symlink target boundary check rejected"
            );
            self.emit_event(
                SkillEvent::new(SkillEventKind::SymlinkAttempt)
                    .with_skill_name(&skill_name)
                    .with_relative_path(&relative_path)
                    .with_action(SkillEventAction::Rejected)
                    .with_errno(libc::EACCES)
                    .with_caller(req.uid(), req.gid())
                    .with_detail(format!("class={} target={}", class_label, target_str)),
            );
            reply.error(libc::EACCES);
            return;
        }

        // Even when the target classifies as SameSkill, refuse if the
        // lexical resolution lands inside `.skill-meta/**` or under any
        // lifecycle reserved root (`.staging`, `.certified`,
        // `.quarantine`, `.archive`). The link path itself is gated
        // earlier, but a same-skill target could still point a fresh
        // link at protected metadata or a hidden lifecycle namespace —
        // following such a link from userspace would expose the
        // protected payload to readers that only see the unprotected
        // link path.
        let link_relative_parent = relative_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(PathBuf::new);
        if let Some(target_in_skill) =
            symlink_policy::resolve_same_skill_relative(&link_relative_parent, target)
        {
            let first_component = target_in_skill.components().next().and_then(|c| match c {
                std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
                _ => None,
            });
            let lands_in_skill_meta = security::is_skill_meta_path(&target_in_skill);
            let lands_in_lifecycle = first_component
                .as_deref()
                .map(is_reserved_lifecycle_name)
                .unwrap_or(false);
            if lands_in_skill_meta || lands_in_lifecycle {
                let sensitive_label = if lands_in_skill_meta {
                    "same_skill_sensitive_target_skill_meta"
                } else {
                    "same_skill_sensitive_target_lifecycle"
                };
                warn!(
                    op = "symlink",
                    link = %path_str,
                    target = %target_str,
                    resolved = %target_in_skill.display(),
                    class = sensitive_label,
                    "same-skill symlink target lands in protected namespace; rejected"
                );
                self.emit_event(
                    SkillEvent::new(SkillEventKind::SymlinkAttempt)
                        .with_skill_name(&skill_name)
                        .with_relative_path(&relative_path)
                        .with_action(SkillEventAction::Rejected)
                        .with_errno(libc::EACCES)
                        .with_caller(req.uid(), req.gid())
                        .with_detail(format!(
                            "class={} target={} resolved={}",
                            sensitive_label,
                            target_str,
                            target_in_skill.display()
                        )),
                );
                reply.error(libc::EACCES);
                return;
            }
        }

        let physical = match self.resolve_physical_path(&path_str) {
            Some(p) => p,
            None => {
                reply.error(libc::EROFS);
                return;
            }
        };

        match std::os::unix::fs::symlink(target, &physical) {
            Ok(()) => {
                let ino = self.inodes.allocate(&path_str, FileType::Symlink, parent);
                let attr = match std::fs::symlink_metadata(&physical) {
                    Ok(meta) => {
                        let mut a = file_attr_from_metadata(&meta);
                        a.ino = ino;
                        a
                    }
                    Err(_) => {
                        let mut a = self.virtual_file_attr(0);
                        a.kind = FileType::Symlink;
                        a.ino = ino;
                        a
                    }
                };
                self.emit_event(
                    SkillEvent::new(SkillEventKind::SymlinkAttempt)
                        .with_skill_name(&skill_name)
                        .with_relative_path(&relative_path)
                        .with_action(SkillEventAction::Allowed)
                        .with_caller(req.uid(), req.gid())
                        .with_detail(format!("class={} target={}", class_label, target_str)),
                );
                reply.entry(&Duration::from_secs(1), &attr, 0);
            }
            Err(e) => {
                let err = errno(&e);
                warn!(op = "symlink", link = %path_str, target = %target_str, error = %e, "symlink failed");
                self.emit_event(
                    SkillEvent::new(SkillEventKind::SymlinkAttempt)
                        .with_skill_name(&skill_name)
                        .with_relative_path(&relative_path)
                        .with_action(SkillEventAction::Failed)
                        .with_errno(err)
                        .with_caller(req.uid(), req.gid())
                        .with_detail(format!("class={} target={}", class_label, target_str)),
                );
                reply.error(err);
            }
        }
    }

    fn link(
        &mut self,
        req: &Request,
        ino: u64,
        newparent: u64,
        newname: &std::ffi::OsStr,
        reply: ReplyEntry,
    ) {
        let new_path_str = match self.build_fuse_path(newparent, newname) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let new_path_type = parse_path(Path::new(&new_path_str), self.in_place);

        // Resolve the source FUSE path from its inode. Without a path
        // mapping we cannot reason about same-skill / cross-skill — the
        // kernel only handed us a number, and the policy decision is
        // boundary-by-name. Bail with ENOENT in that case so audit logs
        // can tell it apart from a refused-by-policy outcome.
        let source_path_str = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => {
                self.emit_event(
                    SkillEvent::new(SkillEventKind::HardlinkAttempt)
                        .with_action(SkillEventAction::Rejected)
                        .with_errno(libc::ENOENT)
                        .with_caller(req.uid(), req.gid())
                        .with_detail(format!("dst={} src_ino={} unmapped", new_path_str, ino)),
                );
                reply.error(libc::ENOENT);
                return;
            }
        };
        let source_path_type = parse_path(Path::new(&source_path_str), self.in_place);

        // Destination must be a passthrough leaf.
        let (dst_skill, dst_rel) = match &new_path_type {
            PathType::Passthrough {
                skill_name,
                relative_path,
            } => (skill_name.clone(), relative_path.clone()),
            _ => {
                self.ro_warn("link", &new_path_str);
                self.emit_event(
                    SkillEvent::new(SkillEventKind::HardlinkAttempt)
                        .with_action(SkillEventAction::Rejected)
                        .with_errno(libc::EROFS)
                        .with_caller(req.uid(), req.gid())
                        .with_detail(format!(
                            "src={} dst={} class=virtual_dst",
                            source_path_str, new_path_str
                        )),
                );
                reply.error(libc::EROFS);
                return;
            }
        };

        // Source must also be a passthrough leaf (not SKILL.md, not a
        // virtual /skills entry). Hardlinks pointing at a virtual file
        // would either pin compiled content to a real inode or duplicate
        // a virtual file that has no on-disk identity.
        let (src_skill, src_rel) = match &source_path_type {
            PathType::Passthrough {
                skill_name,
                relative_path,
            } => (skill_name.clone(), relative_path.clone()),
            _ => {
                self.emit_event(
                    SkillEvent::new(SkillEventKind::HardlinkAttempt)
                        .with_skill_name(&dst_skill)
                        .with_relative_path(&dst_rel)
                        .with_action(SkillEventAction::Rejected)
                        .with_errno(libc::EPERM)
                        .with_caller(req.uid(), req.gid())
                        .with_detail(format!(
                            "src={} dst={} class=virtual_src",
                            source_path_str, new_path_str
                        )),
                );
                reply.error(libc::EPERM);
                return;
            }
        };

        if is_skill_discover_path(&src_skill) || is_skill_discover_path(&dst_skill) {
            self.emit_event(
                SkillEvent::new(SkillEventKind::HardlinkAttempt)
                    .with_skill_name(&dst_skill)
                    .with_relative_path(&dst_rel)
                    .with_action(SkillEventAction::Rejected)
                    .with_errno(libc::EROFS)
                    .with_caller(req.uid(), req.gid())
                    .with_detail(format!(
                        "src={} dst={} class=skill_discover",
                        source_path_str, new_path_str
                    )),
            );
            reply.error(libc::EROFS);
            return;
        }

        if src_skill != dst_skill {
            warn!(
                op = "link",
                src = %source_path_str,
                dst = %new_path_str,
                src_skill = %src_skill,
                dst_skill = %dst_skill,
                "cross-skill hardlink rejected"
            );
            self.emit_event(
                SkillEvent::new(SkillEventKind::HardlinkAttempt)
                    .with_skill_name(&dst_skill)
                    .with_relative_path(&dst_rel)
                    .with_action(SkillEventAction::Rejected)
                    .with_errno(libc::EACCES)
                    .with_caller(req.uid(), req.gid())
                    .with_detail(format!(
                        "src={} dst={} class=cross_skill src_skill={}",
                        source_path_str, new_path_str, src_skill
                    )),
            );
            reply.error(libc::EACCES);
            return;
        }

        // Lifecycle reservation on the destination link path.
        if let Some(errno) = self.enforce_lifecycle_reservation(
            &new_path_type,
            SkillEventKind::HardlinkAttempt,
            req,
            Some(format!("src={}", source_path_str)),
        ) {
            reply.error(errno);
            return;
        }
        // `.skill-meta` gate on the destination (link must not appear
        // under `.skill-meta`).
        if let Some(errno) = self.enforce_skill_meta(
            &new_path_type,
            SkillEventKind::HardlinkAttempt,
            req,
            Some(format!("src={}", source_path_str)),
        ) {
            reply.error(errno);
            return;
        }
        // `.skill-meta` gate on the source — hardlinking a protected
        // file out from under `.skill-meta` would leak the inode under
        // an unprotected name, so refuse before touching the filesystem.
        if let Some(errno) = self.enforce_skill_meta(
            &source_path_type,
            SkillEventKind::HardlinkAttempt,
            req,
            Some(format!("dst={}", new_path_str)),
        ) {
            reply.error(errno);
            return;
        }

        let src_physical = self.source_base().join(&src_skill).join(&src_rel);
        let dst_physical = self.source_base().join(&dst_skill).join(&dst_rel);

        // T2 hardlink scope: same-skill **ordinary regular files only**.
        // `symlink_metadata` deliberately does NOT follow symlinks, so a
        // symlink source surfaces as `is_symlink()` and is refused
        // here rather than being silently followed to its target. Every
        // non-regular kind (directory, symlink, FIFO, socket, block /
        // char device, or any other special file) is rejected with
        // `EPERM` and a `class=non_regular_source` audit event so
        // operators can tell the rejection apart from an unimplemented
        // surface.  `ENOENT` and other stat errors fall through to a
        // `Failed` event preserving the underlying errno.
        match std::fs::symlink_metadata(&src_physical) {
            Ok(meta) if meta.file_type().is_file() => {
                // OK — proceed to `hard_link` below.
            }
            Ok(_) => {
                warn!(
                    op = "link",
                    src = %source_path_str,
                    dst = %new_path_str,
                    "non-regular hardlink source rejected (T2 scope)"
                );
                self.emit_event(
                    SkillEvent::new(SkillEventKind::HardlinkAttempt)
                        .with_skill_name(&dst_skill)
                        .with_relative_path(&dst_rel)
                        .with_action(SkillEventAction::Rejected)
                        .with_errno(libc::EPERM)
                        .with_caller(req.uid(), req.gid())
                        .with_detail(format!(
                            "src={} dst={} class=non_regular_source",
                            source_path_str, new_path_str
                        )),
                );
                reply.error(libc::EPERM);
                return;
            }
            Err(e) => {
                let err = errno(&e);
                self.emit_event(
                    SkillEvent::new(SkillEventKind::HardlinkAttempt)
                        .with_skill_name(&dst_skill)
                        .with_relative_path(&dst_rel)
                        .with_action(SkillEventAction::Failed)
                        .with_errno(err)
                        .with_caller(req.uid(), req.gid())
                        .with_detail(format!(
                            "src={} dst={} class=src_stat_err",
                            source_path_str, new_path_str
                        )),
                );
                reply.error(err);
                return;
            }
        }

        match std::fs::hard_link(&src_physical, &dst_physical) {
            Ok(()) => {
                let dst_ino = self
                    .inodes
                    .allocate(&new_path_str, FileType::RegularFile, newparent);
                let attr = match std::fs::symlink_metadata(&dst_physical) {
                    Ok(meta) => {
                        let mut a = file_attr_from_metadata(&meta);
                        a.ino = dst_ino;
                        a
                    }
                    Err(_) => {
                        let mut a = self.virtual_file_attr(0);
                        a.ino = dst_ino;
                        a
                    }
                };
                self.emit_event(
                    SkillEvent::new(SkillEventKind::HardlinkAttempt)
                        .with_skill_name(&dst_skill)
                        .with_relative_path(&dst_rel)
                        .with_action(SkillEventAction::Allowed)
                        .with_caller(req.uid(), req.gid())
                        .with_detail(format!(
                            "src={} dst={} class=same_skill",
                            source_path_str, new_path_str
                        )),
                );
                reply.entry(&Duration::from_secs(1), &attr, 0);
            }
            Err(e) => {
                let err = errno(&e);
                warn!(op = "link", src = %source_path_str, dst = %new_path_str, error = %e, "hard_link failed");
                self.emit_event(
                    SkillEvent::new(SkillEventKind::HardlinkAttempt)
                        .with_skill_name(&dst_skill)
                        .with_relative_path(&dst_rel)
                        .with_action(SkillEventAction::Failed)
                        .with_errno(err)
                        .with_caller(req.uid(), req.gid())
                        .with_detail(format!(
                            "src={} dst={} class=same_skill",
                            source_path_str, new_path_str
                        )),
                );
                reply.error(err);
            }
        }
    }

    fn statfs(&mut self, _req: &Request, _ino: u64, reply: ReplyStatfs) {
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

    fn access(&mut self, req: &Request, ino: u64, mask: i32, reply: ReplyEmpty) {
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
            PathType::Root | PathType::SkillsDir => {
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

    fn fsyncdir(&mut self, _req: &Request, ino: u64, fh: u64, datasync: bool, reply: ReplyEmpty) {
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
            PathType::Root | PathType::SkillsDir => {
                reply.ok();
            }
            PathType::SkillDir { ref skill_name } => {
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

    // -----------------------------------------------------------------------
    // Extended attributes (Package T3 — minimal Linux passthrough)
    //
    // Only the `user.*` namespace is accepted for ordinary passthrough leaves
    // under a skill. `security.*`, `trusted.*`, `system.*`, and any unknown
    // namespace are rejected up-front with `EOPNOTSUPP` so SkillFS does not
    // become a back door for namespace categories whose policy lives in the
    // kernel/LSM and not in this filesystem.
    //
    // Virtual paths (root, `/skills`, skill dirs, compiled `SKILL.md`,
    // `skill-discover/SKILL.md`, and the lifecycle reserved roots) do not
    // persist xattrs. They return `EOPNOTSUPP` for every xattr surface so
    // callers see a deterministic, non-leaking answer regardless of whether
    // a physical backing path happens to exist.
    //
    // `.skill-meta/**` mutations route through the existing
    // `SkillMetaProtectionPolicy` gate via `enforce_skill_meta`, which emits a
    // `PolicyDenied` event and surfaces `EACCES`. Reads/list under
    // `.skill-meta/**` follow physical errno so administrators can still
    // inspect metadata xattrs through the mount.
    //
    // Physical passthrough goes through the no-follow xattr syscalls
    // (`lgetxattr` / `lsetxattr` / `llistxattr` / `lremovexattr`) to match
    // the `symlink_metadata`-based lookup/getattr behavior introduced in
    // Package I — a symlink leaf operates on the symlink's own xattrs rather
    // than silently following to the target.
    // -----------------------------------------------------------------------

    fn getxattr(
        &mut self,
        req: &Request,
        ino: u64,
        name: &std::ffi::OsStr,
        size: u32,
        reply: ReplyXattr,
    ) {
        let path = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => return reply.error(libc::ENOENT),
        };
        let path_type = parse_path(Path::new(&path), self.in_place);

        if !path_type_supports_xattr_passthrough(&path_type) {
            return reply.error(libc::EOPNOTSUPP);
        }
        if Self::lifecycle_reservation(&path_type).is_some() {
            return reply.error(libc::EOPNOTSUPP);
        }
        if matches!(xattr_namespace(name), XattrNamespace::Disallowed) {
            return reply.error(libc::EOPNOTSUPP);
        }

        let physical = match self.resolve_physical_path(&path) {
            Some(p) => p,
            None => return reply.error(libc::EOPNOTSUPP),
        };

        let res = xattr_lget(&physical, name, size as usize);
        match res {
            Ok(buf) => {
                if size == 0 {
                    reply.size(buf.len() as u32);
                } else {
                    reply.data(&buf);
                }
            }
            Err(err) => {
                let _ = req;
                reply.error(err);
            }
        }
    }

    fn listxattr(&mut self, req: &Request, ino: u64, size: u32, reply: ReplyXattr) {
        let _ = req;
        let path = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => return reply.error(libc::ENOENT),
        };
        let path_type = parse_path(Path::new(&path), self.in_place);

        if !path_type_supports_xattr_passthrough(&path_type) {
            return reply.error(libc::EOPNOTSUPP);
        }
        if Self::lifecycle_reservation(&path_type).is_some() {
            return reply.error(libc::EOPNOTSUPP);
        }

        let physical = match self.resolve_physical_path(&path) {
            Some(p) => p,
            None => return reply.error(libc::EOPNOTSUPP),
        };

        // Always fetch the full physical list first so we can filter to the
        // `user.*` namespace before honoring the caller-supplied `size`. The
        // filter is conservative — T3 only exposes `user.*`, so listing
        // anything else would contradict the get/set/remove namespace gate.
        let full = match xattr_llist(&physical) {
            Ok(v) => v,
            Err(err) => return reply.error(err),
        };
        let filtered = filter_user_xattr_list(&full);

        if size == 0 {
            reply.size(filtered.len() as u32);
        } else if (filtered.len() as u32) > size {
            reply.error(libc::ERANGE);
        } else {
            reply.data(&filtered);
        }
    }

    fn setxattr(
        &mut self,
        req: &Request,
        ino: u64,
        name: &std::ffi::OsStr,
        value: &[u8],
        flags: i32,
        _position: u32,
        reply: ReplyEmpty,
    ) {
        let path = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => return reply.error(libc::ENOENT),
        };
        let path_type = parse_path(Path::new(&path), self.in_place);

        if !path_type_supports_xattr_passthrough(&path_type) {
            self.emit_xattr_event(
                req,
                &path_type,
                "set",
                name,
                SkillEventAction::Rejected,
                Some(libc::EOPNOTSUPP),
                Some("virtual_xattr_path"),
            );
            return reply.error(libc::EOPNOTSUPP);
        }
        if let Some(errno) =
            self.enforce_lifecycle_reservation(&path_type, SkillEventKind::Metadata, req, None)
        {
            return reply.error(errno);
        }
        if let Some(errno) =
            self.enforce_skill_meta(&path_type, SkillEventKind::Metadata, req, None)
        {
            return reply.error(errno);
        }
        if matches!(xattr_namespace(name), XattrNamespace::Disallowed) {
            self.emit_xattr_event(
                req,
                &path_type,
                "set",
                name,
                SkillEventAction::Rejected,
                Some(libc::EOPNOTSUPP),
                Some("unsupported_xattr_namespace"),
            );
            return reply.error(libc::EOPNOTSUPP);
        }

        let physical = match self.resolve_physical_path(&path) {
            Some(p) => p,
            None => {
                self.emit_xattr_event(
                    req,
                    &path_type,
                    "set",
                    name,
                    SkillEventAction::Rejected,
                    Some(libc::EOPNOTSUPP),
                    Some("unresolved_physical_path"),
                );
                return reply.error(libc::EOPNOTSUPP);
            }
        };

        match xattr_lset(&physical, name, value, flags) {
            Ok(()) => {
                self.emit_xattr_event(
                    req,
                    &path_type,
                    "set",
                    name,
                    SkillEventAction::Allowed,
                    None,
                    None,
                );
                reply.ok();
            }
            Err(err) => {
                self.emit_xattr_event(
                    req,
                    &path_type,
                    "set",
                    name,
                    SkillEventAction::Failed,
                    Some(err),
                    None,
                );
                reply.error(err);
            }
        }
    }

    fn removexattr(&mut self, req: &Request, ino: u64, name: &std::ffi::OsStr, reply: ReplyEmpty) {
        let path = match self.inodes.get_path(ino) {
            Some(p) => p,
            None => return reply.error(libc::ENOENT),
        };
        let path_type = parse_path(Path::new(&path), self.in_place);

        if !path_type_supports_xattr_passthrough(&path_type) {
            self.emit_xattr_event(
                req,
                &path_type,
                "remove",
                name,
                SkillEventAction::Rejected,
                Some(libc::EOPNOTSUPP),
                Some("virtual_xattr_path"),
            );
            return reply.error(libc::EOPNOTSUPP);
        }
        if let Some(errno) =
            self.enforce_lifecycle_reservation(&path_type, SkillEventKind::Metadata, req, None)
        {
            return reply.error(errno);
        }
        if let Some(errno) =
            self.enforce_skill_meta(&path_type, SkillEventKind::Metadata, req, None)
        {
            return reply.error(errno);
        }
        if matches!(xattr_namespace(name), XattrNamespace::Disallowed) {
            self.emit_xattr_event(
                req,
                &path_type,
                "remove",
                name,
                SkillEventAction::Rejected,
                Some(libc::EOPNOTSUPP),
                Some("unsupported_xattr_namespace"),
            );
            return reply.error(libc::EOPNOTSUPP);
        }

        let physical = match self.resolve_physical_path(&path) {
            Some(p) => p,
            None => {
                self.emit_xattr_event(
                    req,
                    &path_type,
                    "remove",
                    name,
                    SkillEventAction::Rejected,
                    Some(libc::EOPNOTSUPP),
                    Some("unresolved_physical_path"),
                );
                return reply.error(libc::EOPNOTSUPP);
            }
        };

        match xattr_lremove(&physical, name) {
            Ok(()) => {
                self.emit_xattr_event(
                    req,
                    &path_type,
                    "remove",
                    name,
                    SkillEventAction::Allowed,
                    None,
                    None,
                );
                reply.ok();
            }
            Err(err) => {
                self.emit_xattr_event(
                    req,
                    &path_type,
                    "remove",
                    name,
                    SkillEventAction::Failed,
                    Some(err),
                    None,
                );
                reply.error(err);
            }
        }
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

/// Extract raw OS error code, falling back to EIO.
fn errno(e: &std::io::Error) -> i32 {
    e.raw_os_error().unwrap_or(libc::EIO)
}

/// Rename `old` to `new` with Linux `RENAME_NOREPLACE` semantics: fail with
/// `EEXIST` if the target already exists, otherwise rename atomically.
///
/// Implemented via the `renameat2` syscall so the existence check and the
/// rename are performed atomically in the kernel — a userspace
/// "exists?-then-rename" pattern would race if a file appeared between the
/// two steps.
#[cfg(target_os = "linux")]
fn rename_noreplace(old: &Path, new: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let old_c = CString::new(old.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;
    let new_c = CString::new(new.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;

    let ret = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            old_c.as_ptr(),
            libc::AT_FDCWD,
            new_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if ret == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn rename_noreplace(_old: &Path, _new: &Path) -> std::io::Result<()> {
    Err(std::io::Error::from_raw_os_error(libc::ENOSYS))
}

// ---------------------------------------------------------------------------
// openat-family helpers for long-path passthrough operations.
//
// SkillFS callbacks normally call `std::fs::*` on the full absolute physical
// path (e.g. `<source>/<skill>/<sandbox>/<comp1>/.../<leaf>`). When the
// caller-supplied relative path approaches `PATH_MAX`, that absolute path can
// exceed the kernel's userspace limit and the daemon's `openat(AT_FDCWD,
// huge_path, …)` syscall fails with `ENAMETOOLONG` even though Linux would
// have accepted the operation against a shorter parent fd. These helpers let
// callbacks fall back to *at syscalls anchored at the parent directory: open
// the parent's physical path (which fits when the leaf alone would not), then
// pass only the leaf component to the syscall.
// ---------------------------------------------------------------------------

/// Open a directory by its absolute physical path with flags suitable for
/// passing the resulting fd to `*at` syscalls.
fn open_dir_path(path: &Path) -> std::io::Result<std::fs::File> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::FromRawFd;

    let c = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;
    let fd = unsafe {
        libc::open(
            c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: fd was just produced by open(2) and is owned exclusively here.
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

fn cstring_from_os_str(s: &std::ffi::OsStr) -> std::io::Result<std::ffi::CString> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    CString::new(s.as_bytes()).map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))
}

fn openat_leaf(
    dir: &std::fs::File,
    leaf: &std::ffi::OsStr,
    flags: i32,
    mode: u32,
) -> std::io::Result<std::fs::File> {
    use std::os::unix::io::{AsRawFd, FromRawFd};
    let c = cstring_from_os_str(leaf)?;
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            c.as_ptr(),
            flags | libc::O_CLOEXEC,
            mode as libc::c_uint,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

fn mkdirat_leaf(dir: &std::fs::File, leaf: &std::ffi::OsStr, mode: u32) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let c = cstring_from_os_str(leaf)?;
    let rc = unsafe { libc::mkdirat(dir.as_raw_fd(), c.as_ptr(), mode as libc::mode_t) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn unlinkat_leaf(dir: &std::fs::File, leaf: &std::ffi::OsStr, flags: i32) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let c = cstring_from_os_str(leaf)?;
    let rc = unsafe { libc::unlinkat(dir.as_raw_fd(), c.as_ptr(), flags) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn fstatat_leaf(
    dir: &std::fs::File,
    leaf: &std::ffi::OsStr,
    follow: bool,
) -> std::io::Result<libc::stat> {
    use std::os::unix::io::AsRawFd;
    let c = cstring_from_os_str(leaf)?;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let flags = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
    let rc = unsafe { libc::fstatat(dir.as_raw_fd(), c.as_ptr(), &mut st, flags) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(st)
}

#[cfg(target_os = "linux")]
fn renameat2_leaf(
    old_dir: &std::fs::File,
    old_leaf: &std::ffi::OsStr,
    new_dir: &std::fs::File,
    new_leaf: &std::ffi::OsStr,
    flags: u32,
) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let old_c = cstring_from_os_str(old_leaf)?;
    let new_c = cstring_from_os_str(new_leaf)?;
    let rc = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            old_dir.as_raw_fd(),
            old_c.as_ptr(),
            new_dir.as_raw_fd(),
            new_c.as_ptr(),
            flags,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn renameat2_leaf(
    _old_dir: &std::fs::File,
    _old_leaf: &std::ffi::OsStr,
    _new_dir: &std::fs::File,
    _new_leaf: &std::ffi::OsStr,
    _flags: u32,
) -> std::io::Result<()> {
    Err(std::io::Error::from_raw_os_error(libc::ENOSYS))
}

/// Convert a libc::stat to a fuser::FileAttr. Mirrors `file_attr_from_metadata`
/// for paths that were stat'd via `fstatat` instead of `symlink_metadata`.
fn file_attr_from_stat(st: &libc::stat) -> FileAttr {
    let mode = st.st_mode;
    let kind = match mode & libc::S_IFMT {
        libc::S_IFLNK => FileType::Symlink,
        libc::S_IFDIR => FileType::Directory,
        libc::S_IFBLK => FileType::BlockDevice,
        libc::S_IFCHR => FileType::CharDevice,
        libc::S_IFIFO => FileType::NamedPipe,
        libc::S_IFSOCK => FileType::Socket,
        _ => FileType::RegularFile,
    };
    FileAttr {
        ino: 0,
        size: st.st_size as u64,
        blocks: st.st_blocks as u64,
        atime: system_time_from_secs(st.st_atime, st.st_atime_nsec),
        mtime: system_time_from_secs(st.st_mtime, st.st_mtime_nsec),
        ctime: system_time_from_secs(st.st_ctime, st.st_ctime_nsec),
        crtime: UNIX_EPOCH,
        kind,
        perm: (mode & 0o7777) as u16,
        nlink: st.st_nlink as u32,
        uid: st.st_uid,
        gid: st.st_gid,
        rdev: st.st_rdev as u32,
        flags: 0,
        blksize: st.st_blksize as u32,
    }
}

fn system_time_from_secs(secs: i64, nsecs: i64) -> SystemTime {
    if secs >= 0 {
        UNIX_EPOCH + std::time::Duration::new(secs as u64, nsecs as u32)
    } else {
        UNIX_EPOCH - std::time::Duration::new((-secs) as u64, 0)
    }
}

/// Convert std::fs::Metadata to FUSE FileAttr.
///
/// `kind` is derived from `file_type()` so that symlink identity is preserved
/// when the caller supplies metadata from `symlink_metadata()`. Callers that
/// want symlink-following semantics should pass metadata from `metadata()`
/// instead — that path will set `is_symlink()` to `false` because the kernel
/// has already resolved the target.
fn file_attr_from_metadata(meta: &std::fs::Metadata) -> FileAttr {
    let kind = filetype_from_mode(meta.mode());
    FileAttr {
        ino: 0,
        size: meta.len(),
        blocks: meta.blocks(),
        atime: system_time_from_secs(meta.atime(), meta.atime_nsec()),
        mtime: system_time_from_secs(meta.mtime(), meta.mtime_nsec()),
        ctime: system_time_from_secs(meta.ctime(), meta.ctime_nsec()),
        crtime: meta.created().unwrap_or(UNIX_EPOCH),
        kind,
        perm: (meta.mode() & 0o7777) as u16,
        nlink: meta.nlink() as u32,
        uid: meta.uid(),
        gid: meta.gid(),
        rdev: meta.rdev() as u32,
        flags: 0,
        blksize: meta.blksize() as u32,
    }
}

/// Project a `std::fs::DirEntry`'s file type into the FUSE `FileType` we
/// expose in directory listings. Preserves symlink, FIFO, socket, and
/// device identity so callers see the same kind they would over a native
/// passthrough mount.
fn dir_entry_file_type(entry: &std::fs::DirEntry) -> FileType {
    match entry.metadata() {
        Ok(meta) => filetype_from_mode(meta.mode()),
        // `metadata()` here is `lstat`-style on `DirEntry`; fall back to
        // the cheaper `file_type()` if it failed (e.g. EACCES on the leaf
        // inode) so we still surface symlink / dir identity.
        Err(_) => match entry.file_type() {
            Ok(t) if t.is_dir() => FileType::Directory,
            Ok(t) if t.is_symlink() => FileType::Symlink,
            _ => FileType::RegularFile,
        },
    }
}

/// Map a POSIX mode word's `S_IFMT` bits to the corresponding FUSE
/// [`FileType`]. Centralized so `lookup`, `readdir`, and `mknod`'s reply
/// all agree on how special files (FIFO, socket, block/char device) are
/// reported.
fn filetype_from_mode(mode: u32) -> FileType {
    match mode & libc::S_IFMT {
        libc::S_IFLNK => FileType::Symlink,
        libc::S_IFDIR => FileType::Directory,
        libc::S_IFIFO => FileType::NamedPipe,
        libc::S_IFSOCK => FileType::Socket,
        libc::S_IFBLK => FileType::BlockDevice,
        libc::S_IFCHR => FileType::CharDevice,
        _ => FileType::RegularFile,
    }
}

// ---------------------------------------------------------------------------
// Extended-attribute helpers (Package T3)
// ---------------------------------------------------------------------------

/// Classification of an xattr name's namespace prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XattrNamespace {
    /// Belongs to the `user.` namespace.
    User,
    /// Disallowed: `security.`, `trusted.`, `system.`, missing namespace
    /// prefix, or any other namespace SkillFS does not pass through in T3.
    Disallowed,
}

fn xattr_namespace(name: &std::ffi::OsStr) -> XattrNamespace {
    use std::os::unix::ffi::OsStrExt;
    let bytes = name.as_bytes();
    if bytes.starts_with(b"user.") && bytes.len() > b"user.".len() {
        XattrNamespace::User
    } else {
        XattrNamespace::Disallowed
    }
}

/// Returns `true` for path types whose physical leaf can host an xattr in
/// T3 — only ordinary passthrough leaves under a non-`skill-discover` skill
/// qualify. Other path types are rejected before any libc work.
fn path_type_supports_xattr_passthrough(path_type: &PathType) -> bool {
    match path_type {
        PathType::Passthrough { skill_name, .. } => !is_skill_discover_path(skill_name),
        _ => false,
    }
}

/// Filter a null-separated list of xattr names (as produced by `llistxattr`)
/// to entries starting with `user.`. Returns a fresh null-separated buffer
/// suitable for the FUSE `listxattr` reply.
fn filter_user_xattr_list(raw: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for entry in raw.split(|b| *b == 0u8) {
        if entry.is_empty() {
            continue;
        }
        if entry.starts_with(b"user.") {
            out.extend_from_slice(entry);
            out.push(0u8);
        }
    }
    out
}

fn cstring_from_path(path: &Path) -> std::io::Result<std::ffi::CString> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))
}

fn cstring_from_xattr_name(name: &std::ffi::OsStr) -> std::io::Result<std::ffi::CString> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    CString::new(name.as_bytes()).map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))
}

/// `lgetxattr` wrapper. Returns the xattr value on success; on error
/// returns the underlying errno (or `EIO` as a fallback). When `size` is
/// `0` the function still allocates a one-byte probe so it can return the
/// real value length via a follow-up `lgetxattr(NULL, 0)` size query.
fn xattr_lget(path: &Path, name: &std::ffi::OsStr, size: usize) -> Result<Vec<u8>, i32> {
    let c_path = cstring_from_path(path).map_err(|e| errno(&e))?;
    let c_name = cstring_from_xattr_name(name).map_err(|e| errno(&e))?;
    let needed =
        unsafe { libc::lgetxattr(c_path.as_ptr(), c_name.as_ptr(), std::ptr::null_mut(), 0) };
    if needed < 0 {
        return Err(errno(&std::io::Error::last_os_error()));
    }
    let needed = needed as usize;
    if size == 0 {
        return Ok(vec![0u8; needed]); // length is what the caller wants
    }
    if needed > size {
        return Err(libc::ERANGE);
    }
    let mut buf = vec![0u8; needed];
    let got = unsafe {
        libc::lgetxattr(
            c_path.as_ptr(),
            c_name.as_ptr(),
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len(),
        )
    };
    if got < 0 {
        return Err(errno(&std::io::Error::last_os_error()));
    }
    buf.truncate(got as usize);
    Ok(buf)
}

/// `llistxattr` wrapper that returns the full physical null-separated name
/// list, sized via a probing call so the caller does not have to guess.
fn xattr_llist(path: &Path) -> Result<Vec<u8>, i32> {
    let c_path = cstring_from_path(path).map_err(|e| errno(&e))?;
    let needed = unsafe { libc::llistxattr(c_path.as_ptr(), std::ptr::null_mut(), 0) };
    if needed < 0 {
        return Err(errno(&std::io::Error::last_os_error()));
    }
    let needed = needed as usize;
    if needed == 0 {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u8; needed];
    let got = unsafe {
        libc::llistxattr(
            c_path.as_ptr(),
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
        )
    };
    if got < 0 {
        return Err(errno(&std::io::Error::last_os_error()));
    }
    buf.truncate(got as usize);
    Ok(buf)
}

/// `lsetxattr` wrapper. Preserves the kernel's `XATTR_CREATE` /
/// `XATTR_REPLACE` flag semantics.
fn xattr_lset(path: &Path, name: &std::ffi::OsStr, value: &[u8], flags: i32) -> Result<(), i32> {
    let c_path = cstring_from_path(path).map_err(|e| errno(&e))?;
    let c_name = cstring_from_xattr_name(name).map_err(|e| errno(&e))?;
    let rc = unsafe {
        libc::lsetxattr(
            c_path.as_ptr(),
            c_name.as_ptr(),
            value.as_ptr() as *const libc::c_void,
            value.len(),
            flags as libc::c_int,
        )
    };
    if rc != 0 {
        return Err(errno(&std::io::Error::last_os_error()));
    }
    Ok(())
}

/// `lremovexattr` wrapper.
fn xattr_lremove(path: &Path, name: &std::ffi::OsStr) -> Result<(), i32> {
    let c_path = cstring_from_path(path).map_err(|e| errno(&e))?;
    let c_name = cstring_from_xattr_name(name).map_err(|e| errno(&e))?;
    let rc = unsafe { libc::lremovexattr(c_path.as_ptr(), c_name.as_ptr()) };
    if rc != 0 {
        return Err(errno(&std::io::Error::last_os_error()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Symlink target boundary classifier
// ---------------------------------------------------------------------------

/// Pure helpers for reasoning about where a symlink's target lands relative
/// to the SkillFS source tree.
///
/// This module is **classification only** — no syscalls, no filesystem
/// access, no policy enforcement. It exists so future Skill Security work
/// (Package S0+) can route physical symlinks through a consistent boundary
/// check without coupling to the FUSE callbacks. Callers that need
/// filesystem-validated resolution must perform that separately.
/// Render a symlink classification verdict as a stable, structured label
/// suitable for log fields and audit `detail` strings. Mirrors the variant
/// names but stays in snake_case so log scrapers see a single token.
fn symlink_class_label(class: &symlink_policy::SymlinkTargetClass) -> &'static str {
    match class {
        symlink_policy::SymlinkTargetClass::SameSkill => "same_skill",
        symlink_policy::SymlinkTargetClass::CrossSkill { .. } => "cross_skill",
        symlink_policy::SymlinkTargetClass::InsideSourceOutsideSkill => {
            "inside_source_outside_skill"
        }
        symlink_policy::SymlinkTargetClass::OutsideSource => "outside_source",
        symlink_policy::SymlinkTargetClass::RelativeUnknown => "relative_unknown",
    }
}

pub mod symlink_policy {
    use std::path::{Component, Path, PathBuf};

    /// Where a symlink target lands once resolved against the link's parent
    /// directory.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum SymlinkTargetClass {
        /// Resolves to a path inside the link's own skill directory.
        SameSkill,
        /// Resolves into a different known skill directory under the same
        /// source. The first path component is reported as `other_skill`.
        CrossSkill { other_skill: String },
        /// Resolves inside the source tree, but not into any known skill
        /// directory (e.g. `skillfs-views.toml`, future `.skill-meta`,
        /// or a not-yet-loaded skill name).
        InsideSourceOutsideSkill,
        /// Resolves outside the source tree, either via an absolute target
        /// that does not start with `source_root` or via `..` components
        /// that escape the source root.
        OutsideSource,
        /// The classifier could not lexically resolve the target — for
        /// example a relative target combined with a non-absolute
        /// `link_parent`, an empty target, or a `..` past `/`.
        RelativeUnknown,
    }

    /// Classify `raw_target` lexically (no syscalls, no follow).
    ///
    /// * `source_root` — absolute, normalized path to the SkillFS source
    ///   directory.
    /// * `current_skill` — name of the skill that owns the link.
    /// * `known_skills` — names of skills currently loaded by the store.
    ///   Used to distinguish `CrossSkill` from `InsideSourceOutsideSkill`
    ///   when the target's first component is some other top-level name.
    /// * `link_parent` — absolute, normalized path to the directory that
    ///   contains the link file. Required to resolve relative targets.
    /// * `raw_target` — bytes returned by `readlink` on the link.
    pub fn classify_symlink_target(
        source_root: &Path,
        current_skill: &str,
        known_skills: &[&str],
        link_parent: &Path,
        raw_target: &Path,
    ) -> SymlinkTargetClass {
        if raw_target.as_os_str().is_empty() {
            return SymlinkTargetClass::RelativeUnknown;
        }

        let normalized_source = match normalize_lexical(source_root) {
            Some(p) if p.is_absolute() => p,
            _ => return SymlinkTargetClass::RelativeUnknown,
        };

        let resolved = if raw_target.is_absolute() {
            match normalize_lexical(raw_target) {
                Some(p) => p,
                None => return SymlinkTargetClass::OutsideSource,
            }
        } else {
            let normalized_parent = match normalize_lexical(link_parent) {
                Some(p) if p.is_absolute() => p,
                _ => return SymlinkTargetClass::RelativeUnknown,
            };
            match normalize_lexical(&normalized_parent.join(raw_target)) {
                Some(p) => p,
                None => return SymlinkTargetClass::OutsideSource,
            }
        };

        let after_source = match resolved.strip_prefix(&normalized_source) {
            Ok(rel) => rel,
            Err(_) => return SymlinkTargetClass::OutsideSource,
        };

        let mut comps = after_source.components();
        let first = match comps.next() {
            Some(Component::Normal(name)) => name,
            // Resolved path is the source root itself or has no leading
            // Normal component.
            Some(_) | None => return SymlinkTargetClass::InsideSourceOutsideSkill,
        };
        let first_str = first.to_string_lossy();

        if first_str == current_skill {
            SymlinkTargetClass::SameSkill
        } else if known_skills.iter().any(|s| *s == first_str) {
            SymlinkTargetClass::CrossSkill {
                other_skill: first_str.to_string(),
            }
        } else {
            SymlinkTargetClass::InsideSourceOutsideSkill
        }
    }

    /// Lexical normalization of `.` and `..` components without touching
    /// the filesystem. Returns `None` when `..` would escape the absolute
    /// root or when an unsupported `Prefix` component (Windows) is hit.
    fn normalize_lexical(path: &Path) -> Option<PathBuf> {
        let mut out: Vec<Component> = Vec::new();
        for c in path.components() {
            match c {
                Component::CurDir => {}
                Component::ParentDir => match out.last() {
                    Some(Component::Normal(_)) => {
                        out.pop();
                    }
                    Some(Component::RootDir) => return None,
                    Some(Component::Prefix(_)) => return None,
                    Some(Component::ParentDir) | Some(Component::CurDir) | None => {
                        out.push(Component::ParentDir);
                    }
                },
                other => out.push(other),
            }
        }
        Some(out.iter().collect())
    }

    /// Lexically resolve a relative symlink target inside its own skill and
    /// return the resulting path **relative to the skill root** when it
    /// stays inside that skill.
    ///
    /// * `link_relative_parent` — the parent directory of the link, expressed
    ///   relative to the link's skill (e.g. `sub` for a link at
    ///   `<skill>/sub/link`). May be empty for a link directly under the
    ///   skill root.
    /// * `raw_target` — the user-supplied target. Must be relative; absolute
    ///   targets are out of scope here (callers should reject them up-front).
    ///
    /// Returns `Some(p)` when the lexical resolution yields a path with at
    /// least one `Normal` component and never `..`-escapes the skill root.
    /// Returns `None` for absolute targets, empty results, or any `..`
    /// chain that walks above the skill root.
    ///
    /// Pure lexical: no filesystem access. Mirrors the resolution
    /// `classify_symlink_target` performs after stripping the source prefix
    /// and current-skill component, so callers can match the returned path
    /// against `.skill-meta` / lifecycle constants directly without a second
    /// classifier pass.
    pub fn resolve_same_skill_relative(
        link_relative_parent: &Path,
        raw_target: &Path,
    ) -> Option<PathBuf> {
        if raw_target.is_absolute() {
            return None;
        }
        let combined = link_relative_parent.join(raw_target);
        let mut out: Vec<Component> = Vec::new();
        for c in combined.components() {
            match c {
                Component::CurDir => {}
                Component::ParentDir => match out.last() {
                    Some(Component::Normal(_)) => {
                        out.pop();
                    }
                    // Any `..` past the start of `link_relative_parent`
                    // escapes the skill root — caller treats this as
                    // not-same-skill.
                    _ => return None,
                },
                Component::Normal(_) => out.push(c),
                Component::RootDir | Component::Prefix(_) => return None,
            }
        }
        if out.is_empty() {
            return None;
        }
        Some(out.iter().collect())
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Internal mount that accepts optional Skill Security overrides. Public
/// `mount` and `mount_background` keep their existing signatures and pass
/// `None` for both; test/embedder callers reach the sink/policy injection
/// path through [`mount_background_with_security`].
fn mount_inner(
    mountpoint: &Path,
    source: &Path,
    store: SharedSkillStore,
    options: MountOptions,
    in_place: bool,
    event_sink: Option<Arc<dyn SkillEventSink>>,
    policy: Option<Arc<dyn SecurityPolicy>>,
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

    let mut fs = SkillFs::new(
        mountpoint.to_path_buf(),
        source.to_path_buf(),
        store,
        in_place,
    );
    if let Some(sink) = event_sink {
        fs = fs.with_event_sink(sink);
    }
    if let Some(p) = policy {
        fs = fs.with_policy(p);
    }
    info!("starting FUSE filesystem");

    // Neutralize the daemon process's file-creation mask. The FUSE protocol
    // delivers the caller's umask to `create()` / `mkdir()` callbacks and we
    // apply it explicitly via `effective_mode = mode & !umask`; without this
    // call the daemon's own umask (typically `0o022` inherited from the shell
    // that started `skillfs mount`) would still mask the `mode` argument of
    // the daemon-side `openat`/`mkdirat`, double-masking and clamping bits
    // the caller actually requested. Linux's `umask(2)` is async-signal-safe
    // and always succeeds; we set it once here and leave it for the lifetime
    // of the FUSE event loop.
    //
    // In-process tests that need a non-zero umask wrap their own callers in
    // the `UmaskGuard` defined in
    // `crates/skillfs-fuse/tests/posix_create_mkdir_inode_tests.rs`, which
    // mutates the process umask under a serialization mutex; this startup
    // call merely sets the default daemon umask, not the test-time guard.
    #[cfg(target_family = "unix")]
    unsafe {
        libc::umask(0);
    }

    match fuser::mount2(fs, mountpoint, &fuse_opts) {
        Ok(()) => {
            info!("filesystem unmounted");
            Ok(())
        }
        Err(e) => Err(FuseError::MountFailed(e.to_string())),
    }
}

/// Mount the SkillFS FUSE filesystem (blocking).
pub fn mount(
    mountpoint: &Path,
    source: &Path,
    store: SharedSkillStore,
    options: MountOptions,
    in_place: bool,
) -> Result<(), FuseError> {
    mount_inner(mountpoint, source, store, options, in_place, None, None)
}

/// Mount the SkillFS FUSE filesystem (blocking) with optional Skill Security
/// overrides.
///
/// Both `event_sink` and `policy` default to the values used by
/// [`SkillFs::new`] when set to `None`; supplying `Some(...)` replaces them
/// before the FUSE event loop starts. This is the blocking analog of
/// [`mount_background_with_security`] and is the entry point CLI/operator
/// callers use when wiring runtime audit configuration through to the
/// mount.
pub fn mount_with_security(
    mountpoint: &Path,
    source: &Path,
    store: SharedSkillStore,
    options: MountOptions,
    in_place: bool,
    event_sink: Option<Arc<dyn SkillEventSink>>,
    policy: Option<Arc<dyn SecurityPolicy>>,
) -> Result<(), FuseError> {
    mount_inner(
        mountpoint, source, store, options, in_place, event_sink, policy,
    )
}

/// Mount in background (non-blocking).
pub fn mount_background(
    mountpoint: &Path,
    source: &Path,
    store: SharedSkillStore,
    options: MountOptions,
    in_place: bool,
) -> Result<MountHandle, FuseError> {
    mount_background_with_security(mountpoint, source, store, options, in_place, None, None)
}

/// Mount in background with optional Skill Security overrides.
///
/// Both `event_sink` and `policy` default to the values used by
/// [`SkillFs::new`] when set to `None`; supplying `Some(...)` replaces them
/// before the FUSE event loop starts. This is the entry point integration
/// tests use to capture audit events through a real mount without changing
/// any other call site.
pub fn mount_background_with_security(
    mountpoint: &Path,
    source: &Path,
    store: SharedSkillStore,
    options: MountOptions,
    in_place: bool,
    event_sink: Option<Arc<dyn SkillEventSink>>,
    policy: Option<Arc<dyn SecurityPolicy>>,
) -> Result<MountHandle, FuseError> {
    let mountpoint_path = mountpoint.to_path_buf();
    let source_path = source.to_path_buf();

    let handle = std::thread::spawn(move || {
        let mut opts = options;
        opts.foreground = true;
        if let Err(e) = mount_inner(
            &mountpoint_path,
            &source_path,
            store,
            opts,
            in_place,
            event_sink,
            policy,
        ) {
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
