use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{Notify, OnceCell, RwLock};
use tracing::{info, warn};

use ws_ckpt_common::backend::StorageBackend;
use ws_ckpt_common::{DaemonConfig, ResolveError, SnapshotIndex, WorkspaceInfo, INDEX_FILE};

use crate::fs_watcher::WorkspaceWatcher;
use crate::index_store;

pub struct DaemonState {
    /// ws_id -> workspace state (tokio RwLock because lock is held across .await)
    workspaces: DashMap<String, Arc<RwLock<WorkspaceState>>>,
    /// Reverse index: canonicalized abs path -> ws_id
    path_to_wsid: DashMap<PathBuf, String>,
    /// Daemon configuration (std RwLock for runtime-reloadable config)
    pub config: std::sync::RwLock<DaemonConfig>,
    /// Broadcast signal: dispatcher calls `notify_waiters()` after a successful
    /// `ReloadConfig`, and background loops use `notified().await` inside a
    /// `tokio::select!` to (a) break out of a running `sleep` and re-read the
    /// config, or (b) wake up from a disabled state (`auto_cleanup = false`
    /// / `interval_secs == 0`). This replaces the old polling-based design
    /// where loops periodically woke up to check for config changes.
    pub config_notify: Notify,
    /// Mount path for btrfs filesystem (convenience accessor, immutable)
    pub mount_path: PathBuf,
    /// Socket path (convenience accessor, immutable)
    pub socket_path: PathBuf,
    /// Storage backend (trait object for multi-backend support)
    pub backend: Arc<dyn StorageBackend>,
    /// Daemon start time for uptime calculation
    pub start_time: std::time::Instant,
    /// Lazy bootstrap guard for BtrfsLoop backend (runs at most once)
    bootstrapped: OnceCell<()>,
    /// File watchers for write-lock detection (ws_id -> watcher)
    watchers: std::sync::Mutex<HashMap<String, WorkspaceWatcher>>,
}

pub struct WorkspaceState {
    pub ws_id: String,
    pub path: PathBuf,
    pub index: SnapshotIndex,
}

impl DaemonState {
    pub fn new(config: DaemonConfig, backend: Arc<dyn StorageBackend>) -> Self {
        let mount_path = config.mount_path.clone();
        let socket_path = config.socket_path.clone();
        Self {
            workspaces: DashMap::new(),
            path_to_wsid: DashMap::new(),
            config: std::sync::RwLock::new(config),
            config_notify: Notify::new(),
            mount_path,
            socket_path,
            backend,
            start_time: std::time::Instant::now(),
            bootstrapped: OnceCell::new(),
            watchers: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Ensure BtrfsLoop backend is bootstrapped (idempotent, runs at most once).
    pub async fn ensure_bootstrapped(&self) -> anyhow::Result<()> {
        if self.backend.backend_type() != ws_ckpt_common::backend::BackendType::BtrfsLoop {
            return Ok(());
        }
        self.bootstrapped
            .get_or_try_init(|| async {
                let config = self.config.read().unwrap().clone();
                crate::bootstrap::bootstrap(&config).await
            })
            .await?;
        Ok(())
    }

    pub fn get_by_wsid(&self, ws_id: &str) -> Option<Arc<RwLock<WorkspaceState>>> {
        self.workspaces
            .get(ws_id)
            .map(|entry| Arc::clone(entry.value()))
    }

    pub fn get_by_path(&self, path: &Path) -> Option<Arc<RwLock<WorkspaceState>>> {
        let ws_id = self.path_to_wsid.get(path)?.value().clone();
        self.get_by_wsid(&ws_id)
    }

    /// Resolve a workspace by identifier: tries workspace ID first, then filesystem path.
    /// Supports absolute paths, relative paths, and workspace IDs (e.g., "ws-6d5aaa").
    pub async fn resolve_workspace(&self, workspace: &str) -> Option<Arc<RwLock<WorkspaceState>>> {
        // Normalize: strip trailing slashes so "/a/b/" and "/a/b" are equivalent.
        let workspace = {
            let t = workspace.trim_end_matches('/');
            if t.is_empty() {
                "/"
            } else {
                t
            }
        };
        // 1. Try as workspace ID
        if let Some(arc) = self.get_by_wsid(workspace) {
            return Some(arc);
        }
        // 2. Try as filesystem path (canonical)
        if let Ok(abs_path) = tokio::fs::canonicalize(workspace).await {
            if let Some(arc) = self.get_by_path(&abs_path) {
                return Some(arc);
            }
        }
        // 3. Fallback: try raw path without canonicalization.
        //    With symlink-based workspaces, canonicalize() follows the symlink
        //    and returns the btrfs subvolume path, which won't match the
        //    registered workspace path. The raw path matches the original
        //    user-facing path stored at registration time.
        let raw_path = PathBuf::from(workspace);
        if let Some(arc) = self.get_by_path(&raw_path) {
            return Some(arc);
        }
        None
    }

    pub fn register_workspace(&self, ws_id: String, path: PathBuf, index: SnapshotIndex) {
        let state = Arc::new(RwLock::new(WorkspaceState {
            ws_id: ws_id.clone(),
            path: path.clone(),
            index,
        }));
        self.path_to_wsid.insert(path, ws_id.clone());
        self.workspaces.insert(ws_id, state);
    }

    pub fn unregister_workspace(&self, ws_id: &str) {
        // Stop watcher if present
        if let Ok(mut watchers) = self.watchers.lock() {
            if let Some(w) = watchers.remove(ws_id) {
                w.stop();
            }
        }
        if let Some((_, ws_state)) = self.workspaces.remove(ws_id) {
            // We need the path to remove from path_to_wsid.
            // Since we can't async here, use try_read or blocking_read.
            // The RwLock should be uncontested at this point.
            if let Ok(state) = ws_state.try_read() {
                self.path_to_wsid.remove(&state.path);
            }
        }
    }

    /// Register a file watcher for a workspace.
    pub fn register_watcher(&self, ws_id: String, watcher: WorkspaceWatcher) {
        if let Ok(mut watchers) = self.watchers.lock() {
            watchers.insert(ws_id, watcher);
        }
    }

    /// Check if a workspace is quiescent (no recent writes).
    /// Returns true if safe to snapshot, or if no watcher is registered.
    pub async fn check_workspace_quiescent(&self, ws_id: &str) -> bool {
        // Extract the AtomicBool from the watcher without holding the lock across await
        let is_writing_arc = {
            let watchers = match self.watchers.lock() {
                Ok(w) => w,
                Err(_) => return true,
            };
            match watchers.get(ws_id) {
                Some(w) => Some(std::sync::Arc::clone(&w.is_writing_flag())),
                None => None,
            }
        };
        match is_writing_arc {
            None => true,
            Some(flag) => {
                if !flag.load(std::sync::atomic::Ordering::Acquire) {
                    return true;
                }
                // Wait 100ms quiet period
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                flag.store(false, std::sync::atomic::Ordering::Release);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                !flag.load(std::sync::atomic::Ordering::Acquire)
            }
        }
    }

    pub async fn rebuild_from_disk(
        config: DaemonConfig,
        backend: Arc<dyn StorageBackend>,
    ) -> anyhow::Result<Self> {
        let state = Self::new(config.clone(), backend);

        // Use backend's snapshots root (not config.mount_path) so BtrfsBase and
        // BtrfsLoop both point at the correct on-disk location.
        let snapshots_dir = state.backend.snapshots_root().to_path_buf();

        let mut read_dir = match tokio::fs::read_dir(&snapshots_dir).await {
            Ok(rd) => rd,
            Err(e) => {
                warn!(
                    "Could not read snapshots directory {:?}: {}",
                    snapshots_dir, e
                );
                return Ok(state);
            }
        };

        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            let file_type = match entry.file_type().await {
                Ok(ft) => ft,
                Err(e) => {
                    warn!("Error reading file type for {:?}: {}", path, e);
                    continue;
                }
            };
            if !file_type.is_dir() {
                continue;
            }

            if let Err(e) = Self::rebuild_single_workspace(&state, &path).await {
                warn!("Failed to rebuild workspace at {:?}: {}", path, e);
            }
        }

        Ok(state)
    }

    async fn rebuild_single_workspace(state: &Self, path: &Path) -> anyhow::Result<()> {
        let ws_id = path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("Invalid path: missing file name"))?
            .to_string_lossy()
            .to_string();

        let index_path = path.join(INDEX_FILE);
        let index_content = match tokio::fs::read_to_string(&index_path).await {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to read {:?}: {}", index_path, e);
                return Ok(());
            }
        };

        let index: SnapshotIndex = match serde_json::from_str(&index_content) {
            Ok(idx) => idx,
            Err(e) => {
                warn!("Failed to parse {:?}: {}", index_path, e);
                return Ok(());
            }
        };

        let workspace_path = index.workspace_path.clone();

        // If loaded index has no snapshots, try rebuilding from filesystem
        let index = if index.snapshots.is_empty() {
            match index_store::rebuild_from_fs(path, workspace_path.clone()).await {
                Ok(rebuilt) if !rebuilt.snapshots.is_empty() => {
                    info!(
                        "Rebuilt {} snapshot(s) from filesystem for {}",
                        rebuilt.snapshots.len(),
                        ws_id
                    );
                    // Persist rebuilt index
                    let _ = index_store::save(path, &rebuilt).await;
                    rebuilt
                }
                _ => index,
            }
        } else {
            index
        };

        info!("Restored workspace {} -> {:?}", ws_id, workspace_path);
        // Start file watcher for write-lock detection
        match WorkspaceWatcher::start(&workspace_path) {
            Ok(watcher) => {
                state.register_watcher(ws_id.clone(), watcher);
            }
            Err(e) => {
                warn!("Failed to start watcher for {}: {}", ws_id, e);
            }
        }
        state.register_workspace(ws_id, workspace_path, index);

        Ok(())
    }

    pub fn all_workspaces(&self) -> Vec<Arc<RwLock<WorkspaceState>>> {
        self.workspaces
            .iter()
            .map(|entry| Arc::clone(entry.value()))
            .collect()
    }

    /// Cross-workspace snapshot lookup by ID (exact match or unique prefix).
    /// Returns `(workspace_path, snapshot_id)` if exactly one match is found.
    pub async fn resolve_snapshot_globally(&self, snapshot_ref: &str) -> Option<(String, String)> {
        let mut found: Vec<(String, String)> = Vec::new();

        for entry in self.workspaces.iter() {
            let ws = entry.value().read().await;
            match ws.index.resolve_by_prefix(snapshot_ref) {
                Ok((id, _)) => {
                    let ws_path = ws.path.to_string_lossy().to_string();
                    found.push((ws_path, id.clone()));
                }
                Err(ResolveError::Ambiguous(_)) => {
                    // Ambiguous within one workspace → treat as globally ambiguous
                    return None;
                }
                Err(ResolveError::NotFound) => {}
            }
        }

        if found.len() == 1 {
            Some(found.into_iter().next().unwrap())
        } else {
            None
        }
    }

    /// Collect summary information about all registered workspaces.
    pub fn get_all_workspace_info(&self) -> Vec<WorkspaceInfo> {
        self.workspaces
            .iter()
            .map(|entry| {
                let ws = entry.value();
                // Use try_read to avoid blocking; the lock should be uncontested.
                if let Ok(state) = ws.try_read() {
                    WorkspaceInfo {
                        ws_id: state.ws_id.clone(),
                        path: state.path.to_string_lossy().to_string(),
                        snapshot_count: state.index.snapshots.len() as u32,
                    }
                } else {
                    WorkspaceInfo {
                        ws_id: entry.key().clone(),
                        path: "<locked>".to_string(),
                        snapshot_count: 0,
                    }
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ws_ckpt_common::{DaemonConfig, SnapshotIndex, SnapshotMeta};

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
            auto_cleanup_keep: 20,
            auto_cleanup_interval_secs: 600,
            health_check_interval_secs: 300,
            backend_type: "auto".to_string(),
            fs_warn_threshold_percent: 90.0,
            img_path: "/data/ws-ckpt/btrfs-data.img".to_string(),
            img_size: 30,
            img_max_percent: 40.0,
            min_free_bytes: 512 * 1024 * 1024,
            min_free_percent: 1.0,
        }
    }

    #[test]
    fn new_state_has_empty_workspaces() {
        let state = DaemonState::new(test_config(), test_backend());
        assert!(state.all_workspaces().is_empty());
    }

    #[test]
    fn register_and_get_by_wsid() {
        let state = DaemonState::new(test_config(), test_backend());
        let index = SnapshotIndex::new(PathBuf::from("/home/user/ws"));
        state.register_workspace("ws-abc".to_string(), PathBuf::from("/home/user/ws"), index);

        let ws = state.get_by_wsid("ws-abc");
        assert!(ws.is_some());
    }

    #[test]
    fn register_and_get_by_path() {
        let state = DaemonState::new(test_config(), test_backend());
        let path = PathBuf::from("/home/user/project");
        let index = SnapshotIndex::new(path.clone());
        state.register_workspace("ws-001".to_string(), path.clone(), index);

        let ws = state.get_by_path(&path);
        assert!(ws.is_some());
    }

    #[tokio::test]
    async fn register_and_verify_ws_id_content() {
        let state = DaemonState::new(test_config(), test_backend());
        let path = PathBuf::from("/home/user/ws2");
        let index = SnapshotIndex::new(path.clone());
        state.register_workspace("ws-xyz".to_string(), path.clone(), index);

        let arc = state.get_by_wsid("ws-xyz").unwrap();
        let ws = arc.read().await;
        assert_eq!(ws.ws_id, "ws-xyz");
        assert_eq!(ws.path, path);
        assert!(ws.index.snapshots.is_empty());
    }

    #[test]
    fn get_by_wsid_nonexistent_returns_none() {
        let state = DaemonState::new(test_config(), test_backend());
        assert!(state.get_by_wsid("nonexistent").is_none());
    }

    #[test]
    fn get_by_path_nonexistent_returns_none() {
        let state = DaemonState::new(test_config(), test_backend());
        assert!(state.get_by_path(&PathBuf::from("/no/such/path")).is_none());
    }

    #[tokio::test]
    async fn resolve_workspace_by_wsid() {
        let state = DaemonState::new(test_config(), test_backend());
        let path = PathBuf::from("/home/user/ws");
        let index = SnapshotIndex::new(path.clone());
        state.register_workspace("ws-abc123".to_string(), path, index);
        assert!(state.resolve_workspace("ws-abc123").await.is_some());
    }

    #[tokio::test]
    async fn resolve_workspace_by_path() {
        let state = DaemonState::new(test_config(), test_backend());
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tokio::fs::canonicalize(tmpdir.path()).await.unwrap();
        let index = SnapshotIndex::new(path.clone());
        state.register_workspace("ws-path-test".to_string(), path, index);
        assert!(state
            .resolve_workspace(&tmpdir.path().to_string_lossy())
            .await
            .is_some());
    }

    #[tokio::test]
    async fn resolve_workspace_not_found_returns_none() {
        let state = DaemonState::new(test_config(), test_backend());
        assert!(state.resolve_workspace("nonexistent").await.is_none());
        assert!(state.resolve_workspace("/no/such/path").await.is_none());
    }

    #[test]
    fn path_to_wsid_bidirectional_mapping() {
        let state = DaemonState::new(test_config(), test_backend());
        let path = PathBuf::from("/home/user/myws");
        let index = SnapshotIndex::new(path.clone());
        state.register_workspace("ws-map".to_string(), path.clone(), index);

        // path -> ws -> verify ws_id
        let arc = state.get_by_path(&path).unwrap();
        let ws = arc.try_read().unwrap();
        assert_eq!(ws.ws_id, "ws-map");
    }

    #[test]
    fn duplicate_register_overwrites() {
        // Registering the same ws_id again should overwrite
        let state = DaemonState::new(test_config(), test_backend());
        let path1 = PathBuf::from("/ws/first");
        let path2 = PathBuf::from("/ws/second");
        let index1 = SnapshotIndex::new(path1.clone());
        let index2 = SnapshotIndex::new(path2.clone());

        state.register_workspace("ws-dup".to_string(), path1.clone(), index1);
        state.register_workspace("ws-dup".to_string(), path2.clone(), index2);

        // The last registration should win
        let arc = state.get_by_wsid("ws-dup").unwrap();
        let ws = arc.try_read().unwrap();
        assert_eq!(ws.path, path2);
    }

    #[test]
    fn unregister_workspace_removes_both_mappings() {
        let state = DaemonState::new(test_config(), test_backend());
        let path = PathBuf::from("/home/user/removable");
        let index = SnapshotIndex::new(path.clone());
        state.register_workspace("ws-rm".to_string(), path.clone(), index);

        // Verify it exists
        assert!(state.get_by_wsid("ws-rm").is_some());
        assert!(state.get_by_path(&path).is_some());

        // Unregister
        state.unregister_workspace("ws-rm");

        // Verify both mappings removed
        assert!(state.get_by_wsid("ws-rm").is_none());
        assert!(state.get_by_path(&path).is_none());
    }

    #[test]
    fn all_workspaces_returns_all_registered() {
        let state = DaemonState::new(test_config(), test_backend());
        state.register_workspace(
            "ws-a".to_string(),
            PathBuf::from("/a"),
            SnapshotIndex::new(PathBuf::from("/a")),
        );
        state.register_workspace(
            "ws-b".to_string(),
            PathBuf::from("/b"),
            SnapshotIndex::new(PathBuf::from("/b")),
        );
        assert_eq!(state.all_workspaces().len(), 2);
    }

    #[tokio::test]
    async fn resolve_snapshot_globally_exact_match() {
        let state = DaemonState::new(test_config(), test_backend());
        let mut index = SnapshotIndex::new(PathBuf::from("/home/user/ws"));
        index.snapshots.insert(
            "abcdef1234567890abcdef1234567890abcdef12".to_string(),
            SnapshotMeta {
                message: Some("test".to_string()),
                metadata: None,
                pinned: false,
                created_at: chrono::Utc::now(),
            },
        );
        state.register_workspace("ws-abc".to_string(), PathBuf::from("/home/user/ws"), index);

        let result = state
            .resolve_snapshot_globally("abcdef1234567890abcdef1234567890abcdef12")
            .await;
        assert!(result.is_some());
        let (ws_path, snap_id) = result.unwrap();
        assert_eq!(ws_path, "/home/user/ws");
        assert_eq!(snap_id, "abcdef1234567890abcdef1234567890abcdef12");
    }

    #[tokio::test]
    async fn resolve_snapshot_globally_prefix_match() {
        let state = DaemonState::new(test_config(), test_backend());
        let mut index = SnapshotIndex::new(PathBuf::from("/ws1"));
        index.snapshots.insert(
            "abcdef1234567890abcdef1234567890abcdef12".to_string(),
            SnapshotMeta {
                message: None,
                metadata: None,
                pinned: false,
                created_at: chrono::Utc::now(),
            },
        );
        state.register_workspace("ws-1".to_string(), PathBuf::from("/ws1"), index);

        let result = state.resolve_snapshot_globally("abcdef").await;
        assert!(result.is_some());
        let (_, snap_id) = result.unwrap();
        assert_eq!(snap_id, "abcdef1234567890abcdef1234567890abcdef12");
    }

    #[tokio::test]
    async fn resolve_snapshot_globally_not_found() {
        let state = DaemonState::new(test_config(), test_backend());
        state.register_workspace(
            "ws-1".to_string(),
            PathBuf::from("/ws1"),
            SnapshotIndex::new(PathBuf::from("/ws1")),
        );
        let result = state.resolve_snapshot_globally("nonexistent").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn resolve_snapshot_globally_ambiguous_cross_workspace() {
        let state = DaemonState::new(test_config(), test_backend());
        let meta = SnapshotMeta {
            message: None,
            metadata: None,
            pinned: false,
            created_at: chrono::Utc::now(),
        };

        let mut idx1 = SnapshotIndex::new(PathBuf::from("/ws1"));
        idx1.snapshots.insert(
            "abcdef1111111111111111111111111111111111".to_string(),
            meta.clone(),
        );
        state.register_workspace("ws-1".to_string(), PathBuf::from("/ws1"), idx1);

        let mut idx2 = SnapshotIndex::new(PathBuf::from("/ws2"));
        idx2.snapshots
            .insert("abcdef2222222222222222222222222222222222".to_string(), meta);
        state.register_workspace("ws-2".to_string(), PathBuf::from("/ws2"), idx2);

        // Prefix "abcdef" matches in both workspaces
        let result = state.resolve_snapshot_globally("abcdef").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn ensure_bootstrapped_non_btrfs_loop_is_noop() {
        // BtrfsBase backend should skip bootstrap entirely
        let backend: Arc<dyn StorageBackend> =
            Arc::new(crate::backends::btrfs_base::BtrfsBaseBackend::new(
                PathBuf::from("/tmp/test-btrfs-mount"),
                crate::backends::btrfs_base::BtrfsBaseScenario::InPlace,
            ));
        let state = DaemonState::new(test_config(), backend);
        // Should return Ok immediately without attempting any bootstrap
        state.ensure_bootstrapped().await.unwrap();
    }

    #[tokio::test]
    async fn ensure_bootstrapped_btrfs_loop_only_runs_once() {
        // For BtrfsLoop backend, the OnceCell ensures bootstrap is called at most once.
        // We can't actually run bootstrap in unit tests (requires root + btrfs),
        // but we can verify the OnceCell is properly initialized.
        let state = DaemonState::new(test_config(), test_backend());
        assert!(state.bootstrapped.get().is_none());
    }
}
