use anyhow::{bail, Context};
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{info, warn};

use crate::state::DaemonState;
use ws_ckpt_common::persist::{BackendPaths, LoopImgState};
use ws_ckpt_common::{DaemonConfig, SNAPSHOTS_DIR};

/// Bootstrap result, carries backend path info for back-filling state.json
pub struct BootstrapResult {
    pub paths: BackendPaths,
}

pub async fn bootstrap(config: &DaemonConfig) -> anyhow::Result<BootstrapResult> {
    // Derive image directory from configured image path. We deliberately
    // do NOT fall back to a hard-coded path on None/empty parent: a bare
    // filename in `img_path` is a configuration bug and silently writing
    // to some default would either hide the misconfiguration or attempt
    // to create files under an unwritable directory.
    let img_path = &config.img_path;
    let img_dir = derive_img_dir(img_path)
        .context("Failed to derive image directory from config.img_path")?;

    // 1. Ensure image directory exists
    tokio::fs::create_dir_all(&img_dir)
        .await
        .context("Failed to create ws-ckpt data directory")?;
    info!("Ensured image directory exists: {}", img_dir);

    // 2. Check if btrfs image file exists; create if not.
    //    We remember whether the image pre-existed so that we only run the size
    //    reconciliation step (step 5) on a real pre-existing image. A newly
    //    created image already has the desired size from the calculation below.
    let img_existed_before_bootstrap = tokio::fs::metadata(img_path).await.is_ok();
    if img_existed_before_bootstrap {
        info!("Btrfs image already exists: {}", img_path);
    } else {
        info!("Btrfs image not found, creating...");

        // Query host partition to compute the target image size.
        // `-P` forces POSIX single-line output so long filesystem names
        // (e.g. /dev/mapper/vg-long_name) don't wrap onto a second line
        // and break column-index parsing in parse_df_{total,available}.
        let df_output = run_command("df", &["-P", "-B1", &img_dir])
            .await
            .context("Failed to get partition info")?;
        let total = parse_df_total(&df_output).context("Failed to parse df total")?;
        let avail = parse_df_available(&df_output).context("Failed to parse df output")?;

        // Unified target: the smaller of the absolute cap (img_size GB) and the
        // percentage cap (total * img_max_percent%).
        let target = compute_target_size(config.img_size, config.img_max_percent, total);

        // If the target cannot fit in currently available space, degrade to
        // `avail * img_max_percent%` so the host keeps headroom for other writes.
        const GB: f64 = 1024.0 * 1024.0 * 1024.0;
        let img_size = if target > avail {
            let degraded = (avail as f64 * config.img_max_percent / 100.0) as u64;
            warn!(
                "Target image size {:.1} GB exceeds available {:.1} GB on host partition. \
                 Degrading to {:.1} GB ({}% of available). \
                 Consider freeing disk space or lowering img_size / img_max_percent.",
                target as f64 / GB,
                avail as f64 / GB,
                degraded as f64 / GB,
                config.img_max_percent,
            );
            degraded
        } else {
            target
        };
        info!(
            "Creating sparse image of {} bytes ({:.1} GB), total {:.1} GB, available {:.1} GB",
            img_size,
            img_size as f64 / GB,
            total as f64 / GB,
            avail as f64 / GB,
        );

        // Create sparse file
        run_command_checked("truncate", &["-s", &img_size.to_string(), img_path])
            .await
            .context("Failed to create sparse image file")?;

        // Format as btrfs
        run_command_checked("mkfs.btrfs", &["-f", img_path])
            .await
            .context("Failed to format btrfs image")?;

        info!("Btrfs image created and formatted: {}", img_path);
    }

    // 3. Ensure mount point directory exists
    tokio::fs::create_dir_all(&config.mount_path)
        .await
        .context("Failed to create mount point directory")?;
    info!("Ensured mount point exists: {:?}", config.mount_path);

    // 4. Check if already mounted
    let mount_path_str = config.mount_path.to_string_lossy().to_string();
    if !is_mounted(&mount_path_str).await? {
        info!("Mounting btrfs image at {:?}", config.mount_path);

        // Setup loop device
        let loop_device = run_command("losetup", &["--find", "--show", &config.img_path])
            .await
            .context("Failed to setup loop device")?;
        let loop_device = loop_device.trim().to_string();
        info!("Loop device: {}", loop_device);

        // Mount
        run_command_checked("mount", &[&loop_device, &mount_path_str])
            .await
            .context("Failed to mount btrfs image")?;
        info!("Mounted {} at {}", loop_device, mount_path_str);
    } else {
        info!("Already mounted at {:?}", config.mount_path);
    }

    // 5. Reconcile image size against config.img_size. Only applies to
    //    pre-existing images (a freshly created one already matches the size
    //    chosen by the creation path above). A mismatch triggers either a
    //    `btrfs filesystem resize` grow-in-place or a shrink path that
    //    unmounts + detaches the loop + truncates + remounts.
    if img_existed_before_bootstrap {
        if let Err(e) = reconcile_img_size(config).await {
            warn!(
                "Failed to reconcile btrfs image size: {:#}. Continuing with current size.",
                e
            );
        }
    }

    // 6. Ensure snapshots directory exists
    let snapshots_dir = config.mount_path.join(SNAPSHOTS_DIR);
    tokio::fs::create_dir_all(&snapshots_dir)
        .await
        .context("Failed to create snapshots directory")?;
    info!("Ensured snapshots directory exists: {:?}", snapshots_dir);

    // 6. Orphan cleanup: remove *.rollback-tmp subvolumes
    cleanup_orphans(&config.mount_path).await;

    info!("Bootstrap complete");

    // Build BackendPaths for the return value
    let mount_path = config.mount_path.clone();
    let loop_img = Some(LoopImgState {
        img_path: PathBuf::from(&config.img_path),
        img_size_bytes: tokio::fs::metadata(&config.img_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0),
        last_loop_device: find_loop_device_for(&config.img_path).await.ok(),
    });

    Ok(BootstrapResult {
        paths: BackendPaths::BtrfsLoop {
            mount_path: mount_path.clone(),
            data_root: mount_path.clone(),
            snapshots_root: snapshots_dir,
            loop_img,
        },
    })
}

/// Ensure all registered workspaces have valid symlinks.
/// Called after bootstrap to recover symlinks that may have been lost (e.g. after reboot).
pub async fn ensure_symlinks(state: &DaemonState) {
    let all_ws = state.all_workspaces();
    for arc in all_ws {
        let ws = arc.read().await;
        let expected_subvol_path = state.mount_path.join(&ws.ws_id);
        let ws_path = ws.path.to_string_lossy().to_string();

        // Guard: subvolume must exist, otherwise we'd create a dangling symlink
        if !expected_subvol_path.exists() {
            warn!(
                "subvolume {:?} missing for workspace {}; skipping symlink recovery",
                expected_subvol_path, ws.ws_id
            );
            continue;
        }

        match tokio::fs::read_link(&ws_path).await {
            Ok(target) if target == expected_subvol_path => {
                info!("symlink OK for {}: -> {:?}", ws_path, target);
            }
            Ok(target) => {
                warn!(
                    "symlink {} points to {:?}, expected {:?}; rebuilding",
                    ws_path, target, expected_subvol_path
                );
                let tmp_path = format!("{}.tmp", ws_path);
                if let Err(e) = tokio::fs::symlink(&expected_subvol_path, &tmp_path).await {
                    warn!("failed to create temp symlink for {}: {}", ws_path, e);
                } else if let Err(e) = tokio::fs::rename(&tmp_path, &ws_path).await {
                    warn!(
                        "failed to atomically replace symlink for {}: {}",
                        ws_path, e
                    );
                    let _ = tokio::fs::remove_file(&tmp_path).await;
                } else {
                    info!("rebuilt symlink for {}", ws_path);
                }
            }
            Err(_) => {
                // Symlink doesn't exist or path is not a symlink; rebuild
                warn!("symlink missing or invalid for {}; rebuilding", ws_path);
                let tmp_path = format!("{}.tmp", ws_path);
                if let Err(e) = tokio::fs::symlink(&expected_subvol_path, &tmp_path).await {
                    warn!("failed to create temp symlink for {}: {}", ws_path, e);
                } else if let Err(e) = tokio::fs::rename(&tmp_path, &ws_path).await {
                    warn!(
                        "failed to atomically replace symlink for {}: {}",
                        ws_path, e
                    );
                    let _ = tokio::fs::remove_file(&tmp_path).await;
                } else {
                    info!("created symlink for {}", ws_path);
                }
            }
        }
    }
}

pub async fn is_mounted(mount_path: &str) -> anyhow::Result<bool> {
    let target = Path::new(mount_path);
    let target_norm = target.components().collect::<PathBuf>();

    let file = File::open("/proc/mounts")
        .await
        .context("Failed to open /proc/mounts")?;
    let mut reader = BufReader::new(file).lines();

    while let Some(line) = reader.next_line().await? {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if let Some(mp) = parts.get(1) {
            let mp_path = Path::new(mp);
            if mp_path == target || mp_path.components().collect::<PathBuf>() == target_norm {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

/// Derive the parent directory of the configured image path.
///
/// `img_path` is expected to be an absolute path such as
/// `/var/lib/ws-ckpt/btrfs-data.img`. A bare filename (e.g. `data.img`)
/// yields `Some("")` from `Path::parent`, and `"/"` yields `None`; both
/// cases indicate a malformed config and are rejected up-front instead
/// of being silently rewritten to some hard-coded default that may not
/// even be writable.
fn derive_img_dir(img_path: &str) -> anyhow::Result<String> {
    let parent = Path::new(img_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .with_context(|| {
            format!(
                "Invalid img_path {:?}: must be an absolute path containing a parent directory",
                img_path
            )
        })?;
    Ok(parent.to_string_lossy().to_string())
}

/// Compute the desired image size in bytes.
///
/// The target is the **smaller** of:
///   * the absolute cap: `img_size_gb * GiB`
///   * the percentage cap: `total_bytes * img_max_percent / 100`
///
/// Both caps apply uniformly to first-time creation and to reconcile, so the
/// image never exceeds `img_max_percent%` of the host partition regardless of
/// how `img_size` is configured.
fn compute_target_size(img_size_gb: u64, img_max_percent: f64, total_bytes: u64) -> u64 {
    let by_size = img_size_gb.saturating_mul(1024 * 1024 * 1024);
    let by_percent = (total_bytes as f64 * img_max_percent / 100.0) as u64;
    std::cmp::min(by_size, by_percent)
}

/// Reconcile the on-disk loop image size with the computed target.
///
/// target = min(img_size * GiB, total * img_max_percent / 100)
///
/// * `current == target`  -> no-op.
/// * `current <  target`  -> grow in place, guarded by host avail; if the
///   delta exceeds avail bytes, keep current size and warn.
/// * `current >  target`  -> shrink with unmount/remount cycle; if the
///   mountpoint is still in use by any process, skip shrink and keep
///   the current (larger) image.
///
/// The shrink path is strictly serialized because `truncate` on a mounted
/// loop-backed fs would corrupt the superblock.
async fn reconcile_img_size(config: &DaemonConfig) -> anyhow::Result<()> {
    let img_path = &config.img_path;
    let mount_path_str = config.mount_path.to_string_lossy().to_string();
    let current = tokio::fs::metadata(img_path)
        .await
        .with_context(|| format!("Failed to stat image {}", img_path))?
        .len();

    let img_dir = derive_img_dir(img_path)
        .context("Failed to derive image directory from config.img_path")?;
    // `-P` forces POSIX single-line output (see bootstrap step 2 note).
    let df_output = run_command("df", &["-P", "-B1", &img_dir])
        .await
        .context("Failed to get host partition info for reconcile")?;
    let total = parse_df_total(&df_output).context("Failed to parse df total")?;
    let avail = parse_df_available(&df_output).context("Failed to parse df available")?;

    let target = compute_target_size(config.img_size, config.img_max_percent, total);

    // Tracks whether this bootstrap actually enlarged the backing file +
    // loop in the Less branch. Used at the end to emit a user-visible
    // "grown to" log line only after the final btrfs fs sync succeeds,
    // so the log accurately reflects a fully-completed grow instead of
    // a half-done state.
    let mut grew_to: Option<u64> = None;

    match current.cmp(&target) {
        std::cmp::Ordering::Equal => {
            info!("Btrfs image size already matches target: {} bytes", current);
        }
        std::cmp::Ordering::Less => {
            // Guard: ensure host partition has enough free bytes for the delta,
            // otherwise `truncate` on the sparse file would succeed but later
            // writes would ENOSPC. Keep current size and warn.
            let needed = target - current;
            if needed > avail {
                warn!(
                    "Cannot grow btrfs image: need {} more bytes (target {} bytes) but host \
                     partition has only {} bytes available. Keeping current image size ({} bytes). \
                     Free up disk space or lower img_size / img_max_percent, then restart ws-ckpt.",
                    needed, target, avail, current,
                );
                return Ok(());
            }
            info!("Growing btrfs image: {} bytes -> {} bytes", current, target);
            run_command_checked("truncate", &["-s", &target.to_string(), img_path])
                .await
                .context("Failed to grow sparse image file")?;
            let loop_device = find_loop_device_for(img_path)
                .await
                .context("Failed to locate loop device for image")?;
            run_command_checked("losetup", &["-c", &loop_device])
                .await
                .context("Failed to refresh loop device capacity")?;
            // The actual `btrfs filesystem resize max` runs at the end
            // of this function as a single exit point shared with the
            // Equal branch — see the self-healing step below. This also
            // means a btrfs resize failure leaves a well-defined state
            // (file + loop extended, fs smaller) that the next bootstrap
            // can recover from.
            grew_to = Some(target);
        }
        std::cmp::Ordering::Greater => {
            // Guard: if anything else is still using the mountpoint (a stray
            // shell `cd`-ed in, a monitoring daemon stat-ing it, etc.) the
            // subsequent `umount` would fail mid-sequence and leave the
            // loop + backing file in an inconsistent state. Check first
            // via `fuser -m` and skip shrink entirely if busy — keeping
            // the current (larger) image is always safe.
            if let Some(pids) = check_mount_busy(&mount_path_str).await {
                warn!(
                    "Cannot shrink btrfs image: mount {} is still in use by PIDs [{}]. \
                     Skipping shrink to avoid unmounting a busy filesystem; \
                     keeping current image size ({} bytes). \
                     Stop those processes and restart ws-ckpt to retry.",
                    mount_path_str, pids, current,
                );
                return Ok(());
            }
            warn!(
                "Shrinking btrfs image: {} bytes -> {} bytes. \
                 Shrink will be aborted if btrfs-used bytes exceed the new size.",
                current, target
            );
            // 1. Shrink the btrfs filesystem first; this fails cleanly if the
            //    used bytes don't fit, avoiding data loss.
            run_command_checked(
                "btrfs",
                &["filesystem", "resize", &target.to_string(), &mount_path_str],
            )
            .await
            .context("Failed to shrink btrfs filesystem (data may exceed new size)")?;
            // 2. Detach loop, truncate backing file, then remount on a fresh
            //    loop device to reflect the new length.
            let loop_device = find_loop_device_for(img_path)
                .await
                .context("Failed to locate loop device for image")?;
            run_command_checked("umount", &[&mount_path_str])
                .await
                .context("Failed to unmount for image shrink")?;
            run_command_checked("losetup", &["-d", &loop_device])
                .await
                .context("Failed to detach loop device")?;
            run_command_checked("truncate", &["-s", &target.to_string(), img_path])
                .await
                .context("Failed to truncate image file")?;
            let new_loop = run_command("losetup", &["--find", "--show", img_path])
                .await
                .context("Failed to reattach loop device")?;
            let new_loop = new_loop.trim().to_string();
            run_command_checked("mount", &[&new_loop, &mount_path_str])
                .await
                .context("Failed to remount after image shrink")?;
            info!("Btrfs image shrunk to {} bytes", target);
        }
    }

    // Unified btrfs fs size sync — runs for every non-skip branch
    // (Equal / successful Less / successful Greater). Two roles:
    //   1. Normal grow path: this is the only place that extends the
    //      btrfs fs to match the newly enlarged loop capacity.
    //   2. Self-healing: if a previous run managed to truncate the file
    //      and refresh the loop but died before the btrfs resize, the
    //      next boot lands in `Equal` (file size already == target) and
    //      this line pulls the fs back in sync. `resize max` is
    //      idempotent when the fs is already aligned.
    // The two skip branches (avail-insufficient, mount-busy) return
    // early and deliberately do NOT reach this line.
    run_command_checked("btrfs", &["filesystem", "resize", "max", &mount_path_str])
        .await
        .context("Failed to sync btrfs filesystem size to loop capacity")?;
    if let Some(t) = grew_to {
        info!("Btrfs image grown to {} bytes", t);
    }
    Ok(())
}

/// Best-effort check whether `mount_path` is still being used by some
/// other process (stray shell, monitoring agent, etc.).
///
/// Returns `Some(pid_list)` when `fuser -m` reports occupants, or `None`
/// when nothing is using the mount, `fuser` is not installed, or the
/// invocation failed for any other reason.
///
/// This is purely advisory: the caller still relies on the subsequent
/// `umount` for authoritative enforcement. We explicitly do NOT route
/// through `run_command` because `fuser` exits with status 1 on the
/// (common) "nothing is using it" path, which `run_command` would treat
/// as a hard failure.
async fn check_mount_busy(mount_path: &str) -> Option<String> {
    let output = Command::new("fuser")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .args(["-m", mount_path])
        .output()
        .await
        .ok()?;
    // fuser prints occupying PIDs (space-separated) on stdout. Empty
    // stdout means either "not busy" (exit 1) or "real error" (exit >1);
    // either way we can't usefully flag occupants, so return None and
    // let the caller proceed.
    let pids = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if pids.is_empty() {
        None
    } else {
        Some(pids)
    }
}

/// Find the loop device currently backing `img_path` by parsing `losetup -j`.
///
/// Expected output format (one entry per line):
///     /dev/loop0: [2049]:12345 (/var/lib/ws-ckpt/btrfs-data.img)
async fn find_loop_device_for(img_path: &str) -> anyhow::Result<String> {
    let out = run_command("losetup", &["-j", img_path])
        .await
        .context("Failed to run `losetup -j`")?;
    parse_losetup_j(&out, img_path)
}

fn parse_losetup_j(out: &str, img_path: &str) -> anyhow::Result<String> {
    let line = out
        .lines()
        .next()
        .filter(|s| !s.trim().is_empty())
        .with_context(|| format!("No loop device currently backs image {}", img_path))?;
    let device = line
        .split(':')
        .next()
        .with_context(|| format!("Cannot parse losetup output: {}", line))?;
    Ok(device.trim().to_string())
}

fn parse_df_available(output: &str) -> anyhow::Result<u64> {
    // df -B1 output format:
    // Filesystem     1B-blocks  Used Available Use% Mounted on
    // /dev/sda1      ...        ...  ...       ...  /data
    let line = output
        .lines()
        .nth(1)
        .context("df output has no data line")?;
    let avail_str = line
        .split_whitespace()
        .nth(3)
        .context("df output missing available column")?;
    avail_str
        .parse::<u64>()
        .context("Failed to parse available size from df output")
}

/// Parse total partition size (1B-blocks column, index 1) from `df -B1` output.
fn parse_df_total(output: &str) -> anyhow::Result<u64> {
    let line = output
        .lines()
        .nth(1)
        .context("df output has no data line")?;
    let total_str = line
        .split_whitespace()
        .nth(1)
        .context("df output missing total column")?;
    total_str
        .parse::<u64>()
        .context("Failed to parse total size from df output")
}

async fn run_command(cmd: &str, args: &[&str]) -> anyhow::Result<String> {
    // Force C locale so stdout parsers (df, losetup -j, etc.) see the
    // canonical English column headers and number formats regardless of
    // the host's LANG/LC_* settings or whether we're on GNU coreutils vs
    // BusyBox. LC_ALL overrides all other LC_* and LANG, so setting just
    // LANG would be ignored when LC_ALL is already exported by the caller.
    let output = Command::new(cmd)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .args(args)
        .output()
        .await
        .with_context(|| format!("Failed to execute: {} {:?}", cmd, args))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "Command `{} {:?}` failed with status {}: {}",
            cmd,
            args,
            output.status,
            stderr.trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn run_command_checked(cmd: &str, args: &[&str]) -> anyhow::Result<()> {
    run_command(cmd, args).await?;
    Ok(())
}

async fn cleanup_orphans(mount_path: &std::path::Path) {
    let read_dir = match std::fs::read_dir(mount_path) {
        Ok(rd) => rd,
        Err(e) => {
            warn!("Cannot read mount path for orphan cleanup: {}", e);
            return;
        }
    };

    for entry in read_dir {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.ends_with(".rollback-tmp") {
            let path = entry.path();
            let ft = entry.file_type();
            if ft.is_ok() && ft.unwrap().is_symlink() {
                // Orphan rollback-tmp is a dangling symlink; just remove it
                info!("Removing orphan symlink: {:?}", path);
                if let Err(e) = std::fs::remove_file(&path) {
                    warn!("Failed to remove orphan symlink {:?}: {}", path, e);
                }
            } else {
                // Real subvolume; delete via btrfs
                info!("Cleaning up orphan rollback-tmp subvolume: {:?}", path);
                let path_str = path.to_string_lossy().to_string();
                if let Err(e) = run_command("btrfs", &["subvolume", "delete", &path_str]).await {
                    warn!("Failed to delete orphan subvolume {:?}: {}", path, e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{compute_target_size, parse_df_available, parse_df_total, parse_losetup_j};

    const GB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn parses_available_column_from_df_b1() {
        // df -B1 /data sample output
        let out = "Filesystem     1B-blocks        Used   Available Use% Mounted on\n\
                   /dev/sda1    107374182400 32212254720 75161927680  30% /data\n";
        assert_eq!(parse_df_available(out).unwrap(), 75_161_927_680u64);
    }

    #[test]
    fn returns_err_on_missing_data_line() {
        let out = "Filesystem 1B-blocks Used Available Use% Mounted on\n";
        assert!(parse_df_available(out).is_err());
    }

    #[test]
    fn returns_err_on_non_numeric_available() {
        let out = "Filesystem 1B-blocks Used Available Use% Mounted on\n\
                   /dev/sda1 100 10 NaN 10% /data\n";
        assert!(parse_df_available(out).is_err());
    }

    #[test]
    fn parses_loop_device_from_losetup_j() {
        let out = "/dev/loop0: [2049]:12345 (/var/lib/ws-ckpt/btrfs-data.img)\n";
        assert_eq!(
            parse_losetup_j(out, "/var/lib/ws-ckpt/btrfs-data.img").unwrap(),
            "/dev/loop0"
        );
    }

    #[test]
    fn parse_losetup_j_returns_err_on_empty() {
        assert!(parse_losetup_j("", "/var/lib/ws-ckpt/btrfs-data.img").is_err());
        assert!(parse_losetup_j("\n", "/var/lib/ws-ckpt/btrfs-data.img").is_err());
    }

    #[test]
    fn parses_total_column_from_df_b1() {
        let out = "Filesystem     1B-blocks        Used   Available Use% Mounted on\n\
                   /dev/sda1    107374182400 32212254720 75161927680  30% /data\n";
        assert_eq!(parse_df_total(out).unwrap(), 107_374_182_400u64);
    }

    #[test]
    fn compute_target_picks_size_cap_when_smaller() {
        // 100 GB partition, percent=50 -> 50 GB by_percent
        // img_size=30 GB -> 30 GB by_size
        // min = 30 GB
        let total = 100 * GB;
        let got = compute_target_size(30, 50.0, total);
        assert_eq!(got, 30 * GB);
    }

    #[test]
    fn compute_target_picks_percent_cap_when_smaller() {
        // 40 GB partition, percent=50 -> 20 GB by_percent
        // img_size=30 GB -> 30 GB by_size
        // min = 20 GB  (the percentage cap protects small hosts)
        let total = 40 * GB;
        let got = compute_target_size(30, 50.0, total);
        assert_eq!(got, 20 * GB);
    }

    #[test]
    fn compute_target_handles_equal_caps() {
        // 60 GB partition, percent=50 -> 30 GB by_percent
        // img_size=30 GB -> 30 GB by_size
        let total = 60 * GB;
        let got = compute_target_size(30, 50.0, total);
        assert_eq!(got, 30 * GB);
    }

    #[test]
    fn compute_target_saturates_on_huge_img_size() {
        // img_size enormous but percent cap still bounds the result.
        let total = 100 * GB;
        let got = compute_target_size(u64::MAX / GB, 50.0, total);
        assert_eq!(got, 50 * GB);
    }

    #[test]
    fn derive_img_dir_returns_parent_for_absolute_path() {
        let got = super::derive_img_dir("/var/lib/ws-ckpt/btrfs-data.img").unwrap();
        assert_eq!(got, "/var/lib/ws-ckpt");
    }

    #[test]
    fn derive_img_dir_rejects_bare_filename() {
        // Path::new("data.img").parent() == Some(""), which we treat as
        // invalid config to avoid silently falling back to a hard-coded
        // default directory.
        assert!(super::derive_img_dir("data.img").is_err());
    }

    #[test]
    fn derive_img_dir_rejects_root_and_empty() {
        // "/" and "" both have no meaningful parent for an image file.
        assert!(super::derive_img_dir("/").is_err());
        assert!(super::derive_img_dir("").is_err());
    }
}
