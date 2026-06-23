//! Path classification and parsing helpers for the FUSE layer.
//!
//! Pure, dependency-free logic shared by the FUSE callbacks and the
//! mount/discover code: [`PathType`] is the typed view of every FUSE
//! path SkillFS observes, [`parse_path`] (with the L1-inbox companion
//! [`parse_inbox_components`]) is its sole constructor, and
//! [`find_common_path_prefix`] is the parent-prefix helper used by
//! `skill-discover` when summarizing secondary view sources.

use std::path::{Path, PathBuf};

use crate::security::inbox::is_inbox_dir_name;

/// Types of paths in the SkillFS filesystem.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PathType {
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
    /// L1 inbox virtual root (`/.skillfs-inbox`).
    InboxDir,
    /// L1 inbox skill candidate directory
    /// (`/.skillfs-inbox/{skill_name}`). Maps virtually to the physical
    /// `source/{skill_name}` candidate directory; no physical
    /// `.skillfs-inbox` is ever created on disk.
    InboxSkillDir { skill_name: String },
    /// L1 inbox passthrough leaf
    /// (`/.skillfs-inbox/{skill_name}/{relative_path}`). Maps virtually
    /// to the physical `source/{skill_name}/{relative_path}`. SKILL.md
    /// reads through the inbox are passthrough — only `/skills/<skill>`
    /// runs the compiler.
    InboxPassthrough {
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
pub(crate) fn parse_path(path: &Path, in_place: bool) -> PathType {
    let components: Vec<_> = path.components().collect();

    // Try the L1 inbox namespace first in both modes. The inbox root is a
    // virtual top-level entry (`/.skillfs-inbox`) that lives alongside
    // `/skills` in normal mode and alongside the in-place skills root in
    // in-place mode.
    if components.len() >= 2 {
        let second = components[1].as_os_str().to_string_lossy();
        if is_inbox_dir_name(&second) {
            return parse_inbox_components(&components);
        }
    }

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

/// Parse the `/.skillfs-inbox/...` portion of a FUSE path. Caller must
/// have already verified that `components[1]` matches `INBOX_DIR_NAME`.
/// Mode (in_place / normal) does not affect the inbox layout because the
/// inbox is a virtual top-level entry under the FUSE root in both modes.
pub(crate) fn parse_inbox_components(components: &[std::path::Component<'_>]) -> PathType {
    match components {
        [_, _inbox] => PathType::InboxDir,
        [_, _inbox, skill_name] => PathType::InboxSkillDir {
            skill_name: skill_name.as_os_str().to_string_lossy().to_string(),
        },
        [_, _inbox, skill_name, rest @ ..] => {
            let skill_name = skill_name.as_os_str().to_string_lossy().to_string();
            let relative_path: PathBuf = rest.iter().map(|c| c.as_os_str()).collect();
            PathType::InboxPassthrough {
                skill_name,
                relative_path,
            }
        }
        _ => PathType::Invalid,
    }
}

/// Find the longest common parent-directory prefix across the given file
/// paths.
///
/// Used by `skill-discover` when summarizing secondary view sources so
/// that e.g.
///   `/home/user/skills/github/SKILL.md`
///   `/home/user/skills/discord/SKILL.md`
/// Returns `Some("/home/user/skills")`.
pub(crate) fn find_common_path_prefix(paths: &[std::path::PathBuf]) -> Option<std::path::PathBuf> {
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

/// Check whether a relative path belongs to the skill-discover namespace.
pub(crate) fn is_skill_discover_path(skill_name: &str) -> bool {
    skill_name == "skill-discover"
}
