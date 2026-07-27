//! Background scheduler: auto-cleanup, health check, and orphan recovery.

use std::path::Path;
use std::sync::Arc;

use tokio::time::Duration;
use tracing::{debug, error, info, warn};

use crate::backends::btrfs_common;
use crate::snapshot_mgr::{delete_snapshots_locked, ensure_index_dir, persist_index_after_cleanup};
use crate::state::DaemonState;
use ws_ckpt_common::{CleanupRetention, EffectivePolicy};

/// Start background scheduler tasks: orphan cleanup on boot, periodic auto-cleanup,
/// and periodic health checks.
///
/// Config hot-reload is **push-based**: the dispatcher calls
/// `state.config_notify.notify_waiters()` after updating `state.config`, and
/// every periodic loop uses `tokio::select!` to react. This replaces the old
/// polling design — loops never wake up "just to check", and a disabled task
/// (`auto_cleanup = false` or `*_interval_secs == 0`) blocks on the notify
/// at zero CPU cost until a reload re-enables it.
pub fn start_scheduler(state: Arc<DaemonState>) {
    // Startup orphan cleanup
    let mount_path = state.mount_path.clone();
    tokio::spawn(async move {
        if let Err(e) = cleanup_orphans(&mount_path).await {
            error!("Failed to cleanup orphans: {}", e);
        }
    });

    // Periodic auto-cleanup: reacts to `ReloadConfig` via `config_notify`.
    let state_clone = state.clone();
    tokio::spawn(async move {
        auto_cleanup_loop(state_clone).await;
    });

    // Periodic health check: same notify-driven pattern.
    let state_clone2 = state.clone();
    tokio::spawn(async move {
        health_check_loop(state_clone2).await;
    });

    info!("Background scheduler started");
}

/// Auto-cleanup loop: each iteration re-reads `auto_cleanup`,
/// `auto_cleanup_interval_secs`, and `auto_cleanup_keep.is_disabled()`.
/// Disabled parks on `config_notify`; active races `sleep` vs `config_notify`
/// for immediate reload.
///
/// `notify_waiters()` does **not** store a permit, so a notify that fires
/// before a waiter has registered is lost. To close the window between the
/// config read and registration, we build the `Notified` future and
/// `enable()` it (registers immediately) **before** reading config. Any
/// `notify_waiters()` issued afterwards is then captured by this waiter.
async fn auto_cleanup_loop(state: Arc<DaemonState>) {
    loop {
        let notified = state.config_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        let interval = state.config_snapshot().auto_cleanup_interval_secs;
        // Park unless some ws has an effective (merged local-or-global) policy
        // that would do work this tick; avoids waking every interval just to
        // skip all workspaces.
        let park = interval == 0 || !state.any_ws_has_effective_cleanup().await;
        if park {
            notified.await;
            continue;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(interval)) => {
                auto_cleanup(&state).await;
            }
            _ = notified.as_mut() => {
                // Config changed mid-sleep: skip this cleanup pass and re-read.
            }
        }
    }
}

/// Health-check loop. Same push-based pattern as `auto_cleanup_loop`, keyed
/// off `health_check_interval_secs`. See that function's comment for why
/// `enable()` is called before the config read.
async fn health_check_loop(state: Arc<DaemonState>) {
    loop {
        let notified = state.config_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        let interval = state.config_snapshot().health_check_interval_secs;
        if interval == 0 {
            notified.await;
            continue;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(interval)) => {
                health_check(&state).await;
            }
            _ = notified.as_mut() => {}
        }
    }
}

/// Orphan recovery: clean up `.rollback-tmp` residual directories.
///
/// Scans the mount path for directories ending with `.rollback-tmp`
/// and removes them. Returns the list of cleaned-up paths.
pub async fn cleanup_orphans(mount_path: &Path) -> Result<Vec<String>, anyhow::Error> {
    let mut cleaned = Vec::new();

    let read_dir = match std::fs::read_dir(mount_path) {
        Ok(rd) => rd,
        Err(e) => {
            warn!("Cannot read mount path for orphan cleanup: {}", e);
            return Ok(cleaned);
        }
    };

    for entry in read_dir {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let name = entry.file_name();
        let name_str = name.to_string_lossy().to_string();
        let path = entry.path();

        if name_str.ends_with(".rollback-tmp") {
            info!("Cleaning up orphan directory: {:?}", path);

            // Try btrfs subvolume delete first, fall back to remove_dir_all
            match btrfs_common::delete_subvolume(&path).await {
                Ok(()) => {
                    info!("Deleted orphan subvolume: {:?}", path);
                }
                Err(_) => {
                    // Fallback: try regular directory removal
                    if let Err(e) = tokio::fs::remove_dir_all(&path).await {
                        warn!("Failed to remove orphan directory {:?}: {}", path, e);
                        continue;
                    }
                    info!("Removed orphan directory: {:?}", path);
                }
            }

            cleaned.push(path.to_string_lossy().to_string());
        }
    }

    if !cleaned.is_empty() {
        info!("Orphan cleanup complete: {} items removed", cleaned.len());
    }

    Ok(cleaned)
}

/// Auto-cleanup: purge non-pinned snapshots per `CleanupRetention` (pinned always kept).
/// - `Count(n)`: keep n newest per workspace.
/// - `Age { secs, .. }`: delete if older than `secs` (strict, no count floor).
///
/// Locking: P1 read-lock probe; P2a read-lock plan (P2b rechecks per-snap);
/// P2b unlocked deletes with short per-snap write to drop index entry.
async fn auto_cleanup(state: &DaemonState) {
    info!("Running auto-cleanup pass (per-ws effective retention)...");
    let all_ws = state.all_workspaces();
    let now = chrono::Utc::now();

    for ws_arc in &all_ws {
        // Phase 1: cheap read-only probe — exact list is recomputed in Phase 2.
        let has_potential_work = {
            let ws = ws_arc.read().await;
            let cfg = state.config_snapshot();
            let eff: EffectivePolicy = ws.policy.effective_for(&cfg);
            if eff.is_disabled() {
                false
            } else {
                // Any unpinned snapshot exists; Phase 2 decides the final set.
                // `any` short-circuits on large indexes.
                ws.index.snapshots.values().any(|m| !m.pinned)
            }
        };
        if !has_potential_work {
            continue;
        }

        // P2a: plan under read lock (pure compute; P2b rechecks pin per snap).
        let (ws_id, to_remove, index_dir) = {
            let ws = ws_arc.read().await;
            let ws_id = ws.ws_id.clone();

            let cfg = state.config_snapshot();
            let eff: EffectivePolicy = ws.policy.effective_for(&cfg);
            if eff.is_disabled() {
                // Policy flipped to disabled while awaiting the read lock — skip.
                continue;
            }
            let retention = eff.auto_cleanup_keep.clone();

            let mut unpinned: Vec<(String, chrono::DateTime<chrono::Utc>)> = ws
                .index
                .snapshots
                .iter()
                .filter(|(_, meta)| !meta.pinned)
                .map(|(id, meta)| (id.clone(), meta.created_at))
                .collect();
            unpinned.sort_by_key(|(_, ts)| *ts);

            let to_remove: Vec<String> = match &retention {
                CleanupRetention::Count(n) => {
                    let keep = *n as usize;
                    if unpinned.len() <= keep {
                        Vec::new()
                    } else {
                        unpinned[..unpinned.len() - keep]
                            .iter()
                            .map(|(id, _)| id.clone())
                            .collect()
                    }
                }
                CleanupRetention::Age { secs, .. } => {
                    let cutoff = now - chrono::Duration::seconds(*secs as i64);
                    unpinned
                        .iter()
                        .filter(|(_, ts)| *ts < cutoff)
                        .map(|(id, _)| id.clone())
                        .collect()
                }
            };

            if to_remove.is_empty() {
                continue;
            }

            let index_dir = state.index_dir(&ws_id);
            (ws_id, to_remove, index_dir)
        };

        if !ensure_index_dir(&index_dir, "auto-cleanup").await {
            continue;
        }

        // P2b: shared detach-then-delete + persist. Background loop swallows
        // partial failures (already warn!'d inside the helper) — we never
        // bail the whole loop on a single snap, the next tick retries.
        let outcome =
            delete_snapshots_locked(state, ws_arc, &ws_id, &to_remove, "auto-cleanup").await;
        if !outcome.removed.is_empty() {
            persist_index_after_cleanup(state, ws_arc, &index_dir, "auto-cleanup").await;
            info!(
                "auto-cleanup: removed {} snapshots from {}",
                outcome.removed.len(),
                ws_id
            );
        }
    }
}

/// Health check: verify filesystem usage.
///
/// Skipped when no workspace is registered. WARN on usage above threshold;
/// ERROR when get_usage fails (umount, fs crash, etc.) so upstream monitors can catch it.
async fn health_check(state: &DaemonState) {
    if state.all_workspaces().is_empty() {
        debug!("Health check skipped: no workspace registered");
        return;
    }

    match state.backend.get_usage().await {
        Ok((total, used)) => {
            if total > 0 {
                let usage_pct = (used as f64 / total as f64) * 100.0;
                const FS_WARN_THRESHOLD_PERCENT: f64 = 90.0;
                if usage_pct > FS_WARN_THRESHOLD_PERCENT {
                    warn!(
                        "Filesystem usage critical: {:.1}% ({} / {} bytes)",
                        usage_pct, used, total
                    );
                } else {
                    info!("Health check OK: filesystem usage {:.1}%", usage_pct);
                }
            }
        }
        Err(e) => {
            // `{:#}` prints the full anyhow cause chain (e.g. outer
            // `with_context` + inner `bail!`), not just the outermost message.
            error!(
                "Health check failed on backend {}: {:#}",
                state.backend.backend_type(),
                e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cleanup_orphans_removes_rollback_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let orphan1 = dir.path().join("ws-abc123.rollback-tmp");
        let normal = dir.path().join("ws-normal");

        std::fs::create_dir(&orphan1).unwrap();
        std::fs::create_dir(&normal).unwrap();

        let cleaned = cleanup_orphans(dir.path()).await.unwrap();

        assert_eq!(cleaned.len(), 1);
        assert!(!orphan1.exists(), "rollback-tmp should be removed");
        assert!(normal.exists(), "normal directory should remain");
    }

    #[tokio::test]
    async fn cleanup_orphans_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let cleaned = cleanup_orphans(dir.path()).await.unwrap();
        assert!(cleaned.is_empty());
    }

    #[tokio::test]
    async fn cleanup_orphans_nonexistent_path() {
        let result = cleanup_orphans(Path::new("/nonexistent/path/12345")).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn cleanup_orphans_only_normal_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("ws-abc")).unwrap();
        std::fs::create_dir(dir.path().join("snapshots")).unwrap();

        let cleaned = cleanup_orphans(dir.path()).await.unwrap();
        assert!(cleaned.is_empty());
    }

    // ── Per-workspace effective policy invariants ──
    // Backend-free: only assert the routing rules `auto_cleanup_loop` relies on.
    use ws_ckpt_common::{CleanupRetention, DaemonConfig, WorkspacePolicy};

    fn cfg(global_on: bool, keep: CleanupRetention) -> DaemonConfig {
        DaemonConfig {
            auto_cleanup: global_on,
            auto_cleanup_keep: keep,
            ..DaemonConfig::default()
        }
    }

    #[test]
    fn per_ws_off_overrides_global_on() {
        let g = cfg(true, CleanupRetention::Count(20));
        let local = WorkspacePolicy {
            auto_cleanup: Some(false),
            auto_cleanup_keep: None,
        };
        // Per-ws says off → effective is_disabled, so scheduler will skip.
        assert!(local.effective_for(&g).is_disabled());
    }

    #[test]
    fn per_ws_on_overrides_global_off() {
        let g = cfg(false, CleanupRetention::Count(20));
        let local = WorkspacePolicy {
            auto_cleanup: Some(true),
            auto_cleanup_keep: None,
        };
        // Per-ws says on, even though global is off → effective should run.
        assert!(!local.effective_for(&g).is_disabled());
    }

    #[test]
    fn per_ws_keep_count_overrides_global_keep() {
        let g = cfg(true, CleanupRetention::Count(20));
        let local = WorkspacePolicy {
            auto_cleanup: None,
            auto_cleanup_keep: Some(CleanupRetention::Count(5)),
        };
        let eff = local.effective_for(&g);
        // auto_cleanup is inherited from global (true), keep is overridden.
        assert!(eff.auto_cleanup);
        assert_eq!(eff.auto_cleanup_keep, CleanupRetention::Count(5));
    }
}
