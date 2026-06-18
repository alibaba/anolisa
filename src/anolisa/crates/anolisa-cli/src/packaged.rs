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

/// Crate-wide mutex for tests that mutate `ANOLISA_DATA_DIR`. Cargo runs
/// tests within a crate concurrently, and `ANOLISA_DATA_DIR` is
/// process-global, so every test that sets or reads it must hold this lock.
#[cfg(test)]
pub(crate) static DATA_DIR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// RAII guard that sets `ANOLISA_DATA_DIR` on creation and restores (or
/// removes) the original value on drop — even if the test panics.
#[cfg(test)]
pub(crate) struct DataDirEnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    saved: Option<std::ffi::OsString>,
}

#[cfg(test)]
impl DataDirEnvGuard {
    /// Acquire the env lock, save the current `ANOLISA_DATA_DIR`, and set
    /// the new value.
    pub(crate) fn set(value: &std::path::Path) -> Self {
        let lock = DATA_DIR_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var_os(DATA_DIR_ENV);
        // SAFETY: guarded by DATA_DIR_ENV_LOCK.
        unsafe {
            std::env::set_var(DATA_DIR_ENV, value);
        }
        Self { _lock: lock, saved }
    }

    /// Acquire the env lock and remove `ANOLISA_DATA_DIR` so the test
    /// runs as if no env override is set. The original value is restored
    /// on drop.
    pub(crate) fn clear() -> Self {
        let lock = DATA_DIR_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var_os(DATA_DIR_ENV);
        // SAFETY: guarded by DATA_DIR_ENV_LOCK.
        unsafe {
            std::env::remove_var(DATA_DIR_ENV);
        }
        Self { _lock: lock, saved }
    }
}

#[cfg(test)]
impl Drop for DataDirEnvGuard {
    fn drop(&mut self) {
        // SAFETY: guarded by the lock held in self._lock.
        unsafe {
            match &self.saved {
                Some(v) => std::env::set_var(DATA_DIR_ENV, v),
                None => std::env::remove_var(DATA_DIR_ENV),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// `ANOLISA_DATA_DIR` takes precedence over every other probe.
    #[test]
    fn env_override_wins() {
        let tmp = tempdir().expect("tmp");
        let layout = FsLayout::system(Some(PathBuf::from("/nonexistent-anolisa-prefix")));
        let _guard = DataDirEnvGuard::set(tmp.path());
        let got = packaged_datadir_root(&layout);
        assert_eq!(got.as_deref(), Some(tmp.path()));
    }

    /// When `ANOLISA_DATA_DIR` points at a path that does not exist,
    /// we fall through to the next probe.
    #[test]
    fn env_override_falls_through_when_missing() {
        let layout = FsLayout::system(Some(PathBuf::from("/nonexistent-anolisa-prefix")));
        let _guard =
            DataDirEnvGuard::set(std::path::Path::new("/definitely/does/not/exist/anolisa"));
        let got = packaged_datadir_root(&layout);
        assert!(got.is_none(), "expected fallthrough, got {got:?}");
    }

    /// Without env override, an existing layout.datadir wins over a
    /// missing exe-sibling probe.
    #[test]
    fn layout_datadir_used_when_it_exists() {
        let _guard = DataDirEnvGuard::clear();
        let tmp = tempdir().expect("tmp");
        let prefix = tmp.path().to_path_buf();
        let layout = FsLayout::system(Some(prefix.clone()));
        fs::create_dir_all(&layout.datadir).expect("mkdir datadir");
        let got = packaged_datadir_root(&layout);
        assert_eq!(got.as_deref(), Some(layout.datadir.as_path()));
    }
}
