//! Shared helpers for tier1 / tier2 command handlers.
//!
//! Read-only access to the three skeleton-stable objects:
//! [`FsLayout`], [`InstalledState`], and [`Catalog`]. Keep this module thin —
//! handlers compose these calls; we do not introduce a service layer here.

use std::path::{Path, PathBuf};

use anolisa_core::adapter::manager::AdapterManager;
use anolisa_core::{
    Catalog, CatalogLayers, FetchFailure, HttpFetch, InstalledState, ObjectStatus, UreqFetch,
};
use anolisa_platform::fs_layout::FsLayout;

use crate::context::{CliContext, InstallMode};
use crate::packaged;
use crate::repo_config::{HostVars, RepoConfig, raw_relative_root};
use crate::response::CliError;

/// Subdirectory under `datadir` and `etc_dir` where component
/// manifests live (e.g. `share/anolisa/manifests`, `etc/anolisa/manifests`).
const MANIFESTS_SUBDIR: &str = "manifests";
/// State subdirectory where install stores the exact component contract
/// used for each installed component.
const INSTALLED_COMPONENT_MANIFESTS_SUBDIR: &str = "component-manifests";
/// Filename used for the locally persisted installed component contract.
const INSTALLED_COMPONENT_MANIFEST_FILE: &str = "component.toml";

/// Build the layout for the active install mode, honoring `--prefix`
/// (system-mode) and resolving `$HOME` via `EnvService::detect` (user-mode).
pub fn resolve_layout(ctx: &CliContext) -> FsLayout {
    match ctx.install_mode {
        InstallMode::System => FsLayout::system(ctx.prefix.clone()),
        InstallMode::User => {
            let home = anolisa_env::EnvService::detect().home;
            FsLayout::user(home)
        }
    }
}

/// Load `InstalledState` from the layout's `state_dir/installed.toml`.
/// A missing file yields `Default` — fresh installs are not an error.
pub fn load_installed_state(ctx: &CliContext, command: &str) -> Result<InstalledState, CliError> {
    let layout = resolve_layout(ctx);
    let path = layout.state_dir.join("installed.toml");
    InstalledState::load(&path).map_err(|err| CliError::InvalidArgument {
        command: command.to_string(),
        reason: format!(
            "failed to load installed state at {}: {err}",
            path.display()
        ),
    })
}

/// Path for the component manifest saved as part of an installed component's
/// local state.
pub fn installed_component_manifest_path(
    layout: &FsLayout,
    component: &str,
    command: &str,
) -> Result<PathBuf, CliError> {
    validate_component_path_segment(component, command)?;
    Ok(layout
        .state_dir
        .join(INSTALLED_COMPONENT_MANIFESTS_SUBDIR)
        .join(component)
        .join(INSTALLED_COMPONENT_MANIFEST_FILE))
}

fn validate_component_path_segment(component: &str, command: &str) -> Result<(), CliError> {
    if component.trim().is_empty()
        || component == "."
        || component == ".."
        || component.contains('/')
        || component.contains('\\')
    {
        return Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!("component name '{component}' cannot be used as a local path segment"),
        });
    }
    Ok(())
}

/// Load the layered catalog.
///
/// Layers (low → high precedence):
///   1. **bundled** — packaged manifests under `datadir/manifests` (the
///      install-time location). Falls back to the dev-tree manifests
///      (`CARGO_MANIFEST_DIR/../../manifests`) when the packaged location is
///      absent so `cargo run` in the source tree works without an install.
///   2. **overlay** — `manifests_overlay` (e.g. `/etc/anolisa/manifests` or
///      `~/.config/anolisa/manifests`) attached as the `system` or `user`
///      layer per `ctx.install_mode`. Optional: skipped when the directory
///      does not exist.
///
/// The overlay used to be passed as `bundled` with no system/user layers —
/// that meant any overlay completely replaced the in-tree catalog (and an
/// empty overlay produced an empty catalog). The proper Catalog contract is
/// that the bundled layer is always-present and overlays stack on top.
pub fn load_bundled_catalog(ctx: &CliContext, command: &str) -> Result<Catalog, CliError> {
    let layout = resolve_layout(ctx);
    let bundled = packaged_manifests_root(&layout)
        .or_else(dev_tree_manifests)
        .unwrap_or_else(|| layout.datadir.join(MANIFESTS_SUBDIR));

    let overlay = layout.manifests_overlay.clone();
    let overlay = overlay.is_dir().then_some(overlay);
    let (system, user) = match ctx.install_mode {
        InstallMode::System => (overlay, None),
        InstallMode::User => (None, overlay),
    };

    let layers = CatalogLayers {
        bundled,
        system,
        user,
    };
    Catalog::load(layers).map_err(|err| CliError::InvalidArgument {
        command: command.to_string(),
        reason: format!("failed to load catalog: {err}"),
    })
}

fn packaged_manifests_root(layout: &FsLayout) -> Option<PathBuf> {
    // Discover the packaged datadir (`<prefix>/share/anolisa/`) using
    // the shared probe in [`crate::packaged`] — that helper honors the
    // `ANOLISA_DATA_DIR` env override and binary-location lookup so a
    // user-mode CLI still finds the system-installed datadir under
    // `/usr/local/share/anolisa/` when one is staged by
    // `install-anolisa.sh`. Falls back to `layout.datadir` for the
    // pre-P1-A install layout.
    let datadir = packaged::packaged_datadir_root(layout).unwrap_or_else(|| layout.datadir.clone());
    let candidate = datadir.join(MANIFESTS_SUBDIR);
    candidate.is_dir().then_some(candidate)
}

fn dev_tree_manifests() -> Option<PathBuf> {
    let candidate = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("manifests");
    candidate.is_dir().then_some(candidate)
}

// ── Component catalog URL resolution ────────────────────────────────

/// Resolve the component catalog URL.
///
/// Resolution order (first non-empty wins):
///   1. `$ANOLISA_CATALOG_URL` env var
///   2. `[backends.raw].base_url` in `repo.toml`, plus `/catalog.json`
///
/// `repo.toml` follows its own discovery chain, including the embedded
/// default, so upgraded hosts do not need a pre-existing local config file.
pub fn resolve_catalog_url(ctx: &CliContext, command: &str) -> Result<Option<String>, CliError> {
    if let Ok(url) = std::env::var("ANOLISA_CATALOG_URL") {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return Ok(Some(trimmed.to_string()));
        }
    }

    let layout = resolve_layout(ctx);
    let repo_config = RepoConfig::load(&layout).map_err(|err| CliError::InvalidArgument {
        command: command.to_string(),
        reason: format!("failed to resolve component catalog from repo.toml: {err}"),
    })?;
    let (backend_name, backend) =
        repo_config
            .select_backend(Some("raw"))
            .map_err(|err| CliError::InvalidArgument {
                command: command.to_string(),
                reason: format!("failed to resolve component catalog from repo.toml: {err}"),
            })?;
    let env = anolisa_env::EnvService::detect();
    let host = HostVars {
        os: env.os,
        arch: env.arch,
    };
    let base_url = repo_config
        .resolved_base_url(backend_name, backend, &host)
        .map_err(|err| CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!("failed to resolve component catalog from repo.toml: {err}"),
        })?;
    Ok(Some(format!(
        "{}/catalog.json",
        raw_relative_root(&base_url)
    )))
}

/// Fetch raw bytes from a catalog URL.
///
/// Supports `file://` (local filesystem) and `http(s)://` (via `UreqFetch`).
pub fn fetch_catalog_bytes(url: &str, command: &str) -> Result<Vec<u8>, CliError> {
    if let Some(path) = url.strip_prefix("file://") {
        return std::fs::read(path).map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!("failed to read catalog from {url}: {err}"),
        });
    }

    if url.starts_with("http://") || url.starts_with("https://") {
        return UreqFetch::default()
            .get(url)
            .map_err(|err: FetchFailure| CliError::Runtime {
                command: command.to_string(),
                reason: format!("failed to fetch catalog from {url}: {err}"),
            });
    }

    Err(CliError::InvalidArgument {
        command: command.to_string(),
        reason: format!("unsupported catalog URL scheme: {url}"),
    })
}

/// Wire-friendly label for an [`ObjectStatus`] value. Shared between the
/// `status` and `list` handlers so both surfaces speak the same vocabulary
/// (matches launch spec §7.1: `installed | degraded | disabled | failed |
/// adopted`). The `"not_installed"` label is produced separately by callers
/// when no `InstalledObject` exists at all.
pub(crate) fn object_status_str(status: ObjectStatus) -> &'static str {
    match status {
        ObjectStatus::Installed => "installed",
        ObjectStatus::Partial => "degraded",
        ObjectStatus::Disabled => "disabled",
        ObjectStatus::Failed => "failed",
        ObjectStatus::Adopted => "adopted",
    }
}

/// True iff the wire status label denotes a component that is actively
/// serving (i.e. `installed`, `degraded`, or `adopted`). Used by
/// `list --enabled` to exclude `disabled`/`failed`/`not_installed`.
pub(crate) fn status_is_enabled(status_label: &str) -> bool {
    matches!(status_label, "installed" | "degraded" | "adopted")
}

/// Build an [`AdapterManager`] for the active layout, shared between
/// `adapter` and `status` handlers.
pub(crate) fn build_adapter_manager(ctx: &CliContext) -> AdapterManager {
    use anolisa_core::adapter::manager::VisibleRoot;

    let layout = resolve_layout(ctx);
    let env = anolisa_env::EnvService::detect();
    let mut manager = AdapterManager::new(layout.clone(), Some(env.home), env.user);

    if ctx.install_mode == InstallMode::User {
        // In user mode, the primary visible root is the user layout (set
        // by `new`).  Add a system visible root so user-mode CLI can
        // enable adapters for system-installed components.  The system
        // root pairs with the system datadir + packaged datadir, which
        // may differ (RPM uses /usr/share, CLI installs to /usr/local/share).
        let system_layout = FsLayout::system(ctx.prefix.clone());
        let mut system_datadirs = vec![system_layout.datadir.clone()];
        if let Some(packaged) = packaged::packaged_datadir_root(&system_layout)
            && !system_datadirs.contains(&packaged)
        {
            system_datadirs.push(packaged);
        }
        manager.push_visible_root(VisibleRoot {
            state_dir: system_layout.state_dir,
            contract_datadir_roots: system_datadirs,
        });
    } else {
        // In system mode, the primary visible root uses `layout.datadir`.
        // If the packaged datadir differs (exe-sibling vs install prefix),
        // add it to the primary root's contract datadirs so RPM-installed
        // contracts at /usr/share/... are found.
        if let Some(packaged) = packaged::packaged_datadir_root(&layout)
            && packaged != layout.datadir
        {
            manager.push_primary_datadir_root(packaged);
        }
    }

    manager
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `object_status_str` must cover every variant of `ObjectStatus` and
    /// produce the exact wire vocabulary the spec promises. If a new variant
    /// is added, this test forces us to extend the mapping.
    #[test]
    fn object_status_str_covers_full_vocabulary() {
        assert_eq!(object_status_str(ObjectStatus::Installed), "installed");
        assert_eq!(object_status_str(ObjectStatus::Partial), "degraded");
        assert_eq!(object_status_str(ObjectStatus::Disabled), "disabled");
        assert_eq!(object_status_str(ObjectStatus::Failed), "failed");
        assert_eq!(object_status_str(ObjectStatus::Adopted), "adopted");
    }

    #[test]
    fn status_is_enabled_excludes_disabled_failed_and_unknown() {
        assert!(status_is_enabled("installed"));
        assert!(status_is_enabled("degraded"));
        assert!(status_is_enabled("adopted"));
        assert!(!status_is_enabled("disabled"));
        assert!(!status_is_enabled("failed"));
        assert!(!status_is_enabled("not_installed"));
        assert!(!status_is_enabled(""));
    }
}
