//! Daemon status persistence module
//!
//! Define the disk persistence format `DaemonStateFile`
//! and the atomic loading/saving functions.
//! Persistent location: `/var/lib/ws-ckpt/state.json` (managed by systemd StateDirectory).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::backend::BackendType;
use crate::STATE_FILE;

/// Schema version number, used for future compatible upgrade
pub const DAEMON_STATE_VERSION: u32 = 1;

/// Disk persistence format, written to state_dir/state.json
#[derive(Serialize, Deserialize, Debug, Clone)]
#[non_exhaustive]
pub struct DaemonStateFile {
    /// Schema version number, used for future compatible upgrade
    pub version: u32,
    /// Backend identity information
    pub backend: BackendIdentity,
    /// Backend working paths
    pub paths: BackendPaths,
    /// Registered workspace list (does not contain snapshot details)
    pub workspaces: Vec<WorkspaceEntry>,
}

impl DaemonStateFile {
    /// Construct a new DaemonStateFile.
    ///
    /// Because the struct is `#[non_exhaustive]`, external crates cannot use
    /// struct-literal syntax to build it. This constructor is the only
    /// stable way to create instances from outside this crate and keeps
    /// future field additions backward compatible.
    pub fn new(
        version: u32,
        backend: BackendIdentity,
        paths: BackendPaths,
        workspaces: Vec<WorkspaceEntry>,
    ) -> Self {
        Self {
            version,
            backend,
            paths,
            workspaces,
        }
    }
}

/// Backend identity
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BackendIdentity {
    /// Backend type (reusing existing BackendType enum)
    pub backend_type: BackendType,
    /// Selection method: "config-override" | "persisted" | "auto-detect"
    pub selection_method: String,
    /// Selection time
    pub selected_at: DateTime<Utc>,
}

/// Backend-specific paths and runtime state.
/// Each variant carries only the fields relevant to that backend type.
/// JSON format uses internally tagged enum: {"backend": "BtrfsLoop", "mount_path": "...", ...}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "backend")]
pub enum BackendPaths {
    /// BtrfsLoop: loop-mounted btrfs image
    BtrfsLoop {
        /// btrfs img mount point (= data_root for this backend)
        mount_path: PathBuf,
        /// backend data root directory
        data_root: PathBuf,
        /// snapshot subvolume parent directory
        snapshots_root: PathBuf,
        /// loop img state (populated after bootstrap; None during initial save)
        #[serde(default)]
        loop_img: Option<LoopImgState>,
    },
    /// BtrfsBase: native btrfs partition
    BtrfsBase {
        /// btrfs partition mount point
        mount_path: PathBuf,
        /// backend data root directory
        data_root: PathBuf,
        /// snapshot subvolume parent directory
        snapshots_root: PathBuf,
    },
    /// OverlayFs: overlay filesystem backend
    OverlayFs {
        /// backend data root directory
        data_root: PathBuf,
        /// snapshot parent directory
        snapshots_root: PathBuf,
    },
    // Future: DmThin { pool_device: PathBuf, thin_id: u32, data_root: PathBuf, snapshots_root: PathBuf }
}

/// Img state for BtrfsLoop Mode
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LoopImgState {
    /// Image file path
    pub img_path: PathBuf,
    /// Last bootstrap actual size (bytes)
    pub img_size_bytes: u64,
    /// Last used loop device (for diagnostic purposes, re-assigned on each restart)
    pub last_loop_device: Option<String>,
}

/// Central workspace entry
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WorkspaceEntry {
    /// Workspace ID
    pub ws_id: String,
    /// User original path (symlink path)
    pub workspace_path: PathBuf,
    /// Registration time
    pub registered_at: DateTime<Utc>,
    /// Origin backend type, for detecting orphan workspaces after backend type change
    pub origin_backend: BackendType,
}

/// Load DaemonStateFile from state_dir/state.json.
///
/// - File does not exist: returns `Ok(None)`
/// - File exists but format error: returns `Err`
pub fn load_state(state_dir: &Path) -> Result<Option<DaemonStateFile>> {
    let path = state_dir.join(STATE_FILE);
    match fs::read_to_string(&path) {
        Ok(content) => {
            let state: DaemonStateFile = serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse state file: {}", path.display()))?;
            Ok(Some(state))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("Failed to read state file: {}", path.display())),
    }
}

/// Atomically write to state.json (write-tmp + fsync + rename).
///
/// Write flow:
/// 1. Serialize to JSON (pretty print)
/// 2. Write to temporary file state.json.tmp
/// 3. fsync to ensure data is written to disk
/// 4. rename atomically
pub fn save_state(state_dir: &Path, state: &DaemonStateFile) -> Result<()> {
    fs::create_dir_all(state_dir)
        .with_context(|| format!("Failed to create state directory: {}", state_dir.display()))?;

    let target = state_dir.join(STATE_FILE);
    let tmp = state_dir.join(format!("{}.tmp", STATE_FILE));

    let content =
        serde_json::to_string_pretty(state).context("Failed to serialize DaemonStateFile")?;

    // Write to temporary file
    let mut file = fs::File::create(&tmp)
        .with_context(|| format!("Failed to create temp file: {}", tmp.display()))?;
    file.write_all(content.as_bytes())
        .with_context(|| format!("Failed to write temp file: {}", tmp.display()))?;
    file.sync_all()
        .with_context(|| format!("Failed to fsync temp file: {}", tmp.display()))?;

    // Atomic rename
    fs::rename(&tmp, &target).with_context(|| {
        format!(
            "Failed to atomically rename: {} -> {}",
            tmp.display(),
            target.display()
        )
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::BackendType;

    /// Construct a complete test DaemonStateFile
    fn sample_state() -> DaemonStateFile {
        DaemonStateFile {
            version: DAEMON_STATE_VERSION,
            backend: BackendIdentity {
                backend_type: BackendType::BtrfsLoop,
                selection_method: "auto-detect".to_string(),
                selected_at: Utc::now(),
            },
            paths: BackendPaths::BtrfsLoop {
                mount_path: PathBuf::from("/mnt/btrfs-workspace"),
                data_root: PathBuf::from("/mnt/btrfs-workspace"),
                snapshots_root: PathBuf::from("/mnt/btrfs-workspace/snapshots"),
                loop_img: Some(LoopImgState {
                    img_path: PathBuf::from("/var/lib/ws-ckpt/btrfs-data.img"),
                    img_size_bytes: 30 * 1024 * 1024 * 1024,
                    last_loop_device: Some("/dev/loop0".to_string()),
                }),
            },
            workspaces: vec![WorkspaceEntry {
                ws_id: "ws-a3f2b1".to_string(),
                workspace_path: PathBuf::from("/home/user/project"),
                registered_at: Utc::now(),
                origin_backend: BackendType::BtrfsLoop,
            }],
        }
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let state = sample_state();
        save_state(dir.path(), &state).unwrap();
        let loaded = load_state(dir.path())
            .unwrap()
            .expect("Should be able to load state");
        assert_eq!(loaded.version, DAEMON_STATE_VERSION);
        assert_eq!(loaded.workspaces.len(), 1);
        assert_eq!(loaded.workspaces[0].ws_id, "ws-a3f2b1");
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let result = load_state(dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn load_invalid_json_returns_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        fs::write(&path, "not valid json {{{").unwrap();
        let result = load_state(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn save_empty_workspaces_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let state = DaemonStateFile {
            version: DAEMON_STATE_VERSION,
            backend: BackendIdentity {
                backend_type: BackendType::BtrfsBase,
                selection_method: "config".to_string(),
                selected_at: Utc::now(),
            },
            paths: BackendPaths::BtrfsBase {
                mount_path: PathBuf::from("/mnt/btrfs"),
                data_root: PathBuf::from("/mnt/btrfs"),
                snapshots_root: PathBuf::from("/mnt/btrfs/snapshots"),
            },
            workspaces: vec![],
        };
        save_state(dir.path(), &state).unwrap();
        let loaded = load_state(dir.path())
            .unwrap()
            .expect("Should be able to load state");
        assert_eq!(loaded.version, DAEMON_STATE_VERSION);
        assert!(loaded.workspaces.is_empty());
        match &loaded.paths {
            BackendPaths::BtrfsBase { .. } => {} // OK, no loop_img field
            _ => panic!("Expected BtrfsBase variant"),
        }
    }

    #[test]
    fn atomic_write_no_tmp_residue() {
        let dir = tempfile::tempdir().unwrap();
        let state = sample_state();
        save_state(dir.path(), &state).unwrap();
        // Should not exist .tmp file after successful write
        let tmp_path = dir.path().join(format!("{}.tmp", STATE_FILE));
        assert!(!tmp_path.exists());
    }
}
