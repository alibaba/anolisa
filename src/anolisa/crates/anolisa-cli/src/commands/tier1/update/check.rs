//! `anolisa update --check` — read-only RPM upgrade detection (issue #1410).
//!
//! Produces a report answering "can the installed toolchain be upgraded?" for
//! the RPM / system-image scenario. It is strictly read-only: it inspects the
//! host only through [`PackageQuery`], i.e. read-only `rpm -q` / `dnf repoquery`
//! lookups. It runs **no mutating package operation** (never a
//! [`PackageTransaction`](anolisa_platform::pkg_transaction::PackageTransaction),
//! so no `dnf install/update/remove`), never writes `installed.toml`, and never
//! persists repo/adapter state. Note the repo candidate lookup still calls
//! `dnf repoquery`, which touches the network like any read query — so the MOTD
//! path is cache-backed to keep that cost off the login hot path. Applying
//! upgrades is a separate command (`anolisa upgrade`, issue #1411).
//!
//! Three sources feed the report:
//! - **CLI**: whether the running `anolisa` binary is RPM-owned and has a newer
//!   repo candidate. A non-RPM binary is `unsupported` here (self-update is
//!   handled by `anolisa update self`).
//! - **Installed components**: each `rpm-managed` / `rpm-observed` component is
//!   compared against its repo candidates; `raw-managed` components are reported
//!   `unsupported_in_rpm_upgrade` and never touched.
//! - **Target profile** (optional, `--target`): default components the profile
//!   declares but that are not installed are reported as installable.
//!
//! Repo candidate ordering uses RPM's own EVR comparison
//! ([`rpm_evr_cmp`]), not semver, so real epochs/releases are ordered
//! correctly. A small on-disk cache under
//! `cache_dir/update-check.json` (keyed by the resolved target) backs the
//! low-noise MOTD path; `--refresh` bypasses it.
//!
//! Rendering lives in [`render`]; the cache and target-profile plumbing plus the
//! read-only repo-config load live here alongside the detection logic.

mod render;

#[cfg(test)]
mod tests;

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use anolisa_core::self_update;
use anolisa_core::state::{InstalledObject, InstalledState, ObjectKind, Ownership};
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::pkg_query::{PackageQuery, PackageQueryError, PackageVersion, rpm_evr_cmp};
use anolisa_platform::rpm_query::RpmPackageQuery;

use super::UpdateArgs;
use crate::commands::common;
use crate::context::CliContext;
use crate::repo_config::RepoConfig;
use crate::response::CliError;

/// Command label for JSON envelopes and error routing on the check path.
pub(super) const CHECK_COMMAND: &str = "update --check";

/// Filename for the cached report under the layout's `cache_dir`.
const CACHE_FILE: &str = "update-check.json";

/// How long a cached report is considered fresh for the MOTD path. Off the
/// round hour so scheduled MOTD refreshes across hosts do not all land at once.
const CACHE_TTL_SECS: i64 = 6 * 3600 + 137;

// Stable action vocabulary shared by JSON, cache, and human/MOTD rendering.
const ACTION_UPDATE: &str = "update";
const ACTION_NOOP: &str = "noop";
const ACTION_INSTALL: &str = "install";
const ACTION_UNSUPPORTED: &str = "unsupported";
const ACTION_UNSUPPORTED_RPM: &str = "unsupported_in_rpm_upgrade";
const ACTION_ERROR: &str = "error";

/// Wire shape for `update --check` (`--json`) and the on-disk cache.
///
/// Owned `String`s (rather than `&'static str`) so the same struct round-trips
/// through the cache via `Deserialize`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct UpdateCheckReport {
    /// Target profile evaluated, when `--target` was given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    /// Upgrade backend this report covers. Always `rpm` in the first version.
    backend: String,
    /// True when at least one component/CLI `update` was found. Deliberately
    /// narrow: a missing default is an *install*, not an upgrade, so it does not
    /// set this — see [`action_required`](Self::action_required).
    upgrade_available: bool,
    /// True when there is anything to do: an upgrade **or** a missing default to
    /// install. This is the signal machine callers should gate on when driving
    /// the (future) `anolisa upgrade`; `upgrade_available` alone would report
    /// "nothing to upgrade" on a fresh image that is only missing defaults.
    action_required: bool,
    cli: CliCheck,
    components: Vec<ComponentCheck>,
    summary: CheckSummary,
}

/// Upgrade status of the running `anolisa` CLI binary.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CliCheck {
    /// RPM package that owns the CLI binary; `None` when it is not RPM-owned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    package: Option<String>,
    /// Installed EVR from rpmdb.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    installed: Option<String>,
    /// Newest repo candidate EVR, when an upgrade is available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    available: Option<String>,
    action: String,
    /// Item-level failure that did not abort the whole check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Upgrade status of one installed (or profile-declared) component.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ComponentCheck {
    component: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    package: Option<String>,
    /// Provenance label (`rpm-managed` / `rpm-observed` / `raw-managed`);
    /// `None` for a profile default that is not installed yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ownership: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    installed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    available: Option<String>,
    action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Aggregate counts used by the summary line, MOTD, and exit signalling.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CheckSummary {
    /// Upgrades found (CLI plus components).
    updates: usize,
    /// Profile default components absent from state.
    missing_defaults: usize,
    /// Items outside the RPM upgrade scope (raw-managed, non-RPM CLI).
    unsupported: usize,
    /// Item-level query failures that did not abort the check.
    errors: usize,
}

/// Cache envelope stored under `cache_dir/update-check.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct UpdateCheckCache {
    /// RFC3339 UTC timestamp used to decide cache freshness.
    generated_at: String,
    report: UpdateCheckReport,
}

/// Minimal target-profile reader.
///
/// The repository-side profile schema is not finalized, so this deliberately
/// reads only the one field the check needs (`default_components`) and tolerates
/// any other keys, letting a richer schema land later without breaking callers.
#[derive(Debug, Clone, Default, Deserialize)]
struct TargetProfile {
    #[serde(default)]
    default_components: Vec<String>,
}

impl TargetProfile {
    fn from_toml_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }
}

/// Read-only inputs for [`run_update_check`]; injected so tests drive the whole
/// report without a live rpmdb/dnf.
struct CheckInputs<'a> {
    installed: &'a InstalledState,
    query: &'a dyn PackageQuery,
    /// Path of the running executable, used to find its owning RPM.
    cli_exe_path: &'a str,
    /// Installed architecture used to filter repo candidates.
    arch: &'a str,
    /// Echoed into the report's `target` field.
    target_name: Option<String>,
    /// Loaded profile, when `--target` was given and resolved.
    target: Option<TargetProfile>,
}

/// Production entry point for `anolisa update --check`.
pub(super) fn handle_update_check(args: &UpdateArgs, ctx: &CliContext) -> Result<(), CliError> {
    let layout = common::resolve_layout(ctx);
    let cache_path = cache_path(&layout);

    // MOTD fast path: prefer a fresh cache for the *same* target so the login
    // hook stays cheap and never blocks on the network. JSON output always
    // recomputes so machine callers get the full, current envelope.
    if args.motd
        && !args.refresh
        && !ctx.json
        && let Some(cache) = read_cache(&cache_path)
        && cache_is_usable(&cache, args.target.as_deref())
    {
        render::render_motd(ctx, &cache.report);
        return Ok(());
    }

    let report = match compute_report(args, ctx, &layout) {
        Ok(report) => report,
        Err(err) => {
            // MOTD must stay quiet and low-noise on failure; a JSON/human check
            // surfaces the error as usual.
            if args.motd {
                return Ok(());
            }
            return Err(err);
        }
    };

    // The cache is not authoritative state; a write failure (e.g. a non-root
    // MOTD probe against a root-owned cache dir) is non-fatal.
    let _ = write_cache(&cache_path, &report);

    render::render_report(ctx, args, &report);
    Ok(())
}

/// Build the report from live host state (repo config, rpmdb/dnf, target
/// profile). Split from [`run_update_check`] so the pure logic stays testable.
fn compute_report(
    args: &UpdateArgs,
    ctx: &CliContext,
    layout: &FsLayout,
) -> Result<UpdateCheckReport, CliError> {
    let repo_config = load_repo_config_read_only(layout)?;
    let env = anolisa_env::EnvService::detect();
    let repo = super::rpm_repo_source_for_update(&repo_config, &env, CHECK_COMMAND)?.ok_or_else(
        || CliError::InvalidArgument {
            command: CHECK_COMMAND.to_string(),
            reason: "repo.toml has no [backends.rpm] table; `update --check` needs the configured ANOLISA RPM repository to look up upgrade candidates".to_string(),
        },
    )?;
    let query = RpmPackageQuery::system_with_repo(repo);
    let installed = common::load_installed_state(ctx, CHECK_COMMAND)?;

    let exe = self_update::resolve_current_exe().map_err(|err| CliError::Runtime {
        command: CHECK_COMMAND.to_string(),
        reason: format!("cannot resolve the running executable: {err}"),
    })?;
    let exe_path = exe
        .to_str()
        .ok_or_else(|| CliError::Runtime {
            command: CHECK_COMMAND.to_string(),
            reason: format!(
                "running executable path is not valid UTF-8: {}",
                exe.display()
            ),
        })?
        .to_string();

    let target = match &args.target {
        Some(name) => Some(load_target_profile(layout, name)?),
        None => None,
    };

    Ok(run_update_check(CheckInputs {
        installed: &installed,
        query: &query,
        cli_exe_path: &exe_path,
        arch: &env.arch,
        target_name: args.target.clone(),
        target,
    }))
}

/// Load repo config without ever writing it, keeping `--check` read-only.
///
/// The dry-run load path fetches a missing config into memory only (never
/// persisting `<etc_dir>/repo.toml`), which is exactly the read-only guarantee
/// the check needs even on a host that has not provisioned repo config yet. The
/// no-write behaviour of the dry-run path is covered by
/// `repo_config::tests::load_dry_run_fetches_without_writing`.
fn load_repo_config_read_only(layout: &FsLayout) -> Result<RepoConfig, CliError> {
    RepoConfig::load(layout, true)
        .map(|loaded| loaded.config)
        .map_err(|err| CliError::Runtime {
            command: CHECK_COMMAND.to_string(),
            reason: format!("failed to load repo config: {err}"),
        })
}

/// Pure report builder over the injected read-only inputs.
fn run_update_check(inputs: CheckInputs<'_>) -> UpdateCheckReport {
    let mut summary = CheckSummary::default();

    let cli = build_cli_check(inputs.query, inputs.cli_exe_path, inputs.arch, &mut summary);

    let mut components = Vec::new();
    for obj in &inputs.installed.objects {
        if obj.kind != ObjectKind::Component {
            continue;
        }
        components.push(check_component(
            inputs.query,
            obj,
            inputs.arch,
            &mut summary,
        ));
    }

    // Profile defaults that are absent from state are reported as installable
    // (issue #1411 performs the install; this only surfaces the gap).
    if let Some(profile) = &inputs.target {
        for name in &profile.default_components {
            if inputs
                .installed
                .find_object(ObjectKind::Component, name)
                .is_none()
            {
                components.push(ComponentCheck {
                    component: name.clone(),
                    package: None,
                    ownership: None,
                    installed: None,
                    available: None,
                    action: ACTION_INSTALL.to_string(),
                    error: None,
                });
                summary.missing_defaults += 1;
            }
        }
    }

    let upgrade_available = summary.updates > 0;
    let action_required = upgrade_available || summary.missing_defaults > 0;
    UpdateCheckReport {
        target: inputs.target_name,
        backend: "rpm".to_string(),
        upgrade_available,
        action_required,
        cli,
        components,
        summary,
    }
}

/// Determine whether the running CLI binary is RPM-owned and, if so, whether a
/// newer repo candidate exists.
fn build_cli_check(
    query: &dyn PackageQuery,
    exe_path: &str,
    arch: &str,
    summary: &mut CheckSummary,
) -> CliCheck {
    let providers = match query.what_provides_installed(exe_path) {
        Ok(providers) => providers,
        // No rpm/dnf on the host: the CLI cannot be shown RPM-owned, so treat it
        // as out of RPM upgrade scope rather than a hard failure.
        Err(PackageQueryError::CommandMissing { .. }) => {
            summary.unsupported += 1;
            return cli_unsupported("rpm/dnf not found; cannot determine CLI package ownership");
        }
        Err(err) => {
            summary.errors += 1;
            return cli_error(
                None,
                format!("cannot determine CLI package ownership: {err}"),
            );
        }
    };

    let package = match providers.as_slice() {
        [] => {
            summary.unsupported += 1;
            return cli_unsupported(
                "anolisa CLI is not RPM-owned; use `anolisa update self` to update the binary",
            );
        }
        [only] => only.clone(),
        _ => {
            summary.errors += 1;
            return cli_error(
                None,
                format!(
                    "CLI executable is provided by multiple RPM packages ({})",
                    providers.join(", ")
                ),
            );
        }
    };

    let installed = match query.query_installed(&package) {
        Ok(Some(info)) => info.version,
        Ok(None) => {
            summary.errors += 1;
            return cli_error(
                Some(package),
                "CLI package is reported as owner but is absent from rpmdb".to_string(),
            );
        }
        Err(PackageQueryError::UnexpectedOutput { .. }) => {
            summary.errors += 1;
            return cli_error(
                Some(package),
                "CLI package has multiple installed versions in rpmdb".to_string(),
            );
        }
        Err(err) => {
            summary.errors += 1;
            return cli_error(Some(package), format!("rpm query failed: {err}"));
        }
    };
    let installed_evr = installed.to_string();

    match repo_upgrade(query, &package, arch, &installed) {
        Ok(Some(available)) => {
            summary.updates += 1;
            CliCheck {
                package: Some(package),
                installed: Some(installed_evr),
                available: Some(available),
                action: ACTION_UPDATE.to_string(),
                error: None,
            }
        }
        Ok(None) => CliCheck {
            package: Some(package),
            installed: Some(installed_evr),
            available: None,
            action: ACTION_NOOP.to_string(),
            error: None,
        },
        Err(err) => {
            summary.errors += 1;
            cli_error(Some(package), format!("repo candidate query failed: {err}"))
        }
    }
}

/// Evaluate one installed component against the RPM upgrade scope.
fn check_component(
    query: &dyn PackageQuery,
    obj: &InstalledObject,
    arch: &str,
    summary: &mut CheckSummary,
) -> ComponentCheck {
    let component = obj.name.clone();
    let ownership = obj.effective_ownership();

    // Raw-managed components are explicitly out of the RPM upgrade path. Nothing
    // is queried, touched, or migrated — only reported.
    if ownership == Ownership::RawManaged {
        summary.unsupported += 1;
        return ComponentCheck {
            component,
            package: obj.raw_package.clone(),
            ownership: Some(ownership.label().to_string()),
            installed: Some(obj.version.clone()),
            available: None,
            action: ACTION_UNSUPPORTED_RPM.to_string(),
            error: None,
        };
    }

    let ownership_label = ownership.label().to_string();
    let package = match obj
        .rpm_metadata
        .as_ref()
        .map(|m| m.package_name.clone())
        .filter(|p| !p.is_empty())
    {
        Some(package) => package,
        None => {
            summary.errors += 1;
            return component_error(
                component,
                None,
                ownership_label,
                Some(obj.version.clone()),
                "component is recorded as RPM-backed but has no package metadata; run `anolisa repair` to refresh it".to_string(),
            );
        }
    };

    let installed = match query.query_installed(&package) {
        Ok(Some(info)) => info.version,
        // rpmdb no longer has the package (e.g. removed with `rpm -e`): item
        // error, not a crash — the rest of the check continues.
        Ok(None) => {
            summary.errors += 1;
            return component_error(
                component,
                Some(package),
                ownership_label,
                Some(obj.version.clone()),
                "package recorded in ANOLISA state is not present in rpmdb; run `anolisa forget` or reinstall".to_string(),
            );
        }
        Err(PackageQueryError::UnexpectedOutput { .. }) => {
            summary.errors += 1;
            return component_error(
                component,
                Some(package),
                ownership_label,
                Some(obj.version.clone()),
                "rpmdb reports multiple installed versions for this package".to_string(),
            );
        }
        Err(PackageQueryError::CommandMissing { .. }) => {
            summary.errors += 1;
            return component_error(
                component,
                Some(package),
                ownership_label,
                Some(obj.version.clone()),
                "rpm/dnf not found; cannot query the installed version".to_string(),
            );
        }
        Err(err) => {
            summary.errors += 1;
            return component_error(
                component,
                Some(package),
                ownership_label,
                Some(obj.version.clone()),
                format!("rpm query failed: {err}"),
            );
        }
    };
    let installed_evr = installed.to_string();

    match repo_upgrade(query, &package, arch, &installed) {
        Ok(Some(available)) => {
            summary.updates += 1;
            ComponentCheck {
                component,
                package: Some(package),
                ownership: Some(ownership_label),
                installed: Some(installed_evr),
                available: Some(available),
                action: ACTION_UPDATE.to_string(),
                error: None,
            }
        }
        Ok(None) => ComponentCheck {
            component,
            package: Some(package),
            ownership: Some(ownership_label),
            installed: Some(installed_evr),
            available: None,
            action: ACTION_NOOP.to_string(),
            error: None,
        },
        Err(err) => {
            summary.errors += 1;
            component_error(
                component,
                Some(package),
                ownership_label,
                Some(installed_evr),
                format!("repo candidate query failed: {err}"),
            )
        }
    }
}

/// Newest repo candidate strictly newer than `installed`, filtered to the
/// installed arch (plus `noarch`). Returns `None` when no candidate is a genuine
/// upgrade; ordering uses RPM's own EVR comparison
/// ([`rpm_evr_cmp`]) so epochs/releases/non-semver versions are ordered the way
/// `dnf` would, and a stale/downgrade candidate is never reported as available.
fn repo_upgrade(
    query: &dyn PackageQuery,
    package: &str,
    arch: &str,
    installed: &PackageVersion,
) -> Result<Option<String>, PackageQueryError> {
    let mut best: Option<PackageVersion> = None;
    for info in query.query_available(package)? {
        if info.arch != arch && info.arch != "noarch" {
            continue;
        }
        if rpm_evr_cmp(&info.version, installed) != Ordering::Greater {
            continue;
        }
        best = match best {
            Some(current) if rpm_evr_cmp(&current, &info.version) != Ordering::Less => {
                Some(current)
            }
            _ => Some(info.version),
        };
    }
    Ok(best.map(|version| version.to_string()))
}

fn cli_unsupported(reason: &str) -> CliCheck {
    CliCheck {
        package: None,
        installed: None,
        available: None,
        action: ACTION_UNSUPPORTED.to_string(),
        error: Some(reason.to_string()),
    }
}

fn cli_error(package: Option<String>, reason: String) -> CliCheck {
    CliCheck {
        package,
        installed: None,
        available: None,
        action: ACTION_ERROR.to_string(),
        error: Some(reason),
    }
}

fn component_error(
    component: String,
    package: Option<String>,
    ownership: String,
    installed: Option<String>,
    reason: String,
) -> ComponentCheck {
    ComponentCheck {
        component,
        package,
        ownership: Some(ownership),
        installed,
        available: None,
        action: ACTION_ERROR.to_string(),
        error: Some(reason),
    }
}

/// Resolve and read a target profile. The name is validated as a single path
/// segment so `--target` cannot escape the profiles directory.
fn load_target_profile(layout: &FsLayout, target: &str) -> Result<TargetProfile, CliError> {
    if target.trim().is_empty()
        || target == "."
        || target == ".."
        || target.contains('/')
        || target.contains('\\')
    {
        return Err(CliError::InvalidArgument {
            command: CHECK_COMMAND.to_string(),
            reason: format!("target profile name '{target}' is not a valid profile identifier"),
        });
    }
    let path = layout
        .etc_dir
        .join("profiles")
        .join(format!("{target}.toml"));
    let body = std::fs::read_to_string(&path).map_err(|err| CliError::InvalidArgument {
        command: CHECK_COMMAND.to_string(),
        reason: format!(
            "cannot read target profile '{target}' at {}: {err}",
            path.display()
        ),
    })?;
    TargetProfile::from_toml_str(&body).map_err(|err| CliError::InvalidArgument {
        command: CHECK_COMMAND.to_string(),
        reason: format!("target profile '{target}' is invalid: {err}"),
    })
}

// ── cache ────────────────────────────────────────────────────────────────

/// Cache location under the active layout's `cache_dir`
/// (`/var/cache/anolisa/update-check.json` in system mode).
fn cache_path(layout: &FsLayout) -> PathBuf {
    layout.cache_dir.join(CACHE_FILE)
}

/// Read and parse a cached report; any error (missing, unreadable, malformed)
/// yields `None` so a bad cache never blocks a fresh query.
fn read_cache(path: &Path) -> Option<UpdateCheckCache> {
    let body = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&body).ok()
}

/// Whether a cached report may be reused for a MOTD render: it must be fresh AND
/// have been computed for the same target, so a prior `--target` run never leaks
/// its report into a plain MOTD (or a different target's).
fn cache_is_usable(cache: &UpdateCheckCache, target: Option<&str>) -> bool {
    is_fresh(&cache.generated_at) && cache.report.target.as_deref() == target
}

/// Whether a cache timestamp is within [`CACHE_TTL_SECS`] of now (and not in the
/// future, which would indicate a corrupt/adversarial timestamp).
fn is_fresh(generated_at: &str) -> bool {
    match chrono::DateTime::parse_from_rfc3339(generated_at) {
        Ok(ts) => {
            let age = Utc::now().signed_duration_since(ts.with_timezone(&Utc));
            age >= chrono::Duration::zero() && age <= chrono::Duration::seconds(CACHE_TTL_SECS)
        }
        Err(_) => false,
    }
}

/// Best-effort cache write; failures are swallowed by the caller.
fn write_cache(path: &Path, report: &UpdateCheckReport) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let cache = UpdateCheckCache {
        generated_at: super::now_iso8601(),
        report: report.clone(),
    };
    let body = serde_json::to_string_pretty(&cache)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    std::fs::write(path, body)
}
