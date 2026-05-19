use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use async_trait::async_trait;
use nix::unistd::{chown, Gid, Uid};
use tokio::process::Command;
use tracing::{error, info, warn};

use ws_ckpt_common::backend::*;
use ws_ckpt_common::{DaemonConfig, DiffEntry, WorkspaceInfo, SNAPSHOTS_DIR};

use super::btrfs_common;
use btrfs_common::resolve_symlink_path;

/// Deployment scenario for BtrfsBase backend.
#[derive(Debug, Clone, Copy)]
pub enum BtrfsBaseScenario {
    /// Scenario A: workspace already on btrfs partition, mv is O(1) zero-copy
    InPlace,
    /// Scenario B: workspace on non-btrfs disk, needs rsync migration to btrfs data disk
    CrossDisk,
}

pub struct BtrfsBaseBackend {
    /// Data root on the btrfs partition (e.g. <btrfs_mount>/ws-ckpt-data)
    data_root: PathBuf,
    /// Snapshot storage directory: {data_root}/snapshots
    snapshots_dir: PathBuf,
    /// Deployment scenario
    scenario: BtrfsBaseScenario,
}

impl BtrfsBaseBackend {
    pub fn new(btrfs_mount: PathBuf, scenario: BtrfsBaseScenario) -> Self {
        let data_root = btrfs_mount.join("ws-ckpt-data");
        let snapshots_dir = data_root.join(SNAPSHOTS_DIR);
        Self {
            data_root,
            snapshots_dir,
            scenario,
        }
    }

    /// Internal init implementation; caller wraps with cleanup-on-failure.
    async fn do_init_storage(
        &self,
        original_path: &str,
        ws_id: &str,
        subvol_path: &Path,
        snap_dir: &Path,
    ) -> anyhow::Result<()> {
        // 0. Ensure data_root exists
        tokio::fs::create_dir_all(&self.data_root)
            .await
            .context("failed to create data_root directory")?;

        // 1. Create subvolume
        btrfs_common::create_subvolume(subvol_path).await?;

        // 2. Create snapshots dir for this workspace
        tokio::fs::create_dir_all(snap_dir)
            .await
            .context("failed to create snapshots directory")?;

        // 3. Data migration (scenario-dependent)
        match self.scenario {
            BtrfsBaseScenario::InPlace => {
                // Same btrfs partition: move contents via fs::rename (O(1) per entry)
                self.move_contents(original_path, subvol_path).await?;
            }
            BtrfsBaseScenario::CrossDisk => {
                // Cross-disk: rsync migration
                // --copy-unsafe-links: dereference symlinks that point outside the source tree
                // (e.g. symlinks to other ws-* subvolumes inside mount point)
                let src = format!("{}/", original_path); // trailing / is important
                let status = Command::new("rsync")
                    .args([
                        "-a",
                        "--copy-unsafe-links",
                        &src,
                        &subvol_path.to_string_lossy(),
                    ])
                    .status()
                    .await
                    .context("failed to run rsync")?;
                if !status.success() {
                    anyhow::bail!("rsync failed with exit code: {:?}", status.code());
                }
            }
        }

        // 3a. Flush dirty data to disk so subsequent snapshots are instant
        let sync_status = Command::new("btrfs")
            .args(["filesystem", "sync", &subvol_path.to_string_lossy()])
            .status()
            .await
            .context("failed to run btrfs filesystem sync")?;
        if !sync_status.success() {
            warn!("btrfs filesystem sync returned non-zero, falling back to sync()");
            Command::new("sync").status().await.ok();
        }

        // 4. Record original directory permissions before removal
        let orig_meta = tokio::fs::metadata(original_path)
            .await
            .context("failed to read original directory metadata")?;
        let orig_uid = orig_meta.uid();
        let orig_gid = orig_meta.gid();

        // 5. Remove original directory (data is safely in btrfs subvolume now)
        tokio::fs::remove_dir_all(original_path)
            .await
            .context("failed to remove original directory")?;

        // 6. Create symlink: user path -> btrfs subvolume
        if let Some(parent) = Path::new(original_path).parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .context("failed to create parent directory for symlink")?;
        }
        tokio::fs::symlink(subvol_path, original_path)
            .await
            .context("failed to create symlink")?;

        // 6a. Restore ownership on the subvolume root to match original directory
        chown(
            subvol_path,
            Some(Uid::from_raw(orig_uid)),
            Some(Gid::from_raw(orig_gid)),
        )
        .context("failed to restore subvolume ownership")?;

        // 7. Verify symlink
        let link_target = tokio::fs::read_link(original_path)
            .await
            .context("symlink verification failed: cannot read link")?;
        if link_target != subvol_path {
            anyhow::bail!(
                "symlink verification failed: expected {:?}, got {:?}",
                subvol_path,
                link_target
            );
        }

        info!(
            "BtrfsBaseBackend: storage init complete for ws_id={}, subvol={}, scenario={:?}",
            ws_id,
            subvol_path.display(),
            self.scenario,
        );
        Ok(())
    }

    /// Move all contents from original_path into subvol_path using fs::rename.
    /// On same btrfs partition this is O(1) per entry.
    /// Falls back to rsync if any rename fails (e.g. unexpected cross-device).
    async fn move_contents(&self, original_path: &str, subvol_path: &Path) -> anyhow::Result<()> {
        let mut entries = tokio::fs::read_dir(original_path)
            .await
            .context("failed to read original directory")?;

        while let Some(entry) = entries.next_entry().await? {
            let src = entry.path();
            let file_name = entry.file_name();
            let dst = subvol_path.join(&file_name);

            if let Err(e) = tokio::fs::rename(&src, &dst).await {
                // rename failed (possibly cross-device despite InPlace scenario);
                // fallback to rsync for the entire directory
                warn!(
                    "rename {:?} -> {:?} failed: {}, falling back to rsync",
                    src, dst, e
                );
                let rsync_src = format!("{}/", original_path);
                let status = Command::new("rsync")
                    .args([
                        "-a",
                        "--copy-unsafe-links",
                        &rsync_src,
                        &subvol_path.to_string_lossy(),
                    ])
                    .status()
                    .await
                    .context("fallback rsync failed")?;
                if !status.success() {
                    anyhow::bail!("fallback rsync failed with exit code: {:?}", status.code());
                }
                return Ok(());
            }
        }
        Ok(())
    }

    /// Cleanup partially-created storage on init failure.
    async fn cleanup_init_storage(original_path: &str, subvol_path: &Path, snap_dir: &Path) {
        // Remove symlink if it exists
        let _ = tokio::fs::remove_file(original_path).await;

        // Remove snapshots dir
        let _ = tokio::fs::remove_dir_all(snap_dir).await;

        // Delete subvolume (best effort)
        if let Err(e) = btrfs_common::delete_subvolume(subvol_path).await {
            error!("cleanup: failed to delete subvolume: {}", e);
        }
    }
}

#[async_trait]
impl StorageBackend for BtrfsBaseBackend {
    fn backend_type(&self) -> BackendType {
        BackendType::BtrfsBase
    }

    fn data_root(&self) -> &Path {
        &self.data_root
    }

    fn snapshots_root(&self) -> &Path {
        &self.snapshots_dir
    }

    async fn init_workspace(
        &self,
        original_path: &str,
        ws_id: &str,
    ) -> anyhow::Result<WorkspaceInfo> {
        // Resolve symlink to real path to avoid copying the symlink itself
        let resolved = resolve_symlink_path(original_path).await?;
        let resolved_str = resolved.to_string_lossy().to_string();

        let subvol_path = self.data_root.join(ws_id);
        let snap_dir = self.snapshots_dir.join(ws_id);

        if let Err(e) = self
            .do_init_storage(&resolved_str, ws_id, &subvol_path, &snap_dir)
            .await
        {
            error!("init_workspace storage failed, cleaning up: {:#}", e);
            Self::cleanup_init_storage(&resolved_str, &subvol_path, &snap_dir).await;
            return Err(e);
        }

        Ok(WorkspaceInfo {
            ws_id: ws_id.to_string(),
            path: resolved_str,
            snapshot_count: 0,
        })
    }

    async fn create_snapshot(&self, ws_id: &str, snapshot_id: &str) -> anyhow::Result<()> {
        let ws_subvol = self.data_root.join(ws_id);
        let snap_path = self.snapshots_dir.join(ws_id).join(snapshot_id);
        btrfs_common::create_snapshot(&ws_subvol, &snap_path, true).await
    }

    async fn rollback(&self, ws_id: &str, snapshot_id: &str) -> anyhow::Result<PathBuf> {
        let ws_path = self.data_root.join(ws_id);
        let tmp_path = self.data_root.join(format!("{}.rollback-tmp", ws_id));
        let snap_path = self.snapshots_dir.join(ws_id).join(snapshot_id);

        // Verify ws_path is a real subvolume, not a symlink
        let metadata = tokio::fs::symlink_metadata(&ws_path)
            .await
            .context("Failed to read workspace metadata")?;
        if metadata.file_type().is_symlink() {
            bail!("workspace path {:?} is a symlink, expected btrfs subvolume; aborting rollback to prevent symlink chain corruption", ws_path);
        }

        // Warmup snapshot metadata cache
        btrfs_common::warmup_snapshot_metadata(&snap_path).await;

        // Move current workspace aside
        tokio::fs::rename(&ws_path, &tmp_path).await?;

        // Create writable snapshot from target
        match btrfs_common::create_snapshot(&snap_path, &ws_path, false).await {
            Ok(()) => {}
            Err(e) => {
                // Rollback protection: restore original workspace
                error!("rollback snapshot failed, restoring original: {}", e);
                tokio::fs::rename(&tmp_path, &ws_path).await?;
                return Err(e);
            }
        }

        // Clean up old subvolume (non-fatal)
        if let Err(e) = btrfs_common::delete_subvolume(&tmp_path).await {
            warn!("failed to delete old subvolume (non-fatal): {}", e);
        }

        Ok(ws_path)
    }

    async fn delete_snapshot(&self, ws_id: &str, snapshot_id: &str) -> anyhow::Result<()> {
        let snap_path = self.snapshots_dir.join(ws_id).join(snapshot_id);
        btrfs_common::delete_subvolume(&snap_path).await
    }

    async fn recover_workspace(&self, ws_id: &str, original_path: &str) -> anyhow::Result<()> {
        let subvol_path = self.data_root.join(ws_id);
        let snap_base = self.snapshots_dir.join(ws_id);

        // 1. Remove symlink (skip if not a symlink)
        let is_symlink = match tokio::fs::symlink_metadata(original_path).await {
            Ok(meta) => meta.file_type().is_symlink(),
            Err(_) => false,
        };
        if is_symlink {
            tokio::fs::remove_file(original_path)
                .await
                .context("failed to remove symlink")?;
        }

        // 2. Rsync subvolume contents back to original path (restore as normal directory)
        let src = format!("{}/", subvol_path.to_string_lossy()); // trailing / is important

        // Record subvolume root permissions before rsync
        let subvol_meta = tokio::fs::metadata(&subvol_path)
            .await
            .context("failed to read subvolume metadata")?;
        let sv_uid = subvol_meta.uid();
        let sv_gid = subvol_meta.gid();
        let sv_mode = subvol_meta.mode();

        let rsync_status = Command::new("rsync")
            .args(["-a", "--delete", &src, original_path])
            .status()
            .await
            .context("failed to run rsync")?;
        if !rsync_status.success() {
            error!(
                "rsync failed restoring {} -> {}, exit: {:?}",
                src,
                original_path,
                rsync_status.code()
            );
        } else {
            // Restore directory ownership and permissions to match original
            if let Err(e) = chown(
                Path::new(original_path),
                Some(Uid::from_raw(sv_uid)),
                Some(Gid::from_raw(sv_gid)),
            ) {
                warn!("failed to restore ownership on {}: {}", original_path, e);
            }
            if let Err(e) =
                tokio::fs::set_permissions(original_path, std::fs::Permissions::from_mode(sv_mode))
                    .await
            {
                warn!("failed to restore permissions on {}: {}", original_path, e);
            }
            info!("restored workspace contents to {}", original_path);
        }

        // 3. Delete all snapshot subvolumes by scanning the filesystem directory
        if let Ok(mut entries) = tokio::fs::read_dir(&snap_base).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.is_dir() {
                    if let Err(e) = btrfs_common::delete_subvolume(&path).await {
                        warn!("failed to delete snapshot subvolume {:?}: {:#}", path, e);
                    }
                }
            }
        }

        // 4. Delete workspace subvolume
        if let Err(e) = btrfs_common::delete_subvolume(&subvol_path).await {
            warn!("failed to delete workspace subvolume {}: {:#}", ws_id, e);
        }

        // 5. Remove snapshots/{ws_id} directory
        if let Err(e) = tokio::fs::remove_dir_all(&snap_base).await {
            warn!("failed to remove snapshots dir {:?}: {}", snap_base, e);
        }

        // NOTE: BtrfsBase does NOT need umount, losetup -d, or img deletion
        // (that's the key difference from BtrfsLoop)

        Ok(())
    }

    async fn diff(&self, ws_id: &str, from: &str, to: &str) -> anyhow::Result<Vec<DiffEntry>> {
        let snap_base = self.snapshots_dir.join(ws_id);
        let snap_from = snap_base.join(from);
        let snap_to = snap_base.join(to);
        btrfs_common::diff_between_snapshots(&snap_from, &snap_to).await
    }

    async fn cleanup_snapshots(
        &self,
        ws_id: &str,
        snapshot_ids: &[String],
    ) -> anyhow::Result<Vec<String>> {
        let snap_dir = self.snapshots_dir.join(ws_id);
        let mut removed = Vec::new();
        for snap_id in snapshot_ids {
            let snap_path = snap_dir.join(snap_id);
            match btrfs_common::delete_subvolume(&snap_path).await {
                Ok(()) => {
                    removed.push(snap_id.clone());
                    info!("cleanup: removed snapshot {}", snap_id);
                }
                Err(e) => {
                    warn!("cleanup: failed to delete snapshot {}: {:#}", snap_id, e);
                }
            }
        }
        Ok(removed)
    }

    async fn fork(&self, ws_id: &str, snapshot_id: &str, new_ws_id: &str) -> anyhow::Result<()> {
        let snap_path = self.snapshots_dir.join(ws_id).join(snapshot_id);
        let new_ws_path = self.data_root.join(new_ws_id);
        btrfs_common::create_snapshot(&snap_path, &new_ws_path, false).await
    }

    async fn gc_generations(&self, _ws_id: &str) -> anyhow::Result<GcResult> {
        Ok(GcResult::default())
    }

    async fn check_environment(&self) -> anyhow::Result<EnvironmentStatus> {
        let mut details = Vec::new();
        let mut healthy = true;

        // Check btrfs-progs
        match Command::new("which").arg("btrfs").output().await {
            Ok(output) if output.status.success() => {
                details.push("btrfs-progs: installed".to_string())
            }
            _ => {
                healthy = false;
                details.push("btrfs-progs: NOT installed".to_string());
            }
        }

        // Check root privileges
        if nix::unistd::geteuid().is_root() {
            details.push("privileges: root".to_string());
        } else {
            healthy = false;
            details.push("privileges: NOT root".to_string());
        }

        // Check btrfs partition availability
        if btrfs_common::is_on_btrfs(&self.data_root).await {
            details.push(format!(
                "btrfs partition: {} available",
                self.data_root.display()
            ));
        } else {
            // data_root might not exist yet; check parent
            let parent = self.data_root.parent().unwrap_or(&self.data_root);
            if btrfs_common::is_on_btrfs(parent).await {
                details.push(format!("btrfs partition: {} available", parent.display()));
            } else {
                healthy = false;
                details.push("btrfs partition: NOT available".to_string());
            }
        }

        // Check write permission on data_root (or its parent)
        let check_path = if self.data_root.exists() {
            &self.data_root
        } else {
            self.data_root.parent().unwrap_or(&self.data_root)
        };
        match tokio::fs::metadata(check_path).await {
            Ok(meta) => {
                let mode = meta.mode();
                if mode & 0o200 != 0 {
                    details.push("write permission: ok".to_string());
                } else {
                    healthy = false;
                    details.push("write permission: DENIED".to_string());
                }
            }
            Err(_) => {
                healthy = false;
                details.push("write permission: path not accessible".to_string());
            }
        }

        Ok(EnvironmentStatus {
            backend: BackendType::BtrfsBase,
            healthy,
            details,
        })
    }

    async fn get_usage(&self) -> anyhow::Result<(u64, u64)> {
        btrfs_common::get_filesystem_usage(&self.data_root).await
    }

    /// Ensure data_root and snapshots_dir exist on the already-mounted btrfs partition.
    async fn bootstrap(&self, _config: &DaemonConfig) -> anyhow::Result<()> {
        for dir in [&self.data_root, &self.snapshots_dir] {
            tokio::fs::create_dir_all(dir)
                .await
                .with_context(|| format!("Failed to ensure directory exists: {:?}", dir))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{BtrfsBaseBackend, BtrfsBaseScenario};
    use ws_ckpt_common::backend::StorageBackend;
    use ws_ckpt_common::{CleanupRetention, DaemonConfig};

    fn dummy_config() -> DaemonConfig {
        DaemonConfig {
            mount_path: std::path::PathBuf::from("/tmp/unused"),
            socket_path: std::path::PathBuf::from("/tmp/unused.sock"),
            log_level: "info".to_string(),
            auto_cleanup: false,
            auto_cleanup_keep: CleanupRetention::Count(20),
            auto_cleanup_interval_secs: 86_400,
            health_check_interval_secs: 300,
            backend_type: "btrfs-base".to_string(),
            img_size: 1,
            img_max_percent: 1.0,
            min_free_bytes: 0,
            min_free_percent: 0.0,
        }
    }

    #[tokio::test]
    async fn bootstrap_creates_data_root_and_snapshots_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = BtrfsBaseBackend::new(tmp.path().to_path_buf(), BtrfsBaseScenario::InPlace);
        let data_root = tmp.path().join("ws-ckpt-data");
        let snapshots_dir = data_root.join(ws_ckpt_common::SNAPSHOTS_DIR);

        backend.bootstrap(&dummy_config()).await.unwrap();

        assert!(data_root.is_dir(), "data_root must be created");
        assert!(snapshots_dir.is_dir(), "snapshots_dir must be created");
    }

    #[tokio::test]
    async fn bootstrap_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = BtrfsBaseBackend::new(tmp.path().to_path_buf(), BtrfsBaseScenario::InPlace);

        backend.bootstrap(&dummy_config()).await.unwrap();
        // A second call on existing directories must succeed.
        backend.bootstrap(&dummy_config()).await.unwrap();
    }
}
