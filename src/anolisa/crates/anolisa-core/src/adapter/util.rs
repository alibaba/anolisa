//! Pure, side-effect-free helpers shared by the built-in framework
//! drivers.
//!
//! These never spawn a process or mutate the filesystem beyond reading for
//! a digest, so they are safe to call from `plan`/`status`/`prepare` paths.
//! The Cosh/Codex/Claude Code drivers share them here rather than each
//! re-declaring the same digest/timestamp/formatting logic.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::claim::{AdapterClaim, BUNDLE_CHANGED_REASON, BundleMatch, ClaimFile};
use super::driver::{
    AdapterCondition, AdapterConditionKind, CliOutput, ConditionStatus, FrameworkCommand,
};

/// SHA-256 digest of a directory tree, stable across runs: files are hashed
/// in sorted relative-path order as `path\0len\0bytes`. Returns `None` on
/// any IO error so callers fall back to `Unknown` rather than a wrong
/// verdict.
///
/// Kept for receipts written before the per-file registry existed; new
/// receipts compare through [`hash_bundle_files`].
pub(crate) fn digest_tree(root: &Path) -> Option<String> {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(root, &mut files).ok()?;
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
    Some(format!("sha256:{:x}", hasher.finalize()))
}

/// Digest every file under `root` individually, sorted by relative path.
///
/// This is what enable registers and what status re-derives, so the two must
/// walk the tree identically — both go through [`collect_files`]. Returns
/// `None` on any IO error so a partial reading can never be mistaken for a
/// bundle that shrank.
pub(crate) fn hash_bundle_files(root: &Path) -> Option<Vec<ClaimFile>> {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(root, &mut files).ok()?;
    files.sort();
    let mut out = Vec::with_capacity(files.len());
    for path in &files {
        let rel = path.strip_prefix(root).unwrap_or(path);
        let bytes = std::fs::read(path).ok()?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        out.push(ClaimFile {
            path: relative_path_key(rel),
            sha256: format!("sha256:{:x}", hasher.finalize()),
        });
    }
    Some(out)
}

/// Render a relative path as the `/`-separated key used in receipts, so a
/// registry stays comparable regardless of the platform that wrote it.
fn relative_path_key(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Recursively collect regular-file paths under `dir`. Symlinks are not
/// followed into directories (their link path is recorded as a file).
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// Build the `ResourceBundleMatches` condition for a receipt.
///
/// Every driver reports drift the same way, so the verdict, the wording and
/// the unregistered-file disclosure live here instead of being restated per
/// driver — six copies had already drifted into subtly different text.
pub(crate) fn bundle_match_condition(claim: &AdapterClaim) -> AdapterCondition {
    let inspection = claim.inspect_bundle();
    let (status, mut reason) = match inspection.verdict {
        BundleMatch::Matched => (ConditionStatus::True, None),
        BundleMatch::Changed => {
            let mut parts = Vec::new();
            if !inspection.modified.is_empty() {
                parts.push(format!(
                    "modified {}",
                    summarize_paths(&inspection.modified)
                ));
            }
            if !inspection.added.is_empty() {
                parts.push(format!("added {}", summarize_paths(&inspection.added)));
            }
            (
                ConditionStatus::False,
                Some(format!("{BUNDLE_CHANGED_REASON}: {}", parts.join("; "))),
            )
        }
        BundleMatch::Unknown => (
            ConditionStatus::Unknown,
            Some("no digest recorded or resource root unavailable".to_string()),
        ),
    };
    // Bytecode caches stay out of the verdict, but staying silent about them
    // would present "not verified" as "verified": a `.pyc` beside a hook is
    // loadable by the interpreter even though enable never registered it.
    if !inspection.unverified_cache.is_empty() {
        let note = format!(
            "{} unregistered bytecode cache file(s) present, not verified: {}",
            inspection.unverified_cache.len(),
            summarize_paths(&inspection.unverified_cache)
        );
        reason = Some(match reason {
            Some(existing) => format!("{existing}; {note}"),
            None => note,
        });
    }
    AdapterCondition {
        kind: AdapterConditionKind::ResourceBundleMatches,
        status,
        reason,
        resource: None,
    }
}

/// Render up to three paths inline, counting the rest, so a wide drift stays
/// readable in a terminal condition line.
fn summarize_paths(paths: &[String]) -> String {
    const SHOWN: usize = 3;
    let head = paths
        .iter()
        .take(SHOWN)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    match paths.len().saturating_sub(SHOWN) {
        0 => head,
        rest => format!("{head} (+{rest} more)"),
    }
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
    fn digest_tree_is_stable_and_detects_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), b"hello").expect("write");
        std::fs::create_dir(dir.path().join("sub")).expect("mkdir");
        std::fs::write(dir.path().join("sub/b.txt"), b"world").expect("write");

        let d1 = digest_tree(dir.path()).expect("digest");
        let d2 = digest_tree(dir.path()).expect("digest again");
        assert_eq!(d1, d2, "digest must be stable");

        std::fs::write(dir.path().join("sub/b.txt"), b"WORLD").expect("rewrite");
        let d3 = digest_tree(dir.path()).expect("digest after change");
        assert_ne!(d1, d3, "digest must change when a file changes");
    }

    #[test]
    fn hash_bundle_files_registers_each_file_separately() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = dir.path().join("hooks");
        std::fs::create_dir(&hooks).expect("mkdir hooks");
        std::fs::write(hooks.join("pii_text.py"), b"value = 1\n").expect("write hook");
        std::fs::write(dir.path().join("manifest.json"), b"{}").expect("write manifest");

        let registry = hash_bundle_files(dir.path()).expect("registry");
        let paths: Vec<&str> = registry.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["hooks/pii_text.py", "manifest.json"],
            "paths are relative, '/'-separated and sorted"
        );
        assert!(registry.iter().all(|f| f.sha256.starts_with("sha256:")));

        std::fs::write(hooks.join("pii_text.py"), b"value = 2\n").expect("edit hook");
        let after = hash_bundle_files(dir.path()).expect("registry after edit");
        assert_ne!(
            after[0].sha256, registry[0].sha256,
            "an edited file changes only its own entry"
        );
        assert_eq!(
            after[1].sha256, registry[1].sha256,
            "an untouched file keeps its entry"
        );
    }
}
