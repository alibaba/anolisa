use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thiserror::Error;

pub mod backend;

use backend::BackendType;

// ── Constants ──

pub const DEFAULT_MOUNT_PATH: &str = "/mnt/btrfs-workspace";
pub const DEFAULT_SOCKET_PATH: &str = "/run/ws-ckpt/ws-ckpt.sock";
pub const SNAPSHOTS_DIR: &str = "snapshots";
pub const INDEX_FILE: &str = "index.json";
pub const BTRFS_IMG_PATH: &str = "/data/ws-ckpt/btrfs-data.img";
pub const BTRFS_IMG_DIR: &str = "/data/ws-ckpt";
pub const CONFIG_FILE_PATH: &str = "/etc/ws-ckpt/config.toml";
pub const DEFAULT_FS_WARN_THRESHOLD_PERCENT: f64 = 90.0;
pub const DEFAULT_IMG_SIZE_GB: u64 = 30;
pub const DEFAULT_IMG_MAX_PERCENT: f64 = 0.4; // 40% as fraction for calculation

// ── Error type ──

#[derive(Error, Debug)]
pub enum WsCkptError {
    #[error("bincode error: {0}")]
    Bincode(#[from] bincode::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("frame too large: {size} bytes (max {max})")]
    FrameTooLarge { size: u32, max: u32 },
    #[error("config error: {0}")]
    Config(String),
}

// ── Request / Response ──

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Request {
    Init {
        workspace: String,
    },
    Checkpoint {
        workspace: String,
        id: String,
        message: Option<String>,
        metadata: Option<String>,
        pin: bool,
    },
    Rollback {
        workspace: String,
        to: String,
    },
    Delete {
        workspace: Option<String>,
        snapshot: String,
        force: bool,
    },
    List {
        workspace: Option<String>,
        format: Option<String>,
    },
    Diff {
        workspace: String,
        from: String,
        to: String,
    },
    Status {
        workspace: Option<String>,
    },
    Cleanup {
        workspace: String,
        keep: Option<u32>,
    },
    /// Query current daemon configuration
    Config,
    /// Reload configuration from file
    ReloadConfig,
    /// Recover workspace to a normal directory (undo init)
    Recover {
        workspace: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Response {
    InitOk { ws_id: String },
    CheckpointOk { snapshot_id: String },
    RollbackOk { from: String, to: String },
    DeleteOk { target: String },
    Error { code: ErrorCode, message: String },
    ListOk { snapshots: Vec<SnapshotEntry> },
    DiffOk { changes: Vec<DiffEntry> },
    StatusOk { report: StatusReport },
    CleanupOk { removed: Vec<String> },
    ConfigOk { config: ConfigReport },
    ReloadConfigOk,
    CheckpointSkipped { reason: String },
    RecoverOk { workspace: String },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum ErrorCode {
    WorkspaceNotFound,
    SnapshotNotFound,
    AlreadyInitialized,
    BtrfsError,
    IoError,
    InvalidPath,
    ConfirmationRequired,
    InternalError,
    SnapshotAlreadyExists,
    WriteLockConflict,
    DiskSpaceInsufficient,
}

// ── Snapshot types ──

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SnapshotMeta {
    pub message: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub pinned: bool,
    pub created_at: DateTime<Utc>,
}

/// A snapshot entry combining its ID with metadata.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SnapshotEntry {
    pub id: String,
    pub workspace: String,
    pub meta: SnapshotMeta,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SnapshotIndex {
    pub workspace_path: PathBuf,
    pub snapshots: HashMap<String, SnapshotMeta>,
}

impl SnapshotIndex {
    pub fn new(workspace_path: PathBuf) -> Self {
        Self {
            workspace_path,
            snapshots: HashMap::new(),
        }
    }
}

/// Error type for snapshot prefix resolution.
#[derive(Debug, PartialEq, Eq)]
pub enum ResolveError {
    NotFound,
    Ambiguous(usize),
}

impl SnapshotIndex {
    /// Resolve a snapshot by exact ID or unique prefix.
    pub fn resolve_by_prefix(
        &self,
        prefix: &str,
    ) -> Result<(&String, &SnapshotMeta), ResolveError> {
        // Exact match first
        if let Some((id, meta)) = self.snapshots.get_key_value(prefix) {
            return Ok((id, meta));
        }
        // Prefix match
        let matches: Vec<_> = self
            .snapshots
            .iter()
            .filter(|(id, _)| id.starts_with(prefix))
            .collect();
        match matches.len() {
            0 => Err(ResolveError::NotFound),
            1 => Ok((matches[0].0, matches[0].1)),
            n => Err(ResolveError::Ambiguous(n)),
        }
    }
}

// ── Phase 2 data types ──

/// Type of change detected in a diff between two snapshots.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum ChangeType {
    Added,
    Modified,
    Deleted,
    Renamed,
}

/// A single file change entry in a diff result.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DiffEntry {
    pub path: String,
    pub change_type: ChangeType,
    pub detail: Option<String>,
}

/// Summary information about a single workspace.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct WorkspaceInfo {
    pub ws_id: String,
    pub path: String,
    pub snapshot_count: u32,
}

/// Status report for the daemon and its managed workspaces.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct StatusReport {
    pub uptime_secs: u64,
    pub workspaces: Vec<WorkspaceInfo>,
    pub fs_total_bytes: u64,
    pub fs_used_bytes: u64,
}

/// Report of the current daemon configuration (returned by `Config` request).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ConfigReport {
    pub mount_path: String,
    pub socket_path: String,
    pub log_level: String,
    pub auto_cleanup: bool,
    pub auto_cleanup_keep: u32,
    pub auto_cleanup_interval_secs: u64,
    pub health_check_interval_secs: u64,
    pub fs_warn_threshold_percent: f64,
    pub img_path: String,
    pub img_size: u64,
    pub img_max_percent: f64,
}

// ── Daemon config ──

#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub mount_path: PathBuf,
    pub socket_path: PathBuf,
    pub log_level: String,
    pub auto_cleanup: bool,
    /// Number of recent unpinned snapshots to keep during auto-cleanup (pinned snapshots are excluded from this count)
    pub auto_cleanup_keep: u32,
    /// Interval in seconds between auto-cleanup runs
    pub auto_cleanup_interval_secs: u64,
    /// Interval in seconds between health checks
    pub health_check_interval_secs: u64,
    /// Backend type string from config: "auto" | "btrfs-base" | "btrfs-loop" | "overlayfs"
    pub backend_type: String,
    /// Filesystem usage warning threshold (percentage, 0-100)
    pub fs_warn_threshold_percent: f64,
    /// Loop image file path (runtime-only; always `BTRFS_IMG_PATH`, not user-configurable)
    pub img_path: String,
    /// Target image size in GB. The on-disk image is grown/shrunk to match this at bootstrap.
    pub img_size: u64,
    /// Initial-creation cap as percentage of host partition capacity (0-100).
    /// Only consulted on the very first bootstrap when the image does not yet exist.
    pub img_max_percent: f64,
    /// Minimum free space in bytes (used by health-check reporting, does NOT block checkpoint)
    pub min_free_bytes: u64,
    /// Minimum free space percentage 0-100 (used by health-check reporting, does NOT block checkpoint)
    pub min_free_percent: f64,
}

impl DaemonConfig {
    /// Parse backend_type string into BackendType enum.
    /// Returns None for "auto" (caller should run auto-detect).
    pub fn parse_backend_type(&self) -> Option<BackendType> {
        match self.backend_type.as_str() {
            "btrfs-loop" => Some(BackendType::BtrfsLoop),
            "btrfs-base" => Some(BackendType::BtrfsBase),
            "overlayfs" => Some(BackendType::OverlayFs),
            _ => None, // "auto" or unknown → auto-detect
        }
    }
}

pub const DEFAULT_AUTO_CLEANUP: bool = false;
pub const DEFAULT_AUTO_CLEANUP_KEEP: u32 = 20;
pub const DEFAULT_AUTO_CLEANUP_INTERVAL_SECS: u64 = 600;
pub const DEFAULT_HEALTH_CHECK_INTERVAL_SECS: u64 = 300;

// ── Config file ──

/// BtrfsLoop backend-specific configuration.
///
/// NOTE: These fields only take effect during daemon bootstrap.
/// Changing them via `ReloadConfig` will emit a warning and require a daemon restart.
/// The img file path is fixed to `BTRFS_IMG_PATH` and is not user-configurable.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct BtrfsLoopConfig {
    /// Target image size in GB. Used as the reconcile target at bootstrap.
    pub img_size: Option<u64>,
    /// Initial-creation cap as percentage of host partition capacity (0-100).
    /// Only consulted on the very first bootstrap when the image does not yet exist.
    pub img_max_percent: Option<f64>,
}

/// Backend configuration section in config file.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct BackendConfig {
    /// "auto" | "btrfs-base" | "btrfs-loop" | "overlayfs"
    #[serde(default = "default_backend_type")]
    pub r#type: String,
    /// BtrfsLoop backend-specific settings
    #[serde(default, rename = "btrfs-loop")]
    pub btrfs_loop: Option<BtrfsLoopConfig>,
}

fn default_backend_type() -> String {
    "auto".to_string()
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            r#type: default_backend_type(),
            btrfs_loop: None,
        }
    }
}

/// On-disk config file structure (all fields optional; missing = use defaults).
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct FileConfig {
    pub auto_cleanup: Option<bool>,
    pub auto_cleanup_keep: Option<u32>,
    pub auto_cleanup_interval_secs: Option<u64>,
    pub health_check_interval_secs: Option<u64>,
    /// Filesystem usage warning threshold (percentage, 0-100)
    pub fs_warn_threshold_percent: Option<f64>,
    /// Backend configuration section (optional; defaults to auto-detect)
    #[serde(default)]
    pub backend: BackendConfig,
}

/// Load config from a TOML file. Returns `FileConfig::default()` when the file
/// does not exist.
pub fn load_config_file(path: &Path) -> Result<FileConfig, WsCkptError> {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let fc: FileConfig = toml::from_str(&content)
                .map_err(|e| WsCkptError::Config(format!("parse {}: {}", path.display(), e)))?;
            Ok(fc)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(FileConfig::default()),
        Err(e) => Err(WsCkptError::Io(e)),
    }
}

/// Save config to a TOML file, creating parent directories as needed.
pub fn save_config_file(path: &Path, config: &FileConfig) -> Result<(), WsCkptError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(config)
        .map_err(|e| WsCkptError::Config(format!("serialize config: {}", e)))?;
    std::fs::write(path, content)?;
    Ok(())
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            mount_path: PathBuf::from(DEFAULT_MOUNT_PATH),
            socket_path: PathBuf::from(DEFAULT_SOCKET_PATH),
            log_level: "info".to_string(),
            auto_cleanup: DEFAULT_AUTO_CLEANUP,
            auto_cleanup_keep: DEFAULT_AUTO_CLEANUP_KEEP,
            auto_cleanup_interval_secs: DEFAULT_AUTO_CLEANUP_INTERVAL_SECS,
            health_check_interval_secs: DEFAULT_HEALTH_CHECK_INTERVAL_SECS,
            backend_type: "auto".to_string(),
            fs_warn_threshold_percent: DEFAULT_FS_WARN_THRESHOLD_PERCENT,
            img_path: BTRFS_IMG_PATH.to_string(),
            img_size: DEFAULT_IMG_SIZE_GB,
            img_max_percent: DEFAULT_IMG_MAX_PERCENT * 100.0, // stored as 0-100
            min_free_bytes: 512 * 1024 * 1024,                // 512 MB
            min_free_percent: 1.0,
        }
    }
}

// ── Frame encoding/decoding (sync, no tokio dependency) ──

const MAX_FRAME_SIZE: u32 = 16 * 1024 * 1024; // 16MB max

/// Serialize a message into a length-prefixed frame: [4-byte LE length][bincode payload]
pub fn encode_frame<T: Serialize>(msg: &T) -> Result<Vec<u8>, WsCkptError> {
    let payload = bincode::serialize(msg)?;
    let len = payload.len() as u32;
    if len > MAX_FRAME_SIZE {
        return Err(WsCkptError::FrameTooLarge {
            size: len,
            max: MAX_FRAME_SIZE,
        });
    }
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend(payload);
    Ok(frame)
}

/// Deserialize a bincode payload (caller is responsible for reading the length prefix
/// and then reading exactly N bytes before calling this function)
pub fn decode_payload<T: DeserializeOwned>(data: &[u8]) -> Result<T, WsCkptError> {
    Ok(bincode::deserialize(data)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ── Helper: encode then decode round-trip ──
    fn round_trip_request(req: &Request) -> Request {
        let frame = encode_frame(req).expect("encode_frame failed");
        // Skip first 4 bytes (length prefix)
        let payload = &frame[4..];
        decode_payload::<Request>(payload).expect("decode_payload failed")
    }

    fn round_trip_response(resp: &Response) -> Response {
        let frame = encode_frame(resp).expect("encode_frame failed");
        let payload = &frame[4..];
        decode_payload::<Response>(payload).expect("decode_payload failed")
    }

    // ── Request round-trip tests ──

    #[test]
    fn request_init_round_trip() {
        let req = Request::Init {
            workspace: "/tmp/test-ws".to_string(),
        };
        let decoded = round_trip_request(&req);
        match decoded {
            Request::Init { workspace } => assert_eq!(workspace, "/tmp/test-ws"),
            _ => panic!("expected Init variant"),
        }
    }

    #[test]
    fn request_checkpoint_round_trip() {
        let req = Request::Checkpoint {
            workspace: "/tmp/ws".to_string(),
            id: "msg1-step0".to_string(),
            message: Some("save point".to_string()),
            metadata: None,
            pin: true,
        };
        let decoded = round_trip_request(&req);
        match decoded {
            Request::Checkpoint {
                workspace,
                id,
                message,
                metadata,
                pin,
            } => {
                assert_eq!(workspace, "/tmp/ws");
                assert_eq!(id, "msg1-step0");
                assert_eq!(message.as_deref(), Some("save point"));
                assert!(metadata.is_none());
                assert!(pin);
            }
            _ => panic!("expected Checkpoint variant"),
        }
    }

    #[test]
    fn request_checkpoint_with_metadata_round_trip() {
        // metadata is now Option<String> (JSON string), which bincode handles fine.
        let json_str = r#"{"key":"value"}"#.to_string();
        let req = Request::Checkpoint {
            workspace: "/ws".to_string(),
            id: "msg2-step0".to_string(),
            message: None,
            metadata: Some(json_str.clone()),
            pin: false,
        };
        let decoded = round_trip_request(&req);
        match decoded {
            Request::Checkpoint { metadata, .. } => {
                assert_eq!(metadata, Some(json_str));
            }
            _ => panic!("expected Checkpoint variant"),
        }
    }

    #[test]
    fn request_checkpoint_minimal_round_trip() {
        // Checkpoint with no optional fields
        let req = Request::Checkpoint {
            workspace: "/ws".to_string(),
            id: "msg1-step0".to_string(),
            message: None,
            metadata: None,
            pin: false,
        };
        let decoded = round_trip_request(&req);
        match decoded {
            Request::Checkpoint {
                message,
                metadata,
                pin,
                ..
            } => {
                assert!(message.is_none());
                assert!(metadata.is_none());
                assert!(!pin);
            }
            _ => panic!("expected Checkpoint variant"),
        }
    }

    #[test]
    fn request_rollback_round_trip() {
        let req = Request::Rollback {
            workspace: "/tmp/ws".to_string(),
            to: "msg1-step2".to_string(),
        };
        let decoded = round_trip_request(&req);
        match decoded {
            Request::Rollback { workspace, to } => {
                assert_eq!(workspace, "/tmp/ws");
                assert_eq!(to, "msg1-step2");
            }
            _ => panic!("expected Rollback variant"),
        }
    }

    #[test]
    fn request_delete_round_trip() {
        let req = Request::Delete {
            workspace: Some("/tmp/ws".to_string()),
            snapshot: "msg2-step0".to_string(),
            force: true,
        };
        let decoded = round_trip_request(&req);
        match decoded {
            Request::Delete {
                workspace,
                snapshot,
                force,
            } => {
                assert_eq!(workspace.as_deref(), Some("/tmp/ws"));
                assert_eq!(snapshot, "msg2-step0");
                assert!(force);
            }
            _ => panic!("expected Delete variant"),
        }
    }

    #[test]
    fn request_delete_no_force_round_trip() {
        let req = Request::Delete {
            workspace: Some("/ws".to_string()),
            snapshot: "abc123".to_string(),
            force: false,
        };
        let decoded = round_trip_request(&req);
        match decoded {
            Request::Delete {
                workspace,
                snapshot,
                force,
            } => {
                assert_eq!(workspace.as_deref(), Some("/ws"));
                assert_eq!(snapshot, "abc123");
                assert!(!force);
            }
            _ => panic!("expected Delete variant"),
        }
    }

    #[test]
    fn request_delete_no_workspace_round_trip() {
        let req = Request::Delete {
            workspace: None,
            snapshot: "msg1-step0".to_string(),
            force: false,
        };
        let decoded = round_trip_request(&req);
        match decoded {
            Request::Delete {
                workspace,
                snapshot,
                force,
            } => {
                assert!(workspace.is_none());
                assert_eq!(snapshot, "msg1-step0");
                assert!(!force);
            }
            _ => panic!("expected Delete variant"),
        }
    }

    // ── Response round-trip tests ──

    #[test]
    fn response_init_ok_round_trip() {
        let resp = Response::InitOk {
            ws_id: "ws-a3f2b1".to_string(),
        };
        let decoded = round_trip_response(&resp);
        match decoded {
            Response::InitOk { ws_id } => assert_eq!(ws_id, "ws-a3f2b1"),
            _ => panic!("expected InitOk variant"),
        }
    }

    #[test]
    fn response_checkpoint_ok_round_trip() {
        let resp = Response::CheckpointOk {
            snapshot_id: "msg1-step2".to_string(),
        };
        let decoded = round_trip_response(&resp);
        match decoded {
            Response::CheckpointOk { snapshot_id } => assert_eq!(snapshot_id, "msg1-step2"),
            _ => panic!("expected CheckpointOk variant"),
        }
    }

    #[test]
    fn response_rollback_ok_round_trip() {
        let resp = Response::RollbackOk {
            from: "workspace-abc123".to_string(),
            to: "msg1-step0".to_string(),
        };
        let decoded = round_trip_response(&resp);
        match decoded {
            Response::RollbackOk { from, to } => {
                assert_eq!(from, "workspace-abc123");
                assert_eq!(to, "msg1-step0");
            }
            _ => panic!("expected RollbackOk variant"),
        }
    }

    #[test]
    fn response_delete_ok_round_trip() {
        let resp = Response::DeleteOk {
            target: "msg1-step2".to_string(),
        };
        let decoded = round_trip_response(&resp);
        match decoded {
            Response::DeleteOk { target } => assert_eq!(target, "msg1-step2"),
            _ => panic!("expected DeleteOk variant"),
        }
    }

    #[test]
    fn response_error_round_trip() {
        let resp = Response::Error {
            code: ErrorCode::WorkspaceNotFound,
            message: "workspace not found: /tmp/ws".to_string(),
        };
        let decoded = round_trip_response(&resp);
        match decoded {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::WorkspaceNotFound);
                assert_eq!(message, "workspace not found: /tmp/ws");
            }
            _ => panic!("expected Error variant"),
        }
    }

    #[test]
    fn response_error_all_codes_round_trip() {
        // Verify every ErrorCode variant survives round-trip
        let codes = vec![
            ErrorCode::WorkspaceNotFound,
            ErrorCode::SnapshotNotFound,
            ErrorCode::AlreadyInitialized,
            ErrorCode::BtrfsError,
            ErrorCode::IoError,
            ErrorCode::InvalidPath,
            ErrorCode::ConfirmationRequired,
            ErrorCode::InternalError,
            ErrorCode::SnapshotAlreadyExists,
            ErrorCode::WriteLockConflict,
            ErrorCode::DiskSpaceInsufficient,
        ];
        for code in codes {
            let resp = Response::Error {
                code: code.clone(),
                message: format!("test {:?}", code),
            };
            let decoded = round_trip_response(&resp);
            match decoded {
                Response::Error {
                    code: dc,
                    message: dm,
                } => {
                    assert_eq!(dc, code);
                    assert!(dm.starts_with("test "));
                }
                _ => panic!("expected Error variant"),
            }
        }
    }

    // ── Frame format tests ──

    #[test]
    fn encode_frame_length_prefix_is_le() {
        // Verify the first 4 bytes of encode_frame are LE-encoded payload length
        let req = Request::Init {
            workspace: "/tmp/test".to_string(),
        };
        let frame = encode_frame(&req).expect("encode_frame failed");
        let len_bytes: [u8; 4] = frame[..4].try_into().unwrap();
        let encoded_len = u32::from_le_bytes(len_bytes) as usize;
        // The rest of the frame should be exactly `encoded_len` bytes
        assert_eq!(frame.len() - 4, encoded_len);
    }

    #[test]
    fn encode_frame_payload_matches_bincode() {
        // Verify the payload portion matches direct bincode serialization
        let req = Request::Init {
            workspace: "/hello".to_string(),
        };
        let frame = encode_frame(&req).unwrap();
        let expected_payload = bincode::serialize(&req).unwrap();
        assert_eq!(&frame[4..], &expected_payload[..]);
    }

    // ── SnapshotIndex tests ──

    #[test]
    fn snapshot_index_new_is_empty() {
        let idx = SnapshotIndex::new(PathBuf::from("/tmp/ws"));
        assert_eq!(idx.workspace_path, PathBuf::from("/tmp/ws"));
        assert!(idx.snapshots.is_empty());
    }

    #[test]
    fn snapshot_index_resolve_by_prefix_exact_match() {
        let mut idx = SnapshotIndex::new(PathBuf::from("/ws"));
        idx.snapshots.insert(
            "abcdef1234567890abcdef1234567890abcdef12".to_string(),
            SnapshotMeta {
                message: None,
                metadata: None,
                pinned: true,
                created_at: chrono::Utc::now(),
            },
        );
        let result = idx.resolve_by_prefix("abcdef1234567890abcdef1234567890abcdef12");
        assert!(result.is_ok());
        let (id, _) = result.unwrap();
        assert_eq!(id, "abcdef1234567890abcdef1234567890abcdef12");
    }

    #[test]
    fn snapshot_index_resolve_by_prefix_unique_prefix() {
        let mut idx = SnapshotIndex::new(PathBuf::from("/ws"));
        idx.snapshots.insert(
            "abcdef1234567890abcdef1234567890abcdef12".to_string(),
            SnapshotMeta {
                message: None,
                metadata: None,
                pinned: false,
                created_at: chrono::Utc::now(),
            },
        );
        let result = idx.resolve_by_prefix("abcdef");
        assert!(result.is_ok());
    }

    #[test]
    fn snapshot_index_resolve_by_prefix_not_found() {
        let idx = SnapshotIndex::new(PathBuf::from("/ws"));
        let result = idx.resolve_by_prefix("nonexistent");
        assert_eq!(result.unwrap_err(), ResolveError::NotFound);
    }

    #[test]
    fn snapshot_index_resolve_by_prefix_ambiguous() {
        let mut idx = SnapshotIndex::new(PathBuf::from("/ws"));
        idx.snapshots.insert(
            "abcdef1111111111111111111111111111111111".to_string(),
            SnapshotMeta {
                message: None,
                metadata: None,
                pinned: false,
                created_at: chrono::Utc::now(),
            },
        );
        idx.snapshots.insert(
            "abcdef2222222222222222222222222222222222".to_string(),
            SnapshotMeta {
                message: None,
                metadata: None,
                pinned: false,
                created_at: chrono::Utc::now(),
            },
        );
        let result = idx.resolve_by_prefix("abcdef");
        assert_eq!(result.unwrap_err(), ResolveError::Ambiguous(2));
    }

    // ── DaemonConfig::default() tests ──

    #[test]
    fn daemon_config_default_values() {
        let cfg = DaemonConfig::default();
        assert_eq!(cfg.mount_path, PathBuf::from(DEFAULT_MOUNT_PATH));
        assert_eq!(cfg.socket_path, PathBuf::from(DEFAULT_SOCKET_PATH));
        assert_eq!(cfg.log_level, "info");
    }

    // ── WsCkptError Display tests ──

    #[test]
    fn error_display_frame_too_large() {
        let err = WsCkptError::FrameTooLarge {
            size: 20_000_000,
            max: 16_777_216,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("frame too large"));
        assert!(msg.contains("20000000"));
        assert!(msg.contains("16777216"));
    }

    #[test]
    fn error_display_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let err = WsCkptError::Io(io_err);
        let msg = format!("{}", err);
        assert!(msg.contains("io error"));
    }

    #[test]
    fn error_display_json() {
        // Trigger a real serde_json error
        let json_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let err = WsCkptError::Json(json_err);
        let msg = format!("{}", err);
        assert!(msg.contains("json error"));
    }

    #[test]
    fn error_display_bincode() {
        // Trigger a real bincode error (invalid data for a Request)
        let bad_data = vec![0xFF, 0xFF, 0xFF];
        let bincode_err = bincode::deserialize::<Request>(&bad_data).unwrap_err();
        let err = WsCkptError::Bincode(bincode_err);
        let msg = format!("{}", err);
        assert!(msg.contains("bincode error"));
    }

    // ── Phase 2 Request round-trip tests ──

    #[test]
    fn request_list_round_trip() {
        let req = Request::List {
            workspace: Some("/tmp/ws".to_string()),
            format: Some("json".to_string()),
        };
        let decoded = round_trip_request(&req);
        match decoded {
            Request::List { workspace, format } => {
                assert_eq!(workspace.as_deref(), Some("/tmp/ws"));
                assert_eq!(format.as_deref(), Some("json"));
            }
            _ => panic!("expected List variant"),
        }
    }

    #[test]
    fn request_list_no_format_round_trip() {
        let req = Request::List {
            workspace: Some("/ws".to_string()),
            format: None,
        };
        let decoded = round_trip_request(&req);
        match decoded {
            Request::List { format, .. } => assert!(format.is_none()),
            _ => panic!("expected List variant"),
        }
    }

    #[test]
    fn request_diff_round_trip() {
        let req = Request::Diff {
            workspace: "/tmp/ws".to_string(),
            from: "msg1-step0".to_string(),
            to: "msg2-step0".to_string(),
        };
        let decoded = round_trip_request(&req);
        match decoded {
            Request::Diff {
                workspace,
                from,
                to,
            } => {
                assert_eq!(workspace, "/tmp/ws");
                assert_eq!(from, "msg1-step0");
                assert_eq!(to, "msg2-step0");
            }
            _ => panic!("expected Diff variant"),
        }
    }

    #[test]
    fn request_status_round_trip() {
        let req = Request::Status {
            workspace: Some("/tmp/ws".to_string()),
        };
        let decoded = round_trip_request(&req);
        match decoded {
            Request::Status { workspace } => {
                assert_eq!(workspace.as_deref(), Some("/tmp/ws"));
            }
            _ => panic!("expected Status variant"),
        }
    }

    #[test]
    fn request_status_no_workspace_round_trip() {
        let req = Request::Status { workspace: None };
        let decoded = round_trip_request(&req);
        match decoded {
            Request::Status { workspace } => assert!(workspace.is_none()),
            _ => panic!("expected Status variant"),
        }
    }

    #[test]
    fn request_cleanup_round_trip() {
        let req = Request::Cleanup {
            workspace: "/tmp/ws".to_string(),
            keep: Some(10),
        };
        let decoded = round_trip_request(&req);
        match decoded {
            Request::Cleanup { workspace, keep } => {
                assert_eq!(workspace, "/tmp/ws");
                assert_eq!(keep, Some(10));
            }
            _ => panic!("expected Cleanup variant"),
        }
    }

    #[test]
    fn request_cleanup_no_keep_round_trip() {
        let req = Request::Cleanup {
            workspace: "/ws".to_string(),
            keep: None,
        };
        let decoded = round_trip_request(&req);
        match decoded {
            Request::Cleanup { keep, .. } => assert!(keep.is_none()),
            _ => panic!("expected Cleanup variant"),
        }
    }

    // ── Phase 2 Response round-trip tests ──

    #[test]
    fn response_list_ok_round_trip() {
        let resp = Response::ListOk {
            snapshots: vec![SnapshotEntry {
                id: "abc123def456".to_string(),
                workspace: "/home/user/ws".to_string(),
                meta: SnapshotMeta {
                    message: Some("first".to_string()),
                    metadata: None,
                    pinned: true,
                    created_at: chrono::Utc::now(),
                },
            }],
        };
        let decoded = round_trip_response(&resp);
        match decoded {
            Response::ListOk { snapshots } => {
                assert_eq!(snapshots.len(), 1);
                assert_eq!(snapshots[0].id, "abc123def456");
            }
            _ => panic!("expected ListOk variant"),
        }
    }

    #[test]
    fn snapshot_entry_round_trip() {
        let entry = SnapshotEntry {
            id: "abc123def456".to_string(),
            workspace: "/home/user/ws".to_string(),
            meta: SnapshotMeta {
                message: Some("test message".to_string()),
                metadata: None,
                pinned: false,
                created_at: chrono::Utc::now(),
            },
        };
        let serialized = serde_json::to_string(&entry).unwrap();
        let deserialized: SnapshotEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.id, "abc123def456");
        assert_eq!(deserialized.meta.message.as_deref(), Some("test message"));
        assert!(!deserialized.meta.pinned);
    }

    #[test]
    fn response_diff_ok_round_trip() {
        let resp = Response::DiffOk {
            changes: vec![
                DiffEntry {
                    path: "src/main.rs".to_string(),
                    change_type: ChangeType::Modified,
                    detail: Some("content changed".to_string()),
                },
                DiffEntry {
                    path: "new_file.txt".to_string(),
                    change_type: ChangeType::Added,
                    detail: None,
                },
            ],
        };
        let decoded = round_trip_response(&resp);
        match decoded {
            Response::DiffOk { changes } => {
                assert_eq!(changes.len(), 2);
                assert_eq!(changes[0].change_type, ChangeType::Modified);
                assert_eq!(changes[1].change_type, ChangeType::Added);
            }
            _ => panic!("expected DiffOk variant"),
        }
    }

    #[test]
    fn response_status_ok_round_trip() {
        let resp = Response::StatusOk {
            report: StatusReport {
                uptime_secs: 3600,
                workspaces: vec![WorkspaceInfo {
                    ws_id: "ws-abc".to_string(),
                    path: "/home/user/ws".to_string(),
                    snapshot_count: 5,
                }],
                fs_total_bytes: 1_000_000_000,
                fs_used_bytes: 500_000_000,
            },
        };
        let decoded = round_trip_response(&resp);
        match decoded {
            Response::StatusOk { report } => {
                assert_eq!(report.uptime_secs, 3600);
                assert_eq!(report.workspaces.len(), 1);
                assert_eq!(report.workspaces[0].ws_id, "ws-abc");
                assert_eq!(report.fs_total_bytes, 1_000_000_000);
                assert_eq!(report.fs_used_bytes, 500_000_000);
            }
            _ => panic!("expected StatusOk variant"),
        }
    }

    #[test]
    fn response_cleanup_ok_round_trip() {
        let resp = Response::CleanupOk {
            removed: vec!["msg1-step0".to_string(), "msg1-step1".to_string()],
        };
        let decoded = round_trip_response(&resp);
        match decoded {
            Response::CleanupOk { removed } => {
                assert_eq!(removed.len(), 2);
                assert_eq!(removed[0], "msg1-step0");
                assert_eq!(removed[1], "msg1-step1");
            }
            _ => panic!("expected CleanupOk variant"),
        }
    }

    #[test]
    fn request_config_round_trip() {
        let req = Request::Config;
        let decoded = round_trip_request(&req);
        assert!(matches!(decoded, Request::Config));
    }

    #[test]
    fn response_config_ok_round_trip() {
        let resp = Response::ConfigOk {
            config: ConfigReport {
                mount_path: "/mnt/btrfs-workspace".to_string(),
                socket_path: "/run/ws-ckpt/ws-ckpt.sock".to_string(),
                log_level: "info".to_string(),
                auto_cleanup: false,
                auto_cleanup_keep: 20,
                auto_cleanup_interval_secs: 600,
                health_check_interval_secs: 300,
                fs_warn_threshold_percent: 90.0,
                img_path: "/data/ws-ckpt/btrfs-data.img".to_string(),
                img_size: 30,
                img_max_percent: 40.0,
            },
        };
        let decoded = round_trip_response(&resp);
        match decoded {
            Response::ConfigOk { config } => {
                assert_eq!(config.mount_path, "/mnt/btrfs-workspace");
                assert_eq!(config.auto_cleanup_keep, 20);
                assert_eq!(config.auto_cleanup_interval_secs, 600);
            }
            _ => panic!("expected ConfigOk variant"),
        }
    }

    #[test]
    fn request_reload_config_round_trip() {
        let req = Request::ReloadConfig;
        let decoded = round_trip_request(&req);
        assert!(matches!(decoded, Request::ReloadConfig));
    }

    #[test]
    fn response_reload_config_ok_round_trip() {
        let resp = Response::ReloadConfigOk;
        let decoded = round_trip_response(&resp);
        assert!(matches!(decoded, Response::ReloadConfigOk));
    }

    // ── FileConfig tests ──

    #[test]
    fn file_config_toml_round_trip() {
        let fc = FileConfig {
            auto_cleanup_keep: Some(30),
            auto_cleanup_interval_secs: Some(300),
            health_check_interval_secs: Some(180),
            ..Default::default()
        };
        let s = toml::to_string(&fc).unwrap();
        let parsed: FileConfig = toml::from_str(&s).unwrap();
        assert_eq!(parsed, fc);
    }

    #[test]
    fn file_config_partial_toml() {
        let s = "auto_cleanup_keep = 50\n";
        let fc: FileConfig = toml::from_str(s).unwrap();
        assert_eq!(fc.auto_cleanup_keep, Some(50));
        assert_eq!(fc.auto_cleanup_interval_secs, None);
        assert_eq!(fc.health_check_interval_secs, None);
    }

    #[test]
    fn file_config_empty_toml() {
        let fc: FileConfig = toml::from_str("").unwrap();
        assert_eq!(fc, FileConfig::default());
    }

    #[test]
    fn load_config_file_nonexistent_returns_default() {
        let fc = load_config_file(Path::new("/nonexistent/config.toml")).unwrap();
        assert_eq!(fc, FileConfig::default());
    }

    #[test]
    fn save_and_load_config_file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let fc = FileConfig {
            auto_cleanup_keep: Some(15),
            auto_cleanup_interval_secs: Some(120),
            health_check_interval_secs: Some(60),
            ..Default::default()
        };
        save_config_file(&path, &fc).unwrap();
        let loaded = load_config_file(&path).unwrap();
        assert_eq!(loaded, fc);
    }

    #[test]
    fn save_config_file_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("dir").join("config.toml");
        let fc = FileConfig {
            auto_cleanup_keep: Some(5),
            auto_cleanup_interval_secs: None,
            health_check_interval_secs: None,
            ..Default::default()
        };
        save_config_file(&path, &fc).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn load_config_file_empty_file_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.toml");
        std::fs::write(&path, "").unwrap();
        let fc = load_config_file(&path).unwrap();
        assert_eq!(fc, FileConfig::default());
    }

    #[test]
    fn load_config_file_invalid_toml_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "not = [valid toml {{").unwrap();
        let result = load_config_file(&path);
        assert!(result.is_err());
    }

    // ── Recover round-trip tests ──

    #[test]
    fn request_recover_round_trip() {
        let req = Request::Recover {
            workspace: "/tmp/my-project".to_string(),
        };
        let decoded = round_trip_request(&req);
        match decoded {
            Request::Recover { workspace } => assert_eq!(workspace, "/tmp/my-project"),
            _ => panic!("expected Recover variant"),
        }
    }

    #[test]
    fn response_recover_ok_round_trip() {
        let resp = Response::RecoverOk {
            workspace: "/home/user/project".to_string(),
        };
        let decoded = round_trip_response(&resp);
        match decoded {
            Response::RecoverOk { workspace } => assert_eq!(workspace, "/home/user/project"),
            _ => panic!("expected RecoverOk variant"),
        }
    }
}
