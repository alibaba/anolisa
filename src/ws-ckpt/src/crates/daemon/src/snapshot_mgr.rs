use std::sync::Arc;

use anyhow::Context;
use tokio::sync::RwLock;
use tracing::info;
use ws_ckpt_common::{ErrorCode, ResolveError, Response, SnapshotEntry, SnapshotMeta};

use std::path::{Path, PathBuf};

use crate::index_store;
use crate::state::{DaemonState, WorkspaceState};

/// Result of [`delete_snapshots_locked`]: per-snap outcome, no early bail
/// (caller decides whether failures escalate to an error or just warn).
pub struct CleanupOutcome {
    pub removed: Vec<String>,
    pub failed: Vec<(String, String)>,
}

/// Shared per-snap detach-then-delete loop, used by both
/// [`cleanup_snapshots`] (user-triggered Count cleanup) and the scheduler's
/// background pass. Invariants:
///
/// 1. **detach under write lock, delete unlocked** — a concurrent pin RPC
///    either wins (we skip) or loses (it sees SnapshotNotFound). No
///    read→fs window where pin sneaks in between recheck and delete.
/// 2. **per-snap try/recover** — on backend Err the meta is re-inserted so
///    the index stays in sync with on-disk subvolumes.
/// 3. **no short-circuit** — earlier successes must persist even if a later
///    snap fails; the caller persists `index` + manifest from
///    `outcome.removed`. An early `?` would lose the prior in-memory removes.
///
/// `label` ("cleanup" / "auto-cleanup") prefixes log lines.
pub async fn delete_snapshots_locked(
    state: &DaemonState,
    arc: &Arc<RwLock<WorkspaceState>>,
    ws_id: &str,
    to_remove: &[String],
    label: &str,
) -> CleanupOutcome {
    // Relink DAG + detach all non-pinned snaps in one write lock (pure memory, no fs I/O).
    // Holding a single lock eliminates the TOCTOU window where a concurrent
    // checkpoint/rollback could reference a node we're about to delete.
    let detached: Vec<(String, SnapshotMeta)> = {
        let ids: std::collections::HashSet<String> = to_remove.iter().cloned().collect();
        let mut ws = arc.write().await;
        ws.index.prune_chain(&ids);
        to_remove
            .iter()
            .filter_map(|snap_id| match ws.index.snapshots.get(snap_id) {
                Some(m) if !m.pinned => ws
                    .index
                    .snapshots
                    .remove(snap_id)
                    .map(|m| (snap_id.clone(), m)),
                _ => None,
            })
            .collect()
    };
    let mut removed = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();
    for (snap_id, meta) in &detached {
        match state
            .backend
            .cleanup_snapshots(ws_id, std::slice::from_ref(snap_id))
            .await
        {
            Ok(deleted) if deleted.is_empty() => {
                // Backend reported no-op (already gone); roll back the detach
                // so the index doesn't drift.
                tracing::warn!(
                    "{}: re-inserted {} after backend no-op; DAG links may be stale (orphaned node)",
                    label, snap_id,
                );
                arc.write()
                    .await
                    .index
                    .snapshots
                    .insert(snap_id.clone(), meta.clone());
            }
            Ok(_) => {
                info!("{}: removed snapshot {}", label, snap_id);
                removed.push(snap_id.clone());
            }
            Err(e) => {
                arc.write()
                    .await
                    .index
                    .snapshots
                    .insert(snap_id.clone(), meta.clone());
                tracing::warn!(
                    "{}: backend delete failed for {}: {:#}; re-inserted as orphaned node",
                    label,
                    snap_id,
                    e
                );
                failed.push((snap_id.clone(), format!("{:#}", e)));
            }
        }
    }
    CleanupOutcome { removed, failed }
}

/// After [`delete_snapshots_locked`] succeeded for ≥1 snap, snapshot the
/// in-memory index outside the lock and persist to `index.json` + manifest.
/// Both writes are best-effort and warn-only — by the time we get here the
/// subvolumes are already gone, so an index-save failure just means restart
/// will rebuild from fs (cheap), and a manifest failure leaves stale entries
/// but the ws is still usable.
pub async fn persist_index_after_cleanup(
    state: &DaemonState,
    arc: &Arc<RwLock<WorkspaceState>>,
    snap_dir: &Path,
    label: &str,
) {
    let index_to_persist = {
        let ws = arc.read().await;
        ws.index.clone()
    };
    if let Err(e) = index_store::save(snap_dir, &index_to_persist).await {
        tracing::warn!("{}: failed to save index: {:#}", label, e);
    }
    if let Err(e) = state.save_manifest().await {
        tracing::warn!("{}: save_manifest failed: {:#}", label, e);
    }
}

/// Ensure `dir` exists; warn-only because every caller is in a per-ws loop
/// that should continue to the next ws on a single-dir mkdir failure.
pub async fn ensure_index_dir(dir: &PathBuf, label: &str) -> bool {
    if let Err(e) = tokio::fs::create_dir_all(dir).await {
        tracing::warn!(
            "{}: failed to create index directory {:?}: {}",
            label,
            dir,
            e
        );
        return false;
    }
    true
}

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
        parent_id: ws.index.head.clone(),
        child_ids: vec![ws_ckpt_common::LIVE_CHILD.to_string()],
    };

    // 9. Update index (maintain DAG bidirectional pointers)
    if let Some(old_head) = ws.index.head.clone() {
        if let Some(hm) = ws.index.snapshots.get_mut(&old_head) {
            hm.child_ids.retain(|c| c != ws_ckpt_common::LIVE_CHILD);
            hm.child_ids.push(snapshot_id.clone());
        }
    }
    ws.index.snapshots.insert(snapshot_id.clone(), meta);
    ws.index.head = Some(snapshot_id.clone());

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
    to: Option<&str>,
    num_ancestors: Option<u32>,
) -> anyhow::Result<Response> {
    // 1. Resolve workspace
    let arc = match state.resolve_workspace(workspace).await {
        Some(a) => a,
        None => return Ok(workspace_not_found(workspace)),
    };

    // 2. Read lock: grab workspace path for /proc scan
    let ws_path_str = {
        let ws = arc.read().await;
        ws.index.workspace_path.to_string_lossy().to_string()
    };

    // 3. cwd guard outside lock — /proc scan may be slow
    if let Some(resp) = crate::util::guard_cwd_occupants(&ws_path_str).await {
        return Ok(resp);
    }

    // 4. Write lock: validate snapshot + execute rollback
    let mut ws = arc.write().await;

    let resolved_id = match resolve_rollback_target(&ws.index, to, num_ancestors) {
        Ok(id) => id,
        Err(resp) => return Ok(*resp),
    };

    if let Err(resp) = reject_missing_snapshot(&ws.index, &resolved_id) {
        return Ok(*resp);
    }

    // 5. Rollback via backend (includes warmup, snapshot, cleanup)
    state.backend.rollback(&ws.ws_id, &resolved_id).await?;

    // 6. Update head + migrate LIVE_CHILD
    if let Some(old_head) = ws.index.head.clone() {
        if let Some(hm) = ws.index.snapshots.get_mut(&old_head) {
            hm.child_ids.retain(|c| c != ws_ckpt_common::LIVE_CHILD);
        }
    }
    if let Some(hm) = ws.index.snapshots.get_mut(&resolved_id) {
        if !hm
            .child_ids
            .contains(&ws_ckpt_common::LIVE_CHILD.to_string())
        {
            hm.child_ids.push(ws_ckpt_common::LIVE_CHILD.to_string());
        }
    }
    ws.index.head = Some(resolved_id.clone());
    let snap_dir = state.index_dir(&ws.ws_id);
    if let Err(e) = crate::index_store::save(&snap_dir, &ws.index).await {
        tracing::warn!(
            "rollback index save failed (in-memory state is correct): {:#}",
            e
        );
    }

    Ok(Response::RollbackOk {
        from: ws.ws_id.clone(),
        to: resolved_id,
    })
}

/// Preview the file changes a rollback would apply without replacing the live workspace.
pub async fn rollback_preview(
    state: &Arc<DaemonState>,
    workspace: &str,
    to: Option<&str>,
    num_ancestors: Option<u32>,
) -> anyhow::Result<Response> {
    let arc = match state.resolve_workspace(workspace).await {
        Some(a) => a,
        None => return Ok(workspace_not_found(workspace)),
    };

    let (ws_id, resolved_id) = {
        let ws = arc.read().await;
        let id = match resolve_rollback_target(&ws.index, to, num_ancestors) {
            Ok(id) => id,
            Err(resp) => return Ok(*resp),
        };
        if let Err(resp) = reject_missing_snapshot(&ws.index, &id) {
            return Ok(*resp);
        }
        (ws.ws_id.clone(), id)
    };

    let changes = state.backend.diff(&ws_id, &resolved_id, None).await?;

    Ok(Response::RollbackPreviewOk {
        to: resolved_id,
        changes,
    })
}

fn resolve_rollback_target(
    index: &ws_ckpt_common::SnapshotIndex,
    to: Option<&str>,
    num_ancestors: Option<u32>,
) -> Result<String, Box<Response>> {
    if let Some(n) = num_ancestors {
        return index
            .ancestor(n as usize)
            .map(|(id, _)| id.clone())
            .map_err(|e| {
                Box::new(Response::Error {
                    code: ErrorCode::SnapshotNotFound,
                    message: e.to_string(),
                })
            });
    }

    if let Some(target) = to {
        return index
            .resolve_by_prefix(target)
            .map(|(id, _)| id.clone())
            .map_err(|err| Box::new(snapshot_resolve_error_response(target, err)));
    }

    Err(Box::new(Response::Error {
        code: ErrorCode::SnapshotNotFound,
        message: "either --snapshot or --num-ancestors must be specified".to_string(),
    }))
}

fn reject_missing_snapshot(
    index: &ws_ckpt_common::SnapshotIndex,
    snapshot_id: &str,
) -> Result<(), Box<Response>> {
    if index.snapshots.get(snapshot_id).is_some_and(|s| s.missing) {
        return Err(Box::new(Response::Error {
            code: ErrorCode::SnapshotNotFound,
            message: format!("Snapshot '{}' subvolume is missing (data lost). Use 'ws-ckpt delete --force -w <workspace> -s {}' to remove the record.", snapshot_id, snapshot_id),
        }));
    }

    Ok(())
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
    to: Option<&str>,
) -> anyhow::Result<Response> {
    let arc = match state.resolve_workspace(workspace).await {
        Some(a) => a,
        None => return Ok(workspace_not_found(workspace)),
    };

    let ws = arc.read().await;

    let from_id = match resolve_snapshot_id(&ws.index, from) {
        Ok(id) => id,
        Err(e) => return Ok(snapshot_resolve_error_response(from, e)),
    };
    let to_id = match to {
        Some(t) => {
            let id = match resolve_snapshot_id(&ws.index, t) {
                Ok(id) => id,
                Err(e) => return Ok(snapshot_resolve_error_response(t, e)),
            };
            Some(id)
        }
        None => None,
    };

    let changes = state
        .backend
        .diff(&ws.ws_id, &from_id, to_id.as_deref())
        .await?;

    Ok(Response::DiffOk { changes })
}

/// Resolve a snapshot reference (ID or prefix) to its ID.
///
/// Returns `ResolveError` directly so callers can map it to a user-facing
/// `Response::Error { code: SnapshotNotFound, .. }` rather than bubbling up
/// as an opaque `InternalError` via the dispatcher's anyhow fallback.
fn resolve_snapshot_id(
    index: &ws_ckpt_common::SnapshotIndex,
    reference: &str,
) -> Result<String, ResolveError> {
    index.resolve_by_prefix(reference).map(|(id, _)| id.clone())
}

/// Build a `SnapshotNotFound` response from a `ResolveError`.
fn snapshot_resolve_error_response(reference: &str, err: ResolveError) -> Response {
    let message = match err {
        ResolveError::NotFound => format!("snapshot not found: {}", reference),
        ResolveError::Ambiguous(n) => {
            format!("ambiguous snapshot prefix '{}': {} matches", reference, n)
        }
    };
    Response::Error {
        code: ErrorCode::SnapshotNotFound,
        message,
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

    // P1: plan under read lock — pure in-memory work, no fs I/O.
    let (ws_id, to_remove_ids, snap_dir) = {
        let ws = arc.read().await;
        let ws_id = ws.ws_id.clone();
        let snap_dir = state.index_dir(&ws_id);

        let mut unpinned: Vec<(String, chrono::DateTime<chrono::Utc>)> = ws
            .index
            .snapshots
            .iter()
            .filter(|(_, meta)| !meta.pinned)
            .map(|(id, meta)| (id.clone(), meta.created_at))
            .collect();
        unpinned.sort_by_key(|(_, ts)| *ts);

        let to_remove_ids: Vec<String> = if unpinned.len() > keep {
            unpinned[..unpinned.len() - keep]
                .iter()
                .map(|(id, _)| id.clone())
                .collect()
        } else {
            Vec::new()
        };

        (ws_id, to_remove_ids, snap_dir)
    };

    // P1.5: create index dir unlocked (slow fs must not block the ws write lock).
    tokio::fs::create_dir_all(&snap_dir)
        .await
        .with_context(|| format!("Failed to create index dir: {:?}", snap_dir))?;

    // P2: detach-then-delete (shared helper); persist (shared helper).
    let outcome = delete_snapshots_locked(state, &arc, &ws_id, &to_remove_ids, "cleanup").await;
    if !outcome.removed.is_empty() {
        persist_index_after_cleanup(state, &arc, &snap_dir, "cleanup").await;
    }
    if !outcome.failed.is_empty() {
        // User-triggered → surface partial failure as Err so the CLI exits non-zero.
        anyhow::bail!(
            "cleanup_snapshots: deleted {}/{}, failed: {:?}",
            outcome.removed.len(),
            to_remove_ids.len(),
            outcome.failed
        );
    }
    Ok(Response::CleanupOk {
        removed: outcome.removed,
    })
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
            parent_id: None,
            child_ids: vec![],
        }
    }

    fn make_snapshot_meta_at(pinned: bool, created_at: chrono::DateTime<Utc>) -> SnapshotMeta {
        SnapshotMeta {
            message: None,
            metadata: None,
            pinned,
            created_at,
            missing: false,
            parent_id: None,
            child_ids: vec![],
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
        let resp = rollback(&state, "/nonexistent/ws/12345", Some("msg1-step0"), None)
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
        let resp = rollback(&state, &path, Some("msg1-step0"), None)
            .await
            .unwrap();
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
            parent_id: None,
            child_ids: vec![],
        };
        assert!(pinned.pinned);

        let unpinned = SnapshotMeta {
            message: None,
            metadata: None,
            pinned: false,
            created_at: chrono::Utc::now(),
            missing: false,
            parent_id: None,
            child_ids: vec![],
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
        assert_eq!(result.unwrap_err(), ResolveError::NotFound);
    }

    #[test]
    fn resolve_snapshot_id_ambiguous_prefix() {
        let mut index = SnapshotIndex::new(PathBuf::from("/ws"));
        index
            .snapshots
            .insert("abcd111".to_string(), make_snapshot_meta(false));
        index
            .snapshots
            .insert("abcd222".to_string(), make_snapshot_meta(false));
        assert_eq!(
            resolve_snapshot_id(&index, "abcd").unwrap_err(),
            ResolveError::Ambiguous(2)
        );
    }

    /// Regression: user-input errors on `diff` must surface as
    /// `SnapshotNotFound`, not as `InternalError` via the dispatcher fallback.
    #[tokio::test]
    async fn diff_snapshots_missing_id_returns_snapshot_not_found() {
        let state = Arc::new(crate::state::DaemonState::new(
            test_config(),
            test_backend(),
            test_state_dir(),
        ));
        let mut index = SnapshotIndex::new(PathBuf::from("/home/user/ws"));
        index
            .snapshots
            .insert("real-id".to_string(), make_snapshot_meta(false));
        state.register_workspace("ws-diff".to_string(), PathBuf::from("/home/user/ws"), index);

        let resp = diff_snapshots(&state, "ws-diff", "does-not-exist", Some("real-id"))
            .await
            .unwrap();
        match resp {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::SnapshotNotFound);
                assert!(message.contains("does-not-exist"), "got: {}", message);
            }
            other => panic!("expected SnapshotNotFound, got: {:?}", other),
        }

        // Also covers the `to`-side branch.
        let resp = diff_snapshots(&state, "ws-diff", "real-id", Some("missing-to"))
            .await
            .unwrap();
        match resp {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::SnapshotNotFound);
                assert!(message.contains("missing-to"), "got: {}", message);
            }
            other => panic!("expected SnapshotNotFound, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn diff_and_rollback_preview_reach_live_diff_backend() {
        use ws_ckpt_common::DiffEntry;

        struct DiffStubBackend;

        #[async_trait::async_trait]
        impl StorageBackend for DiffStubBackend {
            fn backend_type(&self) -> ws_ckpt_common::backend::BackendType {
                ws_ckpt_common::backend::BackendType::BtrfsBase
            }
            fn data_root(&self) -> &std::path::Path {
                std::path::Path::new("/tmp/stub")
            }
            fn snapshots_root(&self) -> &std::path::Path {
                std::path::Path::new("/tmp/stub-snaps")
            }
            async fn diff(
                &self,
                ws_id: &str,
                from: &str,
                to: Option<&str>,
            ) -> anyhow::Result<Vec<DiffEntry>> {
                assert_eq!(ws_id, "ws-diff-live");
                assert_eq!(from, "snap-from");
                assert!(to.is_none(), "expected to=None, got {:?}", to);
                Ok(vec![DiffEntry {
                    path: "live-change.txt".into(),
                    change_type: ws_ckpt_common::ChangeType::Added,
                    detail: None,
                }])
            }
            async fn init_workspace(
                &self,
                _: &str,
                _: &str,
            ) -> anyhow::Result<ws_ckpt_common::WorkspaceInfo> {
                unimplemented!()
            }
            async fn create_snapshot(&self, _: &str, _: &str) -> anyhow::Result<()> {
                unimplemented!()
            }
            async fn rollback(&self, _: &str, _: &str) -> anyhow::Result<PathBuf> {
                unimplemented!()
            }
            async fn delete_snapshot(&self, _: &str, _: &str) -> anyhow::Result<()> {
                unimplemented!()
            }
            async fn recover_workspace(&self, _: &str, _: &str) -> anyhow::Result<()> {
                unimplemented!()
            }
            async fn cleanup_snapshots(
                &self,
                _: &str,
                _: &[String],
            ) -> anyhow::Result<Vec<String>> {
                unimplemented!()
            }
            async fn fork(&self, _: &str, _: &str, _: &str) -> anyhow::Result<()> {
                unimplemented!()
            }
            async fn gc_generations(
                &self,
                _: &str,
            ) -> anyhow::Result<ws_ckpt_common::backend::GcResult> {
                unimplemented!()
            }
            async fn check_environment(
                &self,
            ) -> anyhow::Result<ws_ckpt_common::backend::EnvironmentStatus> {
                unimplemented!()
            }
            async fn get_usage(&self) -> anyhow::Result<(u64, u64)> {
                unimplemented!()
            }
        }

        let state = Arc::new(crate::state::DaemonState::new(
            test_config(),
            Arc::new(DiffStubBackend),
            test_state_dir(),
        ));
        let mut index = SnapshotIndex::new(PathBuf::from("/home/user/ws"));
        index
            .snapshots
            .insert("snap-from".to_string(), make_snapshot_meta(false));
        state.register_workspace(
            "ws-diff-live".to_string(),
            PathBuf::from("/home/user/ws"),
            index,
        );

        let resp = diff_snapshots(&state, "ws-diff-live", "snap-from", None)
            .await
            .unwrap();
        match resp {
            Response::DiffOk { changes } => {
                assert_eq!(changes.len(), 1);
                assert_eq!(changes[0].path, "live-change.txt");
            }
            other => panic!("expected DiffOk, got: {:?}", other),
        }

        let resp = rollback_preview(&state, "ws-diff-live", Some("snap-from"), None)
            .await
            .unwrap();
        match resp {
            Response::RollbackPreviewOk { to, changes } => {
                assert_eq!(to, "snap-from");
                assert_eq!(changes.len(), 1);
                assert_eq!(changes[0].path, "live-change.txt");
            }
            other => panic!("expected RollbackPreviewOk, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn rollback_preview_workspace_not_found() {
        let state = Arc::new(crate::state::DaemonState::new(
            test_config(),
            test_backend(),
            test_state_dir(),
        ));

        let resp = rollback_preview(&state, "missing-workspace", Some("snap1"), None)
            .await
            .unwrap();
        match resp {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::WorkspaceNotFound);
                assert!(message.contains("missing-workspace"));
            }
            other => panic!("expected WorkspaceNotFound, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn rollback_preview_snapshot_not_found() {
        let state = Arc::new(crate::state::DaemonState::new(
            test_config(),
            test_backend(),
            test_state_dir(),
        ));
        state.register_workspace(
            "ws-preview".to_string(),
            PathBuf::from("/home/user/ws"),
            SnapshotIndex::new(PathBuf::from("/home/user/ws")),
        );

        let resp = rollback_preview(&state, "ws-preview", Some("missing-snapshot"), None)
            .await
            .unwrap();
        match resp {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::SnapshotNotFound);
                assert!(message.contains("missing-snapshot"));
            }
            other => panic!("expected SnapshotNotFound, got: {:?}", other),
        }
    }

    // ── cleanup_snapshots partial-failure tests ──
    //
    // Backstop for the "no early `?` in the delete loop" invariant
    // (delete_snapshots_locked): if backend.cleanup_snapshots succeeds for
    // snaps A,B,C then fails for D, the index must still have A/B/C removed
    // (and persisted) and D rolled back to its previous meta. The caller
    // (cleanup_snapshots) then bails with a count summary so the CLI exits
    // non-zero. Without the shared helper or with an early `?`, prior
    // in-memory removes would be lost.

    struct PartialFailBackend {
        data_root: PathBuf,
        snapshots_root: PathBuf,
        fail_ids: std::collections::HashSet<String>,
    }

    impl PartialFailBackend {
        fn new(fail_ids: impl IntoIterator<Item = String>) -> Self {
            Self {
                data_root: PathBuf::from("/tmp/pfb-data"),
                snapshots_root: PathBuf::from("/tmp/pfb-snaps"),
                fail_ids: fail_ids.into_iter().collect(),
            }
        }
    }

    #[async_trait::async_trait]
    impl StorageBackend for PartialFailBackend {
        fn backend_type(&self) -> ws_ckpt_common::backend::BackendType {
            ws_ckpt_common::backend::BackendType::BtrfsBase
        }
        fn data_root(&self) -> &std::path::Path {
            &self.data_root
        }
        fn snapshots_root(&self) -> &std::path::Path {
            &self.snapshots_root
        }
        async fn cleanup_snapshots(
            &self,
            _ws_id: &str,
            snapshot_ids: &[String],
        ) -> anyhow::Result<Vec<String>> {
            // Single-snap calls only (matches snapshot_mgr's per-snap loop).
            let id = snapshot_ids
                .first()
                .expect("PartialFailBackend expects per-snap calls");
            if self.fail_ids.contains(id) {
                anyhow::bail!("simulated backend failure for {}", id);
            }
            Ok(vec![id.clone()])
        }
        // Everything else: panic if hit — keeps the test honest about which
        // backend methods cleanup actually exercises.
        async fn init_workspace(
            &self,
            _: &str,
            _: &str,
        ) -> anyhow::Result<ws_ckpt_common::WorkspaceInfo> {
            unimplemented!()
        }
        async fn create_snapshot(&self, _: &str, _: &str) -> anyhow::Result<()> {
            unimplemented!()
        }
        async fn rollback(&self, _: &str, _: &str) -> anyhow::Result<PathBuf> {
            unimplemented!()
        }
        async fn delete_snapshot(&self, _: &str, _: &str) -> anyhow::Result<()> {
            unimplemented!()
        }
        async fn recover_workspace(&self, _: &str, _: &str) -> anyhow::Result<()> {
            unimplemented!()
        }
        async fn diff(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
        ) -> anyhow::Result<Vec<ws_ckpt_common::DiffEntry>> {
            unimplemented!()
        }
        async fn fork(&self, _: &str, _: &str, _: &str) -> anyhow::Result<()> {
            unimplemented!()
        }
        async fn gc_generations(
            &self,
            _: &str,
        ) -> anyhow::Result<ws_ckpt_common::backend::GcResult> {
            unimplemented!()
        }
        async fn check_environment(
            &self,
        ) -> anyhow::Result<ws_ckpt_common::backend::EnvironmentStatus> {
            unimplemented!()
        }
        async fn get_usage(&self) -> anyhow::Result<(u64, u64)> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn cleanup_snapshots_persists_partial_success_and_bails() {
        // 5 unpinned snapshots, the 3rd fails. Expectation:
        //   - bail with "deleted 4/5, failed: [snap-3]"
        //   - in-memory index keeps only snap-3 (others removed)
        //   - failed snap meta is re-inserted (no detach drift)
        let tmp = tempfile::tempdir().unwrap();
        let backend = Arc::new(PartialFailBackend::new(["snap-3".to_string()]));
        let state = Arc::new(crate::state::DaemonState::new(
            test_config(),
            backend as Arc<dyn StorageBackend>,
            tmp.path().to_path_buf(),
        ));

        let ws_path = PathBuf::from("/ws/partial-fail");
        let mut idx = SnapshotIndex::new(ws_path.clone());
        let now = Utc::now();
        for (i, off) in [0i64, 1, 2, 3, 4].iter().enumerate() {
            idx.snapshots.insert(
                format!("snap-{}", i + 1),
                make_snapshot_meta_at(false, now - Duration::seconds(*off)),
            );
        }
        state.register_workspace("ws-partial".to_string(), ws_path.clone(), idx);

        // keep=0 → all 5 are removal candidates. cleanup_snapshots returns
        // Ok(Response) for routing errors (e.g. WorkspaceNotFound) but Err
        // for partial backend failure — so the user-facing CLI exits non-zero.
        let result = cleanup_snapshots(&state, "ws-partial", Some(0)).await;
        let err = result.expect_err("partial failure must bubble up as Err");
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("4/5"),
            "error should report deleted/total, got: {}",
            msg
        );
        assert!(
            msg.contains("snap-3"),
            "error should name the failed id, got: {}",
            msg
        );

        // In-memory index: snap-3 stays (re-inserted), others gone.
        let arc = state
            .get_by_wsid("ws-partial")
            .expect("ws still registered");
        let ws = arc.read().await;
        assert_eq!(ws.index.snapshots.len(), 1, "only the failed snap remains");
        assert!(ws.index.snapshots.contains_key("snap-3"));

        // Persisted index reflects the same — earlier successes were saved
        // even though the caller bailed.
        let on_disk = crate::index_store::load(&state.index_dir("ws-partial"))
            .await
            .expect("index.json saved");
        assert_eq!(on_disk.snapshots.len(), 1);
        assert!(on_disk.snapshots.contains_key("snap-3"));
    }
}
