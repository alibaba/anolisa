use crate::state::DaemonState;
use std::sync::Arc;
use ws_ckpt_common::{
    default_auto_cleanup_keep, load_config_file, ConfigReport, ErrorCode, Request, Response,
    StatusReport, WorkspaceInfo, ADVISORY_SNAPSHOT_LIMIT, CONFIG_FILE_PATH, DEFAULT_AUTO_CLEANUP,
    DEFAULT_AUTO_CLEANUP_INTERVAL_SECS, DEFAULT_HEALTH_CHECK_INTERVAL_SECS,
    DEFAULT_IMG_MAX_PERCENT, DEFAULT_IMG_SIZE_GB,
};

pub async fn dispatch(state: &Arc<DaemonState>, request: Request) -> Response {
    let result = match request {
        Request::Init { workspace } => match state.ensure_bootstrapped().await {
            Err(e) => Err(e),
            Ok(()) => crate::workspace_mgr::init(state, &workspace).await,
        },
        Request::Checkpoint {
            workspace,
            id,
            message,
            metadata,
            pin,
        } => match state.ensure_bootstrapped().await {
            Err(e) => Err(e),
            Ok(()) => match auto_init_workspace(state, &workspace).await {
                Ok(Some(err_resp)) => return err_resp,
                Err(e) => Err(e),
                Ok(None) => {
                    crate::snapshot_mgr::checkpoint(state, &workspace, &id, message, metadata, pin)
                        .await
                }
            },
        },
        Request::Rollback { workspace, to } => match state.ensure_bootstrapped().await {
            Err(e) => Err(e),
            Ok(()) => crate::snapshot_mgr::rollback(state, &workspace, &to).await,
        },
        Request::Delete {
            workspace,
            snapshot,
            force,
        } => match state.ensure_bootstrapped().await {
            Err(e) => Err(e),
            Ok(()) => match workspace {
                Some(ws) => {
                    crate::workspace_mgr::delete_snapshot(state, &ws, &snapshot, force).await
                }
                None => {
                    // Global lookup: find snapshot across all workspaces
                    match state.resolve_snapshot_globally(&snapshot).await {
                        Some((ws_path, resolved_id)) => {
                            crate::workspace_mgr::delete_snapshot(
                                state,
                                &ws_path,
                                &resolved_id,
                                force,
                            )
                            .await
                        }
                        None => {
                            // Check if it's ambiguous or truly not found
                            let mut match_count = 0usize;
                            for entry in state.all_workspaces() {
                                let ws = entry.read().await;
                                match ws.index.resolve_by_prefix(&snapshot) {
                                    Ok(_) => match_count += 1,
                                    Err(ws_ckpt_common::ResolveError::Ambiguous(_)) => {
                                        match_count += 2
                                    }
                                    Err(ws_ckpt_common::ResolveError::NotFound) => {}
                                }
                            }
                            if match_count > 1 {
                                Ok(Response::Error {
                                    code: ErrorCode::SnapshotNotFound,
                                    message: format!(
                                        "snapshot '{}' matches in multiple workspaces, please specify --workspace/-w",
                                        snapshot
                                    ),
                                })
                            } else {
                                Ok(Response::Error {
                                    code: ErrorCode::SnapshotNotFound,
                                    message: format!("snapshot not found: {}", snapshot),
                                })
                            }
                        }
                    }
                }
            },
        },
        Request::List { workspace, .. } => match workspace {
            Some(ws) => crate::snapshot_mgr::list_snapshots(state, &ws).await,
            None => crate::snapshot_mgr::list_all_snapshots(state).await,
        },
        Request::Diff {
            workspace,
            from,
            to,
        } => match state.ensure_bootstrapped().await {
            Err(e) => Err(e),
            Ok(()) => crate::snapshot_mgr::diff_snapshots(state, &workspace, &from, &to).await,
        },
        Request::Status { workspace } => {
            // Inline status query logic
            handle_status(state, workspace.as_deref()).await
        }
        Request::Cleanup { workspace, keep } => match state.ensure_bootstrapped().await {
            Err(e) => Err(e),
            Ok(()) => crate::snapshot_mgr::cleanup_snapshots(state, &workspace, keep).await,
        },
        Request::Config => Ok(handle_config(state)),
        Request::ReloadConfig => Ok(handle_reload_config(state)),
        Request::Recover { workspace } => match state.ensure_bootstrapped().await {
            Err(e) => Err(e),
            Ok(()) => crate::workspace_mgr::recover_workspace(state, &workspace).await,
        },
        Request::HealthAdvisory => Ok(handle_health_advisory(state).await),
    };

    match result {
        Ok(response) => response,
        Err(e) => Response::Error {
            code: ErrorCode::InternalError,
            message: format!("{:#}", e),
        },
    }
}

/// Auto-initialize a workspace if it is not yet registered.
/// Returns `Ok(None)` if the workspace is ready (already existed or was just initialized).
/// Returns `Ok(Some(Response))` with an error response if auto-init fails in a user-facing way.
async fn auto_init_workspace(
    state: &Arc<DaemonState>,
    workspace: &str,
) -> anyhow::Result<Option<Response>> {
    if state.resolve_workspace(workspace).await.is_some() {
        return Ok(None); // already initialized
    }
    tracing::info!(
        "workspace not initialized, auto-initializing: {}",
        workspace
    );
    let resp = crate::workspace_mgr::init(state, workspace).await?;
    match resp {
        Response::InitOk { ws_id } => {
            tracing::info!("auto-init completed: ws_id={}", ws_id);
            Ok(None)
        }
        // AlreadyInitialized is fine (race condition)
        Response::Error {
            code: ErrorCode::AlreadyInitialized,
            ..
        } => Ok(None),
        // Other errors: propagate as-is
        err_resp @ Response::Error { .. } => Ok(Some(err_resp)),
        // Unexpected response variant (should not happen)
        other => Ok(Some(other)),
    }
}

/// Handle the Status request inline: gather daemon info, workspace list, and filesystem usage.
async fn handle_status(
    state: &Arc<DaemonState>,
    workspace: Option<&str>,
) -> anyhow::Result<Response> {
    let uptime_secs = state.start_time.elapsed().as_secs();

    let workspaces = if let Some(ws_str) = workspace {
        // Single-workspace mode: resolve by ID, absolute path, or relative path
        let arc = match state.resolve_workspace(ws_str).await {
            Some(a) => a,
            None => {
                return Ok(Response::Error {
                    code: ErrorCode::WorkspaceNotFound,
                    message: format!("workspace not found: {}", ws_str),
                });
            }
        };

        let ws = arc.read().await;
        vec![WorkspaceInfo {
            ws_id: ws.ws_id.clone(),
            path: ws.path.to_string_lossy().to_string(),
            snapshot_count: ws.index.snapshots.len() as u32,
        }]
    } else {
        // Global mode: return all workspaces
        state.get_all_workspace_info()
    };

    // Try to get filesystem usage; fallback to zeros on error (e.g., macOS)
    let (fs_total_bytes, fs_used_bytes) = match state.backend.get_usage().await {
        Ok((total, used)) => (total, used),
        Err(_) => (0, 0),
    };

    Ok(Response::StatusOk {
        report: StatusReport {
            uptime_secs,
            workspaces,
            fs_total_bytes,
            fs_used_bytes,
        },
    })
}

/// Handle the Config request: return the current daemon configuration.
fn handle_config(state: &Arc<DaemonState>) -> Response {
    let cfg = state.config.read().unwrap();
    Response::ConfigOk {
        config: ConfigReport {
            mount_path: state.mount_path.to_string_lossy().to_string(),
            socket_path: state.socket_path.to_string_lossy().to_string(),
            log_level: cfg.log_level.clone(),
            auto_cleanup: cfg.auto_cleanup,
            auto_cleanup_keep: cfg.auto_cleanup_keep.clone(),
            auto_cleanup_interval_secs: cfg.auto_cleanup_interval_secs,
            health_check_interval_secs: cfg.health_check_interval_secs,
            img_path: cfg.img_path.clone(),
            img_size: cfg.img_size,
            img_max_percent: cfg.img_max_percent,
        },
    }
}

/// Handle the ReloadConfig request: re-read config file and update runtime config.
///
/// NOTE: BtrfsLoop image fields (`img_size`, `img_max_percent`) take effect
/// only during daemon bootstrap. If they differ from the currently loaded values, a
/// warning is emitted to tell the operator that a daemon restart is required for the
/// new values to be applied (via img resize at bootstrap).
fn handle_reload_config(state: &Arc<DaemonState>) -> Response {
    match load_config_file(std::path::Path::new(CONFIG_FILE_PATH)) {
        Ok(file_config) => {
            let mut cfg = state.config.write().unwrap();
            cfg.auto_cleanup = file_config.auto_cleanup.unwrap_or(DEFAULT_AUTO_CLEANUP);
            cfg.auto_cleanup_keep = file_config
                .auto_cleanup_keep
                .clone()
                .unwrap_or_else(default_auto_cleanup_keep);
            cfg.auto_cleanup_interval_secs = file_config
                .auto_cleanup_interval_secs
                .unwrap_or(DEFAULT_AUTO_CLEANUP_INTERVAL_SECS);
            cfg.health_check_interval_secs = file_config
                .health_check_interval_secs
                .unwrap_or(DEFAULT_HEALTH_CHECK_INTERVAL_SECS);
            cfg.backend_type = file_config.backend.r#type.clone();

            // img_* fields are bootstrap-only; compare against the new file values and
            // warn if they changed. We do NOT mutate cfg.img_size / cfg.img_max_percent
            // here because bootstrap has already consumed them; the loop image size is
            // fixed until the next restart (at which point bootstrap will reconcile it).
            //
            // Both fields participate in the target formula:
            //   target = min(img_size * GiB, total * img_max_percent / 100)
            // so a change to either one will affect the reconciled image size after
            // `systemctl restart ws-ckpt`.
            let btrfs_loop = file_config.backend.btrfs_loop.as_ref();
            let new_img_size = btrfs_loop
                .and_then(|b| b.img_size)
                .unwrap_or(DEFAULT_IMG_SIZE_GB);
            let new_img_max_percent = btrfs_loop
                .and_then(|b| b.img_max_percent)
                .unwrap_or(DEFAULT_IMG_MAX_PERCENT * 100.0);
            if new_img_size != cfg.img_size
                || (new_img_max_percent - cfg.img_max_percent).abs() > f64::EPSILON
            {
                tracing::warn!(
                    "BtrfsLoop image sizing changed in config file (img_size: {} -> {} GB, \
                     img_max_percent: {} -> {}). These are bootstrap-only settings; \
                     restart ws-ckpt daemon to apply the new target \
                     min(img_size GB, total * img_max_percent%).",
                    cfg.img_size,
                    new_img_size,
                    cfg.img_max_percent,
                    new_img_max_percent,
                );
            }

            tracing::info!(
                "Config reloaded: auto_cleanup={}, keep={}, cleanup_interval={}s, health_interval={}s \
                 (img fields are bootstrap-only; restart required to apply)",
                cfg.auto_cleanup,
                cfg.auto_cleanup_keep,
                cfg.auto_cleanup_interval_secs,
                cfg.health_check_interval_secs,
            );
            // Drop the write lock before notifying so woken loops can read the
            // fresh config without contending on the lock.
            drop(cfg);
            // Push notification to scheduler loops: break their current sleep
            // (or wake them from a disabled state) so the new config takes
            // effect immediately instead of on the next polling boundary.
            state.config_notify.notify_waiters();
            Response::ReloadConfigOk
        }
        Err(e) => Response::Error {
            code: ErrorCode::InternalError,
            message: format!("Failed to reload config: {}", e),
        },
    }
}

/// Aggregate advisory metrics. Never triggers bootstrap; backend-query failure
/// yields zero bytes so the CLI silently skips the fs warning.
async fn handle_health_advisory(state: &Arc<DaemonState>) -> Response {
    let over_limit_workspace_count: u32 = state
        .get_all_workspace_info()
        .iter()
        .filter(|w| w.snapshot_count > ADVISORY_SNAPSHOT_LIMIT)
        .count() as u32;
    let (fs_total_bytes, fs_used_bytes) = match state.backend.get_usage().await {
        Ok((total, used)) => (total, used),
        Err(_) => (0, 0),
    };
    Response::HealthAdvisoryOk {
        over_limit_workspace_count,
        fs_total_bytes,
        fs_used_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use ws_ckpt_common::backend::StorageBackend;
    use ws_ckpt_common::{CleanupRetention, DaemonConfig, ErrorCode, Request, Response};

    fn test_backend() -> Arc<dyn StorageBackend> {
        // Use BtrfsBase to avoid triggering lazy bootstrap in dispatch tests
        Arc::new(crate::backends::btrfs_base::BtrfsBaseBackend::new(
            PathBuf::from("/tmp/test-mount"),
            crate::backends::btrfs_base::BtrfsBaseScenario::InPlace,
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
            img_path: "/var/lib/ws-ckpt/btrfs-data.img".to_string(),
            img_size: 30,
            img_max_percent: 40.0,
            min_free_bytes: 512 * 1024 * 1024,
            min_free_percent: 1.0,
        }
    }

    // The dispatcher routes all Request variants to handlers. For handlers that
    // call tokio::fs::canonicalize, we can use tempdir to create real paths and
    // test the routing without requiring btrfs.

    #[tokio::test]
    async fn dispatch_init_nonexistent_path_returns_invalid_path() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            PathBuf::from("/tmp/test-state"),
        ));
        let req = Request::Init {
            workspace: "/nonexistent/path/12345".to_string(),
        };
        let resp = dispatch(&state, req).await;
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::InvalidPath),
            _ => panic!("expected InvalidPath error from Init"),
        }
    }

    #[tokio::test]
    async fn dispatch_checkpoint_nonexistent_auto_inits_and_returns_invalid_path() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            PathBuf::from("/tmp/test-state"),
        ));
        let req = Request::Checkpoint {
            workspace: "/nonexistent/path/12345".to_string(),
            id: "snap-1".to_string(),
            message: None,
            metadata: None,
            pin: false,
        };
        let resp = dispatch(&state, req).await;
        // Auto-init triggers, but path doesn't exist → InvalidPath
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::InvalidPath),
            _ => panic!("expected InvalidPath error from Checkpoint auto-init"),
        }
    }

    #[tokio::test]
    async fn dispatch_rollback_nonexistent_returns_workspace_not_found() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            PathBuf::from("/tmp/test-state"),
        ));
        let req = Request::Rollback {
            workspace: "/nonexistent/path/12345".to_string(),
            to: "msg1-step0".to_string(),
        };
        let resp = dispatch(&state, req).await;
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::WorkspaceNotFound),
            _ => panic!("expected WorkspaceNotFound error from Rollback"),
        }
    }

    #[tokio::test]
    async fn dispatch_delete_nonexistent_returns_workspace_not_found() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            PathBuf::from("/tmp/test-state"),
        ));
        let req = Request::Delete {
            workspace: Some("/nonexistent/path/12345".to_string()),
            snapshot: "nonexistent".to_string(),
            force: true,
        };
        let resp = dispatch(&state, req).await;
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::WorkspaceNotFound),
            _ => panic!("expected WorkspaceNotFound error from Delete"),
        }
    }

    #[tokio::test]
    async fn dispatch_delete_snapshot_not_found_returns_error() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            PathBuf::from("/tmp/test-state"),
        ));
        let req = Request::Delete {
            workspace: Some("/nonexistent/ws".to_string()),
            snapshot: "nosuchsnap".to_string(),
            force: false,
        };
        let resp = dispatch(&state, req).await;
        match resp {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::WorkspaceNotFound);
                assert!(message.contains("/nonexistent/ws"));
            }
            _ => panic!("expected WorkspaceNotFound from Delete"),
        }
    }

    #[tokio::test]
    async fn dispatch_checkpoint_unregistered_real_path_auto_inits() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            PathBuf::from("/tmp/test-state"),
        ));
        let tmpdir = tempfile::tempdir().unwrap();
        let req = Request::Checkpoint {
            workspace: tmpdir.path().to_string_lossy().to_string(),
            id: "snap-1".to_string(),
            message: None,
            metadata: None,
            pin: false,
        };
        let resp = dispatch(&state, req).await;
        // Auto-init triggers; since backend cannot actually init in test env,
        // we expect an error (InternalError from backend failure), not WorkspaceNotFound
        if let Response::Error { code, .. } = resp {
            assert!(
                code != ErrorCode::WorkspaceNotFound,
                "should not return WorkspaceNotFound; auto-init should have been attempted"
            );
        }
        // If somehow init succeeded, that's also acceptable
    }

    #[tokio::test]
    async fn dispatch_rollback_unregistered_real_path_returns_workspace_not_found() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            PathBuf::from("/tmp/test-state"),
        ));
        let tmpdir = tempfile::tempdir().unwrap();
        let req = Request::Rollback {
            workspace: tmpdir.path().to_string_lossy().to_string(),
            to: "msg1-step0".to_string(),
        };
        let resp = dispatch(&state, req).await;
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::WorkspaceNotFound),
            _ => panic!("expected WorkspaceNotFound from Rollback on unregistered ws"),
        }
    }

    #[tokio::test]
    async fn dispatch_delete_unregistered_snapshot_returns_not_found() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            PathBuf::from("/tmp/test-state"),
        ));
        let req = Request::Delete {
            workspace: Some("/nonexistent/ws".to_string()),
            snapshot: "abc123".to_string(),
            force: true,
        };
        let resp = dispatch(&state, req).await;
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::WorkspaceNotFound),
            _ => panic!("expected WorkspaceNotFound from Delete on unregistered workspace"),
        }
    }

    // Test that dispatch wraps anyhow errors into InternalError
    // (cannot easily trigger without mocking, so we verify the pattern)
    #[test]
    fn dispatch_error_wrapping_pattern() {
        // Verify that the error wrapping produces correct Response
        let err_resp = Response::Error {
            code: ErrorCode::InternalError,
            message: format!("{:#}", anyhow::anyhow!("test error")),
        };
        match err_resp {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::InternalError);
                assert!(message.contains("test error"));
            }
            _ => panic!("expected Error variant"),
        }
    }

    // ── Phase 2 dispatch tests ──

    #[tokio::test]
    async fn dispatch_list_nonexistent_returns_workspace_not_found() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            PathBuf::from("/tmp/test-state"),
        ));
        let req = Request::List {
            workspace: Some("/nonexistent/path/12345".to_string()),
            format: None,
        };
        let resp = dispatch(&state, req).await;
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::WorkspaceNotFound),
            _ => panic!("expected WorkspaceNotFound error from List"),
        }
    }

    #[tokio::test]
    async fn dispatch_list_unregistered_path_returns_workspace_not_found() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            PathBuf::from("/tmp/test-state"),
        ));
        let tmpdir = tempfile::tempdir().unwrap();
        let req = Request::List {
            workspace: Some(tmpdir.path().to_string_lossy().to_string()),
            format: None,
        };
        let resp = dispatch(&state, req).await;
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::WorkspaceNotFound),
            _ => panic!("expected WorkspaceNotFound from List"),
        }
    }

    #[tokio::test]
    async fn dispatch_diff_nonexistent_returns_workspace_not_found() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            PathBuf::from("/tmp/test-state"),
        ));
        let req = Request::Diff {
            workspace: "/nonexistent/path/12345".to_string(),
            from: "msg1-step0".to_string(),
            to: "msg2-step0".to_string(),
        };
        let resp = dispatch(&state, req).await;
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::WorkspaceNotFound),
            _ => panic!("expected WorkspaceNotFound error from Diff"),
        }
    }

    #[tokio::test]
    async fn dispatch_status_returns_status_ok() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            PathBuf::from("/tmp/test-state"),
        ));
        let req = Request::Status { workspace: None };
        let resp = dispatch(&state, req).await;
        match resp {
            Response::StatusOk { report } => {
                assert!(report.workspaces.is_empty());
            }
            _ => panic!("expected StatusOk, got {:?}", resp),
        }
    }

    #[tokio::test]
    async fn dispatch_status_with_nonexistent_workspace_returns_error() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            PathBuf::from("/tmp/test-state"),
        ));
        let req = Request::Status {
            workspace: Some("/nonexistent/path/12345".to_string()),
        };
        let resp = dispatch(&state, req).await;
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::WorkspaceNotFound),
            _ => panic!("expected WorkspaceNotFound, got {:?}", resp),
        }
    }

    #[tokio::test]
    async fn dispatch_status_with_unregistered_real_path_returns_error() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            PathBuf::from("/tmp/test-state"),
        ));
        let tmpdir = tempfile::tempdir().unwrap();
        let req = Request::Status {
            workspace: Some(tmpdir.path().to_string_lossy().to_string()),
        };
        let resp = dispatch(&state, req).await;
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::WorkspaceNotFound),
            _ => panic!("expected WorkspaceNotFound, got {:?}", resp),
        }
    }

    #[tokio::test]
    async fn dispatch_cleanup_nonexistent_returns_workspace_not_found() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            PathBuf::from("/tmp/test-state"),
        ));
        let req = Request::Cleanup {
            workspace: "/nonexistent/path/12345".to_string(),
            keep: None,
        };
        let resp = dispatch(&state, req).await;
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::WorkspaceNotFound),
            _ => panic!("expected WorkspaceNotFound error from Cleanup"),
        }
    }

    #[tokio::test]
    async fn dispatch_config_returns_config_ok() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            PathBuf::from("/tmp/test-state"),
        ));
        let req = Request::Config;
        let resp = dispatch(&state, req).await;
        match resp {
            Response::ConfigOk { config } => {
                assert_eq!(config.mount_path, "/tmp/test-mount");
                assert_eq!(config.auto_cleanup_keep, CleanupRetention::Count(20));
                assert_eq!(config.auto_cleanup_interval_secs, 86_400);
            }
            _ => panic!("expected ConfigOk, got {:?}", resp),
        }
    }

    #[tokio::test]
    async fn dispatch_reload_config_returns_reload_config_ok() {
        // ReloadConfig reads /etc/ws-ckpt/config.toml; if missing, uses defaults
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            PathBuf::from("/tmp/test-state"),
        ));
        let req = Request::ReloadConfig;
        let resp = dispatch(&state, req).await;
        assert!(matches!(resp, Response::ReloadConfigOk));
    }

    #[tokio::test]
    async fn dispatch_recover_nonexistent_returns_workspace_not_found() {
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            PathBuf::from("/tmp/test-state"),
        ));
        let req = Request::Recover {
            workspace: "/nonexistent/path/12345".to_string(),
        };
        let resp = dispatch(&state, req).await;
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::WorkspaceNotFound),
            _ => panic!("expected WorkspaceNotFound error from Recover"),
        }
    }

    #[tokio::test]
    async fn dispatch_health_advisory_returns_health_advisory_ok() {
        // Empty workspace set -> counter must be 0; fs bytes vary by OS.
        let state = Arc::new(DaemonState::new(
            test_config(),
            test_backend(),
            PathBuf::from("/tmp/test-state"),
        ));
        let req = Request::HealthAdvisory;
        let resp = dispatch(&state, req).await;
        match resp {
            Response::HealthAdvisoryOk {
                over_limit_workspace_count,
                ..
            } => {
                assert_eq!(over_limit_workspace_count, 0);
            }
            _ => panic!("expected HealthAdvisoryOk, got {:?}", resp),
        }
    }
}
