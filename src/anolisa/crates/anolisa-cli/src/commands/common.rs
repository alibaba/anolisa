//! Shared helpers for tier1 / tier2 command handlers.
//!
//! Read-only access to the three skeleton-stable objects:
//! [`FsLayout`], [`InstalledState`], and [`Catalog`]. Keep this module thin —
//! handlers compose these calls; we do not introduce a service layer here.

use std::path::{Path, PathBuf};

use anolisa_core::{
    Catalog, CatalogLayers, DistributionIndex, HttpFetch, IndexFreshness, InstalledState,
    ObjectStatus, RegistryClient, RegistryConfig, RegistryError,
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

/// Build a [`RegistryClient`] for the active layout.
///
/// Remote fetching is **default-on**: [`RegistryConfig::load`] always returns
/// a config (bundled default `< /etc/anolisa/config.toml < ANOLISA_REGISTRY_URL`),
/// so this returns `Some` unless config loading itself fails. The `Option`
/// return is retained because a caller may still treat "no client" as "use the
/// local index only", and the offline path is handled downstream by
/// [`fetch_remote_index_or_local`] rather than by suppressing the client here.
pub fn load_registry_client(
    ctx: &CliContext,
    command: &str,
) -> Result<Option<RegistryClient>, CliError> {
    let layout = resolve_layout(ctx);
    let env_url = std::env::var("ANOLISA_REGISTRY_URL").ok();
    registry_client_from(&layout, env_url.as_deref(), command)
}

/// Env-free body of [`load_registry_client`] so tests can drive the
/// config-layering matrix without mutating process environment.
fn registry_client_from(
    layout: &FsLayout,
    env_url: Option<&str>,
    command: &str,
) -> Result<Option<RegistryClient>, CliError> {
    let config_path = layout.etc_dir.join("config.toml");
    let config =
        RegistryConfig::load(&config_path, env_url).map_err(|err| CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!("registry configuration error: {err}"),
        })?;
    Ok(Some(RegistryClient::new(
        config,
        layout.cache_dir.join("registry"),
    )))
}

/// Outcome of resolving the distribution index for a command run.
pub struct ResolvedIndex {
    /// The index to plan against — remote when reachable, else the local
    /// bundled index (see [`degraded_to_local`](Self::degraded_to_local)).
    pub index: DistributionIndex,
    /// Human-readable freshness/fallback notes to fold into the plan warnings.
    pub warnings: Vec<String>,
    /// `true` when the remote fetch failed offline and we fell back to the
    /// local index. Callers use this to skip the per-component `meta.toml`
    /// overlay — the network is confirmed down, so meta fetches would only
    /// add noise.
    pub degraded_to_local: bool,
}

/// Resolve the distribution index from the remote registry, degrading to the
/// local bundled index when the network is down.
///
/// Only [`RegistryError::Offline`] (cold cache + unreachable endpoint) is
/// swallowed into a local fallback — that is the regression default-on must
/// not introduce: a first-ever offline `enable` should still render a plan
/// against the bundled index instead of hard-failing. Any other registry
/// error (malformed config, corrupt cache, parse failure) is a real fault and
/// surfaces as [`CliError`].
pub fn fetch_remote_index_or_local<H: HttpFetch>(
    client: &RegistryClient<H>,
    ctx: &CliContext,
    command: &str,
) -> Result<ResolvedIndex, CliError> {
    match client.fetch_index() {
        Ok((index, freshness)) => Ok(ResolvedIndex {
            index,
            warnings: freshness_warning(freshness).into_iter().collect(),
            degraded_to_local: false,
        }),
        Err(RegistryError::Offline { .. }) => {
            let index =
                load_distribution_index(ctx, command)?.unwrap_or_else(empty_distribution_index);
            Ok(ResolvedIndex {
                index,
                warnings: vec![
                    "registry unreachable — using local bundled index (offline fallback)"
                        .to_string(),
                ],
                degraded_to_local: true,
            })
        }
        Err(err) => Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!("registry index fetch failed: {err}"),
        }),
    }
}

/// Translate an index-freshness signal into an optional plan warning (`None`
/// for a silent fresh fetch).
fn freshness_warning(freshness: IndexFreshness) -> Option<String> {
    match freshness {
        IndexFreshness::Fresh => None,
        IndexFreshness::CacheHit => {
            Some("registry index served from local cache (TTL valid)".to_string())
        }
        IndexFreshness::StaleOffline => {
            Some("registry unreachable — serving stale cached index (offline fallback)".to_string())
        }
    }
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

    /// Remote registry is default-on: a client is built even with no env
    /// override and no `[registry]` table (it uses the bundled default URL).
    /// An env URL or a config table still works — they just retarget the URL.
    #[test]
    fn registry_client_default_on_and_url_overridable() {
        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));

        let default_on = registry_client_from(&layout, None, "enable").expect("ok");
        assert!(default_on.is_some(), "default-on must build a client");

        let via_env =
            registry_client_from(&layout, Some("http://r.test/i.toml"), "enable").expect("ok");
        assert!(via_env.is_some(), "env URL retargets, still a client");

        fs::create_dir_all(&layout.etc_dir).expect("mkdir etc");
        fs::write(
            layout.etc_dir.join("config.toml"),
            "[registry]\nurl = \"http://file.test/i.toml\"\n",
        )
        .expect("write config");
        let via_file = registry_client_from(&layout, None, "enable").expect("ok");
        assert!(via_file.is_some(), "config [registry] table retargets too");
    }

    /// A cold-cache offline fetch (network down, nothing cached) must not
    /// hard-fail now that remote fetch is default-on: it degrades to the
    /// local bundled index, flags `degraded_to_local`, and carries a warning.
    #[test]
    fn fetch_remote_index_or_local_degrades_when_offline() {
        let tmp = tempdir().expect("tmpdir");
        // Fresh cache root → no cached index → Offline on a down transport.
        let cfg = RegistryConfig::bundled_default();
        let client = RegistryClient::with_http(cfg, tmp.path().join("registry"), FailingHttp);

        // System mode under a tmp prefix so the local lookup is hermetic: the
        // overlay/packaged candidates under the prefix are absent, so
        // load_distribution_index falls through to the dev-tree bundled index
        // (which ships in this repo) without touching the real $HOME.
        let ctx = CliContext {
            install_mode: InstallMode::System,
            prefix: Some(tmp.path().to_path_buf()),
            json: false,
            dry_run: true,
            verbose: false,
            quiet: false,
            no_color: true,
        };
        let resolved = fetch_remote_index_or_local(&client, &ctx, "enable")
            .expect("offline degrades, not errors");

        assert!(resolved.degraded_to_local, "offline must degrade to local");
        assert!(
            resolved
                .warnings
                .iter()
                .any(|w| w.contains("offline fallback")),
            "must warn about the fallback: {:?}",
            resolved.warnings,
        );
    }

    /// Always-failing HTTP transport: every GET reports the endpoint down, so
    /// `fetch_index` returns `RegistryError::Offline` on a cold cache.
    struct FailingHttp;

    impl HttpFetch for FailingHttp {
        fn get(&self, _url: &str) -> Result<Vec<u8>, anolisa_core::FetchFailure> {
            Err(anolisa_core::FetchFailure::Network {
                reason: "connection refused".into(),
            })
        }
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
