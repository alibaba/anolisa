//! Ledger-driven read resolution.
//!
//! Owns the [`ReadResolution`] enum and the SkillFs methods that
//! consume it. These are the read counterparts of the ledger active
//! mapping: `Source` covers the no-resolver default and the
//! `Current` decision, `Snapshot` switches reads to a trusted
//! snapshot directory, and `Hidden` instructs callers to surface
//! `ENOENT` (lookup/getattr) or to drop the entry (readdir).
//!
//! Write paths intentionally bypass this module — D1.1 is read-only by
//! design.

use std::path::{Path, PathBuf};

use skillfs_core::compiler;

use super::SkillFs;
use crate::security::ActiveTarget;

/// Outcome of [`SkillFs::resolve_skill_read`].
#[derive(Debug, Clone)]
pub(super) enum ReadResolution {
    /// Read the live source directory. Returned outside demo mode and
    /// for [`ActiveTarget::Current`]; the actual directory is computed
    /// by [`SkillFs::skill_physical_dir`] at the call site so existing
    /// flat/categorized-layout handling stays in one place.
    Source,
    /// Read the snapshot directory. `dir` is already rewritten through
    /// [`SkillFs::source_base`] so in-place mounts bypass FUSE.
    /// `version` is the ledger-supplied label and is currently only
    /// used by the demo-event consumer.
    Snapshot {
        dir: PathBuf,
        #[allow(dead_code)]
        version: String,
    },
    /// Security mode: skill is hidden by the ledger or has no entry in the
    /// resolver.
    Hidden,
}

impl SkillFs {
    /// Read and compile a skill's SKILL.md content.
    ///
    /// In in-place mode reads via `/proc/self/fd/{n}` to bypass FUSE.
    /// When the D1.1 active resolver maps the skill to
    /// [`ActiveTarget::Snapshot`], the snapshot's `SKILL.md` is read and
    /// compiled instead of the live source, preserving the compiled-read
    /// semantics from the SkillFS invariants. [`ActiveTarget::Hidden`]
    /// returns `None` so the caller surfaces `ENOENT`.
    pub(super) fn compiled_skill_md(&self, skill_name: &str) -> Option<String> {
        if skill_name == "skill-discover" {
            return Some(self.get_skill_discover_content());
        }
        let physical_path = match self.resolve_skill_read(skill_name) {
            ReadResolution::Hidden => return None,
            ReadResolution::Source => {
                if self.in_place {
                    // Bypass the FUSE layer via the pre-opened fd.
                    self.source_base().join(skill_name).join("SKILL.md")
                } else {
                    self.skill_source_path(skill_name)?
                }
            }
            ReadResolution::Snapshot { dir, .. } => dir.join("SKILL.md"),
        };
        let raw = std::fs::read_to_string(&physical_path).ok()?;
        Some(compiler::compile(&raw, &self.env_profile))
    }

    /// Physical directory to read **content from** for `skill_name`.
    ///
    /// For an unattached resolver (default) and for
    /// [`ActiveTarget::Current`] this is the live skill directory
    /// returned by [`Self::skill_physical_dir`]. For
    /// [`ActiveTarget::Snapshot`] it is the snapshot directory rewritten
    /// through [`Self::source_base`] so in-place mounts continue to
    /// bypass the FUSE over-mount via `/proc/self/fd/{n}`. Returns
    /// `None` when the resolver marks the skill as hidden so the caller
    /// can surface `ENOENT` instead of leaking a path.
    ///
    /// Skill-discover bypasses ledger gating entirely and always reads
    /// from the virtual skill-discover dir.
    pub(super) fn skill_read_dir(&self, skill_name: &str) -> Option<PathBuf> {
        if skill_name == "skill-discover" {
            return Some(self.skill_physical_dir(skill_name));
        }
        match self.resolve_skill_read(skill_name) {
            ReadResolution::Hidden => None,
            ReadResolution::Source => Some(self.skill_physical_dir(skill_name)),
            ReadResolution::Snapshot { dir, .. } => Some(dir),
        }
    }

    /// Read and compile using a pinned target instead of the live resolver.
    pub(super) fn compiled_skill_md_pinned(
        &self,
        skill_name: &str,
        pinned: Option<&ActiveTarget>,
    ) -> Option<String> {
        if skill_name == "skill-discover" {
            return Some(self.get_skill_discover_content());
        }
        let resolution = match pinned {
            Some(target) => self.resolve_from_target(skill_name, target),
            None => self.resolve_skill_read(skill_name),
        };
        let physical_path = match resolution {
            ReadResolution::Hidden => return None,
            ReadResolution::Source => {
                if self.in_place {
                    self.source_base().join(skill_name).join("SKILL.md")
                } else {
                    self.skill_source_path(skill_name)?
                }
            }
            ReadResolution::Snapshot { dir, .. } => dir.join("SKILL.md"),
        };
        let raw = std::fs::read_to_string(&physical_path).ok()?;
        Some(compiler::compile(&raw, &self.env_profile))
    }

    /// Resolve from an explicit `ActiveTarget` without consulting the resolver.
    fn resolve_from_target(&self, skill_name: &str, target: &ActiveTarget) -> ReadResolution {
        match target {
            ActiveTarget::Hidden { .. } => ReadResolution::Hidden,
            ActiveTarget::Current { .. } => ReadResolution::Source,
            ActiveTarget::Snapshot {
                snapshot_dir,
                version,
            } => {
                let dir = self.snapshot_read_dir(skill_name, snapshot_dir);
                ReadResolution::Snapshot {
                    dir,
                    version: version.clone(),
                }
            }
        }
    }

    /// Pure resolver consult. Returns `ReadResolution::Source` whenever
    /// no resolver is attached so the pre-security code paths behave
    /// exactly as before. Skill-discover is always `Source`.
    pub(super) fn resolve_skill_read(&self, skill_name: &str) -> ReadResolution {
        if skill_name == "skill-discover" {
            return ReadResolution::Source;
        }
        let resolver = match self.active_resolver.as_ref() {
            Some(r) => r,
            None => return ReadResolution::Source,
        };
        match resolver.get(skill_name) {
            // Default: skills the ledger has no opinion on
            // are treated as not-yet-certified and stay hidden until a
            // future hook handler installs a target for them.
            None => ReadResolution::Hidden,
            Some(ActiveTarget::Hidden { .. }) => ReadResolution::Hidden,
            Some(ActiveTarget::Current { .. }) => ReadResolution::Source,
            Some(ActiveTarget::Snapshot {
                snapshot_dir,
                version,
            }) => {
                let dir = self.snapshot_read_dir(skill_name, &snapshot_dir);
                ReadResolution::Snapshot { dir, version }
            }
        }
    }

    /// Rewrite a `snapshot_dir` from the resolver so reads bypass the
    /// FUSE layer in in-place mode.
    ///
    /// The resolver constructs
    /// `snapshot_dir = source_root.join(skill).join(<rel>)` against the
    /// `source_root` it was built with. The CLI / tests build that
    /// resolver from the same `source` path the FUSE mount uses, so the
    /// prefix matches exactly and the relative segment after
    /// `<skill>/` can be safely rejoined against
    /// [`Self::source_base`]. In normal mode `source_base()` is the
    /// plain source path so the rewrite is a no-op; in in-place mode it
    /// becomes `/proc/self/fd/{n}`, which reads the underlying inode
    /// instead of re-entering the FUSE over-mount (which would deny
    /// `.skill-meta/**` mutations and would not even resolve through
    /// the virtual layer).
    ///
    /// If the prefix does not match (operator passed a canonicalized vs
    /// non-canonicalized source, or a future package starts handing the
    /// resolver an absolute path it did not build), the original
    /// `snapshot_dir` is returned verbatim. Either it resolves and the
    /// operator is happy, or the underlying syscall surfaces a real
    /// errno — no silent fallback to live source.
    pub(super) fn snapshot_read_dir(&self, skill_name: &str, snapshot_dir: &Path) -> PathBuf {
        let prefix = self.source.join(skill_name);
        match snapshot_dir.strip_prefix(&prefix) {
            Ok(rel) => self.source_base().join(skill_name).join(rel),
            Err(_) => snapshot_dir.to_path_buf(),
        }
    }
}
