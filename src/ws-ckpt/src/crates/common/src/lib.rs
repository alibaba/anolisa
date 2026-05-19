use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thiserror::Error;

pub mod backend;
pub mod migration;
pub mod persist;

use backend::BackendType;

// ── Constants ──

pub const DEFAULT_MOUNT_PATH: &str = "/mnt/btrfs-workspace";
pub const DEFAULT_SOCKET_PATH: &str = "/run/ws-ckpt/ws-ckpt.sock";
pub const SNAPSHOTS_DIR: &str = "snapshots";
pub const INDEX_FILE: &str = "index.json";
pub const BTRFS_IMG_PATH: &str = "/var/lib/ws-ckpt/btrfs-data.img";
pub const BTRFS_IMG_DIR: &str = "/var/lib/ws-ckpt";
/// Pre-FHS-migration location (kept for one-shot in-daemon migration on upgrade).
pub const LEGACY_BTRFS_IMG_PATH: &str = "/data/ws-ckpt/btrfs-data.img";
pub const CONFIG_FILE_PATH: &str = "/etc/ws-ckpt/config.toml";
pub const DEFAULT_IMG_SIZE_GB: u64 = 30;
pub const DEFAULT_IMG_MAX_PERCENT: f64 = 0.4; // 40% as fraction for calculation
pub const DEFAULT_STATE_DIR: &str = "/var/lib/ws-ckpt"; // systemd StateDirectory
pub const STATE_FILE: &str = "state.json"; // daemon state file
pub const INDEXES_DIR: &str = "indexes"; // snapshots indexes directory
pub const LOCKFILE_NAME: &str = "daemon.lock"; // daemon write lockfile

/// Snapshot advisory threshold; strict-greater filter shared by daemon and CLI.
pub const ADVISORY_SNAPSHOT_LIMIT: u32 = 1000;

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
    /// Aggregated health metrics for post-command CLI advisories.
    HealthAdvisory,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Response {
    InitOk {
        ws_id: String,
    },
    CheckpointOk {
        snapshot_id: String,
    },
    RollbackOk {
        from: String,
        to: String,
    },
    DeleteOk {
        target: String,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
    ListOk {
        snapshots: Vec<SnapshotEntry>,
    },
    DiffOk {
        changes: Vec<DiffEntry>,
    },
    StatusOk {
        report: StatusReport,
    },
    CleanupOk {
        removed: Vec<String>,
    },
    ConfigOk {
        config: ConfigReport,
    },
    ReloadConfigOk,
    CheckpointSkipped {
        reason: String,
    },
    RecoverOk {
        workspace: String,
    },
    HealthAdvisoryOk {
        /// Count of workspaces exceeding `ADVISORY_SNAPSHOT_LIMIT`.
        over_limit_workspace_count: u32,
        /// Backend usage bytes; `fs_total_bytes == 0` sentinel means unavailable.
        fs_total_bytes: u64,
        fs_used_bytes: u64,
    },
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
    /// Is the subvolume missing in the filesystem (detected in reconcile)
    #[serde(default)]
    pub missing: bool,
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

/// Auto-cleanup retention policy (mutually-exclusive modes):
/// - `Count(N)` ← TOML integer: keep N most-recent non-pinned snapshots.
/// - `Age { raw, secs }` ← TOML string (`"30d"`, units `s/m/h/d/w`): purge non-pinned
///   snapshots older than `secs`. `raw` is the user's original string (round-trip +
///   display); `secs` is pre-parsed once at deserialize time. Strict — no count floor.
///
/// Invariant (Age): `parse_duration_secs(&raw) == Ok(secs)`, enforced by the only
/// public constructor [`CleanupRetention::age`] and by Deserialize.
///
/// Serde: bincode lacks `deserialize_any`, so we dispatch on `is_human_readable()` —
/// TOML/JSON use raw u32/String + Visitor; bincode uses a tagged wire enum carrying
/// only `raw` (secs re-derived on receive).
#[derive(Debug, Clone, PartialEq)]
pub enum CleanupRetention {
    /// Count mode: keep N most recent non-pinned snapshots (0 = disabled).
    Count(u32),
    /// Age mode: keep snapshots within `secs` seconds. `raw` is the user's original
    /// string (e.g. "30d", "2w") preserved for round-trip and display.
    Age { raw: String, secs: u64 },
}

impl CleanupRetention {
    /// Construct an [`Age`](Self::Age) variant from a duration string, parsing and
    /// caching the seconds value. Returns an error if `raw` is not a valid duration.
    pub fn age(raw: impl Into<String>) -> Result<Self, String> {
        let raw = raw.into();
        let secs = parse_duration_secs(&raw)?;
        Ok(Self::Age { raw, secs })
    }

    /// Whether this retention policy disables auto-cleanup entirely:
    /// `Count(0)` or `Age { secs: 0, .. }`.
    pub fn is_disabled(&self) -> bool {
        matches!(self, Self::Count(0) | Self::Age { secs: 0, .. })
    }
}

/// Tagged wire representation used for binary (bincode) encoding only.
/// Only `raw` is transported; `secs` is re-derived from `raw` on the receiving side.
#[derive(Serialize, Deserialize)]
enum CleanupRetentionWire {
    Count(u32),
    Age(String),
}

impl Serialize for CleanupRetention {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        if ser.is_human_readable() {
            match self {
                Self::Count(n) => ser.serialize_u32(*n),
                Self::Age { raw, .. } => ser.serialize_str(raw),
            }
        } else {
            let wire = match self {
                Self::Count(n) => CleanupRetentionWire::Count(*n),
                Self::Age { raw, .. } => CleanupRetentionWire::Age(raw.clone()),
            };
            wire.serialize(ser)
        }
    }
}

impl<'de> Deserialize<'de> for CleanupRetention {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        if de.is_human_readable() {
            struct V;
            impl<'de> serde::de::Visitor<'de> for V {
                type Value = CleanupRetention;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    write!(
                        f,
                        "a non-negative integer (count mode) or a duration string like \"30d\" (age mode)"
                    )
                }
                fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                    if v > u32::MAX as u64 {
                        return Err(E::custom(format!(
                            "auto_cleanup_keep count {} exceeds u32::MAX",
                            v
                        )));
                    }
                    Ok(CleanupRetention::Count(v as u32))
                }
                fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                    if !(0..=u32::MAX as i64).contains(&v) {
                        return Err(E::custom(format!(
                            "auto_cleanup_keep count {} out of u32 range",
                            v
                        )));
                    }
                    Ok(CleanupRetention::Count(v as u32))
                }
                fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                    // Pre-validate + cache the seconds value so the value is rejected
                    // at config load / reload time and the runtime path avoids re-parsing.
                    CleanupRetention::age(v)
                        .map_err(|e| E::custom(format!("auto_cleanup_keep age mode: {}", e)))
                }
                fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
                    CleanupRetention::age(v)
                        .map_err(|e| E::custom(format!("auto_cleanup_keep age mode: {}", e)))
                }
            }
            de.deserialize_any(V)
        } else {
            let wire = CleanupRetentionWire::deserialize(de)?;
            match wire {
                CleanupRetentionWire::Count(n) => Ok(CleanupRetention::Count(n)),
                CleanupRetentionWire::Age(raw) => CleanupRetention::age(raw).map_err(|e| {
                    serde::de::Error::custom(format!("auto_cleanup_keep age mode: {}", e))
                }),
            }
        }
    }
}

impl std::fmt::Display for CleanupRetention {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Count(n) => write!(f, "{}", n),
            Self::Age { raw, .. } => write!(f, "\"{}\"", raw),
        }
    }
}

/// Parse a duration string like `30d`, `2w`, `3600s`, `5m`, `12h` into seconds.
/// Bare numbers without a unit are rejected to force explicit semantics.
pub fn parse_duration_secs(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty duration".to_string());
    }
    let bytes = s.as_bytes();
    let last = bytes[bytes.len() - 1];
    if !last.is_ascii_alphabetic() {
        return Err(format!("duration '{}' missing unit suffix (s/m/h/d/w)", s));
    }
    let (num_str, unit) = s.split_at(s.len() - 1);
    let n: u64 = num_str
        .parse()
        .map_err(|_| format!("duration '{}': invalid number '{}'", s, num_str))?;
    let secs = match unit.to_ascii_lowercase().as_str() {
        "s" => n,
        "m" => n.saturating_mul(60),
        "h" => n.saturating_mul(3600),
        "d" => n.saturating_mul(86400),
        "w" => n.saturating_mul(604800),
        u => {
            return Err(format!(
                "duration '{}': invalid unit '{}' (expected s/m/h/d/w)",
                s, u
            ))
        }
    };
    // Reject values that would overflow i64 when downstream uses chrono::Duration::seconds.
    if secs > i64::MAX as u64 {
        return Err(format!(
            "duration '{}' too large (max supported is {} seconds \u{2248} {} years)",
            s,
            i64::MAX,
            i64::MAX / (365 * 86400),
        ));
    }
    Ok(secs)
}

/// Report of the current daemon configuration (returned by `Config` request).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ConfigReport {
    pub mount_path: String,
    pub socket_path: String,
    pub log_level: String,
    pub auto_cleanup: bool,
    pub auto_cleanup_keep: CleanupRetention,
    pub auto_cleanup_interval_secs: u64,
    pub health_check_interval_secs: u64,
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
    pub auto_cleanup_keep: CleanupRetention,
    /// Interval in seconds between auto-cleanup runs
    pub auto_cleanup_interval_secs: u64,
    /// Interval in seconds between health checks
    pub health_check_interval_secs: u64,
    /// Backend type string from config: "auto" | "btrfs-base" | "btrfs-loop"
    pub backend_type: String,
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
            _ => None, // "auto" or unknown → auto-detect
        }
    }
}

pub const DEFAULT_AUTO_CLEANUP: bool = false;
/// Default count for `CleanupRetention::Count` when no retention is configured.
pub const DEFAULT_AUTO_CLEANUP_KEEP_COUNT: u32 = 20;
/// Factory for the default `CleanupRetention` (Count mode, 20 snapshots).
pub fn default_auto_cleanup_keep() -> CleanupRetention {
    CleanupRetention::Count(DEFAULT_AUTO_CLEANUP_KEEP_COUNT)
}
/// Default interval between auto-cleanup runs: 24 hours (86_400 seconds).
pub const DEFAULT_AUTO_CLEANUP_INTERVAL_SECS: u64 = 86_400;
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
    /// "auto" | "btrfs-base" | "btrfs-loop"
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
    pub auto_cleanup_keep: Option<CleanupRetention>,
    pub auto_cleanup_interval_secs: Option<u64>,
    pub health_check_interval_secs: Option<u64>,
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
            validate_file_config(&fc, path)?;
            Ok(fc)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(FileConfig::default()),
        Err(e) => Err(WsCkptError::Io(e)),
    }
}

/// Validate numeric ranges in a loaded `FileConfig` so downstream consumers
/// (e.g. bootstrap's `f64 -> u64` cast on `avail * img_max_percent / 100.0`)
/// never see NaN/Infinity/out-of-range values.
fn validate_file_config(fc: &FileConfig, path: &Path) -> Result<(), WsCkptError> {
    if let Some(loop_cfg) = &fc.backend.btrfs_loop {
        if let Some(pct) = loop_cfg.img_max_percent {
            if !pct.is_finite() || !(0.0..=100.0).contains(&pct) {
                return Err(WsCkptError::Config(format!(
                    "backend.btrfs-loop.img_max_percent in {}: expected a finite value in 0.0..=100.0 (got {})",
                    path.display(),
                    pct
                )));
            }
        }
    }
    Ok(())
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
            auto_cleanup_keep: default_auto_cleanup_keep(),
            auto_cleanup_interval_secs: DEFAULT_AUTO_CLEANUP_INTERVAL_SECS,
            health_check_interval_secs: DEFAULT_HEALTH_CHECK_INTERVAL_SECS,
            backend_type: "auto".to_string(),
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
                missing: false,
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
                missing: false,
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
                missing: false,
            },
        );
        idx.snapshots.insert(
            "abcdef2222222222222222222222222222222222".to_string(),
            SnapshotMeta {
                message: None,
                metadata: None,
                pinned: false,
                created_at: chrono::Utc::now(),
                missing: false,
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
                    missing: false,
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
                missing: false,
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
                auto_cleanup_keep: CleanupRetention::Count(20),
                auto_cleanup_interval_secs: 86_400,
                health_check_interval_secs: 300,
                img_size: 30,
                img_max_percent: 40.0,
            },
        };
        let decoded = round_trip_response(&resp);
        match decoded {
            Response::ConfigOk { config } => {
                assert_eq!(config.mount_path, "/mnt/btrfs-workspace");
                assert_eq!(config.auto_cleanup_keep, CleanupRetention::Count(20));
                assert_eq!(config.auto_cleanup_interval_secs, 86_400);
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
            auto_cleanup_keep: Some(CleanupRetention::Count(30)),
            auto_cleanup_interval_secs: Some(300),
            health_check_interval_secs: Some(180),
            ..Default::default()
        };
        let s = toml::to_string(&fc).unwrap();
        let parsed: FileConfig = toml::from_str(&s).unwrap();
        assert_eq!(parsed, fc);
    }

    #[test]
    fn file_config_toml_age_mode_round_trip() {
        let fc = FileConfig {
            auto_cleanup_keep: Some(CleanupRetention::age("30d").unwrap()),
            ..Default::default()
        };
        let s = toml::to_string(&fc).unwrap();
        let parsed: FileConfig = toml::from_str(&s).unwrap();
        assert_eq!(parsed, fc);
    }

    #[test]
    fn file_config_toml_rejects_invalid_age_string() {
        // Bare number (missing unit suffix)
        let err = toml::from_str::<FileConfig>("auto_cleanup_keep = \"10\"\n").unwrap_err();
        assert!(
            err.to_string().contains("missing unit suffix"),
            "expected missing-unit error, got: {}",
            err
        );
        // Unknown unit
        let err = toml::from_str::<FileConfig>("auto_cleanup_keep = \"30x\"\n").unwrap_err();
        assert!(
            err.to_string().contains("invalid unit"),
            "expected invalid-unit error, got: {}",
            err
        );
        // Garbage
        let err = toml::from_str::<FileConfig>("auto_cleanup_keep = \"abc\"\n").unwrap_err();
        assert!(
            err.to_string().contains("invalid number"),
            "expected invalid-number error, got: {}",
            err
        );
    }

    #[test]
    fn file_config_partial_toml() {
        let s = "auto_cleanup_keep = 50\n";
        let fc: FileConfig = toml::from_str(s).unwrap();
        assert_eq!(fc.auto_cleanup_keep, Some(CleanupRetention::Count(50)));
        assert_eq!(fc.auto_cleanup_interval_secs, None);
        assert_eq!(fc.health_check_interval_secs, None);
    }

    #[test]
    fn file_config_age_mode_toml() {
        let s = "auto_cleanup_keep = \"2w\"\n";
        let fc: FileConfig = toml::from_str(s).unwrap();
        assert_eq!(
            fc.auto_cleanup_keep,
            Some(CleanupRetention::age("2w").unwrap())
        );
    }

    #[test]
    fn parse_duration_accepts_units() {
        assert_eq!(parse_duration_secs("30s").unwrap(), 30);
        assert_eq!(parse_duration_secs("5m").unwrap(), 300);
        assert_eq!(parse_duration_secs("2h").unwrap(), 7200);
        assert_eq!(parse_duration_secs("30d").unwrap(), 2_592_000);
        assert_eq!(parse_duration_secs("2w").unwrap(), 1_209_600);
    }

    #[test]
    fn parse_duration_rejects_bad_input() {
        assert!(parse_duration_secs("").is_err());
        assert!(parse_duration_secs("30").is_err()); // missing unit
        assert!(parse_duration_secs("abc").is_err());
        assert!(parse_duration_secs("30y").is_err()); // year not supported
    }

    #[test]
    fn parse_duration_rejects_i64_overflow() {
        // u64::MAX weeks clearly saturates past i64::MAX and must be rejected
        // so downstream `chrono::Duration::seconds(secs as i64)` stays safe.
        let huge = format!("{}w", u64::MAX);
        assert!(parse_duration_secs(&huge).is_err());
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
            auto_cleanup_keep: Some(CleanupRetention::Count(15)),
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
            auto_cleanup_keep: Some(CleanupRetention::Count(5)),
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

    #[test]
    fn load_config_file_rejects_img_max_percent_out_of_range() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["-1.0", "100.5", "nan", "inf"] {
            let path = dir.path().join(format!("bad_{}.toml", bad));
            let toml = format!("[backend.btrfs-loop]\nimg_max_percent = {}\n", bad);
            std::fs::write(&path, toml).unwrap();
            let result = load_config_file(&path);
            assert!(result.is_err(), "{} should be rejected", bad);
        }
    }

    #[test]
    fn load_config_file_accepts_img_max_percent_in_range() {
        let dir = tempfile::tempdir().unwrap();
        for good in ["0.0", "40.0", "100.0"] {
            let path = dir.path().join(format!("good_{}.toml", good));
            let toml = format!("[backend.btrfs-loop]\nimg_max_percent = {}\n", good);
            std::fs::write(&path, toml).unwrap();
            let result = load_config_file(&path);
            assert!(
                result.is_ok(),
                "{} should be accepted, got {:?}",
                good,
                result
            );
        }
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

    // ── HealthAdvisory round-trip tests ──

    #[test]
    fn request_health_advisory_round_trip() {
        let req = Request::HealthAdvisory;
        let decoded = round_trip_request(&req);
        match decoded {
            Request::HealthAdvisory => {}
            _ => panic!("expected HealthAdvisory variant"),
        }
    }

    #[test]
    fn response_health_advisory_ok_round_trip() {
        let resp = Response::HealthAdvisoryOk {
            over_limit_workspace_count: 3,
            fs_total_bytes: 100 * 1024 * 1024 * 1024,
            fs_used_bytes: 94 * 1024 * 1024 * 1024,
        };
        let decoded = round_trip_response(&resp);
        match decoded {
            Response::HealthAdvisoryOk {
                over_limit_workspace_count,
                fs_total_bytes,
                fs_used_bytes,
            } => {
                assert_eq!(over_limit_workspace_count, 3);
                assert_eq!(fs_total_bytes, 100 * 1024 * 1024 * 1024);
                assert_eq!(fs_used_bytes, 94 * 1024 * 1024 * 1024);
            }
            _ => panic!("expected HealthAdvisoryOk variant"),
        }
    }

    #[test]
    fn response_health_advisory_ok_zero_round_trip() {
        // Backend query failed or no workspace over limit: all zeros.
        let resp = Response::HealthAdvisoryOk {
            over_limit_workspace_count: 0,
            fs_total_bytes: 0,
            fs_used_bytes: 0,
        };
        let decoded = round_trip_response(&resp);
        match decoded {
            Response::HealthAdvisoryOk {
                over_limit_workspace_count,
                fs_total_bytes,
                fs_used_bytes,
            } => {
                assert_eq!(over_limit_workspace_count, 0);
                assert_eq!(fs_total_bytes, 0);
                assert_eq!(fs_used_bytes, 0);
            }
            _ => panic!("expected HealthAdvisoryOk variant"),
        }
    }
}
