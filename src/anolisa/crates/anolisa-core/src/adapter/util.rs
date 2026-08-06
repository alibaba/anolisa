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

/// SHA-256 digest of a directory tree, stable across runs: files are hashed
/// in sorted relative-path order as `path\0len\0bytes`. Returns `None` on
/// any IO error so callers fall back to `Unknown` rather than a wrong
/// verdict.
///
/// Every regular file counts, including derived artifacts such as
/// `__pycache__/*.pyc`. Bytecode caches must not be filtered out on the
/// strength of a sibling `.py`: CPython imports a header-valid cache without
/// reading its source, so an excluded `.pyc` would be executable content that
/// no integrity check covers.
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

    /// A minimal adapter bundle: a manifest plus a `hooks/` package, i.e. the
    /// shape sec-core installs into an adapter resource root.
    fn hook_bundle() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("anolisa-adapter.toml"), b"framework = 'x'")
            .expect("write manifest");
        std::fs::create_dir(dir.path().join("hooks")).expect("mkdir hooks");
        std::fs::write(dir.path().join("hooks/hook_config.py"), b"CONFIG = 1\n")
            .expect("write source");
        std::fs::create_dir(dir.path().join("hooks/__pycache__")).expect("mkdir __pycache__");
        dir
    }

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

    /// Every file under a resource root counts, with no carve-out for derived
    /// artifacts. Bytecode in particular must stay covered: CPython imports a
    /// cache whose header `mtime`/`size` match the source without reading the
    /// source at all, so a `.pyc` whose marshalled body was swapped executes
    /// while the `.py` still looks clean. Keeping caches out of the resource
    /// root is `agent-sec-core`'s job (`python3 -B` plus a bounded sweep in
    /// the install/update transaction), not something this digest may assume.
    #[test]
    fn added_and_modified_resource_root_files_change_digest() {
        let dir = hook_bundle();
        let mut digest = digest_tree(dir.path()).expect("digest");

        let cache = dir
            .path()
            .join("hooks/__pycache__/hook_config.cpython-311.pyc");
        let steps: [(&str, &dyn Fn()); 4] = [
            ("bytecode appears", &|| {
                std::fs::write(&cache, b"\xcb\r\r\n\0\0\0\0honest").expect("write cache")
            }),
            ("bytecode body swapped under an intact source", &|| {
                std::fs::write(&cache, b"\xcb\r\r\n\0\0\0\0tampered").expect("tamper cache")
            }),
            ("Python source edited", &|| {
                std::fs::write(dir.path().join("hooks/hook_config.py"), b"CONFIG = 2\n")
                    .expect("rewrite source")
            }),
            ("adapter manifest edited", &|| {
                std::fs::write(dir.path().join("anolisa-adapter.toml"), b"framework = 'y'")
                    .expect("rewrite manifest")
            }),
        ];

        for (what, mutate) in steps {
            mutate();
            let next = digest_tree(dir.path()).expect("digest after change");
            assert_ne!(digest, next, "digest must change when {what}");
            digest = next;
        }
    }
}
