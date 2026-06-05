//! Owned-file integrity checks.
//!
//! Single concern: given an [`OwnedFile`] from `installed.toml`, report
//! whether the on-disk file still exists and matches the recorded
//! sha256. Used by `anolisa status` to surface tampering / drift without
//! relying on a component-supplied health probe.
//!
//! The check is intentionally minimal — it does not consult the catalog,
//! does not run manifest-declared health hooks, and does not touch any
//! file outside `OwnedFile.path`. Manifest health hooks are deliberately
//! out of scope and report as `skipped` at the call site.
//!
//! Path safety is layered in front of every IO:
//!
//!   * `OwnedFile.path` is re-validated against [`FsLayout`] via
//!     [`crate::path_safety::validate_owned_path`] **before** any `stat` or open.
//!     A forged `installed.toml` claiming `owner = anolisa` for
//!     `/etc/shadow` (or `<bin_dir>/escape -> /etc/shadow`) is therefore
//!     refused with `OutOfBounds` rather than read.
//!   * Symlinks are refused via [`std::fs::symlink_metadata`] +
//!     `O_NOFOLLOW` on the open call so a symlink planted at the
//!     destination cannot redirect the read to a third-party file.
//!   * Special files (directories, fifos, sockets, devices) are refused
//!     via the regular-file guard so `status` cannot hang on a fifo or
//!     mis-hash a directory.
//!
//! All three guards report through dedicated [`IntegrityStatus`] variants
//! so the wire surface tells operators *why* the probe refused, not just
//! that it failed.

use std::fs;

use sha2::{Digest, Sha256};

use anolisa_platform::fs_layout::FsLayout;

use crate::path_safety::{PathBoundaryError, validate_owned_path};
use crate::state::{FileOwner, OwnedFile};

/// Maximum bytes the integrity probe will read for one file. Owned
/// artifacts in ANOLISA's catalogue are binaries / small data files; a
/// 256 MiB ceiling stops a forged `installed.toml` from making `status`
/// stream a multi-gigabyte path. Anything above this returns
/// [`IntegrityStatus::ReadError`] rather than blocking the CLI.
const MAX_PROBE_BYTES: u64 = 256 * 1024 * 1024;

/// Result of a single integrity probe against one [`OwnedFile`].
///
/// Variants are ordered by severity so callers can fold via `max`:
/// `Ok < Skipped < Unverified < OutOfBounds < Symlink < NotRegularFile
/// < MissingFile < ReadError < ShaMismatch`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum IntegrityStatus {
    /// File exists and sha256 matches the recorded value.
    Ok,
    /// Owner is not ANOLISA-managed — we deliberately don't probe.
    Skipped,
    /// File exists but no sha256 was ever recorded — drift cannot be
    /// proved either way, so we degrade rather than claim health.
    Unverified,
    /// Path escapes the ANOLISA-owned roots in the active [`FsLayout`].
    /// Probe is refused without any filesystem touch; this strongly
    /// suggests a forged or corrupted `installed.toml`.
    OutOfBounds,
    /// `OwnedFile.path` is a symlink. The probe refuses to follow it so
    /// a planted symlink cannot redirect the read to a third-party file.
    Symlink,
    /// Path exists but is not a regular file (directory, fifo, socket,
    /// device, etc.). Refused so `status` cannot hang on a fifo or
    /// mis-hash a directory.
    NotRegularFile,
    /// File is gone from disk.
    MissingFile,
    /// File exists but cannot be read (permissions, broken symlink, etc).
    ReadError(String),
    /// File exists, sha256 was recorded, and bytes diverged.
    ShaMismatch {
        /// Lowercase sha256 recorded in `installed.toml`.
        expected: String,
        /// Lowercase sha256 computed from the current on-disk bytes.
        actual: String,
    },
}

impl IntegrityStatus {
    /// Wire-friendly snake_case label for JSON/log output.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Skipped => "skipped",
            Self::Unverified => "unverified",
            Self::OutOfBounds => "out_of_bounds",
            Self::Symlink => "symlink_refused",
            Self::NotRegularFile => "not_regular_file",
            Self::MissingFile => "missing_file",
            Self::ReadError(_) => "read_error",
            Self::ShaMismatch { .. } => "sha256_mismatch",
        }
    }

    /// `true` when the probe found a real integrity problem (vs. ok /
    /// skipped / unverified). Drives status escalation in `status`.
    /// Out-of-bounds / symlink / not-regular-file all count as failures
    /// because they signal either tampering or a corrupted state file —
    /// neither is "merely degraded".
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            Self::OutOfBounds
                | Self::Symlink
                | Self::NotRegularFile
                | Self::MissingFile
                | Self::ReadError(_)
                | Self::ShaMismatch { .. }
        )
    }
}

/// Run the integrity probe on one [`OwnedFile`]. Side-effect free.
///
/// `layout` is the live [`FsLayout`] for the install mode the caller is
/// reporting on. It is consulted before any filesystem IO so a forged
/// `installed.toml` entry pointing outside ANOLISA-owned roots is
/// refused with [`IntegrityStatus::OutOfBounds`] — `status` does not
/// stat, follow, or read that path.
///
/// Returns [`IntegrityStatus::Skipped`] for non-ANOLISA-owned entries so
/// the caller never accidentally hashes a third-party config file. For
/// ANOLISA-owned entries with no recorded sha256 it returns
/// [`IntegrityStatus::Unverified`] rather than `Ok` — the absence of a
/// recorded hash is a degradation signal, not a clean state.
pub fn check_owned_file(layout: &FsLayout, file: &OwnedFile) -> IntegrityStatus {
    if file.owner != FileOwner::Anolisa {
        return IntegrityStatus::Skipped;
    }

    // Path-boundary guard FIRST so a forged path never reaches stat.
    if let Err(err) = validate_owned_path(layout, &file.path) {
        // Traversal and External both surface as out_of_bounds — the
        // wire surface does not need to leak which sub-rule fired, and
        // either way the probe refuses to touch the path.
        let _: PathBoundaryError = err;
        return IntegrityStatus::OutOfBounds;
    }

    // symlink_metadata does NOT follow — required so a planted symlink
    // cannot redirect the read to a third-party file. `exists()` would
    // follow and lie about a broken symlink.
    let meta = match fs::symlink_metadata(&file.path) {
        Ok(m) => m,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return IntegrityStatus::MissingFile;
        }
        Err(err) => return IntegrityStatus::ReadError(err.to_string()),
    };
    if meta.file_type().is_symlink() {
        return IntegrityStatus::Symlink;
    }
    if !meta.is_file() {
        return IntegrityStatus::NotRegularFile;
    }
    if meta.len() > MAX_PROBE_BYTES {
        return IntegrityStatus::ReadError(format!(
            "file size {} exceeds integrity probe ceiling {}",
            meta.len(),
            MAX_PROBE_BYTES
        ));
    }

    let Some(expected) = file.sha256.clone() else {
        return IntegrityStatus::Unverified;
    };
    match hash_file_sha256(&file.path) {
        Err(err) => IntegrityStatus::ReadError(err.to_string()),
        Ok(actual) if actual != expected => IntegrityStatus::ShaMismatch { expected, actual },
        Ok(_) => IntegrityStatus::Ok,
    }
}

#[cfg(unix)]
fn open_nofollow(path: &std::path::Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    // O_NOFOLLOW: the open() syscall refuses to follow a terminal-segment
    // symlink. Combined with the symlink_metadata pre-check above this
    // closes the TOCTOU window where a symlink could be swapped in
    // between stat and open.
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_nofollow(path: &std::path::Path) -> std::io::Result<fs::File> {
    fs::File::open(path)
}

fn hash_file_sha256(path: &std::path::Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = open_nofollow(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > MAX_PROBE_BYTES {
            return Err(std::io::Error::other(format!(
                "file grew past integrity probe ceiling {MAX_PROBE_BYTES} during read"
            )));
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    fn layout_under(prefix: &Path) -> FsLayout {
        let layout = FsLayout::system(Some(prefix.to_path_buf()));
        // Pre-create the bin dir so the canonical-roots check in
        // path_safety has something to canonicalise. Other tests can
        // create extra subdirs as needed.
        fs::create_dir_all(&layout.bin_dir).expect("mkdir bin_dir");
        layout
    }

    fn anolisa_owned(path: PathBuf, sha256: Option<String>) -> OwnedFile {
        OwnedFile {
            path,
            owner: FileOwner::Anolisa,
            sha256,
        }
    }

    #[test]
    fn external_owned_file_is_skipped_without_hashing() {
        // Non-Anolisa owners must short-circuit before any filesystem
        // touch so we never accidentally hash third-party files. The
        // path-safety guard never even runs.
        let tmp = tempdir().expect("tempdir");
        let layout = layout_under(tmp.path());
        let owned = OwnedFile {
            path: PathBuf::from("/definitely/not/here"),
            owner: FileOwner::External,
            sha256: Some("deadbeef".to_string()),
        };
        assert_eq!(check_owned_file(&layout, &owned), IntegrityStatus::Skipped);
    }

    #[test]
    fn path_outside_owned_roots_is_refused_without_stat() {
        // The path-boundary guard must fire BEFORE any filesystem touch.
        // We pick `/etc/shadow` which does exist on most Linux dev hosts
        // — if integrity were to stat it the test would still pass on
        // status grounds, but on macOS the file does not exist and a
        // missing-file fallback would mask the bug. Asserting
        // OutOfBounds proves we did not reach stat.
        let tmp = tempdir().expect("tempdir");
        let layout = layout_under(tmp.path());
        let owned = anolisa_owned(PathBuf::from("/etc/shadow"), Some("deadbeef".to_string()));
        assert_eq!(
            check_owned_file(&layout, &owned),
            IntegrityStatus::OutOfBounds,
        );
    }

    #[test]
    fn traversal_segment_under_a_root_is_refused() {
        // A forged path that lexically starts under bin_dir but contains
        // a `..` must be refused as out_of_bounds — same wire surface as
        // a fully-external path so a forged state file cannot signal
        // anything more specific than "we refused".
        let tmp = tempdir().expect("tempdir");
        let layout = layout_under(tmp.path());
        let path = layout.bin_dir.join("..").join("escape");
        let owned = anolisa_owned(path, Some("deadbeef".to_string()));
        assert_eq!(
            check_owned_file(&layout, &owned),
            IntegrityStatus::OutOfBounds,
        );
    }

    #[test]
    fn missing_file_reports_missing_status() {
        let tmp = tempdir().expect("tempdir");
        let layout = layout_under(tmp.path());
        let owned = anolisa_owned(layout.bin_dir.join("absent"), Some("deadbeef".to_string()));
        assert_eq!(
            check_owned_file(&layout, &owned),
            IntegrityStatus::MissingFile,
        );
    }

    #[test]
    fn file_present_without_recorded_sha_is_unverified() {
        let tmp = tempdir().expect("tempdir");
        let layout = layout_under(tmp.path());
        let path = layout.bin_dir.join("foo");
        fs::write(&path, b"payload").expect("write");
        let owned = anolisa_owned(path, None);
        assert_eq!(
            check_owned_file(&layout, &owned),
            IntegrityStatus::Unverified,
        );
    }

    #[test]
    fn matching_sha_reports_ok() {
        let tmp = tempdir().expect("tempdir");
        let layout = layout_under(tmp.path());
        let path = layout.bin_dir.join("foo");
        fs::write(&path, b"payload").expect("write");
        let owned = anolisa_owned(
            path,
            Some("239f59ed55e737c77147cf55ad0c1b030b6d7ee748a7426952f9b852d5a935e5".to_string()),
        );
        assert_eq!(check_owned_file(&layout, &owned), IntegrityStatus::Ok);
    }

    #[test]
    fn diverged_sha_reports_mismatch_with_both_values() {
        let tmp = tempdir().expect("tempdir");
        let layout = layout_under(tmp.path());
        let path = layout.bin_dir.join("foo");
        fs::write(&path, b"payload").expect("write");
        let expected = "0000000000000000000000000000000000000000000000000000000000000000";
        let owned = anolisa_owned(path, Some(expected.to_string()));

        match check_owned_file(&layout, &owned) {
            IntegrityStatus::ShaMismatch {
                expected: e,
                actual,
            } => {
                assert_eq!(e, expected);
                assert_eq!(
                    actual,
                    "239f59ed55e737c77147cf55ad0c1b030b6d7ee748a7426952f9b852d5a935e5"
                );
            }
            other => panic!("expected ShaMismatch, got {other:?}"),
        }
    }

    #[test]
    #[cfg(unix)]
    fn symlink_under_owned_root_is_refused_without_following() {
        // A symlink planted under bin_dir must NOT be followed even when
        // its target is itself an ANOLISA-owned file. We park the decoy
        // under `datadir/` (also an owned root) so path-safety passes and
        // the symlink-specific guard is what fires. If integrity followed
        // the symlink it would hash the decoy bytes and the assertion
        // would be Ok or ShaMismatch instead.
        let tmp = tempdir().expect("tempdir");
        let layout = layout_under(tmp.path());
        fs::create_dir_all(&layout.datadir).expect("mkdir datadir");
        let target = layout.datadir.join("decoy");
        fs::write(&target, b"decoy-payload").expect("write decoy");
        let link = layout.bin_dir.join("hello");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        let owned = anolisa_owned(link, Some("deadbeef".to_string()));
        assert_eq!(check_owned_file(&layout, &owned), IntegrityStatus::Symlink);
    }

    #[test]
    fn directory_at_owned_path_is_refused() {
        // A directory exists at the expected file path — `status` must
        // refuse rather than try to hash a directory entry.
        let tmp = tempdir().expect("tempdir");
        let layout = layout_under(tmp.path());
        let path = layout.bin_dir.join("hello");
        fs::create_dir_all(&path).expect("mkdir");
        let owned = anolisa_owned(path, Some("deadbeef".to_string()));
        assert_eq!(
            check_owned_file(&layout, &owned),
            IntegrityStatus::NotRegularFile,
        );
    }

    #[test]
    fn label_and_is_failure_match_severity_intent() {
        // Ok / Skipped / Unverified are NOT failures — they don't escalate
        // status past Installed/Degraded respectively.
        assert!(!IntegrityStatus::Ok.is_failure());
        assert!(!IntegrityStatus::Skipped.is_failure());
        assert!(!IntegrityStatus::Unverified.is_failure());
        // All of the path-safety / IO refusals ARE failures.
        assert!(IntegrityStatus::OutOfBounds.is_failure());
        assert!(IntegrityStatus::Symlink.is_failure());
        assert!(IntegrityStatus::NotRegularFile.is_failure());
        assert!(IntegrityStatus::MissingFile.is_failure());
        assert!(IntegrityStatus::ReadError("permission denied".into()).is_failure());
        assert!(
            IntegrityStatus::ShaMismatch {
                expected: "a".into(),
                actual: "b".into()
            }
            .is_failure()
        );
        // Wire labels are stable snake_case.
        assert_eq!(IntegrityStatus::Ok.label(), "ok");
        assert_eq!(IntegrityStatus::Skipped.label(), "skipped");
        assert_eq!(IntegrityStatus::Unverified.label(), "unverified");
        assert_eq!(IntegrityStatus::OutOfBounds.label(), "out_of_bounds");
        assert_eq!(IntegrityStatus::Symlink.label(), "symlink_refused");
        assert_eq!(IntegrityStatus::NotRegularFile.label(), "not_regular_file");
        assert_eq!(IntegrityStatus::MissingFile.label(), "missing_file");
        assert_eq!(IntegrityStatus::ReadError("x".into()).label(), "read_error");
        assert_eq!(
            IntegrityStatus::ShaMismatch {
                expected: "a".into(),
                actual: "b".into()
            }
            .label(),
            "sha256_mismatch"
        );
    }
}
