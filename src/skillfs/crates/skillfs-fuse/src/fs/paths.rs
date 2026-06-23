//! Path-resolution and inode helpers for `SkillFs`.
//!
//! Covers the conversions between FUSE virtual paths, store keys, and
//! physical filesystem paths, plus the `*at`-syscall parent-fd opener
//! used to sidestep `PATH_MAX`. The base helper [`SkillFs::source_base`]
//! folds in the in-place mount's `/proc/self/fd/{n}` rewrite so every
//! downstream resolver naturally bypasses the FUSE over-mount.

use std::path::{Path, PathBuf};

use fuser::FUSE_ROOT_ID;

use super::SkillFs;
use crate::path::{PathType, is_skill_discover_path, parse_path};
use crate::security::{
    inbox::{is_inbox_dir_name, is_valid_inbox_skill_name},
    lifecycle::is_reserved_lifecycle_name,
};
use crate::sys::{errno, open_dir_path};

impl SkillFs {
    /// Return the base path for physical file access.
    ///
    /// In in-place mode returns `/proc/self/fd/{n}` (the pre-opened dirfd)
    /// so that reads bypass the FUSE mount layer.  Otherwise returns the
    /// plain source directory path.
    pub(super) fn source_base(&self) -> PathBuf {
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
    pub(super) fn skill_inode_path(&self, skill_name: &str) -> String {
        if self.in_place {
            format!("/{}", skill_name)
        } else {
            format!("/skills/{}", skill_name)
        }
    }

    /// Inode for the skills directory (the parent of individual skill dirs).
    pub(super) fn skills_dir_ino(&self) -> u64 {
        if self.in_place {
            FUSE_ROOT_ID
        } else {
            self.inodes.lookup_by_path("/skills").unwrap_or(2)
        }
    }

    /// Resolve the physical directory containing a skill's files.
    ///
    /// In in-place mode uses `source_base()` (the pre-opened fd path) so
    /// reads bypass the FUSE mount layer.
    pub(super) fn skill_physical_dir(&self, skill_name: &str) -> PathBuf {
        if self.in_place {
            // Always go through the fd to bypass the FUSE mount.
            self.source_base().join(skill_name)
        } else {
            self.skill_source_path(skill_name)
                .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                .unwrap_or_else(|| self.source.join(skill_name))
        }
    }

    /// Resolve the physical directory for an inbox skill candidate
    /// regardless of [`crate::security::ActiveSkillResolver`] visibility.
    /// The inbox is the install / repair entrance for hidden skills; it
    /// must keep working even when the runtime view at `/skills/<skill>`
    /// is hidden or pointed at a snapshot.
    pub(super) fn inbox_skill_dir(&self, skill_name: &str) -> PathBuf {
        self.source_base().join(skill_name)
    }

    /// Returns `true` when `name` is an acceptable inbox skill-name top
    /// segment.
    ///
    /// The inbox enumerates the source root and maps
    /// `/.skillfs-inbox/<name>` to `source/<name>`. To match the
    /// "inbox = skill install / repair entrance" contract — and to keep
    /// the namespace from leaking arbitrary source-root directories —
    /// we admit only names the canonical SkillFS validator
    /// (`skillfs-core::parser::validate_name`) would accept: kebab-case
    /// (`[a-z0-9-]+`), no leading/trailing hyphen, length ≤ 64. This
    /// rejects every dot-prefixed entry (`.git`, `.skill-meta`,
    /// `.skillfs-inbox`, lifecycle reserved roots, `.cache`, …) plus
    /// uppercase / underscored / dotted names that the loader would
    /// also refuse to surface as a skill. `skill-discover` happens to
    /// match the kebab-case shape, so it is rejected explicitly.
    pub(super) fn is_inbox_skill_name_allowed(name: &str) -> bool {
        if !is_valid_inbox_skill_name(name) {
            return false;
        }
        if is_skill_discover_path(name) {
            return false;
        }
        // Lifecycle reserved roots are dot-prefixed and already excluded
        // by `is_valid_inbox_skill_name`, but keep the explicit check so
        // a future relaxation cannot accidentally let `.staging` through.
        if is_reserved_lifecycle_name(name) {
            return false;
        }
        // Same defense in depth for the inbox name itself.
        if is_inbox_dir_name(name) {
            return false;
        }
        true
    }

    /// Resolve the physical SKILL.md path for a skill via the store.
    pub(super) fn skill_source_path(&self, skill_name: &str) -> Option<PathBuf> {
        let store = self.store.read();
        store.get(skill_name).map(|e| e.source_path.clone())
    }

    /// Return the list of skill names to show in /skills (default view).
    ///
    /// If views config is present, returns the default view's skills
    /// (filtered to those actually in the store). Otherwise returns all skills.
    pub(super) fn primary_skill_names(&self) -> Vec<String> {
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

    #[allow(dead_code)]
    pub(super) fn skill_physical_path(&self, skill_name: &str) -> Option<PathBuf> {
        let store = self.store.read();
        let entry = store.get(skill_name)?;
        Some(entry.source_path.parent()?.to_path_buf())
    }

    /// Build the canonical FUSE path from a parent inode and child name.
    pub(super) fn build_fuse_path(&self, parent: u64, name: &std::ffi::OsStr) -> Option<String> {
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
    pub(super) fn resolve_physical_path(&self, fuse_path: &str) -> Option<PathBuf> {
        match parse_path(Path::new(fuse_path), self.in_place) {
            PathType::SkillDir { skill_name } => Some(self.source_base().join(&skill_name)),
            PathType::SkillMd { skill_name } => {
                Some(self.source_base().join(&skill_name).join("SKILL.md"))
            }
            PathType::Passthrough {
                skill_name,
                relative_path,
            } => Some(self.source_base().join(&skill_name).join(&relative_path)),
            // L1 inbox virtual mapping: `<inbox>/<skill>/<rel>` shares
            // the physical layout with the live source candidate
            // directory. The inbox root itself has no physical backing
            // (it is purely virtual).
            PathType::InboxSkillDir { skill_name } => Some(self.source_base().join(&skill_name)),
            PathType::InboxPassthrough {
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
    pub(super) fn open_parent_dir_for(
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
            PathType::SkillDir { skill_name } | PathType::InboxSkillDir { skill_name } => {
                self.source_base().join(&skill_name)
            }
            PathType::SkillMd { skill_name } => {
                self.source_base().join(&skill_name).join("SKILL.md")
            }
            PathType::Passthrough {
                skill_name,
                relative_path,
            }
            | PathType::InboxPassthrough {
                skill_name,
                relative_path,
            } => self.source_base().join(&skill_name).join(&relative_path),
            PathType::SkillsDir | PathType::Root | PathType::InboxDir => self.source_base(),
            PathType::Invalid => return Err(libc::ENOTDIR),
        };
        let _ = parent_fuse_str; // suppress unused-binding warning when tracing is off
        open_dir_path(&parent_physical)
            .map(|f| (f, leaf))
            .map_err(|e| errno(&e))
    }

    pub(super) fn is_staging_skill_root(&self, skill_name: &str) -> bool {
        self.staging_matcher
            .as_ref()
            .is_some_and(|m| m.is_staging_root(skill_name))
    }

    pub(super) fn is_pending_install(&self, skill_name: &str) -> bool {
        let ctrl = match self.pending_install_controller.as_ref() {
            Some(c) => c,
            None => return false,
        };
        if !ctrl.is_pending(skill_name) {
            return false;
        }
        if let Some(ref resolver) = self.active_resolver {
            if resolver.get(skill_name).is_some() {
                ctrl.clear_if_activated(skill_name);
                return false;
            }
        }
        true
    }

    pub(super) fn should_reject_hidden_write(
        &self,
        skill_name: &str,
        relative_path: Option<&std::path::Path>,
    ) -> bool {
        use crate::fs::read_resolution::ReadResolution;
        if !matches!(self.resolve_skill_read(skill_name), ReadResolution::Hidden) {
            return false;
        }
        if self.is_staging_skill_root(skill_name) || self.is_pending_install(skill_name) {
            return false;
        }
        !self.is_post_publish_grace_allowed(skill_name, relative_path)
    }

    pub(super) fn is_post_publish_grace_allowed(
        &self,
        skill_name: &str,
        relative_path: Option<&std::path::Path>,
    ) -> bool {
        let ctrl = match self.post_publish_controller.as_ref() {
            Some(c) => c,
            None => return false,
        };
        match relative_path {
            Some(rel) => ctrl.is_grace_allowed(skill_name, rel),
            None => ctrl.has_active_session(skill_name),
        }
    }
}
