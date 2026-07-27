use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{error, info, warn};
use ws_ckpt_common::{ChangeType, DiffEntry};

use crate::util::unescape_proc_mount;

/// init_workspace backup path (#673).
pub fn backup_path_for(original_path: &str) -> String {
    format!("{}.pre-init-bak", original_path.trim_end_matches('/'))
}

/// Recover an orphan `.pre-init-bak` left by an interrupted prior init.
///
/// Called by `do_init_storage` in `btrfs_base.rs` and `btrfs_loop.rs` BEFORE
/// the Step 3 `rename(original_path -> backup_path)`. Two cases handled:
///
/// 1. Backup exists + subvol_path does NOT exist:
///    Prior init was interrupted AFTER Step 3 (rename) but BEFORE Step 4
///    (data migration). User's original data is sitting in the backup. The
///    right thing is to rename the backup back to `original_path`, restoring
///    the user's data, and let the caller proceed with a fresh normal init.
///    A stale empty directory at `original_path` (e.g., from a fixture's
///    `rm -rf + mkdir -p`) is removed first; a non-empty dir is refused to
///    avoid destroying user data.
///
/// 2. Backup exists + subvol_path DOES exist:
///    Prior init completed data migration (Step 4) and may have created the
///    symlink (Step 5). State is ambiguous (subvol might be valid, symlink
///    might be dangling, original_path might be a stale dir, etc.). Auto-
///    recovery would risk data loss, so we bail with an actionable error
///    pointing at `ws-ckpt recover -w <path> --force` (which we also patch
///    in this PR to clean orphan backups at the end of recovery).
///
/// 3. No backup exists: noop.
pub async fn recover_orphan_backup(original_path: &str, subvol_path: &Path) -> Result<()> {
    let backup_path = backup_path_for(original_path);
    if tokio::fs::symlink_metadata(&backup_path).await.is_err() {
        return Ok(());
    }

    let subvol_exists = tokio::fs::metadata(subvol_path)
        .await
        .map(|m| m.is_dir())
        .unwrap_or(false);

    if !subvol_exists {
        // Case 1: prior init crashed between Step 3 (rename) and Step 4
        // (data migration). User data is in backup. Restore it back to
        // original_path so the caller can re-run a clean init.
        warn!(
            "init: orphan backup {:?} detected (prior init interrupted before data migration); \
             restoring user data from backup",
            backup_path
        );
        // Remove stale state at original_path before restoring backup into it.
        match tokio::fs::symlink_metadata(original_path).await {
            Ok(m) if m.file_type().is_symlink() => {
                // dangling or stale symlink — safe to remove
                let _ = tokio::fs::remove_file(original_path).await;
            }
            Ok(m) if m.is_dir() => {
                let mut entries = tokio::fs::read_dir(original_path).await?;
                let is_empty = entries.next_entry().await?.is_none();
                if !is_empty {
                    bail!(
                        "found orphan backup {:?} but {:?} is a non-empty directory; \
                         refusing to overwrite user data. Inspect {:?} (likely contains \
                         data from interrupted init) and {:?}, move data out of {:?} or \
                         remove {:?} manually, then re-run init",
                        backup_path,
                        original_path,
                        backup_path,
                        original_path,
                        original_path,
                        backup_path
                    );
                }
                let _ = tokio::fs::remove_dir(original_path).await;
            }
            Ok(_) => {
                bail!(
                    "found orphan backup {:?} but {:?} is an unexpected file type; \
                     remove {:?} manually before retrying",
                    backup_path,
                    original_path,
                    original_path
                );
            }
            Err(_) => { /* original_path missing — fine, rename will create it */ }
        }
        tokio::fs::rename(&backup_path, original_path)
            .await
            .with_context(|| {
                format!(
                    "failed to restore orphan backup {:?} -> {:?}",
                    backup_path, original_path
                )
            })?;
        info!(
            "init: restored user data from orphan backup {:?} to {:?}; proceeding with fresh init",
            backup_path, original_path
        );
        return Ok(());
    }

    // Case 2: subvol_path exists. Ambiguous state — bail with actionable error.
    bail!(
        "found orphan backup {:?} and existing subvolume {:?} from an interrupted prior init. \
         Run `ws-ckpt recover -w {} --force` to restore user data from the subvolume and clean \
         up the orphan backup, then re-run init. If `ws-ckpt recover` does not list this workspace, \
         manually inspect {:?} (likely contains pre-migration user data) and {:?}, move data out \
         as needed, and remove the backup before retrying",
        backup_path, subvol_path, original_path, backup_path, subvol_path
    );
}

/// Roll back a failed init_workspace; `backup_owned=true` only when this init created the backup (#673).
pub async fn cleanup_init_storage(
    original_path: &str,
    subvol_path: &Path,
    snap_dir: &Path,
    backup_owned: bool,
) {
    if backup_owned {
        restore_original_from_backup(original_path).await;
    } else if let Ok(meta) = tokio::fs::symlink_metadata(original_path).await {
        if meta.file_type().is_symlink() {
            let _ = tokio::fs::remove_file(original_path).await;
        }
    }
    let _ = tokio::fs::remove_dir_all(snap_dir).await;
    if let Err(e) = delete_subvolume(subvol_path).await {
        error!("cleanup: failed to delete subvolume: {}", e);
    }
}

/// Rename our own `.pre-init-bak` back over original_path; foreign data at original is preserved.
async fn restore_original_from_backup(original_path: &str) {
    let backup_path = backup_path_for(original_path);
    match tokio::fs::symlink_metadata(&backup_path).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warn!(
                "cleanup: backup {:?} unexpectedly missing; dropping leftover symlink at {}",
                backup_path, original_path
            );
            if let Ok(meta) = tokio::fs::symlink_metadata(original_path).await {
                if meta.file_type().is_symlink() {
                    let _ = tokio::fs::remove_file(original_path).await;
                }
            }
            return;
        }
        Err(e) => {
            error!(
                "cleanup: cannot stat backup {:?}: {}; aborting restore (manual recovery required)",
                backup_path, e
            );
            return;
        }
    }

    match tokio::fs::symlink_metadata(original_path).await {
        Ok(meta) if meta.file_type().is_symlink() => {
            let _ = tokio::fs::remove_file(original_path).await;
        }
        Ok(meta) if meta.is_dir() => {
            let _ = tokio::fs::remove_dir(original_path).await;
        }
        _ => {}
    }

    match tokio::fs::rename(&backup_path, original_path).await {
        Ok(()) => info!("cleanup: restored {} from backup", original_path),
        Err(e) => error!(
            "cleanup: failed to restore {:?} -> {:?}: {}; backup retained for manual recovery",
            backup_path, original_path, e
        ),
    }
}

/// Ensure the current kernel can mount btrfs.
///
/// Checks `/proc/filesystems`; if absent, tries `modprobe btrfs` once and rechecks.
/// Fails with an actionable message pointing at kernel-modules-extra / CONFIG_BTRFS_FS.
pub async fn ensure_btrfs_support() -> Result<()> {
    if proc_filesystems_has_btrfs().await? {
        return Ok(());
    }

    // Best-effort modprobe; exit code is ignored, the recheck is authoritative.
    let _ = Command::new("modprobe").arg("btrfs").status().await;

    if proc_filesystems_has_btrfs().await? {
        info!("Loaded btrfs kernel module");
        return Ok(());
    }

    bail!(
        "Kernel does not support btrfs (no entry in /proc/filesystems and \
         `modprobe btrfs` did not register the module). Install the matching \
         kernel-modules-extra package or rebuild the kernel with CONFIG_BTRFS_FS, \
         then run `systemctl restart ws-ckpt`."
    );
}

/// True if `btrfs` is listed in `/proc/filesystems`.
async fn proc_filesystems_has_btrfs() -> Result<bool> {
    let file = File::open("/proc/filesystems")
        .await
        .context("Failed to open /proc/filesystems")?;
    let mut reader = BufReader::new(file).lines();
    while let Some(line) = reader.next_line().await? {
        // Line format: "<fstype>" or "nodev <fstype>"; fs name is always the last token.
        if line.split_whitespace().last() == Some("btrfs") {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Resolve a path that may be a symlink to its real (canonical) path.
/// If the path is a symlink, it is resolved via `canonicalize`.
/// If the path does not exist or is not a symlink, it is returned as-is.
pub async fn resolve_symlink_path(path: &str) -> Result<PathBuf> {
    let p = Path::new(path);
    match tokio::fs::symlink_metadata(p).await {
        Ok(meta) if meta.file_type().is_symlink() => {
            let resolved = tokio::fs::canonicalize(p)
                .await
                .with_context(|| format!("failed to resolve workspace symlink: {}", path))?;
            info!(
                "resolved workspace symlink: {} -> {}",
                path,
                resolved.display()
            );
            Ok(resolved)
        }
        _ => Ok(PathBuf::from(path)),
    }
}

/// Create a new btrfs subvolume at the given path
pub async fn create_subvolume(path: &Path) -> Result<()> {
    info!("creating btrfs subvolume: {}", path.display());
    let output = Command::new("btrfs")
        .args(["subvolume", "create"])
        .arg(path)
        .output()
        .await
        .context("failed to execute btrfs subvolume create")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!("btrfs subvolume create failed: {}", stderr);
        bail!("btrfs subvolume create failed: {}", stderr.trim());
    }
    info!("subvolume created: {}", path.display());
    Ok(())
}

/// Create a btrfs snapshot
/// If readonly=true, creates a readonly snapshot (-r flag)
pub async fn create_snapshot(src: &Path, dst: &Path, readonly: bool) -> Result<()> {
    info!(
        "creating snapshot: {} -> {} (readonly={})",
        src.display(),
        dst.display(),
        readonly
    );
    let mut cmd = Command::new("btrfs");
    cmd.arg("subvolume").arg("snapshot");
    if readonly {
        cmd.arg("-r");
    }
    cmd.arg(src).arg(dst);

    let output = cmd
        .output()
        .await
        .context("failed to execute btrfs subvolume snapshot")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!("btrfs snapshot failed: {}", stderr);
        bail!("btrfs snapshot failed: {}", stderr.trim());
    }
    info!("snapshot created: {}", dst.display());
    Ok(())
}

/// Delete a btrfs subvolume
pub async fn delete_subvolume(path: &Path) -> Result<()> {
    info!("deleting btrfs subvolume: {}", path.display());
    let output = Command::new("btrfs")
        .args(["subvolume", "delete"])
        .arg(path)
        .output()
        .await
        .context("failed to execute btrfs subvolume delete")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!("btrfs subvolume delete failed: {}", stderr);
        bail!("btrfs subvolume delete failed: {}", stderr.trim());
    }
    info!("subvolume deleted: {}", path.display());
    Ok(())
}

/// Compute the diff between two btrfs snapshots using `btrfs send --no-data -p`.
///
/// Requires root privileges and a btrfs filesystem.
///
/// Uses `std::process::Command` (blocking) inside `spawn_blocking` to avoid
/// tokio setting the pipe fd to O_NONBLOCK, which causes `btrfs receive --dump`
/// to fail with EAGAIN ("Resource temporarily unavailable").
pub async fn diff_between_snapshots(snap_from: &Path, snap_to: &Path) -> Result<Vec<DiffEntry>> {
    info!(
        "computing diff between {} and {}",
        snap_from.display(),
        snap_to.display()
    );

    let snap_from = snap_from.to_path_buf();
    let snap_to = snap_to.to_path_buf();

    tokio::task::spawn_blocking(move || diff_between_snapshots_blocking(&snap_from, &snap_to))
        .await
        .context("diff task panicked")?
}

/// Diff a snapshot against the live (writable) workspace subvolume.
///
/// Creates a temporary read-only snapshot of `live_subvol` inside `snap_dir`,
/// runs the diff, then removes the temporary snapshot regardless of outcome.
pub async fn diff_against_live(
    snap_from: &Path,
    live_subvol: &Path,
    snap_dir: &Path,
) -> Result<Vec<DiffEntry>> {
    use std::hash::{BuildHasher, Hasher, RandomState};

    let h = RandomState::new().build_hasher().finish();
    let tmp_snap = snap_dir.join(format!(".diff-tmp-{:06x}", h & 0xFFFFFF));

    // Clean up stale temp snapshot from a prior crash before creating a new one.
    if tmp_snap.exists() {
        let _ = delete_subvolume(&tmp_snap).await;
    }

    create_snapshot(live_subvol, &tmp_snap, true)
        .await
        .context("failed to create temporary snapshot of live workspace for diff")?;

    let result = diff_between_snapshots(snap_from, &tmp_snap).await;

    if let Err(e) = delete_subvolume(&tmp_snap).await {
        warn!(error = %e, path = %tmp_snap.display(), "failed to remove temp diff snapshot");
    }

    result
}

/// Blocking implementation of snapshot diff using `btrfs send | btrfs receive --dump`.
fn diff_between_snapshots_blocking(snap_from: &Path, snap_to: &Path) -> Result<Vec<DiffEntry>> {
    use std::process::{Command as StdCommand, Stdio};

    let mut sender = StdCommand::new("btrfs")
        .args(["send", "--no-data", "-p"])
        .arg(snap_from)
        .arg(snap_to)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn btrfs send")?;

    let sender_stdout = sender
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to capture btrfs send stdout"))?;

    // Take sender's stderr before passing stdout to receiver, so we can
    // read the correct error stream when btrfs send fails.
    let sender_stderr = sender
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to capture btrfs send stderr"))?;

    // std::process::ChildStdout implements Into<Stdio>, keeping the fd in blocking mode
    let receiver_output = StdCommand::new("btrfs")
        .args(["receive", "--dump"])
        .stdin(sender_stdout)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("failed to run btrfs receive --dump")?;

    let sender_status = sender.wait().context("failed to wait for btrfs send")?;

    if !sender_status.success() {
        let mut err_msg = String::new();
        use std::io::Read;
        let _ = std::io::BufReader::new(sender_stderr).read_to_string(&mut err_msg);
        error!("btrfs send failed (exit={}): {}", sender_status, err_msg);
        bail!("btrfs send failed: {}", err_msg.trim());
    }

    if !receiver_output.status.success() {
        let stderr = String::from_utf8_lossy(&receiver_output.stderr);
        error!("btrfs receive --dump failed: {}", stderr);
        bail!("btrfs receive --dump failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&receiver_output.stdout);
    let entries = parse_btrfs_diff_output(&stdout);
    Ok(entries)
}

/// Parse `btrfs receive --dump` output into deduplicated DiffEntry items.
///
/// Phase 1 collects: snapshot prefix, temp→real rename map, link pairs,
/// unlinks. A `link new dest=old` paired with `unlink old` encodes an `mv`
/// (btrfs send emits no `rename` line for cross-snapshot mv).
/// Phase 2 emits entries with precedence dedup (Renamed > Added > Deleted > Modified).
fn parse_btrfs_diff_output(output: &str) -> Vec<DiffEntry> {
    let mut snapshot_prefix = String::new();
    let mut rename_map: HashMap<String, String> = HashMap::new();
    let mut link_pairs: Vec<(String, String)> = Vec::new();
    let mut unlinked: HashSet<String> = HashSet::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("snapshot") {
            if let Some(name) = rest.split_whitespace().next() {
                snapshot_prefix = format!("{}/", name);
            }
        } else if let Some(rest) = line.strip_prefix("rename") {
            if let Some((src, dst)) = parse_dest_pair(rest, &snapshot_prefix) {
                rename_map.insert(src, dst);
            }
        } else if let Some(rest) = line.strip_prefix("link") {
            if let Some((new_real, dest_path)) = parse_dest_pair(rest, &snapshot_prefix) {
                link_pairs.push((new_real, dest_path));
            }
        } else if let Some(rest) = line.strip_prefix("unlink") {
            unlinked.insert(strip_snap_prefix(&first_token(rest), &snapshot_prefix));
        }
    }

    // mv detection: a `link new dest=old` paired with `unlink old` folds into
    // a single Renamed and the matching Deleted is suppressed. Each old path
    // can pair with at most one link — additional links to the same old path
    // fall through to real-hardlink (Added) handling in Phase 2.
    let mut mv_renames: HashMap<String, String> = HashMap::new();
    let mut suppressed_unlinks: HashSet<String> = HashSet::new();
    for (new_real, dest_path) in &link_pairs {
        if unlinked.contains(dest_path) && !suppressed_unlinks.contains(dest_path) {
            mv_renames.insert(new_real.clone(), dest_path.clone());
            suppressed_unlinks.insert(dest_path.clone());
        }
    }

    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut entries: Vec<DiffEntry> = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix("mkfile") {
            let path = resolve_path(rest, &snapshot_prefix, &rename_map);
            insert_dedup(&mut seen, &mut entries, path, ChangeType::Added, None);
        } else if let Some(rest) = line.strip_prefix("mkdir") {
            let path = resolve_path(rest, &snapshot_prefix, &rename_map);
            insert_dedup(
                &mut seen,
                &mut entries,
                path,
                ChangeType::Added,
                Some("directory".to_string()),
            );
        } else if let Some(rest) = line.strip_prefix("symlink") {
            // First token is the new symlink path (often a temp inode renamed
            // later); `dest=` is the link target string and isn't used.
            let path = resolve_path(rest, &snapshot_prefix, &rename_map);
            insert_dedup(
                &mut seen,
                &mut entries,
                path,
                ChangeType::Added,
                Some("symlink".to_string()),
            );
        } else if let Some(rest) = line.strip_prefix("link") {
            if let Some((new_real, _)) = parse_dest_pair(rest, &snapshot_prefix) {
                if let Some(old) = mv_renames.get(&new_real).cloned() {
                    insert_dedup(
                        &mut seen,
                        &mut entries,
                        new_real.clone(),
                        ChangeType::Renamed,
                        Some(format!("{} → {}", old, new_real)),
                    );
                } else {
                    insert_dedup(
                        &mut seen,
                        &mut entries,
                        new_real,
                        ChangeType::Added,
                        Some("hardlink".to_string()),
                    );
                }
            }
        } else if let Some(rest) = line.strip_prefix("unlink") {
            let path = strip_snap_prefix(&first_token(rest), &snapshot_prefix);
            if !suppressed_unlinks.contains(&path) {
                insert_dedup(&mut seen, &mut entries, path, ChangeType::Deleted, None);
            }
        } else if let Some(rest) = line.strip_prefix("rmdir") {
            let path = strip_snap_prefix(&first_token(rest), &snapshot_prefix);
            insert_dedup(
                &mut seen,
                &mut entries,
                path,
                ChangeType::Deleted,
                Some("directory".to_string()),
            );
        } else if let Some(rest) = line.strip_prefix("rename") {
            // temp→real renames are folded via rename_map; only emit the rest.
            if let Some((src, dst)) = parse_dest_pair(rest, &snapshot_prefix) {
                if !is_btrfs_temp_ref(&src) {
                    insert_dedup(
                        &mut seen,
                        &mut entries,
                        dst.clone(),
                        ChangeType::Renamed,
                        Some(format!("{} → {}", src, dst)),
                    );
                }
            }
        } else if let Some(rest) = line.strip_prefix("update_extent") {
            // `btrfs send --no-data` emits update_extent instead of write.
            let path = resolve_path(rest, &snapshot_prefix, &rename_map);
            insert_dedup(&mut seen, &mut entries, path, ChangeType::Modified, None);
        } else if let Some(rest) = line.strip_prefix("write") {
            let path = strip_snap_prefix(&first_token(rest), &snapshot_prefix);
            insert_dedup(&mut seen, &mut entries, path, ChangeType::Modified, None);
        } else if let Some(rest) = line.strip_prefix("truncate") {
            let path = strip_snap_prefix(&first_token(rest), &snapshot_prefix);
            insert_dedup(&mut seen, &mut entries, path, ChangeType::Modified, None);
        }
        // Skip metadata-only ops: utimes, chown, chmod, set_xattr, remove_xattr, clone.
    }

    entries
}

/// Strip the snapshot prefix from the first token of `rest`, then resolve
/// through `rename_map` (temp → real) when applicable.
fn resolve_path(rest: &str, snapshot_prefix: &str, rename_map: &HashMap<String, String>) -> String {
    let path = strip_snap_prefix(&first_token(rest), snapshot_prefix);
    rename_map.get(&path).cloned().unwrap_or(path)
}

/// Parse a `<src>  dest=<dst>` line tail into `(src, dst)`, both with the
/// snapshot prefix stripped. `dest=` for `link`/mvs may carry a bare relative
/// path (no prefix), which `strip_snap_prefix` no-ops cleanly.
fn parse_dest_pair(rest: &str, snapshot_prefix: &str) -> Option<(String, String)> {
    let rest = rest.trim();
    let dest_pos = rest.find("dest=")?;
    let src = strip_snap_prefix(&first_token(&rest[..dest_pos]), snapshot_prefix);
    let dst = strip_snap_prefix(&first_token(&rest[dest_pos + 5..]), snapshot_prefix);
    Some((src, dst))
}

/// Insert a DiffEntry, dedup'd by path. Higher-precedence change_type wins
/// on conflict (see `change_precedence`).
fn insert_dedup(
    seen: &mut HashMap<String, usize>,
    entries: &mut Vec<DiffEntry>,
    path: String,
    change_type: ChangeType,
    detail: Option<String>,
) {
    if path.is_empty() {
        return;
    }
    if let Some(&idx) = seen.get(&path) {
        if change_precedence(&change_type) > change_precedence(&entries[idx].change_type) {
            // Replace both fields together: keeping the old `detail` (e.g.
            // `"directory"` from a prior `rmdir`) when a `mkfile` reuses the
            // path leaks misleading metadata into the new entry.
            entries[idx].change_type = change_type;
            entries[idx].detail = detail;
        }
    } else {
        seen.insert(path.clone(), entries.len());
        entries.push(DiffEntry {
            path,
            change_type,
            detail,
        });
    }
}

/// Renamed > Added > Deleted > Modified.
fn change_precedence(c: &ChangeType) -> u8 {
    match c {
        ChangeType::Renamed => 4,
        ChangeType::Added => 3,
        ChangeType::Deleted => 2,
        ChangeType::Modified => 1,
    }
}

/// Extract the first whitespace-delimited token from a string.
fn first_token(s: &str) -> String {
    s.split_whitespace().next().unwrap_or("").to_string()
}

/// Strip the snapshot name prefix (e.g. `./msg1-step1/`) from a path.
fn strip_snap_prefix(path: &str, prefix: &str) -> String {
    if prefix.is_empty() {
        return path.to_string();
    }
    path.strip_prefix(prefix).unwrap_or(path).to_string()
}

/// Check whether a path's filename is a btrfs internal temporary inode
/// reference (e.g. `o261-118-0` from the `btrfs send` stream).
fn is_btrfs_temp_ref(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    if !name.starts_with('o') || name.len() < 4 {
        return false;
    }
    let rest = &name[1..];
    let parts: Vec<&str> = rest.splitn(3, '-').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// Get filesystem usage for the given btrfs mount path.
///
/// Returns (total_bytes, used_bytes). Requires root privileges and a btrfs filesystem.
pub async fn get_filesystem_usage(mount_path: &Path) -> Result<(u64, u64)> {
    let output = Command::new("btrfs")
        .args(["filesystem", "usage", "-b"])
        .arg(mount_path)
        .output()
        .await
        .context("failed to execute btrfs filesystem usage")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("btrfs filesystem usage failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_filesystem_usage(&stdout)
}

/// Parse btrfs filesystem usage -b output to extract total and used bytes.
///
/// Prefers `Free (estimated)` over raw `Used` because the latter only counts
/// bytes inside allocated chunks and ignores chunk-level allocation, which can
/// mislead space checks when data chunks are full but metadata reserves remain.
/// When `Free (estimated)` is available, `used` is derived as `total - free_estimated`
/// so that callers computing `total - used` get the authoritative free-space value.
fn parse_filesystem_usage(output: &str) -> Result<(u64, u64)> {
    let mut total: Option<u64> = None;
    let mut used: Option<u64> = None;
    let mut free_estimated: Option<u64> = None;

    for line in output.lines() {
        let line = line.trim();
        // Handle both "Device size:" and "Device size (approx):" variants
        // across different btrfs-progs versions
        if line.starts_with("Device size") {
            if let Some(val) = extract_last_numeric(line) {
                total = Some(val);
            }
        } else if line.starts_with("Used:") || line.starts_with("Used (approx):") {
            if let Some(val) = extract_last_numeric(line) {
                used = Some(val);
            }
        } else if line.starts_with("Free (estimated):") {
            // Line format: "Free (estimated):  52593926144      (min: 26833035264)"
            // extract_last_numeric would pick the "min" value, so use
            // extract_first_numeric_after_colon instead.
            if let Some(val) = extract_first_numeric_after_colon(line) {
                free_estimated = Some(val);
            }
        }
    }

    match (total, free_estimated, used) {
        (Some(t), Some(f), _) => {
            // Prefer Free (estimated): most accurate btrfs available space
            Ok((t, t.saturating_sub(f)))
        }
        (Some(t), None, Some(u)) => {
            // Fallback: older btrfs-progs without Free (estimated)
            Ok((t, u))
        }
        (None, _, _) => {
            warn!("parse_filesystem_usage: 'Device size' field not found in btrfs output");
            Ok((0, used.unwrap_or(0)))
        }
        (Some(t), None, None) => {
            warn!("parse_filesystem_usage: neither 'Free (estimated)' nor 'Used' field found in btrfs output");
            Ok((t, 0))
        }
    }
}

/// Extract the last numeric value from a line, stripping any non-numeric suffix.
fn extract_last_numeric(line: &str) -> Option<u64> {
    line.split_whitespace().last().and_then(|val| {
        val.trim_end_matches(|c: char| !c.is_ascii_digit())
            .parse()
            .ok()
    })
}

/// Extract the first numeric token that follows the `):` suffix in a line.
///
/// Designed for lines like:
///   `Free (estimated):  52593926144      (min: 26833035264)`
/// where `extract_last_numeric` would incorrectly return the `min` value.
/// We locate the closing `):` of the field label and parse the first number after it.
fn extract_first_numeric_after_colon(line: &str) -> Option<u64> {
    // Find the end of the field label "Free (estimated):"
    let colon_pos = line.find("):")?;
    let after = &line[colon_pos + 2..];
    after
        .split_whitespace()
        .find_map(|tok| tok.parse::<u64>().ok())
}

/// Check whether the given path resides on a btrfs filesystem.
pub async fn is_on_btrfs(path: &Path) -> bool {
    let output = Command::new("stat")
        .args(["-f", "-c", "%T"])
        .arg(path)
        .output()
        .await;
    match output {
        Ok(o) if o.status.success() => {
            let fs_type = String::from_utf8_lossy(&o.stdout).trim().to_string();
            fs_type == "btrfs"
        }
        _ => false,
    }
}

/// Information about a mounted btrfs partition.
#[derive(Debug, Clone)]
pub struct MountInfo {
    pub device: String,
    pub mount_point: String,
}

/// Find the first available btrfs partition by scanning /proc/mounts.
/// Skips read-only mounts and subvolume mounts (prefers physical /dev/ devices).
/// Returns an error if no writable physical btrfs partition is found.
pub async fn find_available_btrfs_partition() -> Result<MountInfo> {
    let file = File::open("/proc/mounts")
        .await
        .context("Failed to open /proc/mounts")?;
    let mut lines = BufReader::new(file).lines();

    let mut found_ro = false;

    while let Some(line) = lines.next_line().await? {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 && parts[2] == "btrfs" {
            // Skip read-only mounts
            if parts.len() >= 4 && parts[3].split(',').any(|opt| opt == "ro") {
                found_ro = true;
                continue;
            }
            // Skip subvolume mounts: prefer physical device partitions (/dev/xxx)
            if !parts[0].starts_with("/dev/") {
                continue;
            }
            // Skip loop devices (created by BtrfsLoop backend)
            if parts[0].starts_with("/dev/loop") {
                continue;
            }
            return Ok(MountInfo {
                device: unescape_proc_mount(parts[0]),
                mount_point: unescape_proc_mount(parts[1]),
            });
        }
    }

    if found_ro {
        bail!("Found btrfs partition(s), but all are read-only")
    } else {
        bail!("No available btrfs partition found in /proc/mounts")
    }
}

/// Warmup snapshot metadata cache to speed up subsequent btrfs operations.
///
/// Traverses the snapshot directory to trigger the kernel to load btrfs metadata
/// into page cache, significantly reducing cold-start latency for rollback
/// (up to 60-70% improvement for large file scenarios).
/// This is a read-only operation; failure does not affect the main flow.
pub async fn warmup_snapshot_metadata(snap_path: &Path) {
    use tokio::process::Command as TokioCommand;
    info!(
        "warming up snapshot metadata cache for: {}",
        snap_path.display()
    );
    let _ = TokioCommand::new("find")
        .arg(snap_path)
        .arg("-type")
        .arg("f")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // NOTE: All btrfs_common tests require:
    //   1. Root privileges (CAP_SYS_ADMIN)
    //   2. A mounted btrfs filesystem
    //   3. btrfs-progs installed
    // They are marked #[ignore] and must be run manually:
    //   cargo test -p ws-ckpt-daemon btrfs_common -- --ignored

    #[tokio::test]
    #[ignore = "requires root + btrfs filesystem"]
    async fn create_and_delete_subvolume() {
        let path = PathBuf::from("/mnt/btrfs-workspace/test-subvol-unit");
        // Clean up from prior runs
        let _ = delete_subvolume(&path).await;

        create_subvolume(&path)
            .await
            .expect("create_subvolume failed");
        assert!(path.exists());

        delete_subvolume(&path)
            .await
            .expect("delete_subvolume failed");
        assert!(!path.exists());
    }

    #[tokio::test]
    #[ignore = "requires root + btrfs filesystem"]
    async fn create_readonly_snapshot() {
        let src = PathBuf::from("/mnt/btrfs-workspace/test-snap-src");
        let dst = PathBuf::from("/mnt/btrfs-workspace/test-snap-dst-ro");
        let _ = delete_subvolume(&dst).await;
        let _ = delete_subvolume(&src).await;

        create_subvolume(&src).await.expect("create src subvolume");
        create_snapshot(&src, &dst, true)
            .await
            .expect("create readonly snapshot");
        assert!(dst.exists());

        // Cleanup
        let _ = delete_subvolume(&dst).await;
        let _ = delete_subvolume(&src).await;
    }

    #[tokio::test]
    #[ignore = "requires root + btrfs filesystem"]
    async fn create_writable_snapshot() {
        let src = PathBuf::from("/mnt/btrfs-workspace/test-snap-src-w");
        let dst = PathBuf::from("/mnt/btrfs-workspace/test-snap-dst-rw");
        let _ = delete_subvolume(&dst).await;
        let _ = delete_subvolume(&src).await;

        create_subvolume(&src).await.expect("create src subvolume");
        create_snapshot(&src, &dst, false)
            .await
            .expect("create writable snapshot");
        assert!(dst.exists());

        // Cleanup
        let _ = delete_subvolume(&dst).await;
        let _ = delete_subvolume(&src).await;
    }

    #[tokio::test]
    #[ignore = "requires root + btrfs filesystem"]
    async fn diff_between_two_snapshots() {
        let src = PathBuf::from("/mnt/btrfs-workspace/test-diff-src");
        let snap1 = PathBuf::from("/mnt/btrfs-workspace/test-diff-snap1");
        let snap2 = PathBuf::from("/mnt/btrfs-workspace/test-diff-snap2");
        // Cleanup prior
        let _ = delete_subvolume(&snap2).await;
        let _ = delete_subvolume(&snap1).await;
        let _ = delete_subvolume(&src).await;

        create_subvolume(&src).await.unwrap();
        create_snapshot(&src, &snap1, true).await.unwrap();
        // Modify src
        tokio::fs::write(src.join("newfile.txt"), "hello")
            .await
            .unwrap();
        create_snapshot(&src, &snap2, true).await.unwrap();

        let entries = diff_between_snapshots(&snap1, &snap2).await.unwrap();
        assert!(!entries.is_empty());

        // Cleanup
        let _ = delete_subvolume(&snap2).await;
        let _ = delete_subvolume(&snap1).await;
        let _ = delete_subvolume(&src).await;
    }

    #[tokio::test]
    #[ignore = "requires root + btrfs filesystem"]
    async fn diff_against_live_workspace() {
        let base = PathBuf::from("/mnt/btrfs-workspace");
        let src = base.join("test-diff-live-src");
        let snap1 = base.join("test-diff-live-snap1");
        // Cleanup prior
        let _ = delete_subvolume(&snap1).await;
        let _ = delete_subvolume(&src).await;

        create_subvolume(&src).await.unwrap();
        create_snapshot(&src, &snap1, true).await.unwrap();
        // Modify the live subvolume after snapshot
        tokio::fs::write(src.join("live-change.txt"), "world")
            .await
            .unwrap();

        let entries = diff_against_live(&snap1, &src, &base).await.unwrap();
        assert!(!entries.is_empty());

        // Cleanup
        let _ = delete_subvolume(&snap1).await;
        let _ = delete_subvolume(&src).await;
    }

    #[tokio::test]
    #[ignore = "requires root + btrfs filesystem"]
    async fn get_fs_usage() {
        let (total, used) = get_filesystem_usage(Path::new("/mnt/btrfs-workspace"))
            .await
            .unwrap();
        assert!(total > 0);
        assert!(used <= total);
    }

    #[test]
    fn parse_btrfs_diff_output_handles_common_ops() {
        // Use real `btrfs receive --dump` format: rename uses "dest=" syntax
        let output = "snapshot  ./snap  uuid=abc transid=42\nmkfile  ./snap/src/main.rs\nunlink  ./snap/old.txt\nrename  ./snap/old_name  dest=./snap/new_name\nwrite   ./snap/src/lib.rs\nmkdir   ./snap/new_dir\nrmdir   ./snap/old_dir\ntruncate  ./snap/data.bin\nupdate_extent  ./snap/src/config.rs  offset=0 len=128\n";
        let entries = parse_btrfs_diff_output(output);
        assert_eq!(entries.len(), 8);
        assert_eq!(entries[0].change_type, ChangeType::Added); // mkfile
        assert_eq!(entries[0].path, "src/main.rs");
        assert_eq!(entries[1].change_type, ChangeType::Deleted); // unlink
        assert_eq!(entries[2].change_type, ChangeType::Renamed); // rename (real rename, not temp)
        assert_eq!(entries[3].change_type, ChangeType::Modified); // write
        assert_eq!(entries[4].change_type, ChangeType::Added); // mkdir
        assert_eq!(entries[5].change_type, ChangeType::Deleted); // rmdir
        assert_eq!(entries[6].change_type, ChangeType::Modified); // truncate
        assert_eq!(entries[7].change_type, ChangeType::Modified); // update_extent
    }

    #[test]
    fn parse_btrfs_diff_output_mapper_resolves_temp_inodes() {
        let output = "snapshot  ./msg1-step1  uuid=abc transid=42\n\
                       mkfile    ./msg1-step1/o261-118-0\n\
                       rename    ./msg1-step1/o261-118-0  dest=./msg1-step1/src/lib.rs\n\
                       update_extent  ./msg1-step1/src/lib.rs  offset=0 len=84\n\
                       utimes    ./msg1-step1/src/lib.rs\n\
                       update_extent  ./msg1-step1/src/main.rs  offset=0 len=50\n\
                       mkfile    ./msg1-step1/o262-119-0\n\
                       rename    ./msg1-step1/o262-119-0  dest=./msg1-step1/.gitignore\n\
                       utimes    ./msg1-step1/\n";
        let entries = parse_btrfs_diff_output(output);

        assert_eq!(entries.len(), 3, "entries: {:?}", entries);
        assert_eq!(entries[0].path, "src/lib.rs");
        assert_eq!(entries[0].change_type, ChangeType::Added);
        assert_eq!(entries[1].path, "src/main.rs");
        assert_eq!(entries[1].change_type, ChangeType::Modified);
        assert_eq!(entries[2].path, ".gitignore");
        assert_eq!(entries[2].change_type, ChangeType::Added);
    }

    #[test]
    fn parse_btrfs_diff_output_empty() {
        let entries = parse_btrfs_diff_output("");
        assert!(entries.is_empty());
    }

    #[test]
    fn backup_path_for_appends_suffix() {
        assert_eq!(backup_path_for("/tmp/ws"), "/tmp/ws.pre-init-bak");
        assert_eq!(backup_path_for("/tmp/ws/"), "/tmp/ws.pre-init-bak");
    }

    /// Backup restores user data when symlink already replaced original (#673).
    #[tokio::test]
    async fn restore_swaps_symlink_back_to_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let orig = tmp.path().join("ws");
        let bak = tmp.path().join("ws.pre-init-bak");
        let target = tmp.path().join("subvol");

        tokio::fs::create_dir(&bak).await.unwrap();
        tokio::fs::write(bak.join("foo.txt"), b"important")
            .await
            .unwrap();
        tokio::fs::create_dir(&target).await.unwrap();
        tokio::fs::symlink(&target, &orig).await.unwrap();

        restore_original_from_backup(orig.to_str().unwrap()).await;

        assert!(!bak.exists(), "backup should be renamed away");
        assert!(orig.is_dir(), "original must be a real dir again");
        let payload = tokio::fs::read_to_string(orig.join("foo.txt"))
            .await
            .unwrap();
        assert_eq!(payload, "important");
    }

    /// TOCTOU racer: an empty foreign dir appears at original between rename
    /// and symlink. Backup must still restore (#673).
    #[tokio::test]
    async fn restore_clears_empty_racer_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let orig = tmp.path().join("ws");
        let bak = tmp.path().join("ws.pre-init-bak");

        tokio::fs::create_dir(&bak).await.unwrap();
        tokio::fs::write(bak.join("foo.txt"), b"keep")
            .await
            .unwrap();
        tokio::fs::create_dir(&orig).await.unwrap();

        restore_original_from_backup(orig.to_str().unwrap()).await;

        assert!(!bak.exists());
        assert!(orig.join("foo.txt").exists(), "user data must be back");
    }

    /// Non-empty foreign dir at original must NOT be deleted; backup stays put.
    #[tokio::test]
    async fn restore_preserves_non_empty_foreign_dir_and_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let orig = tmp.path().join("ws");
        let bak = tmp.path().join("ws.pre-init-bak");

        tokio::fs::create_dir(&bak).await.unwrap();
        tokio::fs::write(bak.join("foo.txt"), b"keep")
            .await
            .unwrap();
        tokio::fs::create_dir(&orig).await.unwrap();
        tokio::fs::write(orig.join("racer.txt"), b"foreign")
            .await
            .unwrap();

        restore_original_from_backup(orig.to_str().unwrap()).await;

        assert!(bak.exists(), "backup must be retained for manual recovery");
        assert!(orig.join("racer.txt").exists());
        assert!(bak.join("foo.txt").exists());
    }

    /// No backup -> noop, must not touch anything else.
    #[tokio::test]
    async fn restore_is_noop_when_backup_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let orig = tmp.path().join("ws");
        tokio::fs::create_dir(&orig).await.unwrap();
        tokio::fs::write(orig.join("x"), b"y").await.unwrap();

        restore_original_from_backup(orig.to_str().unwrap()).await;

        assert!(orig.join("x").exists());
    }

    /// Foreign .pre-init-bak must not be restored when backup_owned=false (#673).
    #[tokio::test]
    async fn cleanup_does_not_restore_unowned_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let orig = tmp.path().join("ws");
        let bak = tmp.path().join("ws.pre-init-bak");
        let subvol = tmp.path().join("subvol");
        let snap = tmp.path().join("snap");

        tokio::fs::create_dir(&bak).await.unwrap();
        tokio::fs::write(bak.join("attacker.txt"), b"foreign")
            .await
            .unwrap();
        tokio::fs::create_dir(&orig).await.unwrap();
        tokio::fs::write(orig.join("user.txt"), b"real")
            .await
            .unwrap();
        tokio::fs::create_dir(&snap).await.unwrap();

        cleanup_init_storage(orig.to_str().unwrap(), &subvol, &snap, false).await;

        assert!(orig.join("user.txt").exists(), "user data must remain");
        assert!(
            bak.join("attacker.txt").exists(),
            "foreign backup not restored"
        );
        assert!(!snap.exists(), "snap dir cleaned");
    }

    /// cleanup with backup_owned=false drops a leftover symlink we created in step 6.
    #[tokio::test]
    async fn cleanup_drops_leftover_symlink_when_unowned() {
        let tmp = tempfile::tempdir().unwrap();
        let orig = tmp.path().join("ws");
        let target = tmp.path().join("subvol");
        let snap = tmp.path().join("snap");

        tokio::fs::create_dir(&target).await.unwrap();
        tokio::fs::symlink(&target, &orig).await.unwrap();
        tokio::fs::create_dir(&snap).await.unwrap();

        cleanup_init_storage(orig.to_str().unwrap(), &target, &snap, false).await;

        assert!(!orig.exists(), "leftover symlink dropped");
    }

    /// backup_owned=true restores the backup over original (legit happy path).
    #[tokio::test]
    async fn cleanup_restores_owned_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let orig = tmp.path().join("ws");
        let bak = tmp.path().join("ws.pre-init-bak");
        let target = tmp.path().join("subvol");
        let snap = tmp.path().join("snap");

        tokio::fs::create_dir(&bak).await.unwrap();
        tokio::fs::write(bak.join("user.txt"), b"keep")
            .await
            .unwrap();
        tokio::fs::create_dir(&target).await.unwrap();
        tokio::fs::symlink(&target, &orig).await.unwrap();
        tokio::fs::create_dir(&snap).await.unwrap();

        cleanup_init_storage(orig.to_str().unwrap(), &target, &snap, true).await;

        assert!(orig.is_dir(), "original restored as real dir");
        assert!(orig.join("user.txt").exists(), "user data back at original");
        assert!(!bak.exists(), "backup consumed");
    }

    #[test]
    fn parse_filesystem_usage_parses_output() {
        let output = r#"Overall:
    Device size:                 107374182400
    Device allocated:             10737418240
    Device unallocated:           96636764160
    Used:                          5368709120
"#;
        let (total, used) = parse_filesystem_usage(output).unwrap();
        assert_eq!(total, 107374182400);
        assert_eq!(used, 5368709120);
    }

    #[test]
    fn parse_filesystem_usage_with_free_estimated() {
        let output = r#"Overall:
    Device size:                  53686042624
    Device allocated:              2164260864
    Device unallocated:           51521781760
    Used:                             2121728
    Free (estimated):             52593926144      (min: 26833035264)
    Free (statfs, df):            52592877568
"#;
        let (total, used) = parse_filesystem_usage(output).unwrap();
        assert_eq!(total, 53686042624);
        // used should be total - free_estimated, NOT the raw Used field
        assert_eq!(used, 53686042624 - 52593926144);
        assert_eq!(used, 1092116480);
    }

    #[test]
    fn parse_filesystem_usage_free_estimated_without_min() {
        let output = r#"Overall:
    Device size:                  53686042624
    Device allocated:              2164260864
    Used:                             2121728
    Free (estimated):             52593926144
"#;
        let (total, used) = parse_filesystem_usage(output).unwrap();
        assert_eq!(total, 53686042624);
        assert_eq!(used, 53686042624 - 52593926144);
    }

    #[test]
    fn parse_filesystem_usage_missing_fields() {
        let output = "some random output\n";
        let (total, used) = parse_filesystem_usage(output).unwrap();
        assert_eq!(total, 0);
        assert_eq!(used, 0);
    }

    #[test]
    fn parse_btrfs_diff_output_unknown_ops_are_skipped() {
        let output = "mkfile  new.txt\nchown  foo.txt\nxattr  bar.txt\n";
        let entries = parse_btrfs_diff_output(output);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].change_type, ChangeType::Added);
    }

    // mkfile temp + rename temp→foo.txt + update_extent foo.txt → Added wins.
    #[test]
    fn parse_btrfs_diff_output_added_file_with_temp_rename() {
        let output = "snapshot  ./snap_a_ro  uuid=abc transid=1\n\
                      mkfile          ./snap_a_ro/o257-34321-0\n\
                      rename          ./snap_a_ro/o257-34321-0  dest=./snap_a_ro/foo.txt\n\
                      update_extent   ./snap_a_ro/foo.txt  offset=0 len=6\n";
        let entries = parse_btrfs_diff_output(output);
        assert_eq!(entries.len(), 1, "entries: {:?}", entries);
        assert_eq!(entries[0].path, "foo.txt");
        assert_eq!(entries[0].change_type, ChangeType::Added);
    }

    // symlink temp + rename temp→mylink → Added(mylink, "symlink").
    #[test]
    fn parse_btrfs_diff_output_symlink_with_temp_rename() {
        let output = "snapshot  ./snap_a_ro  uuid=abc transid=1\n\
                      symlink         ./snap_a_ro/o258-34321-0  dest=/etc/passwd\n\
                      rename          ./snap_a_ro/o258-34321-0  dest=./snap_a_ro/mylink\n";
        let entries = parse_btrfs_diff_output(output);
        assert_eq!(entries.len(), 1, "entries: {:?}", entries);
        assert_eq!(entries[0].path, "mylink");
        assert_eq!(entries[0].change_type, ChangeType::Added);
        assert_eq!(entries[0].detail.as_deref(), Some("symlink"));
    }

    // link new dest=existing where existing is NOT unlinked → real hardlink.
    #[test]
    fn parse_btrfs_diff_output_real_hardlink_emits_added() {
        let output = "snapshot  ./snap_a_ro  uuid=abc transid=1\n\
                      mkfile          ./snap_a_ro/o259-34321-0\n\
                      rename          ./snap_a_ro/o259-34321-0  dest=./snap_a_ro/target.txt\n\
                      link            ./snap_a_ro/hardlink_to_target  dest=target.txt\n";
        let entries = parse_btrfs_diff_output(output);
        assert_eq!(entries.len(), 2, "entries: {:?}", entries);
        assert_eq!(entries[0].path, "target.txt");
        assert_eq!(entries[0].change_type, ChangeType::Added);
        assert_eq!(entries[1].path, "hardlink_to_target");
        assert_eq!(entries[1].change_type, ChangeType::Added);
        assert_eq!(entries[1].detail.as_deref(), Some("hardlink"));
    }

    // mv foo.txt → bar.txt: link bar dest=foo + unlink foo → single Renamed,
    // Deleted(foo) suppressed.
    #[test]
    fn parse_btrfs_diff_output_mv_emits_renamed_and_drops_deleted() {
        let output = "snapshot  ./snap_b_ro  uuid=abc transid=2\n\
                      link            ./snap_b_ro/bar.txt  dest=foo.txt\n\
                      unlink          ./snap_b_ro/foo.txt\n";
        let entries = parse_btrfs_diff_output(output);
        assert_eq!(entries.len(), 1, "entries: {:?}", entries);
        assert_eq!(entries[0].path, "bar.txt");
        assert_eq!(entries[0].change_type, ChangeType::Renamed);
        assert_eq!(entries[0].detail.as_deref(), Some("foo.txt → bar.txt"));
    }

    // rmdir foo + mkfile foo: Added wins over Deleted, and the old "directory"
    // detail must NOT leak into the new file entry.
    #[test]
    fn parse_btrfs_diff_output_replace_clears_stale_detail() {
        let output = "snapshot  ./snap  uuid=abc transid=1\n\
                      rmdir   ./snap/foo\n\
                      mkfile  ./snap/o100-1-0\n\
                      rename  ./snap/o100-1-0  dest=./snap/foo\n";
        let entries = parse_btrfs_diff_output(output);
        assert_eq!(entries.len(), 1, "entries: {:?}", entries);
        assert_eq!(entries[0].path, "foo");
        assert_eq!(entries[0].change_type, ChangeType::Added);
        assert_eq!(entries[0].detail, None, "stale 'directory' detail leaked");
    }

    // Two `link X dest=foo` plus one `unlink foo`: only the first link is
    // treated as the mv rename; the second is a real hardlink Added.
    #[test]
    fn parse_btrfs_diff_output_multi_link_to_same_old_path() {
        let output = "snapshot  ./snap  uuid=abc transid=1\n\
                      link    ./snap/bar  dest=foo\n\
                      link    ./snap/baz  dest=foo\n\
                      unlink  ./snap/foo\n";
        let entries = parse_btrfs_diff_output(output);
        assert_eq!(entries.len(), 2, "entries: {:?}", entries);
        assert_eq!(entries[0].path, "bar");
        assert_eq!(entries[0].change_type, ChangeType::Renamed);
        assert_eq!(entries[0].detail.as_deref(), Some("foo → bar"));
        assert_eq!(entries[1].path, "baz");
        assert_eq!(entries[1].change_type, ChangeType::Added);
        assert_eq!(entries[1].detail.as_deref(), Some("hardlink"));
    }

    // PB-004: update_extent before mkfile (both resolve to same real path);
    // Added must win over the earlier-seen Modified via precedence dedup.
    #[test]
    fn parse_btrfs_diff_output_added_wins_over_modified_when_extent_first() {
        let output = "snapshot  ./snap  uuid=abc transid=1\n\
                      update_extent   ./snap/foo.txt  offset=0 len=6\n\
                      mkfile          ./snap/o100-1-0\n\
                      rename          ./snap/o100-1-0  dest=./snap/foo.txt\n";
        let entries = parse_btrfs_diff_output(output);
        assert_eq!(entries.len(), 1, "entries: {:?}", entries);
        assert_eq!(entries[0].path, "foo.txt");
        assert_eq!(entries[0].change_type, ChangeType::Added);
    }

    #[test]
    fn parse_filesystem_usage_approx_variant() {
        let output = r#"Overall:
    Device size (approx):        107374182400
    Device allocated:             10737418240
    Device unallocated:           96636764160
    Used (approx):                 5368709120
"#;
        let (total, used) = parse_filesystem_usage(output).unwrap();
        assert_eq!(total, 107374182400);
        assert_eq!(used, 5368709120);
    }

    #[test]
    fn extract_first_numeric_after_colon_picks_correct_value() {
        assert_eq!(
            extract_first_numeric_after_colon(
                "Free (estimated):  52593926144      (min: 26833035264)"
            ),
            Some(52593926144)
        );
        assert_eq!(
            extract_first_numeric_after_colon("Free (estimated):  12345"),
            Some(12345)
        );
        assert_eq!(extract_first_numeric_after_colon("no colon here"), None);
    }

    // -------------------------------------------------------------------------
    // Tests for recover_orphan_backup
    // -------------------------------------------------------------------------

    /// No orphan backup → noop. Does not touch original_path or subvol_path.
    #[tokio::test]
    async fn recover_orphan_backup_noop_when_no_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let orig = tmp.path().join("ws");
        let subvol = tmp.path().join("subvol");
        tokio::fs::create_dir(&orig).await.unwrap();
        tokio::fs::write(orig.join("user.txt"), b"keep")
            .await
            .unwrap();

        recover_orphan_backup(orig.to_str().unwrap(), &subvol)
            .await
            .unwrap();

        assert!(orig.join("user.txt").exists(), "original untouched");
        assert!(!subvol.exists(), "subvol untouched");
        assert!(
            !tmp.path().join("ws.pre-init-bak").exists(),
            "no backup created"
        );
    }

    /// Orphan backup + no subvol → restores backup to original_path. (Case 1)
    #[tokio::test]
    async fn recover_orphan_backup_restores_user_data_when_no_subvol() {
        let tmp = tempfile::tempdir().unwrap();
        let orig = tmp.path().join("ws");
        let bak = tmp.path().join("ws.pre-init-bak");
        let subvol = tmp.path().join("subvol");

        tokio::fs::create_dir(&bak).await.unwrap();
        tokio::fs::write(bak.join("foo.txt"), b"important")
            .await
            .unwrap();

        recover_orphan_backup(orig.to_str().unwrap(), &subvol)
            .await
            .unwrap();

        assert!(!bak.exists(), "backup consumed by rename");
        assert!(orig.is_dir(), "original restored as real dir");
        assert!(orig.join("foo.txt").exists(), "user data restored");
    }

    /// Orphan backup + no subvol + stale empty dir at original → removes the
    /// stale dir and restores backup. (Case 1 with fixture-like state.)
    #[tokio::test]
    async fn recover_orphan_backup_clears_stale_empty_dir_at_original() {
        let tmp = tempfile::tempdir().unwrap();
        let orig = tmp.path().join("ws");
        let bak = tmp.path().join("ws.pre-init-bak");
        let subvol = tmp.path().join("subvol");

        tokio::fs::create_dir(&bak).await.unwrap();
        tokio::fs::write(bak.join("foo.txt"), b"keep")
            .await
            .unwrap();
        // Simulate a test fixture's `rm -rf + mkdir -p` leaving an empty dir.
        tokio::fs::create_dir(&orig).await.unwrap();

        recover_orphan_backup(orig.to_str().unwrap(), &subvol)
            .await
            .unwrap();

        assert!(!bak.exists());
        assert!(
            orig.join("foo.txt").exists(),
            "user data restored over empty dir"
        );
    }

    /// Orphan backup + no subvol + stale dangling symlink at original → removes
    /// the symlink and restores backup. (Case 1 with broken symlink.)
    #[tokio::test]
    async fn recover_orphan_backup_clears_dangling_symlink_at_original() {
        let tmp = tempfile::tempdir().unwrap();
        let orig = tmp.path().join("ws");
        let bak = tmp.path().join("ws.pre-init-bak");
        let subvol = tmp.path().join("subvol");
        let ghost = tmp.path().join("nonexistent-target");

        tokio::fs::create_dir(&bak).await.unwrap();
        tokio::fs::write(bak.join("foo.txt"), b"keep")
            .await
            .unwrap();
        tokio::fs::symlink(&ghost, &orig).await.unwrap(); // dangling symlink

        recover_orphan_backup(orig.to_str().unwrap(), &subvol)
            .await
            .unwrap();

        assert!(!bak.exists());
        assert!(orig.is_dir(), "original is real dir, not symlink");
        assert!(orig.join("foo.txt").exists());
    }

    /// Orphan backup + non-empty dir at original → refuses and preserves user
    /// data in both locations. (Case 1 safety bail.)
    #[tokio::test]
    async fn recover_orphan_backup_refuses_non_empty_dir_at_original() {
        let tmp = tempfile::tempdir().unwrap();
        let orig = tmp.path().join("ws");
        let bak = tmp.path().join("ws.pre-init-bak");
        let subvol = tmp.path().join("subvol");

        tokio::fs::create_dir(&bak).await.unwrap();
        tokio::fs::write(bak.join("from_backup.txt"), b"keep")
            .await
            .unwrap();
        tokio::fs::create_dir(&orig).await.unwrap();
        tokio::fs::write(orig.join("racer.txt"), b"foreign")
            .await
            .unwrap();

        let err = recover_orphan_backup(orig.to_str().unwrap(), &subvol)
            .await
            .unwrap_err();
        let msg = format!("{:#}", err);
        assert!(msg.contains("non-empty directory"), "got: {}", msg);
        assert!(msg.contains("remove"), "actionable error: {}", msg);

        // Both must be preserved — no data loss.
        assert!(bak.join("from_backup.txt").exists());
        assert!(orig.join("racer.txt").exists());
    }

    /// Orphan backup + subvol exists → bails with actionable error pointing
    /// at `ws-ckpt recover`. Does not touch either path. (Case 2.)
    #[tokio::test]
    async fn recover_orphan_backup_bails_when_subvol_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let orig = tmp.path().join("ws");
        let bak = tmp.path().join("ws.pre-init-bak");
        let subvol = tmp.path().join("subvol");

        tokio::fs::create_dir(&bak).await.unwrap();
        tokio::fs::write(bak.join("user.txt"), b"keep")
            .await
            .unwrap();
        tokio::fs::create_dir(&subvol).await.unwrap();
        tokio::fs::write(subvol.join("migrated.txt"), b"partial")
            .await
            .unwrap();

        let err = recover_orphan_backup(orig.to_str().unwrap(), &subvol)
            .await
            .unwrap_err();
        let msg = format!("{:#}", err);
        assert!(msg.contains("ws-ckpt recover"), "actionable error: {}", msg);
        assert!(msg.contains("interrupted prior init"), "context: {}", msg);

        // Nothing should be destroyed on ambiguous state.
        assert!(bak.join("user.txt").exists());
        assert!(subvol.join("migrated.txt").exists());
    }
}
