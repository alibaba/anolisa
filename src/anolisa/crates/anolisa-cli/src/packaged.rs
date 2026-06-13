//! Shared "where does the packaged datadir live?" helper.
//!
//! `install-anolisa.sh` (P1-A) lays down the packaged tree at
//! `${ANOLISA_PREFIX}/share/anolisa/`. The CLI needs to find that tree
//! at runtime so commands like `enable agent-observability --dry-run`
//! work without a source tree, an overlay, or matching `--install-mode`
//! to the install prefix.
//!
//! Lookup order (first existing directory wins):
//!
//!   1. `$ANOLISA_DATA_DIR` — explicit caller override. Set by smoke
//!      harnesses that stage anolisa under a tmpdir and need the binary
//!      to ignore the FHS default.
//!   2. `<exe-parent>/../share/anolisa/` — FHS sibling of the binary's
//!      bin/ directory. This is the canonical location after
//!      `install-anolisa.sh`: a binary at `/usr/local/bin/anolisa`
//!      finds its datadir at `/usr/local/share/anolisa/`, regardless of
//!      `--install-mode`.
//!   3. The install-mode default `layout.datadir` — what the
//!      [`FsLayout`] resolution returns for the current
//!      `--install-mode` (system: `/usr/local/share/anolisa`; user:
//!      `~/.local/share/anolisa`). Kept as the final fallback so
//!      pre-P1-A installs (where the datadir matches the install-mode
//!      root directly) still resolve.
//!
//! `cargo run` from the source tree falls through every probe (the
//! debug binary lives under `target/debug/` which has no sibling
//! `share/anolisa/`), at which point the dev-tree fallback in
//! [`crate::commands::common`] takes
//! over. That dev-tree fallback is the reason this helper returns
//! `Option<PathBuf>` rather than panicking.

use std::path::PathBuf;

use anolisa_platform::fs_layout::FsLayout;

/// Name of the env var that overrides the packaged datadir lookup.
pub const DATA_DIR_ENV: &str = "ANOLISA_DATA_DIR";

/// Discover the packaged `share/anolisa/` root for the running binary.
///
/// Returns `None` when none of the three lookup steps point at an
/// existing directory. Callers must fall back to whatever non-packaged
/// source they care about (dev-tree manifests / embedded execution
/// policy) — this helper deliberately does NOT consult those because
/// it lives in a separate concern.
pub fn packaged_datadir_root(layout: &FsLayout) -> Option<PathBuf> {
    if let Some(env_dir) = std::env::var_os(DATA_DIR_ENV) {
        let candidate = PathBuf::from(env_dir);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        // <exe>.parent() == bin/, then .parent() == prefix. Datadir is
        // sibling of bin/ named share/anolisa/.
        if let Some(prefix) = exe.parent().and_then(|p| p.parent()) {
            let candidate = prefix.join("share").join("anolisa");
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }
    if layout.datadir.is_dir() {
        return Some(layout.datadir.clone());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// `ANOLISA_DATA_DIR` takes precedence over every other probe. We
    /// scope the env mutation to this test only — and use a tmpdir we
    /// know exists so the `is_dir()` guard passes deterministically.
    ///
    /// `std::env::set_var` is not thread-safe; we run the env-mutating
    /// tests in a single module-level mutex to avoid racing other
    /// tests that read env vars. Cargo test runs tests in the same
    /// crate concurrently so this matters.
    #[test]
    fn env_override_wins() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempdir().expect("tmp");
        // Layout that points at a non-existent path so we can prove env
        // wins rather than just matching layout.datadir.
        let layout = FsLayout::system(Some(PathBuf::from("/nonexistent-anolisa-prefix")));
        // SAFETY: env mutation guarded by ENV_LOCK.
        unsafe {
            std::env::set_var(DATA_DIR_ENV, tmp.path());
        }
        let got = packaged_datadir_root(&layout);
        unsafe {
            std::env::remove_var(DATA_DIR_ENV);
        }
        assert_eq!(got.as_deref(), Some(tmp.path()));
    }

    /// When `ANOLISA_DATA_DIR` points at a path that does not exist,
    /// we fall through to the next probe instead of returning the
    /// missing path. Pins that the env override is gated on
    /// `is_dir()`, not blindly returned.
    #[test]
    fn env_override_falls_through_when_missing() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let layout = FsLayout::system(Some(PathBuf::from("/nonexistent-anolisa-prefix")));
        unsafe {
            std::env::set_var(DATA_DIR_ENV, "/definitely/does/not/exist/anolisa");
        }
        let got = packaged_datadir_root(&layout);
        unsafe {
            std::env::remove_var(DATA_DIR_ENV);
        }
        // Layout.datadir is also missing, and current_exe() in test
        // builds points at the test runner under target/, whose
        // ../share/anolisa is unlikely to exist on the host — so we
        // assert None instead of pinning a specific fallback.
        assert!(got.is_none(), "expected fallthrough, got {got:?}");
    }

    /// Without env override, an existing layout.datadir wins over a
    /// missing exe-sibling probe. The exe-sibling probe is gated on
    /// `is_dir()`, so we can rely on it failing for a test binary.
    #[test]
    fn layout_datadir_used_when_it_exists() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempdir().expect("tmp");
        let prefix = tmp.path().to_path_buf();
        let layout = FsLayout::system(Some(prefix.clone()));
        // System layout under prefix → datadir = prefix/usr/local/share/anolisa.
        fs::create_dir_all(&layout.datadir).expect("mkdir datadir");
        // Clear env to make sure step 1 falls through.
        unsafe {
            std::env::remove_var(DATA_DIR_ENV);
        }
        let got = packaged_datadir_root(&layout);
        assert_eq!(got.as_deref(), Some(layout.datadir.as_path()));
    }

    /// A serial-execution mutex so the env-mutating tests above can
    /// share a single `set_var` / `remove_var` window without racing.
    /// Plain `Mutex<()>` instead of `OnceLock<Mutex>` because the
    /// tests are few and ordering is not load-bearing.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
