//! Shared helpers for tier1 / tier2 command handlers.
//!
//! Read-only access to the three skeleton-stable objects:
//! [`FsLayout`], [`InstalledState`], and [`Catalog`]. Keep this module thin —
//! handlers compose these calls; we do not introduce a service layer here.

use std::path::{Path, PathBuf};

use anolisa_core::{
    Catalog, CatalogLayers, DistributionIndex, IndexFreshness, InstalledState, ObjectStatus,
    RegistryClient, RegistryConfig,
};
use anolisa_platform::fs_layout::FsLayout;

use crate::context::{CliContext, InstallMode};
use crate::packaged;
use crate::response::CliError;

/// Subdirectory under `datadir` and `etc_dir` where capability/component
/// manifests live (e.g. `share/anolisa/manifests`, `etc/anolisa/manifests`).
const MANIFESTS_SUBDIR: &str = "manifests";

/// Subdirectory under `manifests/` that holds DistributionIndex files.
const DIST_INDEX_SUBDIR: &str = "distribution-index";

/// Default file name for the bundled DistributionIndex.
const DIST_INDEX_FILE: &str = "index.toml";

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

/// Load the `DistributionIndex`. Search order mirrors
/// [`load_bundled_catalog`]'s layering so an overlay can substitute the
/// index without rebuilding the bundle:
///
///   1. `manifests_overlay/distribution-index/index.toml` (e.g.
///      `/etc/anolisa/manifests/...` in system mode,
///      `~/.config/anolisa/manifests/...` in user mode).
///   2. Packaged: `datadir/manifests/distribution-index/index.toml`.
///   3. Dev-tree fallback so `cargo run` works without an install.
///
/// Returns `Ok(None)` when no index file is present anywhere — callers may
/// treat that as "no prebuilt artifacts known" rather than an error so
/// fresh checkouts without an index still produce a useful plan. The
/// `enable --dry-run` handler in particular substitutes an empty
/// [`DistributionIndex`] in that case so the plan still renders.
///
/// Today the overlay fully replaces the bundled index when present (no
/// per-entry merging). The launch spec leaves merge semantics for a later
/// milestone; document the current behavior in the user-facing docs.
pub fn load_distribution_index(
    ctx: &CliContext,
    command: &str,
) -> Result<Option<DistributionIndex>, CliError> {
    let layout = resolve_layout(ctx);
    let path = distribution_index_path(&layout);
    let Some(path) = path else {
        return Ok(None);
    };
    DistributionIndex::load(&path)
        .map(Some)
        .map_err(|err| CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "failed to load distribution index at {}: {err}",
                path.display(),
            ),
        })
}

fn distribution_index_path(layout: &FsLayout) -> Option<PathBuf> {
    let overlay_candidate = layout
        .manifests_overlay
        .join(DIST_INDEX_SUBDIR)
        .join(DIST_INDEX_FILE);
    if overlay_candidate.is_file() {
        return Some(overlay_candidate);
    }
    let manifests_root = packaged_manifests_root(layout).or_else(dev_tree_manifests)?;
    let candidate = manifests_root.join(DIST_INDEX_SUBDIR).join(DIST_INDEX_FILE);
    candidate.is_file().then_some(candidate)
}

/// Build a [`RegistryClient`] when the remote registry is configured.
///
/// Remote fetching is strictly **opt-in** for the MVP: returns `None`
/// unless `ANOLISA_REGISTRY_URL` is set or the layout's
/// `etc_dir/config.toml` carries a `[registry]` table. Without opt-in the
/// caller falls back to [`load_distribution_index`] (bundled local index),
/// so default installs never block on a network timeout.
pub fn load_registry_client(
    ctx: &CliContext,
    command: &str,
) -> Result<Option<RegistryClient>, CliError> {
    let layout = resolve_layout(ctx);
    let env_url = std::env::var("ANOLISA_REGISTRY_URL").ok();
    registry_client_from(&layout, env_url.as_deref(), command)
}

/// Env-free body of [`load_registry_client`] so tests can drive the opt-in
/// matrix without mutating process environment.
fn registry_client_from(
    layout: &FsLayout,
    env_url: Option<&str>,
    command: &str,
) -> Result<Option<RegistryClient>, CliError> {
    let config_path = layout.etc_dir.join("config.toml");
    let config = RegistryConfig::load_if_configured(&config_path, env_url).map_err(|err| {
        CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!("registry configuration error: {err}"),
        }
    })?;
    Ok(config.map(|c| RegistryClient::new(c, layout.cache_dir.join("registry"))))
}

/// Fetch the distribution index from the remote registry, translating the
/// freshness signal into an optional human-readable plan warning (`None`
/// for a silent fresh fetch).
pub fn fetch_remote_index(
    client: &RegistryClient,
    command: &str,
) -> Result<(DistributionIndex, Option<String>), CliError> {
    let (index, freshness) = client.fetch_index().map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("registry index fetch failed: {err}"),
    })?;
    let warning = match freshness {
        IndexFreshness::Fresh => None,
        IndexFreshness::CacheHit => {
            Some("registry index served from local cache (TTL valid)".to_string())
        }
        IndexFreshness::StaleOffline => {
            Some("registry unreachable — serving stale cached index (offline fallback)".to_string())
        }
    };
    Ok((index, warning))
}

/// Construct an empty in-memory [`DistributionIndex`]. Used by handlers
/// that want a safe fallback when no index file exists so the planner can
/// still produce a structured `blocked` plan instead of erroring out.
pub fn empty_distribution_index() -> DistributionIndex {
    DistributionIndex {
        schema_version: 1,
        channel: None,
        generated_at: None,
        expires_at: None,
        publisher: None,
        signature: None,
        entries: Vec::new(),
    }
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

/// True iff the wire status label denotes a capability that is actively
/// serving (i.e. `installed`, `degraded`, or `adopted`). Used by
/// `list --enabled` to exclude `disabled`/`failed`/`not_installed`.
pub(crate) fn status_is_enabled(status_label: &str) -> bool {
    matches!(status_label, "installed" | "degraded" | "adopted")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

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

    /// Empty fallback is what `enable --dry-run` substitutes when no
    /// index file is found. It must be safe to pass straight into the
    /// resolver (no entries, schema_version set so future migrations can
    /// detect it).
    #[test]
    fn empty_distribution_index_is_empty_and_loadable() {
        let idx = empty_distribution_index();
        assert!(idx.entries.is_empty());
        assert_eq!(idx.schema_version, 1);
    }

    /// Remote registry must stay opt-in: no env override and no
    /// `[registry]` table in `etc_dir/config.toml` → no client. Env URL or
    /// a config table flips it on.
    #[test]
    fn registry_client_is_opt_in() {
        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));

        let none = registry_client_from(&layout, None, "enable").expect("ok");
        assert!(none.is_none(), "no opt-in must mean no client");

        let via_env =
            registry_client_from(&layout, Some("http://r.test/i.toml"), "enable").expect("ok");
        assert!(via_env.is_some(), "env URL must opt in");

        fs::create_dir_all(&layout.etc_dir).expect("mkdir etc");
        fs::write(
            layout.etc_dir.join("config.toml"),
            "[registry]\nurl = \"http://file.test/i.toml\"\n",
        )
        .expect("write config");
        let via_file = registry_client_from(&layout, None, "enable").expect("ok");
        assert!(via_file.is_some(), "config [registry] table must opt in");
    }

    /// Overlay-distributed `index.toml` must win over the bundled
    /// (dev-tree) one. We use system-mode with a tmp prefix so the
    /// overlay sits at a path we control, and a unique `publisher`
    /// marker so we can distinguish overlay vs bundled.
    #[test]
    fn distribution_index_overlay_wins_over_bundled() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().to_path_buf();
        let layout = FsLayout::system(Some(prefix.clone()));
        let overlay_dir = layout.manifests_overlay.join(DIST_INDEX_SUBDIR);
        fs::create_dir_all(&overlay_dir).expect("mkdir overlay");
        let overlay_index = overlay_dir.join(DIST_INDEX_FILE);
        fs::write(
            &overlay_index,
            r#"schema_version = 1
publisher = "overlay-marker"
entries = []
"#,
        )
        .expect("write overlay index");

        let resolved = distribution_index_path(&layout).expect("overlay path resolved");
        assert_eq!(resolved, overlay_index);
        // Sanity: confirm the loaded index reflects the overlay (not the
        // dev-tree bundled one) by checking the unique marker.
        let idx = DistributionIndex::load(&resolved).expect("load");
        assert_eq!(idx.publisher.as_deref(), Some("overlay-marker"));
    }

    /// Without an overlay file the lookup must fall through to the
    /// packaged / dev-tree location — that's how `cargo run` works in
    /// the source tree today.
    #[test]
    fn distribution_index_falls_back_when_overlay_missing() {
        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        // No file created under the overlay path. The dev-tree fallback
        // ships an index.toml, so the resolver must return SOME path —
        // we only assert that we did fall back (i.e. did not return the
        // overlay candidate, which doesn't exist).
        let resolved = distribution_index_path(&layout);
        assert!(resolved.is_some(), "dev-tree fallback should resolve");
        let resolved = resolved.unwrap();
        assert!(
            !resolved.starts_with(&layout.manifests_overlay),
            "must not return non-existent overlay path: {}",
            resolved.display(),
        );
    }
}
