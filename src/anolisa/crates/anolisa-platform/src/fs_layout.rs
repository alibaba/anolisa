//! Filesystem layout resolution for user-mode vs system-mode installs.
//!
//! `system-mode` strictly follows [FHS 3.0](https://refspecs.linuxfoundation.org/FHS_3.0/fhs/index.html)
//! (binaries under `/usr/local/bin`, state under `/var/lib/anolisa`, etc.);
//! `user-mode` strictly follows the [XDG Base Directory
//! Specification](https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html)
//! (`$XDG_DATA_HOME/anolisa` and friends). A custom prefix (typically
//! `/opt/<name>`) may be supplied in system-mode to relocate the whole tree.
//!
//! Source of truth: `docs/anolisa/anolisa-cli-design.md` §"Filesystem Layout"
//! (Path Mapping table) and `docs/anolisa/anolisa-cli-launch-spec.md` §8.5
//! (Lock).
//!
//! The [`InstallMode`] enum here mirrors `anolisa_cli::context::InstallMode`
//! but lives in this crate to keep `anolisa-platform` independent of the
//! CLI; the CLI layer converts between the two at the boundary.

use std::path::{Path, PathBuf};

/// Application namespace folder appended to most FHS/XDG roots.
const NS: &str = "anolisa";
/// Audit-log file name written under `log_dir`.
const CENTRAL_LOG_NAME: &str = "central.jsonl";
/// Lock file name written under `state_dir`.
const LOCK_NAME: &str = "lock";
/// Catalog overlay folder name appended to the configuration root.
const OVERLAY_NAME: &str = "manifests";
/// Backup folder name appended to the state root.
const BACKUPS_NAME: &str = "backups";

// ---- System-mode (FHS) defaults -----------------------------------------

/// Default `/usr/local` prefix tree — all system-mode paths derive from
/// these literals and are rebased under [`FsLayout::system`]'s `prefix`.
mod fhs {
    pub const BIN: &str = "/usr/local/bin";
    pub const LIB: &str = "/usr/local/lib/anolisa";
    pub const LIBEXEC: &str = "/usr/local/libexec/anolisa";
    pub const DATADIR: &str = "/usr/local/share/anolisa";
    pub const ETC: &str = "/etc/anolisa";
    pub const STATE: &str = "/var/lib/anolisa";
    pub const CACHE: &str = "/var/cache/anolisa";
    pub const LOG: &str = "/var/log/anolisa";
    pub const RUNTIME: &str = "/run/anolisa";
    pub const SYSTEMD_UNITS: &str = "/etc/systemd/system";
}

// ---- User-mode (XDG) leaf names -----------------------------------------

/// Component-relative folders appended under the XDG roots for user-mode.
mod xdg {
    /// `~/.local/bin` is the de-facto XDG_BIN_HOME, kept independent of
    /// `$XDG_DATA_HOME` per design.md L514.
    pub const HOME_LOCAL_BIN: &str = ".local/bin";
    pub const DATA_HOME: &str = ".local/share";
    pub const CONFIG_HOME: &str = ".config";
    pub const STATE_HOME: &str = ".local/state";
    pub const CACHE_HOME: &str = ".cache";

    /// Sub-folders inside the per-user `$XDG_DATA_HOME/anolisa` tree.
    pub const LIB_SUB: &str = "lib";
    pub const LIBEXEC_SUB: &str = "libexec";
    /// Sub-folder inside `$XDG_STATE_HOME/anolisa` used as the runtime
    /// fallback when `$XDG_RUNTIME_DIR` is unset.
    pub const RUNTIME_FALLBACK_SUB: &str = "runtime";
    /// User-mode systemd unit search dir, relative to `$XDG_CONFIG_HOME`.
    pub const SYSTEMD_USER_SUB: &str = "systemd/user";
}

/// Where ANOLISA installs files: user-mode (XDG under `$HOME`) or
/// system-mode (FHS under `/`, redirectable via a prefix).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    /// FHS-style installation under `/usr/local`, `/etc`, and `/var`.
    System,
    /// Per-user installation under XDG roots derived from `$HOME`.
    User,
}

/// Resolved filesystem paths for a given install mode.
///
/// All fields are absolute. `lock_file` and `central_log` are
/// individual file paths; everything else is a directory.
///
/// Note on `prefix`: in system-mode it is the install root that rebases
/// every other path. In user-mode it is **not** a single root; it is kept
/// for compatibility and points at the primary install root (i.e.
/// `datadir`, `$XDG_DATA_HOME/anolisa`). User-mode honors the four XDG
/// roots independently — `bin_dir`, `etc_dir`, `state_dir`, `cache_dir`
/// each derive from a different XDG variable and are not children of
/// `prefix`.
#[derive(Debug, Clone)]
pub struct FsLayout {
    /// Install scope that selected the path policy.
    pub mode: InstallMode,
    /// Primary install root; in user mode this is the data root, not a
    /// parent of every other XDG-derived path.
    pub prefix: PathBuf,
    /// Directory where user-invoked binaries are linked or copied.
    pub bin_dir: PathBuf,
    /// Directory for shared runtime libraries owned by ANOLISA.
    pub lib_dir: PathBuf,
    /// Directory for helper executables not intended as direct commands.
    pub libexec_dir: PathBuf,
    /// Shared read-only data root (skills, adapters, packaged catalogs).
    pub datadir: PathBuf,
    /// Configuration root.
    pub etc_dir: PathBuf,
    /// State root — `installed-state.toml` lives here.
    pub state_dir: PathBuf,
    /// Cache root for downloaded artifacts and reusable probe results.
    pub cache_dir: PathBuf,
    /// Log root; [`Self::central_log`] is placed inside it.
    pub log_dir: PathBuf,
    /// Backup root used by transactional uninstall/rollback paths.
    pub backup_dir: PathBuf,
    /// Advisory install lock file shared by mutating operations.
    pub lock_file: PathBuf,
    /// Central JSONL log file consumed by `anolisa logs`.
    pub central_log: PathBuf,
    /// Volatile runtime root for sockets / PID files.
    pub runtime_dir: PathBuf,
    /// Catalog overlay directory for this install mode.
    pub manifests_overlay: PathBuf,
    /// Distribution-specific systemd unit search directory.
    pub systemd_unit_dir: PathBuf,
}

impl FsLayout {
    /// System (FHS) install layout. `prefix` defaults to `/`.
    ///
    /// When a non-`/` prefix is supplied, every default absolute path
    /// below is rebased under it (so `prefix=/opt/x` yields
    /// `/opt/x/usr/local/bin`, `/opt/x/etc/anolisa`, etc.).
    pub fn system(prefix: Option<PathBuf>) -> Self {
        let prefix = prefix.unwrap_or_else(|| PathBuf::from("/"));
        let rebase = |p: &str| rebase_under(&prefix, p);

        let state_dir = rebase(fhs::STATE);
        let log_dir = rebase(fhs::LOG);
        let backup_dir = state_dir.join(BACKUPS_NAME);
        let lock_file = state_dir.join(LOCK_NAME);
        let central_log = log_dir.join(CENTRAL_LOG_NAME);
        let etc_dir = rebase(fhs::ETC);
        let manifests_overlay = etc_dir.join(OVERLAY_NAME);

        Self {
            mode: InstallMode::System,
            bin_dir: rebase(fhs::BIN),
            lib_dir: rebase(fhs::LIB),
            libexec_dir: rebase(fhs::LIBEXEC),
            datadir: rebase(fhs::DATADIR),
            etc_dir,
            state_dir,
            cache_dir: rebase(fhs::CACHE),
            log_dir,
            backup_dir,
            lock_file,
            central_log,
            runtime_dir: rebase(fhs::RUNTIME),
            manifests_overlay,
            systemd_unit_dir: rebase(fhs::SYSTEMD_UNITS),
            prefix,
        }
    }

    /// User (XDG) install layout under `home`. XDG environment
    /// variables, when set, take precedence over the `$HOME`-based
    /// defaults.
    pub fn user(home: PathBuf) -> Self {
        let xdg_data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
        let xdg_config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        let xdg_state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from);
        let xdg_cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from);
        let xdg_runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
        Self::user_with_xdg(
            home,
            xdg_data,
            xdg_config,
            xdg_state,
            xdg_cache,
            xdg_runtime,
        )
    }

    /// Test-friendly variant of [`Self::user`] that takes the XDG
    /// directories explicitly instead of reading them from the process
    /// environment.
    ///
    /// Each `Option` corresponds to one XDG variable; `None` selects the
    /// `$HOME`-based fallback documented by the XDG spec.
    pub(crate) fn user_with_xdg(
        home: PathBuf,
        xdg_data: Option<PathBuf>,
        xdg_config: Option<PathBuf>,
        xdg_state: Option<PathBuf>,
        xdg_cache: Option<PathBuf>,
        xdg_runtime: Option<PathBuf>,
    ) -> Self {
        let data = xdg_data.unwrap_or_else(|| home.join(xdg::DATA_HOME));
        let config = xdg_config.unwrap_or_else(|| home.join(xdg::CONFIG_HOME));
        let state = xdg_state.unwrap_or_else(|| home.join(xdg::STATE_HOME));
        let cache = xdg_cache.unwrap_or_else(|| home.join(xdg::CACHE_HOME));

        let datadir = data.join(NS);
        let etc_dir = config.join(NS);
        let state_dir = state.join(NS);
        let cache_dir = cache.join(NS);

        // bin lives at the user's de-facto XDG_BIN_HOME, independent of
        // any of the other XDG roots — see design.md L514 and L530.
        let bin_dir = home.join(xdg::HOME_LOCAL_BIN);

        let lib_dir = datadir.join(xdg::LIB_SUB);
        let libexec_dir = datadir.join(xdg::LIBEXEC_SUB);

        // Logs / lock / backups live under XDG_STATE_HOME so a cache wipe
        // does not destroy audit history — see design.md L519 + L535 and
        // launch-spec §8.5.
        let log_dir = state_dir.clone();
        let backup_dir = state_dir.join(BACKUPS_NAME);
        let lock_file = state_dir.join(LOCK_NAME);
        let central_log = state_dir.join(CENTRAL_LOG_NAME);

        // XDG_RUNTIME_DIR is not guaranteed to be set (e.g. headless
        // installs); fall back to a subdir under state_dir per design.md
        // L536 so socket/pid paths still resolve.
        let runtime_dir = xdg_runtime
            .map(|r| r.join(NS))
            .unwrap_or_else(|| state_dir.join(xdg::RUNTIME_FALLBACK_SUB));

        let manifests_overlay = etc_dir.join(OVERLAY_NAME);
        let systemd_unit_dir = config.join(xdg::SYSTEMD_USER_SUB);

        Self {
            mode: InstallMode::User,
            // "primary install root" for compatibility — points at the
            // shared data root, not at a single rebase prefix.
            prefix: datadir.clone(),
            bin_dir,
            lib_dir,
            libexec_dir,
            datadir,
            etc_dir,
            state_dir,
            cache_dir,
            log_dir,
            backup_dir,
            lock_file,
            central_log,
            runtime_dir,
            manifests_overlay,
            systemd_unit_dir,
        }
    }
}

/// Join `path` under `prefix`, stripping the leading `/` so that
/// `Path::join` does not discard the prefix.
fn rebase_under(prefix: &Path, path: &str) -> PathBuf {
    if prefix == Path::new("/") {
        return PathBuf::from(path);
    }
    let stripped = path.strip_prefix('/').unwrap_or(path);
    prefix.join(stripped)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- system-mode --------------------------------------------------

    #[test]
    fn system_default_uses_fhs_root() {
        let layout = FsLayout::system(None);
        assert_eq!(layout.mode, InstallMode::System);
        assert_eq!(layout.prefix, PathBuf::from("/"));
        assert_eq!(layout.bin_dir, PathBuf::from("/usr/local/bin"));
        assert_eq!(layout.lib_dir, PathBuf::from("/usr/local/lib/anolisa"));
        assert_eq!(
            layout.libexec_dir,
            PathBuf::from("/usr/local/libexec/anolisa")
        );
        assert_eq!(layout.datadir, PathBuf::from("/usr/local/share/anolisa"));
        assert_eq!(layout.etc_dir, PathBuf::from("/etc/anolisa"));
        assert_eq!(layout.state_dir, PathBuf::from("/var/lib/anolisa"));
        assert_eq!(layout.cache_dir, PathBuf::from("/var/cache/anolisa"));
        assert_eq!(layout.log_dir, PathBuf::from("/var/log/anolisa"));
        assert_eq!(layout.backup_dir, PathBuf::from("/var/lib/anolisa/backups"));
        assert_eq!(layout.runtime_dir, PathBuf::from("/run/anolisa"));
        assert_eq!(
            layout.systemd_unit_dir,
            PathBuf::from("/etc/systemd/system")
        );
        assert_eq!(
            layout.central_log,
            PathBuf::from("/var/log/anolisa/central.jsonl")
        );
        assert_eq!(
            layout.manifests_overlay,
            PathBuf::from("/etc/anolisa/manifests")
        );
    }

    #[test]
    fn system_default_lock_under_var_lib() {
        // launch-spec §8.5: system-mode lock lives at
        // /var/lib/anolisa/lock, NOT under /run.
        let layout = FsLayout::system(None);
        assert_eq!(layout.lock_file, PathBuf::from("/var/lib/anolisa/lock"));
    }

    #[test]
    fn system_default_bin_is_usr_local_bin() {
        // design.md L514/L530: system-mode binaries install under
        // /usr/local/bin, NOT /usr/bin.
        let layout = FsLayout::system(None);
        assert_eq!(layout.bin_dir, PathBuf::from("/usr/local/bin"));
    }

    #[test]
    fn system_custom_prefix_rebases_paths() {
        let layout = FsLayout::system(Some(PathBuf::from("/opt/x")));
        assert_eq!(layout.bin_dir, PathBuf::from("/opt/x/usr/local/bin"));
        assert_eq!(layout.etc_dir, PathBuf::from("/opt/x/etc/anolisa"));
        assert_eq!(
            layout.lock_file,
            PathBuf::from("/opt/x/var/lib/anolisa/lock")
        );
        assert_eq!(
            layout.central_log,
            PathBuf::from("/opt/x/var/log/anolisa/central.jsonl")
        );
        assert_eq!(layout.runtime_dir, PathBuf::from("/opt/x/run/anolisa"));
        assert_eq!(
            layout.systemd_unit_dir,
            PathBuf::from("/opt/x/etc/systemd/system")
        );
    }

    // ---- user-mode ----------------------------------------------------

    fn user_no_xdg(home: &str) -> FsLayout {
        // env-free helper so parallel tests in other crates can't race
        // us by mutating XDG_* in their own processes.
        FsLayout::user_with_xdg(PathBuf::from(home), None, None, None, None, None)
    }

    #[test]
    fn user_layout_under_home_with_no_xdg() {
        let layout = user_no_xdg("/tmp/h");
        assert_eq!(layout.mode, InstallMode::User);
        assert_eq!(layout.prefix, PathBuf::from("/tmp/h/.local/share/anolisa"));
        assert_eq!(layout.datadir, PathBuf::from("/tmp/h/.local/share/anolisa"));
        assert_eq!(layout.bin_dir, PathBuf::from("/tmp/h/.local/bin"));
        assert_eq!(
            layout.lib_dir,
            PathBuf::from("/tmp/h/.local/share/anolisa/lib")
        );
        assert_eq!(
            layout.libexec_dir,
            PathBuf::from("/tmp/h/.local/share/anolisa/libexec")
        );
        assert_eq!(layout.etc_dir, PathBuf::from("/tmp/h/.config/anolisa"));
        assert_eq!(
            layout.state_dir,
            PathBuf::from("/tmp/h/.local/state/anolisa")
        );
        assert_eq!(layout.cache_dir, PathBuf::from("/tmp/h/.cache/anolisa"));
        assert_eq!(layout.log_dir, PathBuf::from("/tmp/h/.local/state/anolisa"));
        assert_eq!(
            layout.backup_dir,
            PathBuf::from("/tmp/h/.local/state/anolisa/backups")
        );
        assert_eq!(
            layout.lock_file,
            PathBuf::from("/tmp/h/.local/state/anolisa/lock")
        );
        assert_eq!(
            layout.central_log,
            PathBuf::from("/tmp/h/.local/state/anolisa/central.jsonl")
        );
        assert_eq!(
            layout.manifests_overlay,
            PathBuf::from("/tmp/h/.config/anolisa/manifests")
        );
        assert_eq!(
            layout.systemd_unit_dir,
            PathBuf::from("/tmp/h/.config/systemd/user")
        );
    }

    #[test]
    fn user_layout_honors_explicit_xdg_dirs() {
        let layout = FsLayout::user_with_xdg(
            PathBuf::from("/tmp/h"),
            Some(PathBuf::from("/data")),
            Some(PathBuf::from("/conf")),
            Some(PathBuf::from("/state")),
            Some(PathBuf::from("/cache")),
            Some(PathBuf::from("/run/user/1000")),
        );
        assert_eq!(layout.prefix, PathBuf::from("/data/anolisa"));
        assert_eq!(layout.datadir, PathBuf::from("/data/anolisa"));
        assert_eq!(layout.etc_dir, PathBuf::from("/conf/anolisa"));
        assert_eq!(layout.state_dir, PathBuf::from("/state/anolisa"));
        assert_eq!(layout.cache_dir, PathBuf::from("/cache/anolisa"));
        assert_eq!(layout.log_dir, PathBuf::from("/state/anolisa"));
        assert_eq!(layout.lock_file, PathBuf::from("/state/anolisa/lock"));
        assert_eq!(layout.runtime_dir, PathBuf::from("/run/user/1000/anolisa"));
        assert_eq!(layout.systemd_unit_dir, PathBuf::from("/conf/systemd/user"));
        // bin is HOME-rooted regardless of XDG_DATA_HOME — see next test.
        assert_eq!(layout.bin_dir, PathBuf::from("/tmp/h/.local/bin"));
    }

    #[test]
    fn user_log_under_xdg_state() {
        // design.md L519/L535: audit log lives under $XDG_STATE_HOME so
        // it survives a cache wipe.
        let layout = user_no_xdg("/tmp/h");
        assert_eq!(layout.log_dir, layout.state_dir);
        assert_ne!(layout.log_dir, layout.cache_dir);
        let with_xdg = FsLayout::user_with_xdg(
            PathBuf::from("/tmp/h"),
            None,
            None,
            Some(PathBuf::from("/state")),
            Some(PathBuf::from("/cache")),
            None,
        );
        assert_eq!(with_xdg.log_dir, PathBuf::from("/state/anolisa"));
        assert_ne!(with_xdg.log_dir, with_xdg.cache_dir);
    }

    #[test]
    fn user_bin_is_local_bin() {
        // bin must be HOME/.local/bin regardless of XDG_DATA_HOME — see
        // design.md L514/L530.
        let layout = FsLayout::user_with_xdg(
            PathBuf::from("/tmp/h"),
            Some(PathBuf::from("/somewhere/else/data")),
            None,
            None,
            None,
            None,
        );
        assert_eq!(layout.bin_dir, PathBuf::from("/tmp/h/.local/bin"));
    }

    #[test]
    fn user_runtime_falls_back_when_xdg_runtime_unset() {
        // design.md L536 / runtime_dir fallback path.
        let layout = FsLayout::user_with_xdg(
            PathBuf::from("/tmp/h"),
            None,
            None,
            Some(PathBuf::from("/state")),
            None,
            None,
        );
        assert_eq!(layout.runtime_dir, PathBuf::from("/state/anolisa/runtime"));
        // ...and when XDG_RUNTIME_DIR is set, it wins.
        let with_runtime = FsLayout::user_with_xdg(
            PathBuf::from("/tmp/h"),
            None,
            None,
            Some(PathBuf::from("/state")),
            None,
            Some(PathBuf::from("/run/user/42")),
        );
        assert_eq!(
            with_runtime.runtime_dir,
            PathBuf::from("/run/user/42/anolisa")
        );
    }
}
