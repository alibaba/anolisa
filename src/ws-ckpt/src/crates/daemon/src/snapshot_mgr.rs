use std::sync::Arc;

use anyhow::Context;
use tracing::info;
use ws_ckpt_common::{ErrorCode, ResolveError, Response, SnapshotEntry, SnapshotMeta};

use std::path::Path;

use crate::index_store;
use crate::state::DaemonState;

fn workspace_not_found(workspace: &str) -> Response {
    Response::Error {
        code: ErrorCode::WorkspaceNotFound,
        message: format!("workspace not found: {}", workspace),
    }
}

pub async fn checkpoint(
    state: &Arc<DaemonState>,
    workspace: &str,
    id: &str,
    message: Option<String>,
    metadata: Option<String>,
    pin: bool,
) -> anyhow::Result<Response> {
    // 1. Resolve workspace (by ID, absolute path, or relative path)
    let arc = match state.resolve_workspace(workspace).await {
        Some(a) => a,
        None => return Ok(workspace_not_found(workspace)),
    };

    // 2. Acquire write lock
    let mut ws = arc.write().await;

    // 2a. Check write-lock quiescence (inotify-based)
    if !state.check_workspace_quiescent(&ws.ws_id).await {
        return Ok(Response::Error {
            code: ErrorCode::WriteLockConflict,
            message: "Workspace has active write operations. Please wait and retry.".to_string(),
        });
    }

    // 3. Check snapshot ID uniqueness within this workspace
    if ws.index.snapshots.contains_key(id) {
        return Ok(Response::Error {
            code: ErrorCode::SnapshotAlreadyExists,
            message: format!("snapshot id '{}' already exists in workspace", id),
        });
    }
    let snapshot_id = id.to_string();

    // 4. Check if workspace directory is empty
    let is_empty = {
        let mut entries = tokio::fs::read_dir(&ws.path).await?;
        entries.next_entry().await?.is_none()
    };
    if is_empty {
        info!("Workspace {} is empty, skipping snapshot", ws.ws_id);
        return Ok(Response::CheckpointSkipped {
            reason: "Empty workspace, no snapshot created.".to_string(),
        });
    }

    // 5. Disk space note: btrfs snapshot creation is a pure metadata/COW
    //    operation that succeeds even on a full disk, so we do NOT block
    //    checkpoint here.  Space reporting is still available via `ws-ckpt status`
    //    and the health-check scheduler.

    // 6. Construct paths
    let snap_dir = state.index_dir(&ws.ws_id);
    // make sure index directory exists
    tokio::fs::create_dir_all(&snap_dir).await?;

    // 7. Create readonly snapshot via backend
    state
        .backend
        .create_snapshot(&ws.ws_id, &snapshot_id)
        .await?;

    // 8. Build metadata
    let parsed_metadata = match metadata {
        Some(ref s) => Some(serde_json::from_str(s)?),
        None => None,
    };
    let meta = SnapshotMeta {
        message,
        metadata: parsed_metadata,
        pinned: pin,
        created_at: chrono::Utc::now(),
        missing: false,
    };

    // 9. Update index
    ws.index.snapshots.insert(snapshot_id.clone(), meta);

    // 10. Persist index
    index_store::save(&snap_dir, &ws.index).await?;

    // 10a. Release write lock before save_manifest (try_read inside
    //      collect_workspace_entries would fail while write lock is held)
    drop(ws);

    // 10b. Save manifest
    if let Err(e) = state.save_manifest().await {
        tracing::warn!("save_manifest failed after checkpoint: {:#}", e);
    }

    // 11. Return success
    Ok(Response::CheckpointOk { snapshot_id })
}

pub async fn rollback(
    state: &Arc<DaemonState>,
    workspace: &str,
    to: &str,
) -> anyhow::Result<Response> {
    // 1. Resolve workspace (by ID, absolute path, or relative path)
    let arc = match state.resolve_workspace(workspace).await {
        Some(a) => a,
        None => return Ok(workspace_not_found(workspace)),
    };

    // 2. Acquire write lock
    let ws = arc.write().await;

    // 3. Resolve target snapshot by prefix
    let resolved_id = match ws.index.resolve_by_prefix(to) {
        Ok((id, _)) => id.clone(),
        Err(ResolveError::NotFound) => {
            return Ok(Response::Error {
                code: ErrorCode::SnapshotNotFound,
                message: format!("snapshot not found: {}", to),
            });
        }
        Err(ResolveError::Ambiguous(n)) => {
            return Ok(Response::Error {
                code: ErrorCode::SnapshotNotFound,
                message: format!("ambiguous snapshot prefix '{}': {} matches", to, n),
            });
        }
    };

    // 3a. Reject missing snapshots — user must manually delete with --force
    if ws
        .index
        .snapshots
        .get(&resolved_id)
        .is_some_and(|s| s.missing)
    {
        return Ok(Response::Error {
            code: ErrorCode::SnapshotNotFound,
            message: format!("Snapshot '{}' subvolume is missing (data lost). Use 'ws-ckpt delete --force -w <workspace> -s {}' to remove the record.", resolved_id, resolved_id),
        });
    }

    // 4. Construct paths
    let _abs_path_str = ws.path.to_string_lossy().to_string();

    // 5. Rollback via backend (includes warmup, snapshot, cleanup)
    state.backend.rollback(&ws.ws_id, &resolved_id).await?;

    // 6. Return success
    Ok(Response::RollbackOk {
        from: ws.ws_id.clone(),
        to: resolved_id,
    })
}

/// Warm up snapshot metadata cache — forwards to backends::btrfs_common.
pub async fn warmup_snapshot_metadata(snap_path: &Path) {
    crate::backends::btrfs_common::warmup_snapshot_metadata(snap_path).await;
}

/// List all snapshots for a workspace, sorted by created_at ascending.
pub async fn list_snapshots(state: &Arc<DaemonState>, workspace: &str) -> anyhow::Result<Response> {
    let arc = match state.resolve_workspace(workspace).await {
        Some(a) => a,
        None => return Ok(workspace_not_found(workspace)),
    };

    let ws = arc.read().await;
    let ws_path = ws.index.workspace_path.to_string_lossy().to_string();
    let mut snapshots: Vec<(String, SnapshotMeta)> = ws
        .index
        .snapshots
        .iter()
        .map(|(id, meta)| (id.clone(), meta.clone()))
        .collect();

    // Sort by created_at ascending
    snapshots.sort_by_key(|a| a.1.created_at);

    let snapshot_entries: Vec<SnapshotEntry> = snapshots
        .into_iter()
        .map(|(id, meta)| SnapshotEntry {
            id,
            workspace: ws_path.clone(),
            meta,
        })
        .collect();

    Ok(Response::ListOk {
        snapshots: snapshot_entries,
    })
}

/// List snapshots across all registered workspaces, sorted by created_at ascending.
pub async fn list_all_snapshots(state: &Arc<DaemonState>) -> anyhow::Result<Response> {
    let all_ws = state.all_workspaces();
    let mut all_entries: Vec<SnapshotEntry> = Vec::new();

    for arc in all_ws {
        let ws = arc.read().await;
        let ws_path = ws.index.workspace_path.to_string_lossy().to_string();
        for (id, meta) in &ws.index.snapshots {
            all_entries.push(SnapshotEntry {
                id: id.clone(),
                workspace: ws_path.clone(),
                meta: meta.clone(),
            });
        }
    }

    // Sort by created_at ascending
    all_entries.sort_by_key(|a| a.meta.created_at);

    Ok(Response::ListOk {
        snapshots: all_entries,
    })
}

/// Compute diff between two snapshots.
pub async fn diff_snapshots(
    state: &Arc<DaemonState>,
    workspace: &str,
    from: &str,
    to: &str,
) -> anyhow::Result<Response> {
    let arc = match state.resolve_workspace(workspace).await {
        Some(a) => a,
        None => return Ok(workspace_not_found(workspace)),
    };

    let ws = arc.read().await;

    // Resolve from
    let from_id = resolve_snapshot_id(&ws.index, from)?;
    let to_id = resolve_snapshot_id(&ws.index, to)?;

    let changes = state.backend.diff(&ws.ws_id, &from_id, &to_id).await?;

    Ok(Response::DiffOk { changes })
}

/// Resolve a snapshot reference (ID or prefix) to its ID.
fn resolve_snapshot_id(
    index: &ws_ckpt_common::SnapshotIndex,
    reference: &str,
) -> anyhow::Result<String> {
    match index.resolve_by_prefix(reference) {
        Ok((id, _)) => Ok(id.clone()),
        Err(ResolveError::NotFound) => anyhow::bail!("snapshot not found: {}", reference),
        Err(ResolveError::Ambiguous(n)) => {
            anyhow::bail!("ambiguous snapshot prefix '{}': {} matches", reference, n)
        }
    }
}

/// Cleanup old snapshots for a workspace, keeping the most recent `keep` unpinned ones.
pub async fn cleanup_snapshots(
    state: &Arc<DaemonState>,
    workspace: &str,
    keep: Option<u32>,
) -> anyhow::Result<Response> {
    let keep = keep.unwrap_or(20) as usize;

    let arc = match state.resolve_workspace(workspace).await {
        Some(a) => a,
        None => return Ok(workspace_not_found(workspace)),
    };

    let mut ws = arc.write().await;
    let snap_dir = state.index_dir(&ws.ws_id);
    // Ensure index directory exists
    tokio::fs::create_dir_all(&snap_dir)
        .await
        .with_context(|| format!("Failed to create index dir: {:?}", snap_dir))?;

    // Collect non-pinned snapshots, sorted by created_at ascending (oldest first)
    let mut unpinned: Vec<(String, chrono::DateTime<chrono::Utc>)> = ws
        .index
        .snapshots
        .iter()
        .filter(|(_, meta)| !meta.pinned)
        .map(|(id, meta)| (id.clone(), meta.created_at))
        .collect();
    unpinned.sort_by_key(|(_, ts)| *ts);

    // Determine which to remove (oldest beyond keep count)
    let to_remove = if unpinned.len() > keep {
        unpinned[..unpinned.len() - keep].to_vec()
    } else {
        vec![]
    };

    let to_remove_ids: Vec<String> = to_remove.iter().map(|(id, _)| id.clone()).collect();
    let removed = state
        .backend
        .cleanup_snapshots(&ws.ws_id, &to_remove_ids)
        .await?;

    // Update index for actually removed snapshots
    for snap_id in &removed {
        ws.index.snapshots.remove(snap_id);
        info!("cleanup: removed snapshot {}", snap_id);
    }

    // Save index if any were removed
    if !removed.is_empty() {
        index_store::save(&snap_dir, &ws.index).await?;
    }

    // Release write lock before save_manifest (try_read inside
    // collect_workspace_entries would fail while write lock is held)
    drop(ws);

    if !removed.is_empty() {
        if let Err(e) = state.save_manifest().await {
            tracing::warn!("save_manifest failed after cleanup_snapshots: {:#}", e);
        }
    }

    Ok(Response::CleanupOk { removed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use std::path::PathBuf;
    use ws_ckpt_common::backend::StorageBackend;
    use ws_ckpt_common::{
        CleanupRetention, DaemonConfig, ErrorCode, Response, SnapshotIndex, SnapshotMeta,
    };

    fn test_backend() -> Arc<dyn StorageBackend> {
        Arc::new(crate::backends::btrfs_loop::BtrfsLoopBackend::new(
            PathBuf::from("/tmp/test-mount"),
            PathBuf::from("/tmp/test.img"),
        ))
    }

    fn test_config() -> DaemonConfig {
        DaemonConfig {
            mount_path: PathBuf::from("/tmp/test-mount"),
            socket_path: PathBuf::from("/tmp/test.sock"),
            log_level: "info".to_string(),
            auto_cleanup: false,
            auto_cleanup_keep: CleanupRetention::Count(20),
            auto_cleanup_interval_secs: 86_400,
            health_check_interval_secs: 300,
            backend_type: "auto".to_string(),
            img_size: 30,
            img_max_percent: 40.0,
            min_free_bytes: 512 * 1024 * 1024,
            min_free_percent: 1.0,
        }
    }

    fn test_state_dir() -> PathBuf {
        PathBuf::from("/tmp/test-state")
    }

    fn make_snapshot_meta(pinned: bool) -> SnapshotMeta {
        SnapshotMeta {
            message: None,
            metadata: None,
            pinned,
            created_at: chrono::Utc::now(),
            missing: false,
        }
    }

    fn make_snapshot_meta_at(pinned: bool, created_at: chrono::DateTime<Utc>) -> SnapshotMeta {
        SnapshotMeta {
            message: None,
            metadata: None,
            pinned,
            created_at,
            missing: false,
        }
    }

    // ── Duplicate snapshot ID tests ──

    #[test]
    fn snapshot_id_uniqueness_check() {
        let mut index = SnapshotIndex::new(PathBuf::from("/ws"));
        index
            .snapshots
            .insert("existing-id".to_string(), make_snapshot_meta(false));
        assert!(index.snapshots.contains_key("existing-id"));
        assert!(!index.snapshots.contains_key("new-id"));
    }

    // ── Rollback target resolution tests ──
    // These test the resolution logic used in rollback() by exercising SnapshotIndex directly.

    #[test]
    fn rollback_target_by_id_found() {
        let mut index = SnapshotIndex::new(PathBuf::from("/ws"));
        index.snapshots.insert(
            "abcdef1234567890abcdef1234567890abcdef12".to_string(),
            make_snapshot_meta(false),
        );

        // Resolve by exact ID
        assert!(index
            .resolve_by_prefix("abcdef1234567890abcdef1234567890abcdef12")
            .is_ok());
    }

    #[test]
    fn rollback_target_by_prefix_found() {
        let mut index = SnapshotIndex::new(PathBuf::from("/ws"));
        index.snapshots.insert(
            "abcdef1234567890abcdef1234567890abcdef12".to_string(),
            make_snapshot_meta(true),
        );

        // Resolve by prefix
        let result = index.resolve_by_prefix("abcdef");
        assert!(result.is_ok());
        let (id, _) = result.unwrap();
        assert_eq!(id, "abcdef1234567890abcdef1234567890abcdef12");
    }

    #[test]
    fn rollback_target_not_found() {
        let mut index = SnapshotIndex::new(PathBuf::from("/ws"));
        index.snapshots.insert(
            "abcdef1234567890abcdef1234567890abcdef12".to_string(),
            make_snapshot_meta(false),
        );

        // Target doesn't match any prefix
        assert!(index.resolve_by_prefix("zzz999").is_err());
    }

    #[test]
    fn rollback_resolution_prefers_exact_over_prefix() {
        // If target matches as exact ID, it should be preferred over prefix
        let mut index = SnapshotIndex::new(PathBuf::from("/ws"));
        index.snapshots.insert(
            "abcdef1234567890abcdef1234567890abcdef12".to_string(),
            make_snapshot_meta(false),
        );

        // Exact match
        let result = index.resolve_by_prefix("abcdef1234567890abcdef1234567890abcdef12");
        assert!(result.is_ok());
    }

    // ── Checkpoint duplicate detection test ──

    #[tokio::test]
    async fn checkpoint_duplicate_id_returns_already_exists() {
        let state = Arc::new(crate::state::DaemonState::new(
            test_config(),
            test_backend(),
            test_state_dir(),
        ));
        // Register a workspace with an existing snapshot
        let mut index = SnapshotIndex::new(PathBuf::from("/home/user/ws"));
        index
            .snapshots
            .insert("existing-id".to_string(), make_snapshot_meta(false));
        state.register_workspace("ws-dup".to_string(), PathBuf::from("/home/user/ws"), index);

        let resp = checkpoint(&state, "ws-dup", "existing-id", None, None, false)
            .await
            .unwrap();
        match resp {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::SnapshotAlreadyExists);
                assert!(message.contains("existing-id"));
            }
            _ => panic!("expected SnapshotAlreadyExists error"),
        }
    }

    // ── SnapshotMeta pinned logic test ──

    #[test]
    fn snapshot_pinned_flag_logic() {
        // Pinned is now set directly via `pin` field
        let meta_pinned = make_snapshot_meta(true);
        assert!(meta_pinned.pinned);

        let meta_unpinned = make_snapshot_meta(false);
        assert!(!meta_unpinned.pinned);
    }

    // ── Non-ignored async tests (use tempdir, no btrfs needed) ──

    #[tokio::test]
    async fn checkpoint_nonexistent_path_returns_workspace_not_found() {
        let state = Arc::new(crate::state::DaemonState::new(
            test_config(),
            test_backend(),
            test_state_dir(),
        ));
        let resp = checkpoint(&state, "/nonexistent/ws/12345", "snap-1", None, None, false)
            .await
            .unwrap();
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::WorkspaceNotFound),
            _ => panic!("expected WorkspaceNotFound error"),
        }
    }

    #[tokio::test]
    async fn checkpoint_unregistered_workspace_returns_workspace_not_found() {
        let state = Arc::new(crate::state::DaemonState::new(
            test_config(),
            test_backend(),
            test_state_dir(),
        ));
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().to_string_lossy().to_string();
        let resp = checkpoint(&state, &path, "snap-1", None, None, false)
            .await
            .unwrap();
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::WorkspaceNotFound),
            _ => panic!("expected WorkspaceNotFound error"),
        }
    }

    #[tokio::test]
    async fn rollback_nonexistent_path_returns_workspace_not_found() {
        let state = Arc::new(crate::state::DaemonState::new(
            test_config(),
            test_backend(),
            test_state_dir(),
        ));
        let resp = rollback(&state, "/nonexistent/ws/12345", "msg1-step0")
            .await
            .unwrap();
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::WorkspaceNotFound),
            _ => panic!("expected WorkspaceNotFound error"),
        }
    }

    #[tokio::test]
    async fn rollback_unregistered_workspace_returns_workspace_not_found() {
        let state = Arc::new(crate::state::DaemonState::new(
            test_config(),
            test_backend(),
            test_state_dir(),
        ));
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().to_string_lossy().to_string();
        let resp = rollback(&state, &path, "msg1-step0").await.unwrap();
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::WorkspaceNotFound),
            _ => panic!("expected WorkspaceNotFound error"),
        }
    }

    // ── Additional pure logic tests ──

    #[test]
    fn snapshot_id_uniqueness_in_index() {
        let mut index = SnapshotIndex::new(PathBuf::from("/ws"));
        index
            .snapshots
            .insert("snap-1".to_string(), make_snapshot_meta(false));
        // Duplicate check should detect existing ID
        assert!(index.snapshots.contains_key("snap-1"));
        // New ID should not exist
        assert!(!index.snapshots.contains_key("snap-2"));
    }

    #[test]
    fn resolve_by_prefix_with_multiple_snapshots() {
        let mut index = SnapshotIndex::new(PathBuf::from("/ws"));
        index.snapshots.insert(
            "aaa1111111111111111111111111111111111111".to_string(),
            make_snapshot_meta(true),
        );
        index.snapshots.insert(
            "bbb2222222222222222222222222222222222222".to_string(),
            make_snapshot_meta(true),
        );
        index.snapshots.insert(
            "ccc3333333333333333333333333333333333333".to_string(),
            make_snapshot_meta(false),
        );

        let result = index.resolve_by_prefix("bbb");
        assert!(result.is_ok());
        let (id, _) = result.unwrap();
        assert_eq!(id, "bbb2222222222222222222222222222222222222");
    }

    #[test]
    fn snapshot_meta_pinned_logic() {
        let pinned = SnapshotMeta {
            message: Some("Release v1".to_string()),
            metadata: None,
            pinned: true,
            created_at: chrono::Utc::now(),
            missing: false,
        };
        assert!(pinned.pinned);

        let unpinned = SnapshotMeta {
            message: None,
            metadata: None,
            pinned: false,
            created_at: chrono::Utc::now(),
            missing: false,
        };
        assert!(!unpinned.pinned);
    }

    // ── list_snapshots sorting tests ──

    #[test]
    fn list_sorting_by_created_at() {
        let now = Utc::now();
        let mut index = SnapshotIndex::new(PathBuf::from("/ws"));
        index.snapshots.insert(
            "snap-b".to_string(),
            make_snapshot_meta_at(false, now - Duration::seconds(10)),
        );
        index.snapshots.insert(
            "snap-a".to_string(),
            make_snapshot_meta_at(false, now - Duration::seconds(30)),
        );
        index
            .snapshots
            .insert("snap-c".to_string(), make_snapshot_meta_at(false, now));

        let mut snapshots: Vec<(String, SnapshotMeta)> = index
            .snapshots
            .iter()
            .map(|(id, meta)| (id.clone(), meta.clone()))
            .collect();
        snapshots.sort_by_key(|a| a.1.created_at);

        assert_eq!(snapshots[0].0, "snap-a");
        assert_eq!(snapshots[1].0, "snap-b");
        assert_eq!(snapshots[2].0, "snap-c");
    }

    #[test]
    fn list_empty_index_returns_empty() {
        let index = SnapshotIndex::new(PathBuf::from("/ws"));
        let snapshots: Vec<(String, SnapshotMeta)> = index
            .snapshots
            .iter()
            .map(|(id, meta)| (id.clone(), meta.clone()))
            .collect();
        assert!(snapshots.is_empty());
    }

    // ── cleanup strategy tests ──

    #[test]
    fn cleanup_strategy_keeps_recent_unpinned() {
        let now = Utc::now();
        let mut index = SnapshotIndex::new(PathBuf::from("/ws"));
        // Add 5 unpinned snapshots
        for i in 0..5 {
            index.snapshots.insert(
                format!("snap{}", i),
                make_snapshot_meta_at(false, now - Duration::seconds(50 - i * 10)),
            );
        }

        let keep = 3usize;
        let mut unpinned: Vec<(String, chrono::DateTime<Utc>)> = index
            .snapshots
            .iter()
            .filter(|(_, meta)| !meta.pinned)
            .map(|(id, meta)| (id.clone(), meta.created_at))
            .collect();
        unpinned.sort_by_key(|(_, ts)| *ts);

        let to_remove = if unpinned.len() > keep {
            unpinned[..unpinned.len() - keep].to_vec()
        } else {
            vec![]
        };

        assert_eq!(to_remove.len(), 2); // 5 - 3 = 2 to remove
    }

    #[test]
    fn cleanup_strategy_pinned_snapshots_are_protected() {
        let now = Utc::now();
        let mut index = SnapshotIndex::new(PathBuf::from("/ws"));
        // 2 pinned (old) + 3 unpinned
        index.snapshots.insert(
            "snap-old1".to_string(),
            make_snapshot_meta_at(true, now - Duration::seconds(100)),
        );
        index.snapshots.insert(
            "snap-old2".to_string(),
            make_snapshot_meta_at(true, now - Duration::seconds(200)),
        );
        for i in 2..5 {
            index.snapshots.insert(
                format!("snap{}", i),
                make_snapshot_meta_at(false, now - Duration::seconds(50 - i * 10)),
            );
        }

        let keep = 2usize;
        let mut unpinned: Vec<(String, chrono::DateTime<Utc>)> = index
            .snapshots
            .iter()
            .filter(|(_, meta)| !meta.pinned)
            .map(|(id, meta)| (id.clone(), meta.created_at))
            .collect();
        unpinned.sort_by_key(|(_, ts)| *ts);

        let to_remove = if unpinned.len() > keep {
            unpinned[..unpinned.len() - keep].to_vec()
        } else {
            vec![]
        };

        // Only 1 unpinned should be removed (3 unpinned - 2 keep = 1)
        assert_eq!(to_remove.len(), 1);
        // Pinned snapshots should NOT appear in to_remove
        assert!(!to_remove
            .iter()
            .any(|(id, _)| id == "snap-old1" || id == "snap-old2"));
    }

    #[test]
    fn cleanup_strategy_fewer_than_keep_removes_nothing() {
        let now = Utc::now();
        let mut index = SnapshotIndex::new(PathBuf::from("/ws"));
        for i in 0..3 {
            index.snapshots.insert(
                format!("snap{}", i),
                make_snapshot_meta_at(false, now - Duration::seconds(i * 10)),
            );
        }

        let keep = 20usize;
        let unpinned: Vec<(String, chrono::DateTime<Utc>)> = index
            .snapshots
            .iter()
            .filter(|(_, meta)| !meta.pinned)
            .map(|(id, meta)| (id.clone(), meta.created_at))
            .collect();

        let to_remove = if unpinned.len() > keep {
            unpinned[..unpinned.len() - keep].to_vec()
        } else {
            vec![]
        };

        assert!(to_remove.is_empty());
    }

    #[test]
    fn resolve_snapshot_id_by_id() {
        let mut index = SnapshotIndex::new(PathBuf::from("/ws"));
        index.snapshots.insert(
            "abcdef1234567890abcdef1234567890abcdef12".to_string(),
            make_snapshot_meta(false),
        );
        let result = resolve_snapshot_id(&index, "abcdef1234567890abcdef1234567890abcdef12");
        assert_eq!(result.unwrap(), "abcdef1234567890abcdef1234567890abcdef12");
    }

    #[test]
    fn resolve_snapshot_id_by_prefix() {
        let mut index = SnapshotIndex::new(PathBuf::from("/ws"));
        index.snapshots.insert(
            "abcdef1234567890abcdef1234567890abcdef12".to_string(),
            make_snapshot_meta(false),
        );
        let result = resolve_snapshot_id(&index, "abcdef");
        assert_eq!(result.unwrap(), "abcdef1234567890abcdef1234567890abcdef12");
    }

    #[test]
    fn resolve_snapshot_id_not_found() {
        let index = SnapshotIndex::new(PathBuf::from("/ws"));
        let result = resolve_snapshot_id(&index, "nonexistent");
        assert!(result.is_err());
    }
}
