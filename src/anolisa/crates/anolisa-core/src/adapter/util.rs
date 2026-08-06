//! Pure, side-effect-free helpers shared by the built-in framework
//! drivers.
//!
//! These never spawn a process or mutate the filesystem beyond reading for
//! a digest, so they are safe to call from `plan`/`status`/`prepare` paths.
//! The Cosh/Codex/Claude Code drivers share them here rather than each
//! re-declaring the same digest/timestamp/formatting logic.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::driver::{CliOutput, ConditionStatus, FrameworkCommand};

/// True for Python bytecode caches CPython derives inside an executed
/// tree: `__pycache__/` directories and stray `*.pyc` files.
///
/// Excluded from bundle digests: link-mode adapters (qwencode, codex)
/// execute the resource root in place, so the first hook run writes
/// bytecode caches into the very tree the enable-time digest sealed and
/// flips a healthy adapter to degraded (#2252). Deliberately narrow — the
/// manifest, hook sources, and every other managed file stay digested, so
/// real tampering and same-version content changes remain detectable. The
/// known residual (a planted `.pyc` is loadable yet undigested) is
/// tracked in #2279 together with the broader staleness redesign.
pub(crate) fn is_python_bytecode(path: &Path, is_dir: bool) -> bool {
    if is_dir {
        return path.file_name().is_some_and(|name| name == "__pycache__");
    }
    path.extension().is_some_and(|ext| ext == "pyc")
}

/// Seal prefix written by pre-exclusion releases: all files hashed,
/// bytecode caches included.
const LEGACY_SEAL_PREFIX: &str = "sha256:";
/// Seal prefix for digests computed with bytecode caches excluded. The
/// explicit semantics marker makes every future verdict deterministic: a
/// v2 seal is always compared under exactly the semantics that wrote it.
const SEAL_PREFIX: &str = "sha256/2:";

/// SHA-256 digest of a directory tree, stable across runs: files are hashed
/// in sorted relative-path order as `path\0len\0bytes`. Runtime-derived
/// Python bytecode caches are excluded (see [`is_python_bytecode`]), and
/// the seal carries the [`SEAL_PREFIX`] semantics marker. Returns `None`
/// on any IO error so callers fall back to `Unknown` rather than a wrong
/// verdict.
pub(crate) fn digest_tree(root: &Path) -> Option<String> {
    Some(format!("{SEAL_PREFIX}{}", digest_hex(root, false)?))
}

/// Verdict of comparing an enable-time seal against the tree on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SealVerdict {
    /// The tree provably matches the sealed state.
    Matched,
    /// The tree provably differs from the sealed state.
    Changed,
    /// Legacy seal (bytecode included at seal time) on a tree whose
    /// bytecode caches have changed since: the original cache bytes are
    /// unrecoverable, so runtime cache churn and real drift cannot be told
    /// apart. Callers report Unknown with a re-enable hint — never a false
    /// degrade, never a fake match.
    LegacyUndecidable,
}

/// Compare an enable-time `recorded` seal against `root`.
///
/// A [`SEAL_PREFIX`] seal compares under the bytecode-excluding semantics
/// that wrote it. A legacy seal (bare `sha256:`, written by pre-exclusion
/// releases with bytecode included — notably the #2252 re-enable
/// workaround population) is accepted when the tree reproduces it under
/// either semantics: a digest is a binding commitment to one tree state,
/// so any reproduction proves the tree unchanged. When neither reproduces
/// it and the tree currently carries bytecode caches, the verdict is
/// [`SealVerdict::LegacyUndecidable`]; without bytecode present both
/// semantics hash the same files, so a mismatch is a real change. Returns
/// `None` when the tree cannot be digested.
pub(crate) fn verify_seal(recorded: &str, root: &Path) -> Option<SealVerdict> {
    if let Some(sealed_hex) = recorded.strip_prefix(SEAL_PREFIX) {
        return Some(if digest_hex(root, false)? == sealed_hex {
            SealVerdict::Matched
        } else {
            SealVerdict::Changed
        });
    }
    let Some(sealed_hex) = recorded.strip_prefix(LEGACY_SEAL_PREFIX) else {
        // Unrecognized seal format: it cannot equal any digest this code
        // computes, which is exactly what the pre-marker comparison would
        // have concluded.
        return Some(SealVerdict::Changed);
    };
    let current_hex = digest_hex(root, false)?;
    if current_hex == sealed_hex {
        return Some(SealVerdict::Matched);
    }
    let legacy_hex = digest_hex(root, true)?;
    if legacy_hex == sealed_hex {
        return Some(SealVerdict::Matched);
    }
    Some(if current_hex == legacy_hex {
        SealVerdict::Changed
    } else {
        SealVerdict::LegacyUndecidable
    })
}

/// Hex digest with bytecode caches either excluded (current sealing
/// semantics) or included (pre-#2252 semantics, kept for legacy receipt
/// comparison).
fn digest_hex(root: &Path, include_bytecode: bool) -> Option<String> {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(root, include_bytecode, &mut files).ok()?;
    files.sort();
    let mut hasher = Sha256::new();
    for path in &files {
        let rel = path.strip_prefix(root).unwrap_or(path);
        let bytes = std::fs::read(path).ok()?;
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update([0u8]);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update([0u8]);
        hasher.update(&bytes);
    }
    Some(format!("{:x}", hasher.finalize()))
}

/// Recursively collect regular-file paths under `dir`, skipping Python
/// bytecode caches unless `include_bytecode`. Symlinks are not followed
/// into directories (their link path is recorded as a file).
fn collect_files(
    dir: &Path,
    include_bytecode: bool,
    out: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if !include_bytecode && is_python_bytecode(&path, ft.is_dir()) {
            continue;
        }
        if ft.is_dir() {
            collect_files(&path, include_bytecode, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// ISO 8601 UTC timestamp, second precision.
pub(crate) fn now_iso8601() -> String {
    use chrono::{SecondsFormat, Utc};
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Map a bool to a [`ConditionStatus`] (`true` -> `True`, `false` -> `False`).
pub(crate) fn bool_status(b: bool) -> ConditionStatus {
    if b {
        ConditionStatus::True
    } else {
        ConditionStatus::False
    }
}

/// Compose a failure reason string from a non-success [`CliOutput`].
pub(crate) fn cli_failure_reason(verb: &str, output: &CliOutput) -> String {
    if output.timed_out {
        return format!("'{verb}' timed out");
    }
    let code = output
        .status
        .map(|c| c.to_string())
        .unwrap_or_else(|| "killed".to_string());
    let mut reason = format!("'{verb}' exited with {code}");
    let stderr = output.stderr.trim();
    if !stderr.is_empty() {
        reason.push_str(": ");
        reason.push_str(stderr);
    }
    reason
}

/// Human-readable form of a command for dry-run/preview output. Display
/// only — never parsed back into an argv.
pub(crate) fn display_command(cmd: &FrameworkCommand) -> String {
    let mut s = String::new();
    for (k, v) in &cmd.env_set {
        s.push_str(&format!("{k}={v} "));
    }
    s.push_str(&cmd.program);
    for a in &cmd.args {
        s.push(' ');
        s.push_str(a);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_tree_ignores_python_bytecode_caches() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("hook.py"), b"print('hi')").expect("write");
        let sealed = digest_tree(dir.path()).expect("digest");

        // A hook run derives bytecode caches inside the tree.
        std::fs::create_dir(dir.path().join("__pycache__")).expect("mkdir");
        std::fs::write(
            dir.path().join("__pycache__/hook.cpython-311.pyc"),
            b"bytecode",
        )
        .expect("pyc");
        std::fs::write(dir.path().join("stray.pyc"), b"bytecode").expect("stray pyc");
        assert_eq!(
            digest_tree(dir.path()).as_deref(),
            Some(sealed.as_str()),
            "bytecode caches must not change the digest"
        );

        // Managed files stay digested: real changes are still detected.
        std::fs::write(dir.path().join("hook.py"), b"print('changed')").expect("rewrite");
        assert_ne!(
            digest_tree(dir.path()).as_deref(),
            Some(sealed.as_str()),
            "source changes must still change the digest"
        );
    }

    #[test]
    fn verify_seal_handles_legacy_receipts_and_cache_churn() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pyc = dir.path().join("__pycache__/hook.cpython-311.pyc");
        std::fs::write(dir.path().join("hook.py"), b"print('hi')").expect("write");
        std::fs::create_dir(dir.path().join("__pycache__")).expect("mkdir");
        std::fs::write(&pyc, b"cache A").expect("pyc");

        // A pre-exclusion release sealed this exact tree with bytecode
        // included (the #2252 re-enable workaround population).
        let legacy_sealed = format!(
            "{LEGACY_SEAL_PREFIX}{}",
            digest_hex(dir.path(), true).expect("legacy digest")
        );
        assert_eq!(
            verify_seal(&legacy_sealed, dir.path()),
            Some(SealVerdict::Matched),
            "an unchanged tree must match its pre-exclusion seal after upgrade"
        );

        // A v2 seal of the same tree is deterministic under cache churn.
        let sealed = digest_tree(dir.path()).expect("digest");
        assert!(
            sealed.starts_with(SEAL_PREFIX),
            "new seals carry the marker"
        );
        assert_eq!(verify_seal(&sealed, dir.path()), Some(SealVerdict::Matched));
        std::fs::write(&pyc, b"cache B").expect("regenerate pyc");
        assert_eq!(
            verify_seal(&sealed, dir.path()),
            Some(SealVerdict::Matched),
            "cache churn must not disturb a v2 seal"
        );

        // The legacy seal committed to cache A, which is gone: churn and
        // drift are indistinguishable, so the verdict is undecidable —
        // never a false Changed.
        assert_eq!(
            verify_seal(&legacy_sealed, dir.path()),
            Some(SealVerdict::LegacyUndecidable)
        );

        // A genuine change is Changed under both seal generations once the
        // ambiguity is gone (v2), and stays non-Matched for legacy.
        std::fs::write(dir.path().join("hook.py"), b"print('changed')").expect("rewrite");
        assert_eq!(verify_seal(&sealed, dir.path()), Some(SealVerdict::Changed));
        assert_ne!(
            verify_seal(&legacy_sealed, dir.path()),
            Some(SealVerdict::Matched)
        );

        // Legacy seal on a bytecode-free tree: both semantics hash the
        // same files, so a mismatch is decidably a real change.
        std::fs::remove_file(&pyc).expect("rm pyc");
        std::fs::remove_dir(dir.path().join("__pycache__")).expect("rmdir");
        assert_eq!(
            verify_seal(&legacy_sealed, dir.path()),
            Some(SealVerdict::Changed)
        );
    }
}
