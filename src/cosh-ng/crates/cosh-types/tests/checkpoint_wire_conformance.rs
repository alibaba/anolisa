use chrono::{TimeZone, Utc};
use cosh_types::checkpoint as local;
use ws_ckpt_common as wire;

fn assert_same_bincode<L: serde::Serialize, R: serde::Serialize>(left: &L, right: &R) {
    assert_eq!(
        bincode::serialize(left).unwrap(),
        bincode::serialize(right).unwrap()
    );
}

fn local_retention(value: &wire::CleanupRetention) -> local::CleanupRetention {
    match value {
        wire::CleanupRetention::Count(count) => local::CleanupRetention::Count(*count),
        wire::CleanupRetention::Age { raw, secs } => local::CleanupRetention::Age {
            raw: raw.clone(),
            secs: *secs,
        },
    }
}

fn wire_retention(value: &local::CleanupRetention) -> wire::CleanupRetention {
    match value {
        local::CleanupRetention::Count(count) => wire::CleanupRetention::Count(*count),
        local::CleanupRetention::Age { raw, secs } => wire::CleanupRetention::Age {
            raw: raw.clone(),
            secs: *secs,
        },
    }
}

fn local_bool_op(value: &wire::PolicyFieldOp<bool>) -> local::PolicyFieldOp<bool> {
    match value {
        wire::PolicyFieldOp::Unchanged => local::PolicyFieldOp::Unchanged,
        wire::PolicyFieldOp::Set(value) => local::PolicyFieldOp::Set(*value),
    }
}

fn wire_bool_op(value: &local::PolicyFieldOp<bool>) -> wire::PolicyFieldOp<bool> {
    match value {
        local::PolicyFieldOp::Unchanged => wire::PolicyFieldOp::Unchanged,
        local::PolicyFieldOp::Set(value) => wire::PolicyFieldOp::Set(*value),
    }
}

fn local_retention_op(
    value: &wire::PolicyFieldOp<wire::CleanupRetention>,
) -> local::PolicyFieldOp<local::CleanupRetention> {
    match value {
        wire::PolicyFieldOp::Unchanged => local::PolicyFieldOp::Unchanged,
        wire::PolicyFieldOp::Set(value) => local::PolicyFieldOp::Set(local_retention(value)),
    }
}

fn wire_retention_op(
    value: &local::PolicyFieldOp<local::CleanupRetention>,
) -> wire::PolicyFieldOp<wire::CleanupRetention> {
    match value {
        local::PolicyFieldOp::Unchanged => wire::PolicyFieldOp::Unchanged,
        local::PolicyFieldOp::Set(value) => wire::PolicyFieldOp::Set(wire_retention(value)),
    }
}

fn wire_request(value: &local::WsCkptRequest) -> wire::Request {
    match value {
        local::WsCkptRequest::Init { workspace } => wire::Request::Init {
            workspace: workspace.clone(),
        },
        local::WsCkptRequest::Checkpoint {
            workspace,
            id,
            message,
            metadata,
            pin,
        } => wire::Request::Checkpoint {
            workspace: workspace.clone(),
            id: id.clone(),
            message: message.clone(),
            metadata: metadata.clone(),
            pin: *pin,
        },
        local::WsCkptRequest::Rollback {
            workspace,
            to,
            num_ancestors,
        } => wire::Request::Rollback {
            workspace: workspace.clone(),
            to: to.clone(),
            num_ancestors: *num_ancestors,
        },
        local::WsCkptRequest::Delete {
            workspace,
            snapshot,
            force,
        } => wire::Request::Delete {
            workspace: workspace.clone(),
            snapshot: snapshot.clone(),
            force: *force,
        },
        local::WsCkptRequest::List { workspace, format } => wire::Request::List {
            workspace: workspace.clone(),
            format: format.clone(),
        },
        local::WsCkptRequest::Diff {
            workspace,
            from,
            to,
        } => wire::Request::Diff {
            workspace: workspace.clone(),
            from: from.clone(),
            to: to.clone(),
        },
        local::WsCkptRequest::Status { workspace } => wire::Request::Status {
            workspace: workspace.clone(),
        },
        local::WsCkptRequest::Cleanup { workspace, keep } => wire::Request::Cleanup {
            workspace: workspace.clone(),
            keep: *keep,
        },
        local::WsCkptRequest::Config => wire::Request::Config,
        local::WsCkptRequest::ReloadConfig => wire::Request::ReloadConfig,
        local::WsCkptRequest::ReloadGlobalConfig => wire::Request::ReloadGlobalConfig,
        local::WsCkptRequest::ReloadWorkspacePolicy { workspace } => {
            wire::Request::ReloadWorkspacePolicy {
                workspace: workspace.clone(),
            }
        }
        local::WsCkptRequest::ConfigOverview => wire::Request::ConfigOverview,
        local::WsCkptRequest::Recover { workspace } => wire::Request::Recover {
            workspace: workspace.clone(),
        },
        local::WsCkptRequest::HealthAdvisory => wire::Request::HealthAdvisory,
        local::WsCkptRequest::GetWorkspacePolicy { workspace } => {
            wire::Request::GetWorkspacePolicy {
                workspace: workspace.clone(),
            }
        }
        local::WsCkptRequest::ResetWorkspacePolicy { workspace } => {
            wire::Request::ResetWorkspacePolicy {
                workspace: workspace.clone(),
            }
        }
        local::WsCkptRequest::PatchWorkspacePolicy {
            workspace,
            auto_cleanup,
            auto_cleanup_keep,
        } => wire::Request::PatchWorkspacePolicy {
            workspace: workspace.clone(),
            auto_cleanup: wire_bool_op(auto_cleanup),
            auto_cleanup_keep: wire_retention_op(auto_cleanup_keep),
        },
        local::WsCkptRequest::RollbackPreview {
            workspace,
            to,
            num_ancestors,
        } => wire::Request::RollbackPreview {
            workspace: workspace.clone(),
            to: to.clone(),
            num_ancestors: *num_ancestors,
        },
    }
}

fn local_request(value: &wire::Request) -> local::WsCkptRequest {
    match value {
        wire::Request::Init { workspace } => local::WsCkptRequest::Init {
            workspace: workspace.clone(),
        },
        wire::Request::Checkpoint {
            workspace,
            id,
            message,
            metadata,
            pin,
        } => local::WsCkptRequest::Checkpoint {
            workspace: workspace.clone(),
            id: id.clone(),
            message: message.clone(),
            metadata: metadata.clone(),
            pin: *pin,
        },
        wire::Request::Rollback {
            workspace,
            to,
            num_ancestors,
        } => local::WsCkptRequest::Rollback {
            workspace: workspace.clone(),
            to: to.clone(),
            num_ancestors: *num_ancestors,
        },
        wire::Request::Delete {
            workspace,
            snapshot,
            force,
        } => local::WsCkptRequest::Delete {
            workspace: workspace.clone(),
            snapshot: snapshot.clone(),
            force: *force,
        },
        wire::Request::List { workspace, format } => local::WsCkptRequest::List {
            workspace: workspace.clone(),
            format: format.clone(),
        },
        wire::Request::Diff {
            workspace,
            from,
            to,
        } => local::WsCkptRequest::Diff {
            workspace: workspace.clone(),
            from: from.clone(),
            to: to.clone(),
        },
        wire::Request::Status { workspace } => local::WsCkptRequest::Status {
            workspace: workspace.clone(),
        },
        wire::Request::Cleanup { workspace, keep } => local::WsCkptRequest::Cleanup {
            workspace: workspace.clone(),
            keep: *keep,
        },
        wire::Request::Config => local::WsCkptRequest::Config,
        wire::Request::ReloadConfig => local::WsCkptRequest::ReloadConfig,
        wire::Request::ReloadGlobalConfig => local::WsCkptRequest::ReloadGlobalConfig,
        wire::Request::ReloadWorkspacePolicy { workspace } => {
            local::WsCkptRequest::ReloadWorkspacePolicy {
                workspace: workspace.clone(),
            }
        }
        wire::Request::ConfigOverview => local::WsCkptRequest::ConfigOverview,
        wire::Request::Recover { workspace } => local::WsCkptRequest::Recover {
            workspace: workspace.clone(),
        },
        wire::Request::HealthAdvisory => local::WsCkptRequest::HealthAdvisory,
        wire::Request::GetWorkspacePolicy { workspace } => {
            local::WsCkptRequest::GetWorkspacePolicy {
                workspace: workspace.clone(),
            }
        }
        wire::Request::ResetWorkspacePolicy { workspace } => {
            local::WsCkptRequest::ResetWorkspacePolicy {
                workspace: workspace.clone(),
            }
        }
        wire::Request::PatchWorkspacePolicy {
            workspace,
            auto_cleanup,
            auto_cleanup_keep,
        } => local::WsCkptRequest::PatchWorkspacePolicy {
            workspace: workspace.clone(),
            auto_cleanup: local_bool_op(auto_cleanup),
            auto_cleanup_keep: local_retention_op(auto_cleanup_keep),
        },
        wire::Request::RollbackPreview {
            workspace,
            to,
            num_ancestors,
        } => local::WsCkptRequest::RollbackPreview {
            workspace: workspace.clone(),
            to: to.clone(),
            num_ancestors: *num_ancestors,
        },
    }
}

fn local_request_samples() -> Vec<local::WsCkptRequest> {
    vec![
        local::WsCkptRequest::Init {
            workspace: "/ws".into(),
        },
        local::WsCkptRequest::Checkpoint {
            workspace: "/ws".into(),
            id: "s1".into(),
            message: Some("m".into()),
            metadata: Some("{\"k\":1}".into()),
            pin: true,
        },
        local::WsCkptRequest::Rollback {
            workspace: "/ws".into(),
            to: Some("s1".into()),
            num_ancestors: Some(2),
        },
        local::WsCkptRequest::Delete {
            workspace: Some("/ws".into()),
            snapshot: "s1".into(),
            force: true,
        },
        local::WsCkptRequest::List {
            workspace: Some("/ws".into()),
            format: Some("json".into()),
        },
        local::WsCkptRequest::Diff {
            workspace: "/ws".into(),
            from: "s1".into(),
            to: Some("s2".into()),
        },
        local::WsCkptRequest::Status {
            workspace: Some("/ws".into()),
        },
        local::WsCkptRequest::Cleanup {
            workspace: "/ws".into(),
            keep: Some(3),
        },
        local::WsCkptRequest::Config,
        local::WsCkptRequest::ReloadConfig,
        local::WsCkptRequest::ReloadGlobalConfig,
        local::WsCkptRequest::ReloadWorkspacePolicy {
            workspace: "/ws".into(),
        },
        local::WsCkptRequest::ConfigOverview,
        local::WsCkptRequest::Recover {
            workspace: "/ws".into(),
        },
        local::WsCkptRequest::HealthAdvisory,
        local::WsCkptRequest::GetWorkspacePolicy {
            workspace: "/ws".into(),
        },
        local::WsCkptRequest::ResetWorkspacePolicy {
            workspace: "/ws".into(),
        },
        local::WsCkptRequest::PatchWorkspacePolicy {
            workspace: "/ws".into(),
            auto_cleanup: local::PolicyFieldOp::Set(true),
            auto_cleanup_keep: local::PolicyFieldOp::Set(local::CleanupRetention::Count(7)),
        },
        local::WsCkptRequest::RollbackPreview {
            workspace: "/ws".into(),
            to: None,
            num_ancestors: Some(3),
        },
    ]
}

fn wire_request_samples() -> Vec<wire::Request> {
    local_request_samples().iter().map(wire_request).collect()
}

#[test]
fn every_request_variant_matches_authoritative_bincode() {
    for value in local_request_samples() {
        assert_same_bincode(&value, &wire_request(&value));
    }
    for value in wire_request_samples() {
        assert_same_bincode(&local_request(&value), &value);
    }
}

fn wire_error(value: &local::WsCkptErrorCode) -> wire::ErrorCode {
    match value {
        local::WsCkptErrorCode::WorkspaceNotFound => wire::ErrorCode::WorkspaceNotFound,
        local::WsCkptErrorCode::SnapshotNotFound => wire::ErrorCode::SnapshotNotFound,
        local::WsCkptErrorCode::AlreadyInitialized => wire::ErrorCode::AlreadyInitialized,
        local::WsCkptErrorCode::BtrfsError => wire::ErrorCode::BtrfsError,
        local::WsCkptErrorCode::IoError => wire::ErrorCode::IoError,
        local::WsCkptErrorCode::InvalidPath => wire::ErrorCode::InvalidPath,
        local::WsCkptErrorCode::ConfirmationRequired => wire::ErrorCode::ConfirmationRequired,
        local::WsCkptErrorCode::InternalError => wire::ErrorCode::InternalError,
        local::WsCkptErrorCode::SnapshotAlreadyExists => wire::ErrorCode::SnapshotAlreadyExists,
        local::WsCkptErrorCode::WriteLockConflict => wire::ErrorCode::WriteLockConflict,
        local::WsCkptErrorCode::DiskSpaceInsufficient => wire::ErrorCode::DiskSpaceInsufficient,
        local::WsCkptErrorCode::CwdOccupied => wire::ErrorCode::CwdOccupied,
        local::WsCkptErrorCode::CwdScanFailed => wire::ErrorCode::CwdScanFailed,
    }
}

fn local_error(value: &wire::ErrorCode) -> local::WsCkptErrorCode {
    match value {
        wire::ErrorCode::WorkspaceNotFound => local::WsCkptErrorCode::WorkspaceNotFound,
        wire::ErrorCode::SnapshotNotFound => local::WsCkptErrorCode::SnapshotNotFound,
        wire::ErrorCode::AlreadyInitialized => local::WsCkptErrorCode::AlreadyInitialized,
        wire::ErrorCode::BtrfsError => local::WsCkptErrorCode::BtrfsError,
        wire::ErrorCode::IoError => local::WsCkptErrorCode::IoError,
        wire::ErrorCode::InvalidPath => local::WsCkptErrorCode::InvalidPath,
        wire::ErrorCode::ConfirmationRequired => local::WsCkptErrorCode::ConfirmationRequired,
        wire::ErrorCode::InternalError => local::WsCkptErrorCode::InternalError,
        wire::ErrorCode::SnapshotAlreadyExists => local::WsCkptErrorCode::SnapshotAlreadyExists,
        wire::ErrorCode::WriteLockConflict => local::WsCkptErrorCode::WriteLockConflict,
        wire::ErrorCode::DiskSpaceInsufficient => local::WsCkptErrorCode::DiskSpaceInsufficient,
        wire::ErrorCode::CwdOccupied => local::WsCkptErrorCode::CwdOccupied,
        wire::ErrorCode::CwdScanFailed => local::WsCkptErrorCode::CwdScanFailed,
    }
}

#[test]
fn every_wire_error_variant_matches_authoritative_bincode() {
    let local_values = [
        local::WsCkptErrorCode::WorkspaceNotFound,
        local::WsCkptErrorCode::SnapshotNotFound,
        local::WsCkptErrorCode::AlreadyInitialized,
        local::WsCkptErrorCode::BtrfsError,
        local::WsCkptErrorCode::IoError,
        local::WsCkptErrorCode::InvalidPath,
        local::WsCkptErrorCode::ConfirmationRequired,
        local::WsCkptErrorCode::InternalError,
        local::WsCkptErrorCode::SnapshotAlreadyExists,
        local::WsCkptErrorCode::WriteLockConflict,
        local::WsCkptErrorCode::DiskSpaceInsufficient,
        local::WsCkptErrorCode::CwdOccupied,
        local::WsCkptErrorCode::CwdScanFailed,
    ];
    for value in local_values {
        assert_same_bincode(&value, &wire_error(&value));
    }
    let wire_values = [
        wire::ErrorCode::WorkspaceNotFound,
        wire::ErrorCode::SnapshotNotFound,
        wire::ErrorCode::AlreadyInitialized,
        wire::ErrorCode::BtrfsError,
        wire::ErrorCode::IoError,
        wire::ErrorCode::InvalidPath,
        wire::ErrorCode::ConfirmationRequired,
        wire::ErrorCode::InternalError,
        wire::ErrorCode::SnapshotAlreadyExists,
        wire::ErrorCode::WriteLockConflict,
        wire::ErrorCode::DiskSpaceInsufficient,
        wire::ErrorCode::CwdOccupied,
        wire::ErrorCode::CwdScanFailed,
    ];
    for value in wire_values {
        assert_same_bincode(&local_error(&value), &value);
    }
}

fn local_snapshot() -> local::SnapshotEntry {
    local::SnapshotEntry {
        id: "s1".into(),
        workspace: "/ws".into(),
        meta: local::SnapshotMeta {
            message: Some("message".into()),
            metadata: Some(serde_json::json!({"k": 1})),
            pinned: true,
            created_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            missing: true,
            parent_id: Some("s0".into()),
            child_ids: vec!["s2".into()],
        },
    }
}

fn wire_snapshot() -> wire::SnapshotEntry {
    wire::SnapshotEntry {
        id: "s1".into(),
        workspace: "/ws".into(),
        meta: wire::SnapshotMeta {
            message: Some("message".into()),
            metadata: Some(serde_json::json!({"k": 1})),
            pinned: true,
            created_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            missing: true,
            parent_id: Some("s0".into()),
            child_ids: vec!["s2".into()],
        },
    }
}

fn local_change() -> local::DiffEntry {
    local::DiffEntry {
        path: "src/lib.rs".into(),
        change_type: local::ChangeType::Renamed,
        detail: Some("old.rs".into()),
    }
}

fn wire_change() -> wire::DiffEntry {
    wire::DiffEntry {
        path: "src/lib.rs".into(),
        change_type: wire::ChangeType::Renamed,
        detail: Some("old.rs".into()),
    }
}

fn local_status() -> local::StatusReport {
    local::StatusReport {
        uptime_secs: 9,
        workspaces: vec![local::WorkspaceInfo {
            ws_id: "id".into(),
            path: "/ws".into(),
            snapshot_count: 2,
        }],
        fs_total_bytes: 100,
        fs_used_bytes: 30,
    }
}

fn wire_status() -> wire::StatusReport {
    wire::StatusReport {
        uptime_secs: 9,
        workspaces: vec![wire::WorkspaceInfo {
            ws_id: "id".into(),
            path: "/ws".into(),
            snapshot_count: 2,
        }],
        fs_total_bytes: 100,
        fs_used_bytes: 30,
    }
}

fn local_config() -> local::ConfigReport {
    local::ConfigReport {
        mount_path: "/mnt".into(),
        socket_path: "/run/x".into(),
        log_level: "debug".into(),
        auto_cleanup: true,
        auto_cleanup_keep: local::CleanupRetention::Age {
            raw: "2d".into(),
            secs: 172_800,
        },
        auto_cleanup_interval_secs: 11,
        health_check_interval_secs: 12,
        img_size: 13,
        img_max_percent: 0.4,
    }
}

fn wire_config() -> wire::ConfigReport {
    wire::ConfigReport {
        mount_path: "/mnt".into(),
        socket_path: "/run/x".into(),
        log_level: "debug".into(),
        auto_cleanup: true,
        auto_cleanup_keep: wire::CleanupRetention::Age {
            raw: "2d".into(),
            secs: 172_800,
        },
        auto_cleanup_interval_secs: 11,
        health_check_interval_secs: 12,
        img_size: 13,
        img_max_percent: 0.4,
    }
}

fn wire_response(value: &local::WsCkptResponse) -> wire::Response {
    match value {
        local::WsCkptResponse::InitOk { ws_id } => wire::Response::InitOk {
            ws_id: ws_id.clone(),
        },
        local::WsCkptResponse::CheckpointOk { snapshot_id } => wire::Response::CheckpointOk {
            snapshot_id: snapshot_id.clone(),
        },
        local::WsCkptResponse::RollbackOk { from, to } => wire::Response::RollbackOk {
            from: from.clone(),
            to: to.clone(),
        },
        local::WsCkptResponse::DeleteOk { target } => wire::Response::DeleteOk {
            target: target.clone(),
        },
        local::WsCkptResponse::Error { code, message } => wire::Response::Error {
            code: wire_error(code),
            message: message.clone(),
        },
        local::WsCkptResponse::ListOk { snapshots } => {
            assert_eq!(snapshots.len(), 1);
            wire::Response::ListOk {
                snapshots: vec![wire_snapshot()],
            }
        }
        local::WsCkptResponse::DiffOk { changes } => {
            assert_eq!(changes.len(), 1);
            wire::Response::DiffOk {
                changes: vec![wire_change()],
            }
        }
        local::WsCkptResponse::StatusOk { report } => {
            assert_eq!(report.uptime_secs, 9);
            wire::Response::StatusOk {
                report: wire_status(),
            }
        }
        local::WsCkptResponse::CleanupOk { removed } => wire::Response::CleanupOk {
            removed: removed.clone(),
        },
        local::WsCkptResponse::ConfigOk { config } => {
            assert_eq!(config.img_size, 13);
            wire::Response::ConfigOk {
                config: wire_config(),
            }
        }
        local::WsCkptResponse::ReloadConfigOk { config } => {
            assert_eq!(config.img_size, 13);
            wire::Response::ReloadConfigOk {
                config: wire_config(),
            }
        }
        local::WsCkptResponse::CheckpointSkipped { reason } => wire::Response::CheckpointSkipped {
            reason: reason.clone(),
        },
        local::WsCkptResponse::RecoverOk { workspace } => wire::Response::RecoverOk {
            workspace: workspace.clone(),
        },
        local::WsCkptResponse::HealthAdvisoryOk {
            over_limit_workspace_count,
            fs_total_bytes,
            fs_used_bytes,
        } => wire::Response::HealthAdvisoryOk {
            over_limit_workspace_count: *over_limit_workspace_count,
            fs_total_bytes: *fs_total_bytes,
            fs_used_bytes: *fs_used_bytes,
        },
        local::WsCkptResponse::WorkspacePolicyOk {
            ws_id,
            effective,
            local,
            global,
        } => wire::Response::WorkspacePolicyOk {
            ws_id: ws_id.clone(),
            effective: wire::EffectivePolicy {
                auto_cleanup: effective.auto_cleanup,
                auto_cleanup_keep: wire_retention(&effective.auto_cleanup_keep),
            },
            local: wire::WorkspacePolicy {
                auto_cleanup: local.auto_cleanup,
                auto_cleanup_keep: local.auto_cleanup_keep.as_ref().map(wire_retention),
            },
            global: wire::GlobalPolicySnapshot {
                auto_cleanup: global.auto_cleanup,
                auto_cleanup_keep: wire_retention(&global.auto_cleanup_keep),
            },
        },
        local::WsCkptResponse::ConfigOverviewOk {
            config,
            ws_total,
            ws_with_override,
        } => {
            assert_eq!(config.img_size, 13);
            wire::Response::ConfigOverviewOk {
                config: wire_config(),
                ws_total: *ws_total,
                ws_with_override: *ws_with_override,
            }
        }
        local::WsCkptResponse::RollbackPreviewOk { to, changes } => {
            assert_eq!(changes.len(), 1);
            wire::Response::RollbackPreviewOk {
                to: to.clone(),
                changes: vec![wire_change()],
            }
        }
    }
}

fn local_response(value: &wire::Response) -> local::WsCkptResponse {
    match value {
        wire::Response::InitOk { ws_id } => local::WsCkptResponse::InitOk {
            ws_id: ws_id.clone(),
        },
        wire::Response::CheckpointOk { snapshot_id } => local::WsCkptResponse::CheckpointOk {
            snapshot_id: snapshot_id.clone(),
        },
        wire::Response::RollbackOk { from, to } => local::WsCkptResponse::RollbackOk {
            from: from.clone(),
            to: to.clone(),
        },
        wire::Response::DeleteOk { target } => local::WsCkptResponse::DeleteOk {
            target: target.clone(),
        },
        wire::Response::Error { code, message } => local::WsCkptResponse::Error {
            code: local_error(code),
            message: message.clone(),
        },
        wire::Response::ListOk { snapshots } => {
            assert_eq!(snapshots.len(), 1);
            local::WsCkptResponse::ListOk {
                snapshots: vec![local_snapshot()],
            }
        }
        wire::Response::DiffOk { changes } => {
            assert_eq!(changes.len(), 1);
            local::WsCkptResponse::DiffOk {
                changes: vec![local_change()],
            }
        }
        wire::Response::StatusOk { report } => {
            assert_eq!(report.uptime_secs, 9);
            local::WsCkptResponse::StatusOk {
                report: local_status(),
            }
        }
        wire::Response::CleanupOk { removed } => local::WsCkptResponse::CleanupOk {
            removed: removed.clone(),
        },
        wire::Response::ConfigOk { config } => {
            assert_eq!(config.img_size, 13);
            local::WsCkptResponse::ConfigOk {
                config: local_config(),
            }
        }
        wire::Response::ReloadConfigOk { config } => {
            assert_eq!(config.img_size, 13);
            local::WsCkptResponse::ReloadConfigOk {
                config: local_config(),
            }
        }
        wire::Response::CheckpointSkipped { reason } => local::WsCkptResponse::CheckpointSkipped {
            reason: reason.clone(),
        },
        wire::Response::RecoverOk { workspace } => local::WsCkptResponse::RecoverOk {
            workspace: workspace.clone(),
        },
        wire::Response::HealthAdvisoryOk {
            over_limit_workspace_count,
            fs_total_bytes,
            fs_used_bytes,
        } => local::WsCkptResponse::HealthAdvisoryOk {
            over_limit_workspace_count: *over_limit_workspace_count,
            fs_total_bytes: *fs_total_bytes,
            fs_used_bytes: *fs_used_bytes,
        },
        wire::Response::WorkspacePolicyOk {
            ws_id,
            effective,
            local,
            global,
        } => local::WsCkptResponse::WorkspacePolicyOk {
            ws_id: ws_id.clone(),
            effective: local::EffectivePolicy {
                auto_cleanup: effective.auto_cleanup,
                auto_cleanup_keep: local_retention(&effective.auto_cleanup_keep),
            },
            local: local::WorkspacePolicy {
                auto_cleanup: local.auto_cleanup,
                auto_cleanup_keep: local.auto_cleanup_keep.as_ref().map(local_retention),
            },
            global: local::GlobalPolicySnapshot {
                auto_cleanup: global.auto_cleanup,
                auto_cleanup_keep: local_retention(&global.auto_cleanup_keep),
            },
        },
        wire::Response::ConfigOverviewOk {
            config,
            ws_total,
            ws_with_override,
        } => {
            assert_eq!(config.img_size, 13);
            local::WsCkptResponse::ConfigOverviewOk {
                config: local_config(),
                ws_total: *ws_total,
                ws_with_override: *ws_with_override,
            }
        }
        wire::Response::RollbackPreviewOk { to, changes } => {
            assert_eq!(changes.len(), 1);
            local::WsCkptResponse::RollbackPreviewOk {
                to: to.clone(),
                changes: vec![local_change()],
            }
        }
    }
}

fn local_response_samples() -> Vec<local::WsCkptResponse> {
    vec![
        local::WsCkptResponse::InitOk { ws_id: "id".into() },
        local::WsCkptResponse::CheckpointOk {
            snapshot_id: "s1".into(),
        },
        local::WsCkptResponse::RollbackOk {
            from: "s2".into(),
            to: "s1".into(),
        },
        local::WsCkptResponse::DeleteOk {
            target: "s1".into(),
        },
        local::WsCkptResponse::Error {
            code: local::WsCkptErrorCode::CwdScanFailed,
            message: "failed".into(),
        },
        local::WsCkptResponse::ListOk {
            snapshots: vec![local_snapshot()],
        },
        local::WsCkptResponse::DiffOk {
            changes: vec![local_change()],
        },
        local::WsCkptResponse::StatusOk {
            report: local_status(),
        },
        local::WsCkptResponse::CleanupOk {
            removed: vec!["s0".into()],
        },
        local::WsCkptResponse::ConfigOk {
            config: local_config(),
        },
        local::WsCkptResponse::ReloadConfigOk {
            config: local_config(),
        },
        local::WsCkptResponse::CheckpointSkipped {
            reason: "unchanged".into(),
        },
        local::WsCkptResponse::RecoverOk {
            workspace: "/ws".into(),
        },
        local::WsCkptResponse::HealthAdvisoryOk {
            over_limit_workspace_count: 2,
            fs_total_bytes: 100,
            fs_used_bytes: 20,
        },
        local::WsCkptResponse::WorkspacePolicyOk {
            ws_id: "id".into(),
            effective: local::EffectivePolicy {
                auto_cleanup: true,
                auto_cleanup_keep: local::CleanupRetention::Count(3),
            },
            local: local::WorkspacePolicy {
                auto_cleanup: Some(false),
                auto_cleanup_keep: Some(local::CleanupRetention::Count(2)),
            },
            global: local::GlobalPolicySnapshot {
                auto_cleanup: true,
                auto_cleanup_keep: local::CleanupRetention::Count(4),
            },
        },
        local::WsCkptResponse::ConfigOverviewOk {
            config: local_config(),
            ws_total: 10,
            ws_with_override: 2,
        },
        local::WsCkptResponse::RollbackPreviewOk {
            to: "s1".into(),
            changes: vec![local_change()],
        },
    ]
}

#[test]
fn every_response_variant_matches_authoritative_bincode() {
    for value in local_response_samples() {
        assert_same_bincode(&value, &wire_response(&value));
    }
    for value in local_response_samples().iter().map(wire_response) {
        assert_same_bincode(&local_response(&value), &value);
    }
}
