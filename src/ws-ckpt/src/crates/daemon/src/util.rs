//! Backend-agnostic helpers: LC-locked command execution, mount probing, symlink recovery.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::state::DaemonState;

/// Run a command and return stdout; non-zero exit is a hard failure.
///
/// Forces `LC_ALL=C LANG=C` so parsers (df, losetup -j, ...) see canonical output.
pub async fn run_command(cmd: &str, args: &[&str]) -> anyhow::Result<String> {
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

/// Same as `run_command` but discards stdout.
pub async fn run_command_checked(cmd: &str, args: &[&str]) -> anyhow::Result<()> {
    run_command(cmd, args).await?;
    Ok(())
}

/// Decode `\NNN` octal escapes used by /proc/mounts for whitespace and
/// backslashes in mount-point paths (e.g. space → \040, tab → \011).
/// Unrecognised sequences are left literal so a malformed line never panics.
pub fn unescape_proc_mount(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let d0 = bytes[i + 1];
            let d1 = bytes[i + 2];
            let d2 = bytes[i + 3];
            if (b'0'..=b'7').contains(&d0)
                && (b'0'..=b'7').contains(&d1)
                && (b'0'..=b'7').contains(&d2)
            {
                let v = ((d0 - b'0') << 6) | ((d1 - b'0') << 3) | (d2 - b'0');
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Return true if `mount_path` appears in `/proc/mounts`.
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
            let decoded = unescape_proc_mount(mp);
            let mp_path = Path::new(&decoded);
            if mp_path == target || mp_path.components().collect::<PathBuf>() == target_norm {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

/// Ensure every registered workspace's user-facing path is a symlink pointing at
/// `data_root/<ws_id>`; rebuild if missing or wrong target.
pub async fn ensure_symlinks(state: &DaemonState) {
    let all_ws = state.all_workspaces();
    for arc in all_ws {
        let ws = arc.read().await;
        let expected_subvol_path = state.backend.data_root().join(&ws.ws_id);
        let ws_path = ws.path.to_string_lossy().to_string();

        // Guard against dangling symlinks when the subvolume is missing.
        if !expected_subvol_path.exists() {
            warn!(
                "subvolume {:?} missing for workspace {}; skipping symlink recovery",
                expected_subvol_path, ws.ws_id
            );
            continue;
        }

        match tokio::fs::read_link(&ws_path).await {
            Ok(target) if target == expected_subvol_path => {
                debug!("symlink OK for {}: -> {:?}", ws_path, target);
            }
            Ok(target) => {
                warn!(
                    "symlink {} points to {:?}, expected {:?}; rebuilding",
                    ws_path, target, expected_subvol_path
                );
                rebuild_symlink(&ws_path, &expected_subvol_path).await;
            }
            Err(_) => {
                warn!("symlink missing or invalid for {}; rebuilding", ws_path);
                rebuild_symlink(&ws_path, &expected_subvol_path).await;
            }
        }
    }
}

/// Atomically replace the symlink via temp-file + rename.
async fn rebuild_symlink(ws_path: &str, expected_subvol_path: &Path) {
    let tmp_path = format!("{}.tmp", ws_path);
    // Best-effort cleanup of leftover residue from a prior daemon crash between
    // symlink() and rename(); without this, symlink() returns EEXIST and
    // recovery wedges permanently for this workspace.
    let _ = tokio::fs::remove_file(&tmp_path).await;
    if let Err(e) = tokio::fs::symlink(expected_subvol_path, &tmp_path).await {
        warn!("failed to create temp symlink for {}: {}", ws_path, e);
        return;
    }
    if let Err(e) = tokio::fs::rename(&tmp_path, ws_path).await {
        warn!(
            "failed to atomically replace symlink for {}: {}",
            ws_path, e
        );
        let _ = tokio::fs::remove_file(&tmp_path).await;
    } else {
        info!("rebuilt symlink for {}", ws_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape_space_and_tab() {
        assert_eq!(unescape_proc_mount("/mnt/my\\040dir"), "/mnt/my dir");
        assert_eq!(unescape_proc_mount("/a\\011b"), "/a\tb");
    }

    #[test]
    fn unescape_backslash() {
        assert_eq!(unescape_proc_mount("/path\\134name"), "/path\\name");
    }

    #[test]
    fn unescape_passthrough_plain_ascii() {
        assert_eq!(unescape_proc_mount("/var/lib/ws-ckpt"), "/var/lib/ws-ckpt");
    }

    #[test]
    fn unescape_incomplete_sequence_left_literal() {
        // Trailing `\04` has only 2 digits — not a valid octal triple.
        assert_eq!(unescape_proc_mount("/end\\04"), "/end\\04");
    }

    #[test]
    fn unescape_non_octal_digit_left_literal() {
        // `\089` — '8' is not octal, sequence left untouched.
        assert_eq!(unescape_proc_mount("/x\\089"), "/x\\089");
    }
}
