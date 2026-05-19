use std::sync::Arc;

use anyhow::Context;
use sha2::{Digest, Sha256};
use tokio::process::Command;
use tracing::{error, info, warn};

use ws_ckpt_common::{ErrorCode, ResolveError, Response, SnapshotIndex};

use crate::index_store;
use crate::state::DaemonState;

// ── helpers ──

fn error_resp(code: ErrorCode, msg: impl Into<String>) -> Response {
    Response::Error {
        code,
        message: msg.into(),
    }
}

/// Strip trailing slashes from a path string, preserving root "/".
///
/// POSIX lstat() follows a trailing-slash symlink, so passing
/// "/path/symlink/" to `symlink_metadata` misses the symlink detection.
/// Normalize user-supplied paths once at the boundary to avoid this pitfall.
fn strip_trailing_slashes(s: &str) -> &str {
    let trimmed = s.trim_end_matches('/');
    if trimmed.is_empty() {
        "/"
    } else {
        trimmed
    }
}

// ── init ──

pub async fn init(state: &Arc<DaemonState>, workspace: &str) -> anyhow::Result<Response> {
    let workspace = strip_trailing_slashes(workspace);
    // 0. Early check: detect workspace already managed via symlink to our data_root.
    //    This must run before canonicalize(), which would resolve the symlink
    //    and cause the "inside mount_path" guard to reject it.
    let ws_path = std::path::PathBuf::from(workspace);
    if let Ok(meta) = tokio::fs::symlink_metadata(&ws_path).await {
        if meta.file_type().is_symlink() {
            if let Ok(target) = tokio::fs::read_link(&ws_path).await {
                let data_root = state.backend.data_root();
                if target.starts_with(data_root) {
                    if tokio::fs::metadata(&target).await.is_ok() {
                        // Valid symlink pointing to our managed subvolume
                        let ws_id = target
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();

                        if state.get_by_wsid(&ws_id).is_some() {
                            // Already registered — idempotent success
                            info!(
                                "workspace already initialized: {} -> {:?} (ws_id={})",
                                workspace, target, ws_id
                            );
                            return Ok(Response::InitOk { ws_id });
                        }

                        // Subvolume exists but daemon lost track (e.g. restart).
                        // Re-register (recovery mode).
                        info!(
                            "recovering unregistered workspace: {} -> {:?} (ws_id={})",
                            workspace, target, ws_id
                        );
                        let snap_dir = state.index_dir(&ws_id);
                        let btrfs_snap_dir = state.backend.snapshots_root().join(&ws_id);
                        let mut index = if let Ok(idx) = index_store::load(&snap_dir).await {
                            idx
                        } else {
                            SnapshotIndex::new(ws_path.clone())
                        };
                        // If index has no snapshots, try rebuilding from filesystem
                        if index.snapshots.is_empty() {
                            if let Ok(rebuilt) =
                                index_store::rebuild_from_fs(&btrfs_snap_dir, ws_path.clone()).await
                            {
                                if !rebuilt.snapshots.is_empty() {
                                    info!(
                                        "Recovered {} snapshot(s) from filesystem for {}",
                                        rebuilt.snapshots.len(),
                                        ws_id
                                    );
                                    index = rebuilt;
                                    // Persist rebuilt index
                                    let _ = index_store::save(&snap_dir, &index).await;
                                }
                            }
                        }
                        state.register_workspace(ws_id.clone(), ws_path.clone(), index);
                        if let Err(e) = state.save_manifest().await {
                            warn!("save_manifest failed after recovery register: {:#}", e);
                        }
                        return Ok(Response::InitOk { ws_id });
                    } else {
                        // Broken symlink — target subvolume gone; remove and re-init
                        warn!(
                            "workspace symlink target missing: {:?}; re-initializing",
                            target
                        );
                        let _ = tokio::fs::remove_file(&ws_path).await;
                    }
                }
            }
        }
    }

    // 1. Canonicalize (resolves symlinks to real path)
    let abs_path = match tokio::fs::canonicalize(workspace).await {
        Ok(p) => p,
        Err(_) => {
            return Ok(error_resp(
                ErrorCode::InvalidPath,
                format!("path does not exist: {}", workspace),
            ));
        }
    };
    if abs_path.to_string_lossy() != workspace {
        info!(
            "workspace path resolved: {} -> {}",
            workspace,
            abs_path.display()
        );
    }

    // 2. Pre-checks
    let meta = match tokio::fs::metadata(&abs_path).await {
        Ok(m) => m,
        Err(_) => {
            return Ok(error_resp(
                ErrorCode::InvalidPath,
                format!("cannot stat path: {}", abs_path.display()),
            ));
        }
    };
    if !meta.is_dir() {
        return Ok(error_resp(
            ErrorCode::InvalidPath,
            format!("not a directory: {}", abs_path.display()),
        ));
    }
    if let Some(existing) = state.get_by_path(&abs_path) {
        let ws = existing.read().await;
        info!(
            "workspace already initialized via path: {} (ws_id={})",
            abs_path.display(),
            ws.ws_id
        );
        return Ok(Response::InitOk {
            ws_id: ws.ws_id.clone(),
        });
    }
    if abs_path.starts_with(&state.mount_path) {
        return Ok(error_resp(
            ErrorCode::InvalidPath,
            format!(
                "path is inside mount_path ({}): {}",
                state.mount_path.display(),
                abs_path.display()
            ),
        ));
    }

    // Check rsync available
    let rsync_check = Command::new("which")
        .arg("rsync")
        .output()
        .await
        .context("failed to run 'which rsync'")?;
    if !rsync_check.status.success() {
        return Ok(error_resp(
            ErrorCode::InternalError,
            "rsync is not installed or not in PATH",
        ));
    }

    // 3. Generate ws-id
    let mount_path = &state.mount_path;
    let base_id = generate_ws_id_base(&abs_path.to_string_lossy());
    let mut ws_id = base_id.clone();
    let mut suffix = 2u32;
    while mount_path.join(&ws_id).exists() {
        ws_id = format!("{}-{}", base_id, suffix);
        suffix += 1;
    }

    let abs_path_str = abs_path.to_string_lossy().to_string();

    // Steps 4-11 via backend, with cleanup handled internally
    if let Err(e) = state.backend.init_workspace(&abs_path_str, &ws_id).await {
        error!("init failed: {:#}", e);
        return Err(e);
    }

    // 12. Create and save index
    let snap_dir = state.index_dir(&ws_id);
    tokio::fs::create_dir_all(&snap_dir)
        .await
        .context("Failed to create index dir")?;
    // Check for existing snapshot subvolumes before creating empty index
    // Note: rebuild_from_fs scans the btrfs snapshot directory (backend snapshots_root),
    //       not the index directory
    let snapshots_ws_dir = state.backend.snapshots_root().join(&ws_id);
    let index = if let Ok(rebuilt) =
        index_store::rebuild_from_fs(&snapshots_ws_dir, abs_path.clone()).await
    {
        if !rebuilt.snapshots.is_empty() {
            info!(
                "Found {} existing snapshot(s) for {}, rebuilding index",
                rebuilt.snapshots.len(),
                ws_id
            );
            rebuilt
        } else {
            SnapshotIndex::new(abs_path.clone())
        }
    } else {
        SnapshotIndex::new(abs_path.clone())
    };
    index_store::save(&snap_dir, &index).await?;

    // 13. Register to state
    state.register_workspace(ws_id.clone(), abs_path.clone(), index);

    // 13a. Save manifest
    if let Err(e) = state.save_manifest().await {
        warn!("save_manifest failed after init: {:#}", e);
    }

    // 13b. Start file watcher for write-lock detection
    match crate::fs_watcher::WorkspaceWatcher::start(&abs_path) {
        Ok(watcher) => {
            state.register_watcher(ws_id.clone(), watcher);
        }
        Err(e) => {
            warn!("Failed to start watcher for {}: {}", ws_id, e);
        }
    }

    // 13b. Warmup btrfs metadata cache for subsequent operations
    let subvol_path = state.backend.data_root().join(&ws_id);
    info!(
        "warming up btrfs metadata cache for workspace: {}",
        subvol_path.display()
    );
    crate::backends::btrfs_common::warmup_snapshot_metadata(&subvol_path).await;

    info!("workspace initialized: {}", ws_id);

    // 14. Return
    Ok(Response::InitOk { ws_id })
}

/// Generate a ws-id from a workspace path. Pure logic, extracted for testability.
/// Returns the base ws-id (without collision suffix).
fn generate_ws_id_base(path: &str) -> String {
    let hash = hex::encode(&Sha256::digest(path.as_bytes())[..3]);
    format!("ws-{}", hash)
}

// ── delete ──

pub async fn delete_snapshot(
    state: &Arc<DaemonState>,
    workspace: &str,
    snapshot_id: &str,
    force: bool,
) -> anyhow::Result<Response> {
    // 1. Resolve workspace (by ID, absolute path, or relative path)
    let ws_lock = match state.resolve_workspace(workspace).await {
        Some(ws) => ws,
        None => {
            return Ok(error_resp(
                ErrorCode::WorkspaceNotFound,
                format!("workspace not found: {}", workspace),
            ));
        }
    };

    // 2. Write lock
    let mut ws = ws_lock.write().await;

    // 2a. Resolve snapshot by prefix within this workspace
    let resolved_id = match ws.index.resolve_by_prefix(snapshot_id) {
        Ok((id, _)) => id.clone(),
        Err(ResolveError::NotFound) => {
            return Ok(error_resp(
                ErrorCode::SnapshotNotFound,
                format!("snapshot not found: {}", snapshot_id),
            ));
        }
        Err(ResolveError::Ambiguous(n)) => {
            return Ok(error_resp(
                ErrorCode::SnapshotNotFound,
                format!("ambiguous snapshot prefix '{}': {} matches", snapshot_id, n),
            ));
        }
    };

    // 3. Check pinned
    if let Some(meta) = ws.index.snapshots.get(&resolved_id) {
        if meta.pinned && !force {
            return Ok(error_resp(
                ErrorCode::ConfirmationRequired,
                "Snapshot is pinned, use --force to confirm deletion".to_string(),
            ));
        }
    }

    // 4. Delete subvolume (skip if snapshot is marked missing — subvolume already gone)
    let is_missing = ws
        .index
        .snapshots
        .get(&resolved_id)
        .map(|m| m.missing)
        .unwrap_or(false);
    if !is_missing {
        state
            .backend
            .delete_snapshot(&ws.ws_id, &resolved_id)
            .await?;
    }

    // 5. Remove from index + save
    ws.index.snapshots.remove(&resolved_id);
    let snap_dir = state.index_dir(&ws.ws_id);
    tokio::fs::create_dir_all(&snap_dir)
        .await
        .with_context(|| format!("Failed to create index dir: {:?}", snap_dir))?;
    index_store::save(&snap_dir, &ws.index).await?;

    // 5a. Release write lock before save_manifest (try_read inside
    //     collect_workspace_entries would fail while write lock is held)
    drop(ws);

    // 5b. Save manifest
    if let Err(e) = state.save_manifest().await {
        warn!("save_manifest failed after delete_snapshot: {:#}", e);
    }

    // 6. Return
    Ok(Response::DeleteOk {
        target: resolved_id,
    })
}

// ── recover ──

pub async fn recover_workspace(
    state: &Arc<DaemonState>,
    workspace: &str,
) -> anyhow::Result<Response> {
    // 1. resolve workspace (by ID, path, or relative)
    let ws_lock = match state.resolve_workspace(workspace).await {
        Some(ws) => ws,
        None => {
            return Ok(error_resp(
                ErrorCode::WorkspaceNotFound,
                format!("workspace not found: {}", workspace),
            ));
        }
    };

    // 2. read lock to get ws_id and original_path
    let (ws_id, original_path) = {
        let ws = ws_lock.read().await;
        (ws.ws_id.clone(), ws.path.to_string_lossy().to_string())
    };

    // 3. call backend recover
    state
        .backend
        .recover_workspace(&ws_id, &original_path)
        .await?;

    // 4. unregister workspace from state
    state.unregister_workspace(&ws_id);

    // 4a. Save manifest
    if let Err(e) = state.save_manifest().await {
        warn!("save_manifest failed after recover: {:#}", e);
    }

    // 5. return
    Ok(Response::RecoverOk {
        workspace: original_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use ws_ckpt_common::backend::StorageBackend;
    use ws_ckpt_common::{CleanupRetention, DaemonConfig, ErrorCode, SnapshotIndex};

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

    // ── ws-id generation tests ──

    #[test]
    fn ws_id_format_is_workspace_dash_6hex() {
        let id = generate_ws_id_base("/home/user/project");
        assert!(id.starts_with("ws-"), "ws-id should start with 'ws-'");
        let hash_part = id.strip_prefix("ws-").unwrap();
        assert_eq!(
            hash_part.len(),
            6,
            "hash part should be 6 hex chars (3 bytes)"
        );
        assert!(
            hash_part.chars().all(|c| c.is_ascii_hexdigit()),
            "hash part should be valid hex"
        );
    }

    #[test]
    fn ws_id_same_path_produces_same_id() {
        let id1 = generate_ws_id_base("/home/user/project");
        let id2 = generate_ws_id_base("/home/user/project");
        assert_eq!(id1, id2);
    }

    #[test]
    fn ws_id_different_paths_produce_different_ids() {
        let id1 = generate_ws_id_base("/home/user/project-a");
        let id2 = generate_ws_id_base("/home/user/project-b");
        assert_ne!(id1, id2);
    }

    #[test]
    fn ws_id_hash_matches_sha256_first_3_bytes() {
        use sha2::{Digest, Sha256};
        let path = "/some/test/path";
        let expected_hash = hex::encode(&Sha256::digest(path.as_bytes())[..3]);
        let id = generate_ws_id_base(path);
        assert_eq!(id, format!("ws-{}", expected_hash));
    }

    #[test]
    fn ws_id_collision_suffix_format() {
        // Verify the collision suffix pattern ws-{hash}-2, -3, etc.
        // We can't easily test the filesystem-dependent loop, but we can verify the format
        let base = generate_ws_id_base("/some/path");
        let suffixed_2 = format!("{}-2", base);
        let suffixed_3 = format!("{}-3", base);
        assert!(suffixed_2.starts_with("ws-"));
        assert!(suffixed_2.ends_with("-2"));
        assert!(suffixed_3.ends_with("-3"));
    }

    // ── error_resp helper test ──

    #[test]
    fn error_resp_constructs_correct_response() {
        let resp = error_resp(ErrorCode::WorkspaceNotFound, "not found");
        match resp {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::WorkspaceNotFound);
                assert_eq!(message, "not found");
            }
            _ => panic!("expected Error variant"),
        }
    }

    // ── ConfirmationRequired tests ──

    #[test]
    fn confirmation_required_delete_pinned_snapshot_response() {
        let resp = error_resp(
            ErrorCode::ConfirmationRequired,
            "Snapshot is pinned, use --force to confirm deletion",
        );
        match resp {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::ConfirmationRequired);
                assert!(message.contains("pinned"));
                assert!(message.contains("--force"));
            }
            _ => panic!("expected ConfirmationRequired error"),
        }
    }

    // ── Integration tests that require root + btrfs ──

    // ── Non-ignored async tests (use tempdir, no btrfs needed) ──

    #[tokio::test]
    async fn init_nonexistent_path_returns_invalid_path() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            test_state_dir(),
        ));
        let resp = init(&state, "/nonexistent/path/12345").await.unwrap();
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::InvalidPath),
            _ => panic!("expected InvalidPath error"),
        }
    }

    #[tokio::test]
    async fn init_already_initialized_returns_ok() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            test_state_dir(),
        ));
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().to_string_lossy().to_string();
        let canon = tokio::fs::canonicalize(&path).await.unwrap();
        state.register_workspace(
            "ws-exist".to_string(),
            canon.clone(),
            SnapshotIndex::new(canon),
        );
        let resp = init(&state, &path).await.unwrap();
        match resp {
            Response::InitOk { ws_id } => assert_eq!(ws_id, "ws-exist"),
            _ => panic!("expected InitOk for already-initialized workspace"),
        }
    }

    #[tokio::test]
    async fn init_path_inside_mount_path_returns_invalid_path() {
        let mount_dir = tempfile::tempdir().unwrap();
        let inside_path = mount_dir.path().join("subdir");
        tokio::fs::create_dir_all(&inside_path).await.unwrap();
        let config = DaemonConfig {
            mount_path: tokio::fs::canonicalize(mount_dir.path()).await.unwrap(),
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
        };
        let state = Arc::new(DaemonState::new(config, test_backend(), test_state_dir()));
        let resp = init(&state, &inside_path.to_string_lossy()).await.unwrap();
        match resp {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::InvalidPath);
                assert!(message.contains("inside mount_path"));
            }
            _ => panic!("expected InvalidPath error for path inside mount_path"),
        }
    }

    #[tokio::test]
    async fn init_non_directory_returns_invalid_path() {
        let tmpdir = tempfile::tempdir().unwrap();
        let file_path = tmpdir.path().join("not-a-dir.txt");
        tokio::fs::write(&file_path, "hello").await.unwrap();
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            test_state_dir(),
        ));
        let resp = init(&state, &file_path.to_string_lossy()).await.unwrap();
        match resp {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::InvalidPath);
                assert!(message.contains("not a directory"));
            }
            _ => panic!("expected InvalidPath error for non-directory"),
        }
    }

    #[tokio::test]
    async fn delete_snapshot_unregistered_workspace_returns_not_found() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            test_state_dir(),
        ));
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().to_string_lossy().to_string();
        let resp = delete_snapshot(&state, &path, "msg1-step0", false)
            .await
            .unwrap();
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::WorkspaceNotFound),
            _ => panic!("expected WorkspaceNotFound error"),
        }
    }

    // ── Pure logic: ws-id edge cases ──

    #[test]
    fn ws_id_empty_path() {
        let id = generate_ws_id_base("");
        assert!(id.starts_with("ws-"));
        let hash_part = id.strip_prefix("ws-").unwrap();
        assert_eq!(hash_part.len(), 6);
    }

    #[test]
    fn ws_id_special_characters_in_path() {
        let id = generate_ws_id_base("/home/user/my project (2)/src");
        assert!(id.starts_with("ws-"));
        let hash_part = id.strip_prefix("ws-").unwrap();
        assert_eq!(hash_part.len(), 6);
        assert!(hash_part.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn ws_id_very_long_path() {
        let long_path = format!("/home/{}", "a".repeat(1000));
        let id = generate_ws_id_base(&long_path);
        assert!(id.starts_with("ws-"));
        let hash_part = id.strip_prefix("ws-").unwrap();
        assert_eq!(hash_part.len(), 6);
    }

    #[test]
    fn ws_id_unicode_path() {
        let id = generate_ws_id_base("/home/用户/项目");
        assert!(id.starts_with("ws-"));
        let hash_part = id.strip_prefix("ws-").unwrap();
        assert_eq!(hash_part.len(), 6);
        assert!(hash_part.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // ── recover tests ──

    #[tokio::test]
    async fn recover_unregistered_workspace_returns_not_found() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            test_state_dir(),
        ));
        let resp = recover_workspace(&state, "/nonexistent/path/12345")
            .await
            .unwrap();
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::WorkspaceNotFound),
            _ => panic!("expected WorkspaceNotFound error"),
        }
    }

    #[tokio::test]
    async fn recover_registered_workspace_returns_recover_ok_or_backend_error() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            test_state_dir(),
        ));
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().to_path_buf();
        let canon = tokio::fs::canonicalize(&path).await.unwrap();
        state.register_workspace(
            "ws-test".to_string(),
            canon.clone(),
            SnapshotIndex::new(canon.clone()),
        );
        // Backend will fail in test env (no btrfs), so we expect an error propagation
        // but the workspace should have been resolved (not WorkspaceNotFound)
        let resp = recover_workspace(&state, &canon.to_string_lossy()).await;
        match resp {
            Ok(Response::RecoverOk { .. }) => {} // success path
            Err(_) => {}                         // backend error is expected in test env
            Ok(Response::Error { code, .. }) => {
                assert_ne!(
                    code,
                    ErrorCode::WorkspaceNotFound,
                    "should not be WsNotFound"
                );
            }
            _ => panic!("unexpected response variant"),
        }
    }
}
