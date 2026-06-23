//! FUSE virtual filesystem layer for SkillFS.
//!
//! Exposes skills as a virtual filesystem. The default view (from
//! \`skillfs-views.toml\`) is shown directly under \`/skills/\`. Secondary
//! views are accessible via the always-visible \`skill-discover\` virtual
//! skill, which lists their real source paths so the AI can open them
//! directly.
#![allow(clippy::too_many_arguments)]

use std::path::PathBuf;

use thiserror::Error;
use tracing::info;

pub mod security;
pub mod symlink_policy;

mod attr;
mod fs;
mod handles;
mod inode;
mod mount;
mod path;
mod sync;
mod sys;
mod xattr;

pub use fs::SkillFs;
pub use mount::{MountConfig, mount_background_configured, mount_configured};

#[allow(deprecated)]
pub use mount::{
    mount, mount_background, mount_background_with_security,
    mount_background_with_security_active_resolver_and_demo_refresh,
    mount_background_with_security_active_resolver_demo_refresh_and_trusted_writer,
    mount_background_with_security_and_active_resolver, mount_with_security,
    mount_with_security_active_resolver_and_demo_refresh,
    mount_with_security_active_resolver_demo_refresh_and_trusted_writer,
    mount_with_security_and_active_resolver,
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
    /// Background session (if mounted in background).
    /// Wrapped in `Option` so both explicit `unmount` and `Drop` can
    /// take it without double-joining.
    session: Option<std::thread::JoinHandle<()>>,
}

impl MountHandle {
    /// Unmount the filesystem and wait for the FUSE event loop thread
    /// to exit.
    pub fn unmount(mut self) -> Result<(), FuseError> {
        self.unmount_inner()
    }

    fn unmount_inner(&mut self) -> Result<(), FuseError> {
        info!(mountpoint = %self.mountpoint.display(), "unmounting filesystem");

        #[cfg(target_os = "linux")]
        {
            let output = std::process::Command::new("fusermount3")
                .args(["-u", &self.mountpoint.to_string_lossy()])
                .output();

            match output {
                Ok(output) if output.status.success() => {
                    info!("unmount successful");
                }
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(FuseError::UnmountFailed(stderr.to_string()));
                }
                Err(e) => {
                    return Err(FuseError::IoError(e));
                }
            }
        }

        if let Some(handle) = self.session.take() {
            if handle.join().is_err() {
                return Err(FuseError::UnmountFailed(
                    "FUSE event loop thread panicked".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Check if the mount is still active.
    pub fn is_mounted(&self) -> bool {
        std::fs::metadata(&self.mountpoint).is_ok()
    }
}

impl Drop for MountHandle {
    fn drop(&mut self) {
        if self.session.is_some() {
            let _ = self.unmount_inner();
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inode::InodeManager;
    use crate::path::{PathType, parse_path};
    use fuser::{FUSE_ROOT_ID, FileType};
    use std::path::Path;

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

    #[test]
    fn unmount_inner_skips_join_when_fusermount_fails() {
        let mut handle = MountHandle {
            mountpoint: PathBuf::from("/nonexistent/mount/point"),
            session: Some(std::thread::spawn(|| {})),
        };
        let result = handle.unmount_inner();
        assert!(result.is_err(), "bogus mountpoint must produce an error");
        assert!(
            handle.session.is_some(),
            "session must remain when fusermount3 fails (join skipped)"
        );
    }

    #[test]
    fn drop_with_failed_fusermount_does_not_block() {
        use std::time::{Duration, Instant};

        let handle = MountHandle {
            mountpoint: PathBuf::from("/nonexistent/mount/point"),
            session: Some(std::thread::spawn(|| {})),
        };
        let start = Instant::now();
        drop(handle);
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "Drop must not block on failed fusermount3"
        );
    }

    #[test]
    fn drop_without_session_is_noop() {
        let handle = MountHandle {
            mountpoint: PathBuf::from("/tmp/test-mount"),
            session: None,
        };
        drop(handle);
    }
}
