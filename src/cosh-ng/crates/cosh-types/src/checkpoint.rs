//! Types for workspace checkpoint operations (ws-ckpt daemon IPC).
//!
//! These types mirror the ws-ckpt daemon protocol exactly.
//! The enum variant order is critical — bincode serializes enums by index.

use serde::{Deserialize, Serialize};

/// Default socket path for ws-ckpt daemon.
pub const DEFAULT_SOCKET_PATH: &str = "/run/ws-ckpt/ws-ckpt.sock";

// ===========================================================================
// Wire protocol types (must match ws-ckpt-common exactly)
// ===========================================================================

/// Request sent to ws-ckpt daemon over Unix socket (bincode wire format).
/// CRITICAL: variant order must match ws-ckpt-common/src/lib.rs exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WsCkptRequest {
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
    Config,
    ReloadConfig,
    Recover {
        workspace: String,
    },
    HealthAdvisory,
}

/// Response received from ws-ckpt daemon (bincode wire format).
/// CRITICAL: variant order must match ws-ckpt-common/src/lib.rs exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WsCkptResponse {
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
        code: WsCkptErrorCode,
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
        over_limit_workspace_count: u32,
        fs_total_bytes: u64,
        fs_used_bytes: u64,
    },
}

/// Error codes from ws-ckpt daemon.
/// CRITICAL: variant order must match ws-ckpt-common/src/lib.rs exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WsCkptErrorCode {
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

// ===========================================================================
// Auxiliary types (match ws-ckpt-common)
// ===========================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotEntry {
    pub id: String,
    pub workspace: String,
    pub meta: SnapshotMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMeta {
    pub message: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub pinned: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeType {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffEntry {
    pub path: String,
    pub change_type: ChangeType,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusReport {
    pub uptime_secs: u64,
    pub workspaces: Vec<WorkspaceInfo>,
    pub fs_total_bytes: u64,
    pub fs_used_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub ws_id: String,
    pub path: String,
    pub snapshot_count: u32,
}

/// Cleanup retention policy — mirrors ws-ckpt-common exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CleanupRetention {
    Count(u32),
    Age { raw: String, secs: u64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigReport {
    pub mount_path: String,
    pub socket_path: String,
    pub log_level: String,
    pub auto_cleanup: bool,
    pub auto_cleanup_keep: CleanupRetention,
    pub auto_cleanup_interval_secs: u64,
    pub health_check_interval_secs: u64,
    pub img_path: String,
    pub img_size: u64,
    pub img_max_percent: f64,
}

// ===========================================================================
// CLI output types (used for CoshResponse mapping)
// ===========================================================================

/// Result of creating a checkpoint (CLI display layer).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CkptCreated {
    /// Daemon-assigned identifier, present exactly when a snapshot was created.
    pub snapshot_id: Option<String>,
    /// Original request path, preserved even when the daemon skips creation.
    pub workspace: String,
    /// Indicates the daemon accepted the request but created no snapshot.
    #[serde(default)]
    pub skipped: bool,
    /// Daemon explanation, present exactly when creation was skipped.
    pub reason: Option<String>,
}

/// A single checkpoint entry (CLI display layer).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CkptEntry {
    pub id: String,
    pub workspace: String,
    pub message: Option<String>,
    pub pinned: bool,
    pub created_at: String,
}

/// Result of listing checkpoints (CLI display layer).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CkptListResult {
    pub snapshots: Vec<CkptEntry>,
    pub total: usize,
}

/// Result of restoring a checkpoint (CLI display layer).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CkptRestored {
    pub from: String,
    pub to: String,
}

/// Result of querying workspace checkpoint status (CLI display layer).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CkptStatusResult {
    pub uptime_secs: u64,
    pub workspaces: Vec<WorkspaceInfo>,
    pub fs_total_bytes: u64,
    pub fs_used_bytes: u64,
}

/// Result of deleting a snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CkptDeleted {
    pub target: String,
}

/// Result of a diff operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CkptDiffResult {
    pub changes: Vec<DiffEntry>,
}

/// Result of a cleanup operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CkptCleanupResult {
    pub removed: Vec<String>,
}

/// Result of init operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CkptInitResult {
    pub ws_id: String,
}

/// Result of recover operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CkptRecoverResult {
    pub workspace: String,
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ckpt_created_deserializes_historical_success_json() {
        let created: CkptCreated = serde_json::from_value(serde_json::json!({
            "snapshot_id": "snap-1",
            "workspace": "/tmp/ws",
        }))
        .unwrap();

        assert_eq!(created.snapshot_id.as_deref(), Some("snap-1"));
        assert_eq!(created.workspace, "/tmp/ws");
        assert!(!created.skipped);
        assert!(created.reason.is_none());
    }

    #[test]
    fn test_request_bincode_roundtrip() {
        let requests = vec![
            WsCkptRequest::Init {
                workspace: "/home/user/project".into(),
            },
            WsCkptRequest::Checkpoint {
                workspace: "/home/user/project".into(),
                id: "snap-001".into(),
                message: Some("initial checkpoint".into()),
                metadata: Some(r#"{"key":"val"}"#.into()),
                pin: true,
            },
            WsCkptRequest::Rollback {
                workspace: "/tmp/ws".into(),
                to: "snap-001".into(),
            },
            WsCkptRequest::Delete {
                workspace: Some("/tmp/ws".into()),
                snapshot: "snap-001".into(),
                force: false,
            },
            WsCkptRequest::List {
                workspace: Some("/tmp/ws".into()),
                format: None,
            },
            WsCkptRequest::Diff {
                workspace: "/tmp/ws".into(),
                from: "snap-001".into(),
                to: "snap-002".into(),
            },
            WsCkptRequest::Status { workspace: None },
            WsCkptRequest::Cleanup {
                workspace: "/tmp/ws".into(),
                keep: Some(5),
            },
            WsCkptRequest::Config,
            WsCkptRequest::ReloadConfig,
            WsCkptRequest::Recover {
                workspace: "/home/user/project".into(),
            },
            WsCkptRequest::HealthAdvisory,
        ];

        for req in &requests {
            let encoded = bincode::serialize(req).unwrap();
            let decoded: WsCkptRequest = bincode::deserialize(&encoded).unwrap();
            // Verify roundtrip by re-encoding
            let re_encoded = bincode::serialize(&decoded).unwrap();
            assert_eq!(encoded, re_encoded);
        }
    }

    #[test]
    fn test_response_bincode_roundtrip() {
        let responses = vec![
            WsCkptResponse::InitOk {
                ws_id: "ws-abc".into(),
            },
            WsCkptResponse::CheckpointOk {
                snapshot_id: "snap-001".into(),
            },
            WsCkptResponse::RollbackOk {
                from: "snap-003".into(),
                to: "snap-001".into(),
            },
            WsCkptResponse::DeleteOk {
                target: "snap-001".into(),
            },
            WsCkptResponse::Error {
                code: WsCkptErrorCode::WorkspaceNotFound,
                message: "not found".into(),
            },
            WsCkptResponse::ListOk { snapshots: vec![] },
            WsCkptResponse::DiffOk {
                changes: vec![DiffEntry {
                    path: "src/main.rs".into(),
                    change_type: ChangeType::Modified,
                    detail: None,
                }],
            },
            WsCkptResponse::StatusOk {
                report: StatusReport {
                    uptime_secs: 3600,
                    workspaces: vec![WorkspaceInfo {
                        ws_id: "ws-1".into(),
                        path: "/tmp".into(),
                        snapshot_count: 3,
                    }],
                    fs_total_bytes: 100_000_000,
                    fs_used_bytes: 50_000_000,
                },
            },
            WsCkptResponse::CleanupOk {
                removed: vec!["snap-old".into()],
            },
            WsCkptResponse::ConfigOk {
                config: ConfigReport {
                    mount_path: "/mnt/snapshots".into(),
                    socket_path: "/run/ws-ckpt/ws-ckpt.sock".into(),
                    log_level: "info".into(),
                    auto_cleanup: true,
                    auto_cleanup_keep: CleanupRetention::Count(5),
                    auto_cleanup_interval_secs: 3600,
                    health_check_interval_secs: 60,
                    img_path: "/var/lib/ws-ckpt/img".into(),
                    img_size: 10_737_418_240,
                    img_max_percent: 80.0,
                },
            },
            WsCkptResponse::ReloadConfigOk,
            WsCkptResponse::CheckpointSkipped {
                reason: "no changes".into(),
            },
            WsCkptResponse::RecoverOk {
                workspace: "/tmp/ws".into(),
            },
            WsCkptResponse::HealthAdvisoryOk {
                over_limit_workspace_count: 2,
                fs_total_bytes: 1_000_000,
                fs_used_bytes: 800_000,
            },
        ];

        for resp in &responses {
            let encoded = bincode::serialize(resp).unwrap();
            let decoded: WsCkptResponse = bincode::deserialize(&encoded).unwrap();
            let re_encoded = bincode::serialize(&decoded).unwrap();
            assert_eq!(encoded, re_encoded);
        }
    }

    #[test]
    fn test_request_bincode_variant_index() {
        // Verify that each WsCkptRequest variant is serialized with the correct
        // bincode index — this is the wire contract with ws-ckpt daemon.
        let variants: Vec<(u32, WsCkptRequest)> = vec![
            (
                0,
                WsCkptRequest::Init {
                    workspace: "/ws".into(),
                },
            ),
            (
                1,
                WsCkptRequest::Checkpoint {
                    workspace: "/ws".into(),
                    id: "snap".into(),
                    message: None,
                    metadata: None,
                    pin: false,
                },
            ),
            (
                2,
                WsCkptRequest::Rollback {
                    workspace: "/ws".into(),
                    to: "snap".into(),
                },
            ),
            (
                3,
                WsCkptRequest::Delete {
                    workspace: None,
                    snapshot: "snap".into(),
                    force: false,
                },
            ),
            (
                4,
                WsCkptRequest::List {
                    workspace: None,
                    format: None,
                },
            ),
            (
                5,
                WsCkptRequest::Diff {
                    workspace: "/ws".into(),
                    from: "a".into(),
                    to: "b".into(),
                },
            ),
            (6, WsCkptRequest::Status { workspace: None }),
            (
                7,
                WsCkptRequest::Cleanup {
                    workspace: "/ws".into(),
                    keep: None,
                },
            ),
            (8, WsCkptRequest::Config),
            (9, WsCkptRequest::ReloadConfig),
            (
                10,
                WsCkptRequest::Recover {
                    workspace: "/ws".into(),
                },
            ),
            (11, WsCkptRequest::HealthAdvisory),
        ];

        for (expected_idx, req) in &variants {
            let encoded = bincode::serialize(req).unwrap();
            let variant_idx = u32::from_le_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]);
            assert_eq!(
                variant_idx, *expected_idx,
                "WsCkptRequest variant index mismatch: expected {}, got {}",
                expected_idx, variant_idx
            );
        }
    }

    #[test]
    fn test_error_code_bincode_index_order() {
        // Verify that bincode serializes enum variants by index (0, 1, 2, ...)
        let codes = vec![
            WsCkptErrorCode::WorkspaceNotFound,
            WsCkptErrorCode::SnapshotNotFound,
            WsCkptErrorCode::AlreadyInitialized,
            WsCkptErrorCode::BtrfsError,
            WsCkptErrorCode::IoError,
            WsCkptErrorCode::InvalidPath,
            WsCkptErrorCode::ConfirmationRequired,
            WsCkptErrorCode::InternalError,
            WsCkptErrorCode::SnapshotAlreadyExists,
            WsCkptErrorCode::WriteLockConflict,
            WsCkptErrorCode::DiskSpaceInsufficient,
        ];

        for (idx, code) in codes.iter().enumerate() {
            let encoded = bincode::serialize(code).unwrap();
            // bincode 1.x encodes enums as u32 index
            let variant_idx = u32::from_le_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]);
            assert_eq!(variant_idx, idx as u32, "ErrorCode variant index mismatch");
        }
    }

    #[test]
    fn test_default_socket_path() {
        assert_eq!(DEFAULT_SOCKET_PATH, "/run/ws-ckpt/ws-ckpt.sock");
    }

    #[test]
    fn test_cleanup_retention_bincode_roundtrip() {
        let count = CleanupRetention::Count(5);
        let bytes = bincode::serialize(&count).unwrap();
        let decoded: CleanupRetention = bincode::deserialize(&bytes).unwrap();
        assert_eq!(decoded, CleanupRetention::Count(5));

        let age = CleanupRetention::Age {
            raw: "7d".into(),
            secs: 604800,
        };
        let bytes = bincode::serialize(&age).unwrap();
        let decoded: CleanupRetention = bincode::deserialize(&bytes).unwrap();
        assert_eq!(
            decoded,
            CleanupRetention::Age {
                raw: "7d".into(),
                secs: 604800
            }
        );
    }

    #[test]
    fn test_config_report_bincode_roundtrip() {
        let report = ConfigReport {
            mount_path: "/mnt/ws-ckpt".into(),
            socket_path: "/run/ws-ckpt/ws-ckpt.sock".into(),
            log_level: "info".into(),
            auto_cleanup: true,
            auto_cleanup_keep: CleanupRetention::Count(10),
            auto_cleanup_interval_secs: 3600,
            health_check_interval_secs: 60,
            img_path: "/var/lib/ws-ckpt.img".into(),
            img_size: 536870912,
            img_max_percent: 80.0,
        };
        let bytes = bincode::serialize(&report).unwrap();
        let decoded: ConfigReport = bincode::deserialize(&bytes).unwrap();
        assert_eq!(decoded.mount_path, "/mnt/ws-ckpt");
        assert_eq!(decoded.socket_path, "/run/ws-ckpt/ws-ckpt.sock");
        assert_eq!(decoded.log_level, "info");
        assert!(decoded.auto_cleanup);
        assert_eq!(decoded.auto_cleanup_keep, CleanupRetention::Count(10));
        assert_eq!(decoded.auto_cleanup_interval_secs, 3600);
        assert_eq!(decoded.health_check_interval_secs, 60);
        assert_eq!(decoded.img_path, "/var/lib/ws-ckpt.img");
        assert_eq!(decoded.img_size, 536870912);
        assert_eq!(decoded.img_max_percent, 80.0);
    }
}
