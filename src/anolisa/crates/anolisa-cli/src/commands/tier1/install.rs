//! `anolisa install` — install a component through a configured backend.
//!
//! `install` takes a component noun and resolves it through the configured
//! backend. The resolution chain — repo.toml loading, backend selection
//! (`--backend` > `default_backend`), base_url variable substitution, and
//! package-name mapping (`--package` > `package_map` > scope > component
//! name) — feeds the **raw** backend executor: fetch the distribution index
//! from the raw repository root, resolve an artifact, then execute by
//! downloading it with mandatory sha256 verification, loading the install
//! contract, installing the declared files, and recording state plus a
//! central-log audit entry.
//!
//! The `rpm` backend additionally supports **adopt** (issue #958): in system
//! mode, when a component is already present as a system RPM, `install`
//! records it as `rpm-observed` state without downloading or running
//! `dnf install`. The backend decision is two-layered — pick a backend name
//! (`--backend` > existing state > system RPM presence > `default_backend`),
//! then pick an action by `(backend, rpmdb hit, install mode)`. Delegated
//! `dnf install` for not-yet-installed RPM components is out of scope (#959),
//! and `npm` remains NOT_IMPLEMENTED.
//!
//! Deliberately out of scope for this milestone: execution-policy gating,
//! pre/post hooks, health checks, and service start/enable. Installed
//! services are recorded in state with `enabled: false`.

use clap::Parser;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anolisa_core::central_log::{CentralLog, LogKind, LogRecord, LogStatus, Severity};
use anolisa_core::download::{DownloadCache, DownloadError};
use anolisa_core::install_runner::{
    InstallRunner, ResolvedInstallFile, SUPPORTED_ARTIFACT_TYPES,
    read_embedded_component_manifest_text,
};
use anolisa_core::lock::InstallLock;
use anolisa_core::path_safety::validate_owned_path;
use anolisa_core::state::{
    FileOwner, InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectKind,
    ObjectStatus, OperationRecord, OwnedFile, Ownership, RpmMetadata, ServiceRef,
};
use anolisa_core::{
    ArtifactType, ComponentManifest, DistributionEntry, DistributionIndex, FileKind, ResolveQuery,
    expand_layout_placeholders,
};
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::pkg_query::{PackageInfo, PackageQuery, PackageQueryError};
use anolisa_platform::rpm_query::RpmPackageQuery;
use chrono::{SecondsFormat, Utc};

use crate::color::Palette;
use crate::commands::common;
use crate::context::{CliContext, InstallMode};
use crate::repo_config::{
    BackendConfig, HostVars, RepoConfig, RepoConfigError, normalize_override_url, raw_artifact_url,
    raw_index_url, raw_relative_root,
};
use crate::response::{CliError, render_json, render_json_with_status};

const COMMAND: &str = "install";

#[derive(Debug, Parser)]
// `--version` here means the *component* version (the `cargo install`
// convention), so the auto-generated CLI-version flag must be disabled
// to free the name. `anolisa --version` still works at the top level.
#[command(disable_version_flag = true)]
#[command(group(
    clap::ArgGroup::new("target")
        .required(true)
        .args(["component", "all"]),
))]
pub struct InstallArgs {
    /// Component name to install
    #[arg(value_name = "COMPONENT")]
    pub component: Option<String>,
    /// Install every component listed in the catalog (mutually exclusive with COMPONENT)
    #[arg(long, conflicts_with_all = ["component", "version", "package"])]
    pub all: bool,
    /// With --all, stop on the first failure instead of continuing
    #[arg(long, requires = "all")]
    pub fail_fast: bool,
    /// Install a specific version instead of the latest in the channel
    #[arg(long, value_name = "VERSION")]
    pub version: Option<String>,
    /// Backend override (raw | rpm | npm); defaults to repo.toml default_backend
    #[arg(long, value_name = "BACKEND")]
    pub backend: Option<String>,
    /// One-off base_url override for the selected backend
    #[arg(long, value_name = "URL")]
    pub repo: Option<String>,
    /// Override the backend-native package name for the component
    #[arg(long, value_name = "NAME")]
    pub package: Option<String>,
}

/// Raw backend resolution shared by dry-run preview and real execution.
struct RawResolution {
    component: String,
    package: String,
    backend: String,
    base_url: String,
    entry: DistributionEntry,
    artifact_url: String,
    warnings: Vec<String>,
}

/// Dry-run preview after optional lightweight metadata expansion.
struct InstallPreview {
    resolution: RawResolution,
    files: Vec<ResolvedInstallFile>,
    services: Vec<String>,
}

/// Execution input after the artifact has been verified and its install
/// contract has been resolved.
struct PreparedInstall {
    resolution: RawResolution,
    artifact_path: PathBuf,
    files: Vec<ResolvedInstallFile>,
    services: Vec<String>,
    manifest_toml: String,
}

/// Parsed install contract plus the TOML persisted as the local install fact.
struct LoadedInstallContract {
    manifest: ComponentManifest,
    source: InstallContractSource,
    toml: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallContractSource {
    EmbeddedArtifact,
    SidecarMeta,
    LocalCatalog,
}

#[derive(Serialize)]
struct ArtifactInfo {
    r#type: String,
    url: String,
    sha256: Option<String>,
}

/// Wire shape for `--dry-run`: the resolution result without downloading
/// the install artifact.
#[derive(Serialize)]
struct InstallPlanPayload {
    component: String,
    package: String,
    version: String,
    backend: String,
    base_url: String,
    install_mode: String,
    artifact: ArtifactInfo,
    files: Vec<String>,
    services: Vec<String>,
    dry_run: bool,
    warnings: Vec<String>,
}

/// Wire shape for a completed install.
#[derive(Serialize)]
struct InstallResultPayload {
    component: String,
    package: String,
    version: String,
    backend: String,
    base_url: String,
    install_mode: String,
    operation_id: String,
    artifact_url: String,
    files_installed: Vec<String>,
    services: Vec<String>,
    warnings: Vec<String>,
}

/// What `handle_one` did, so `--all` can distinguish a fresh install from an
/// RPM adopt in its batch summary (§7.5). The dry-run vs real distinction is
/// layered on by the caller from [`CliContext::dry_run`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallOutcome {
    /// A raw install (downloaded + placed files, or its dry-run preview).
    Installed,
    /// An existing system RPM recorded as `rpm-observed` (or its dry-run
    /// preview); no bytes fetched, no owned files written.
    Adopted,
}

/// Source that decided the backend name in layer 1 (§4). Only used to phrase
/// conflict errors; the action is chosen by layer 2 from `(backend, rpmdb,
/// mode)`, independent of how the name was picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendSource {
    /// Explicit `--backend`.
    Explicit,
    /// Component already in state; backend follows its recorded provenance.
    ExistingState,
    /// State miss; system mode + rpmdb hit selected `rpm`.
    SystemRpm,
    /// None of the above; fell back to `default_backend`.
    Default,
}

pub fn handle(args: InstallArgs, ctx: &CliContext) -> Result<(), CliError> {
    if args.fail_fast && !args.all {
        return Err(CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: "--fail-fast is only meaningful with --all".to_string(),
        });
    }
    if args.all {
        return handle_all(args, ctx);
    }
    // clap ArgGroup guarantees at least one of `component` / `--all`; with
    // `--all` ruled out above, `component` is necessarily Some.
    let component = args
        .component
        .clone()
        .expect("clap ArgGroup ensures component is set when --all is absent");
    handle_one(component, args, ctx).map(|_| ())
}

fn handle_one(
    component: String,
    args: InstallArgs,
    ctx: &CliContext,
) -> Result<InstallOutcome, CliError> {
    // Production uses the real rpm/dnf-backed query; tests inject a fake via
    // `handle_one_with_query`. Construction is side-effect-free (it only holds
    // a command runner), so building it on the raw path costs nothing.
    let query = RpmPackageQuery::system();
    handle_one_with_query(component, args, ctx, &query)
}

/// Core of [`handle_one`] with the package query injected, so tests can drive
/// the adopt path without a live rpmdb.
fn handle_one_with_query(
    component: String,
    args: InstallArgs,
    ctx: &CliContext,
    query: &dyn PackageQuery,
) -> Result<InstallOutcome, CliError> {
    let command = format!("install {component}");

    let layout = common::resolve_layout(ctx);
    let env = anolisa_env::EnvService::detect();
    let repo_config = RepoConfig::load(&layout).map_err(|err| repo_config_err(err, false))?;
    let installed = common::load_installed_state(ctx, COMMAND)?;

    // ── Layer 1: pick the backend name + its source (§4). ──
    //
    // Priority: explicit --backend > existing state > system RPM presence
    // (system mode only) > default_backend. The system-RPM probe runs only
    // when nothing earlier decided, so non-RPM hosts and the common raw path
    // never shell out to rpm/dnf.
    let mut adopt_situation: Option<RpmSituation> = None;
    let (backend_name, source): (String, BackendSource) = if let Some(explicit) =
        args.backend.as_deref()
    {
        (explicit.to_string(), BackendSource::Explicit)
    } else if let Some(label) = installed
        .find_object(ObjectKind::Component, &component)
        .and_then(installed_backend_label)
    {
        // Provenance is sticky: a re-`install` of an adopted rpm-observed
        // component lands on `rpm` here and is routed to adopt-refresh by
        // layer 2, rather than being rejected by the raw trunk.
        (label.to_string(), BackendSource::ExistingState)
    } else if ctx.install_mode == InstallMode::System {
        let situation = probe_rpm_situation(&component, &args, &repo_config, ctx, query, &command)?;
        if matches!(situation, RpmSituation::Absent) {
            (repo_config.default_backend.clone(), BackendSource::Default)
        } else {
            adopt_situation = Some(situation);
            ("rpm".to_string(), BackendSource::SystemRpm)
        }
    } else {
        (repo_config.default_backend.clone(), BackendSource::Default)
    };

    // ── Layer 2: pick the action by (backend, rpmdb, mode) (§7.1). ──
    if backend_name == "rpm" {
        return route_rpm_adopt(
            &component,
            &args,
            ctx,
            &command,
            &layout,
            &repo_config,
            &installed,
            source,
            adopt_situation,
            query,
        );
    }

    handle_raw_install(
        component,
        args,
        ctx,
        &command,
        &layout,
        &env,
        &repo_config,
        &installed,
        &backend_name,
    )
}

/// Existing raw-backend trunk: repo.toml → base_url → package → resolve →
/// (dry-run preview | download + execute). Backends other than `raw` that
/// reach here have no executor yet and return a not-implemented hint.
#[allow(clippy::too_many_arguments)]
fn handle_raw_install(
    component: String,
    args: InstallArgs,
    ctx: &CliContext,
    command: &str,
    layout: &FsLayout,
    env: &anolisa_env::EnvFacts,
    repo_config: &RepoConfig,
    installed: &InstalledState,
    backend_name: &str,
) -> Result<InstallOutcome, CliError> {
    // Re-resolve through `select_backend` so the configured `[backends.<name>]`
    // table (base_url, package_map, scope) is in hand. This stays on the raw
    // path only; the rpm/adopt branch above never calls it (no table required).
    let (backend_name, backend) = repo_config
        .select_backend(Some(backend_name))
        .map_err(|err| repo_config_err(err, true))?;

    ensure_component_backend_compatible(installed, &component, backend_name, command)?;

    // Backend gate: only raw can execute today. The selection above already
    // validated the name/configuration, so this is purely "executor missing".
    if backend_name != "raw" {
        return Err(CliError::not_implemented_with_hint(
            format!("install --backend {backend_name}"),
            format!(
                "the '{backend_name}' backend is configured but its executor is not implemented yet — only 'raw' can install today",
            ),
        ));
    }

    let mut warnings: Vec<String> = Vec::new();
    let base_url = match args.repo.as_deref() {
        Some(override_url) => {
            let normalized =
                normalize_override_url(override_url).map_err(|err| repo_config_err(err, true))?;
            if normalized.starts_with("http://") {
                warnings.push(format!(
                    "--repo uses plaintext http ({normalized}) — artifacts are still sha256-verified on the raw backend, but the index itself is unauthenticated",
                ));
            }
            normalized
        }
        None => {
            let host = HostVars {
                os: env.os.clone(),
                arch: env.arch.clone(),
            };
            repo_config
                .resolved_base_url(backend_name, backend, &host)
                // Variable errors are fixed by editing [vars] in repo.toml.
                .map_err(|err| repo_config_err(err, true))?
        }
    };
    let package = repo_config.package_name(backend, &component, args.package.as_deref());

    let resolved = resolve_raw(
        ctx,
        layout,
        env,
        ResolveInputs {
            component,
            package,
            backend: backend_name.to_string(),
            base_url,
            version: args.version.as_deref(),
            warnings,
        },
    )?;

    if ctx.dry_run {
        let preview = build_install_preview(ctx, layout, resolved)?;
        render_plan(ctx, &preview)?;
        return Ok(InstallOutcome::Installed);
    }

    let prepared = prepare_raw_execution(ctx, layout, resolved)?;
    execute_raw(ctx, layout, command, prepared)?;
    Ok(InstallOutcome::Installed)
}

// ── rpm adopt path (#958) ───────────────────────────────────────────

/// Result of probing whether `component` is present as a system RPM (§5/§7.1).
enum RpmSituation {
    /// Exactly one candidate package name, installed once — ready to adopt.
    Adoptable {
        /// Resolved package name (the candidate that hit rpmdb).
        package: String,
        /// rpmdb query result carrying EVR/arch for the state record.
        info: PackageInfo,
    },
    /// Not present as a system RPM: the single candidate is not installed, or
    /// the host has no rpm tooling at all. Layer 1 falls through to the
    /// default backend; an explicit `--backend rpm` turns this into the
    /// "dnf install not implemented" hint (§7.4).
    Absent,
    /// `provides` reverse-lookup matched several distinct installed packages
    /// (§5.5). Reported, never silently adopted.
    Ambiguous(Vec<String>),
    /// The candidate resolved but rpmdb holds several installed versions of it
    /// (`UnexpectedOutput`, §5.5) — a drift state, not a clean adopt target.
    MultiVersion(String),
}

/// Resolve the candidate RPM package name(s) for `component` and probe rpmdb.
///
/// Errors only when a query hard-fails in a way that is not the host's normal
/// "absent" signal; a missing `rpm`/`dnf` binary is treated as [`Absent`]
/// (the host is not RPM-based, so no component can be a system RPM there).
///
/// [`Absent`]: RpmSituation::Absent
fn probe_rpm_situation(
    component: &str,
    args: &InstallArgs,
    repo_config: &RepoConfig,
    ctx: &CliContext,
    query: &dyn PackageQuery,
    command: &str,
) -> Result<RpmSituation, CliError> {
    let manifest = load_optional_manifest(ctx, component);
    let rpm_backend = repo_config.backends.get("rpm");
    let candidates = rpm_package_candidates(
        args.package.as_deref(),
        manifest.as_ref(),
        rpm_backend,
        query,
        component,
    )
    .map_err(|err| pkg_query_err(err, command))?;

    if candidates.len() >= 2 {
        return Ok(RpmSituation::Ambiguous(candidates));
    }
    // `rpm_package_candidates` always backfills the default name, so exactly
    // one candidate remains here.
    let package = candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| format!("anolisa-{component}"));

    match query.query_installed(&package) {
        Ok(Some(info)) => Ok(RpmSituation::Adoptable { package, info }),
        Ok(None) => Ok(RpmSituation::Absent),
        // Same name, several installed versions: a drift the caller reports.
        Err(PackageQueryError::UnexpectedOutput { .. }) => Ok(RpmSituation::MultiVersion(package)),
        // No rpm/dnf on this host → it cannot host system RPMs at all.
        Err(PackageQueryError::CommandMissing { .. }) => Ok(RpmSituation::Absent),
        Err(err) => Err(pkg_query_err(err, command)),
    }
}

/// Resolve candidate RPM package names for `component`, highest precedence
/// first (§5): CLI `--package` > manifest `[backends.rpm].package` > repo.toml
/// `package_map` > RPM `provides` reverse-lookup > default `anolisa-<name>`.
///
/// The first four levels short-circuit to a single candidate; only the
/// `provides` level can yield several (the ambiguous case, §5.5). The default
/// name backfills whenever `provides` is empty, so the result is never empty.
///
/// # Errors
/// Propagates a hard [`PackageQueryError`] from the `provides` query; an empty
/// `provides` result is the normal "no contract / no match" branch and falls
/// through to default naming.
pub(crate) fn rpm_package_candidates(
    cli_override: Option<&str>,
    manifest: Option<&ComponentManifest>,
    rpm_backend: Option<&BackendConfig>,
    query: &dyn PackageQuery,
    component: &str,
) -> Result<Vec<String>, PackageQueryError> {
    if let Some(name) = cli_override {
        return Ok(vec![name.to_string()]);
    }
    if let Some(name) = manifest.and_then(ComponentManifest::rpm_package) {
        return Ok(vec![name.to_string()]);
    }
    if let Some(mapped) = rpm_backend.and_then(|b| b.package_map.get(component)) {
        return Ok(vec![mapped.clone()]);
    }
    // Virtual-provides contract (`anolisa-component(<name>)`). It does not yet
    // exist on the packaging side, so this returns empty today and the chain
    // falls through to default naming — by design, not an error (§5.3).
    let provides = query.what_provides_installed(&format!("anolisa-component({component})"))?;
    if !provides.is_empty() {
        return Ok(provides);
    }
    Ok(vec![format!("anolisa-{component}")])
}

/// Best-effort lookup of a component's manifest from the bundled catalog, for
/// the `[backends.rpm].package` mapping level. Adopt does not require a local
/// manifest (§5.1): any failure or miss yields `None` and the package-name
/// chain falls through to the lower tiers.
fn load_optional_manifest(ctx: &CliContext, component: &str) -> Option<ComponentManifest> {
    let catalog = common::load_bundled_catalog(ctx, COMMAND).ok()?;
    catalog.component(component).cloned()
}

/// Layer 2 for the `rpm` backend: reject in user mode, otherwise adopt an
/// installed package, or surface the ambiguous / drift / not-yet-installed
/// cases (§7.1). `situation` is reused from layer 1's probe when present
/// (the `SystemRpm` source), and computed here otherwise (`Explicit` rpm).
#[allow(clippy::too_many_arguments)]
fn route_rpm_adopt(
    component: &str,
    args: &InstallArgs,
    ctx: &CliContext,
    command: &str,
    layout: &FsLayout,
    repo_config: &RepoConfig,
    installed: &InstalledState,
    source: BackendSource,
    situation: Option<RpmSituation>,
    query: &dyn PackageQuery,
) -> Result<InstallOutcome, CliError> {
    // Adopt is a system-scope action (§4.1). The only way to reach `rpm` in
    // user mode is an explicit `--backend rpm`, which we reject rather than
    // record a system RPM into user-scope state.
    if ctx.install_mode != InstallMode::System {
        return Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "--backend rpm adopts a system RPM and requires system scope; re-run with --install-mode system (got {} mode)",
                ctx.install_mode.as_str()
            ),
        });
    }

    // Explicit `--backend rpm` may switch an already-installed component's
    // provenance; reuse the same guard the raw path uses.
    if source == BackendSource::Explicit {
        ensure_component_backend_compatible(installed, component, "rpm", command)?;
    }

    let situation = match situation {
        Some(s) => s,
        None => probe_rpm_situation(component, args, repo_config, ctx, query, command)?,
    };

    match situation {
        RpmSituation::Adoptable { package, info } => {
            execute_adopt(ctx, layout, command, component, package, info, query)
        }
        RpmSituation::Absent => Err(CliError::not_implemented_with_hint(
            format!("install --backend rpm {component}"),
            "--backend rpm requires the package to be installed for adopt; delegated 'dnf install' is not implemented yet (tracked in #959)"
                .to_string(),
        )),
        RpmSituation::Ambiguous(names) => Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "multiple installed RPMs provide component '{component}': {}; cannot adopt unambiguously — pin one with `--package <name>` or fix the manifest/package_map mapping",
                names.join(", ")
            ),
        }),
        RpmSituation::MultiVersion(package) => Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "RPM package '{package}' has multiple installed versions; refusing to adopt a single version automatically — resolve the duplicate first",
            ),
        }),
    }
}

/// Wire shape for an adopt result (`--json`) and its dry-run preview.
#[derive(Serialize)]
struct AdoptResultPayload {
    component: String,
    package: String,
    backend: &'static str,
    /// Always `rpm-observed`: adopt only records observation, never ownership.
    ownership: &'static str,
    version: String,
    arch: Option<String>,
    source_repo: Option<String>,
    install_mode: String,
    /// `None` on dry-run (nothing written).
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    dry_run: bool,
    warnings: Vec<String>,
}

/// Record an installed system RPM as `rpm-observed` state (§7.2). Fetches
/// nothing, writes no owned files, touches no RPM-owned paths — only rpmdb
/// reads plus a state write. On `--dry-run` it renders the plan without
/// writing.
fn execute_adopt(
    ctx: &CliContext,
    layout: &FsLayout,
    command: &str,
    component: &str,
    package: String,
    info: PackageInfo,
    query: &dyn PackageQuery,
) -> Result<InstallOutcome, CliError> {
    let mut warnings: Vec<String> = Vec::new();
    // source_repo is supplementary metadata: a failed origin lookup degrades
    // to `None` with a warning and never fails the adopt (§7.2).
    let source_repo = match query.installed_origin(&package) {
        Ok(origin) => origin,
        Err(err) => {
            warnings.push(format!(
                "could not determine source repo for '{package}': {err}"
            ));
            None
        }
    };
    let evr = info.version.to_string();

    let mut payload = AdoptResultPayload {
        component: component.to_string(),
        package: package.clone(),
        backend: "rpm",
        ownership: "rpm-observed",
        version: evr.clone(),
        arch: Some(info.arch.clone()),
        source_repo: source_repo.clone(),
        install_mode: ctx.install_mode.as_str().to_string(),
        operation_id: None,
        dry_run: ctx.dry_run,
        warnings: warnings.clone(),
    };

    if ctx.dry_run {
        render_adopt(ctx, &payload);
        return Ok(InstallOutcome::Adopted);
    }

    // Acquire the lock, then load state inside it so a concurrent writer is
    // not clobbered — mirrors `execute_raw`'s ordering.
    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;
    let mut state =
        common::load_installed_state(ctx, command).map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!("failed to load installed state: {err}"),
        })?;

    let started_at = now_iso8601();
    let lock_ts = Utc::now();
    let operation_id = format!(
        "op-install-{}-{}",
        lock_ts.format("%Y%m%d%H%M%S"),
        lock_ts.timestamp_subsec_nanos()
    );

    // Adopt is system-scope by construction (route_rpm_adopt rejects user mode).
    state.install_mode = StateInstallMode::System;
    state.prefix = layout.prefix.clone();
    state.upsert_object(InstalledObject {
        kind: ObjectKind::Component,
        name: component.to_string(),
        // EVR form, the observed version.
        version: evr.clone(),
        // `Adopted` is the lifecycle status (state.rs); `RpmObserved` below is
        // the orthogonal provenance. Together they model proposal §12 Adopted.
        status: ObjectStatus::Adopted,
        manifest_digest: None,
        // Not an ANOLISA-delivered artifact.
        distribution_source: None,
        install_backend: Some("rpm".to_string()),
        ownership: Some(Ownership::RpmObserved),
        rpm_metadata: Some(RpmMetadata {
            package_name: info.name.clone(),
            evr: Some(evr.clone()),
            arch: Some(info.arch.clone()),
            source_repo: source_repo.clone(),
        }),
        installed_at: started_at.clone(),
        last_operation_id: Some(operation_id.clone()),
        // ANOLISA does not own the file transaction (owns_removal=false).
        managed: false,
        // Audit/UI vocabulary: explicit adoption.
        adopted: true,
        subscription_scope: Default::default(),
        enabled_features: Vec::new(),
        component_refs: Vec::new(),
        // RPM-owned files stay out of ANOLISA owned-files: status/uninstall
        // must not treat them as ANOLISA-owned.
        files: Vec::new(),
        external_modified_files: Vec::new(),
        services: Vec::new(),
        health: Vec::new(),
    });
    state.operations.push(OperationRecord {
        id: operation_id.clone(),
        command: command.to_string(),
        status: "ok".to_string(),
        started_at: started_at.clone(),
        finished_at: Some(now_iso8601()),
    });

    let state_path = layout.state_dir.join("installed.toml");
    state.save(&state_path).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to save state: {err}"),
    })?;

    // Audit log is best-effort: the adopt already persisted, so a log failure
    // downgrades to a warning instead of unwinding.
    let log = CentralLog::open(layout.central_log.clone());
    let record = LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.clone()),
        command: command.to_string(),
        source: "anolisa-cli".to_string(),
        component: Some(component.to_string()),
        severity: Severity::Info,
        message: format!(
            "adopted existing RPM package {package} ({evr}) as rpm-observed for component {component}"
        ),
        actor: "cli".to_string(),
        install_mode: Some(ctx.install_mode.as_str().to_string()),
        started_at,
        finished_at: Some(now_iso8601()),
        status: Some(LogStatus::Ok),
        objects: vec![component.to_string()],
        backup_ids: Vec::new(),
        warnings: warnings.clone(),
        details: serde_json::Value::Null,
    };
    if let Err(err) = log.append(&record) {
        eprintln!("warning: failed to write central log: {err}");
    }

    payload.operation_id = Some(operation_id);
    render_adopt(ctx, &payload);
    Ok(InstallOutcome::Adopted)
}

/// Render an adopt result (JSON envelope or the proposal §6.1 human text).
/// Silent in quiet mode; the `--all` batch path drives its own summary.
fn render_adopt(ctx: &CliContext, payload: &AdoptResultPayload) {
    if ctx.json {
        // Errors here are unreachable for a plain Serialize struct; ignore the
        // Result so the (already-persisted) adopt is not reported as failed.
        let _ = render_json(COMMAND, payload);
        return;
    }
    if ctx.quiet {
        return;
    }
    let color = Palette::new(ctx.no_color);
    let repo = payload.source_repo.as_deref().unwrap_or("unknown repo");
    let suffix = if payload.dry_run {
        " (dry-run — nothing recorded)"
    } else {
        ""
    };
    println!(
        "{} {} ({}, {}){}",
        color.label("Detected existing RPM package:"),
        payload.package,
        payload.version,
        repo,
        color.muted(suffix),
    );
    println!(
        "{}",
        color.ok("Adopted as rpm-observed. ANOLISA will not replace it with raw.")
    );
    render_warnings(&payload.warnings, &color);
}

/// Map a [`PackageQueryError`] onto a CLI error. Spawn/permission/query
/// failures are runtime faults; output-shape problems are runtime faults too
/// (the caller has already split off the benign "not installed" branches).
fn pkg_query_err(err: PackageQueryError, command: &str) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: format!("rpm query failed: {err}"),
    }
}

// ── --all support ───────────────────────────────────────────────────

/// Minimal catalog shape used by `--all`. We only need the component name
/// and (optionally) the `available` status, so this is a thin parse rather
/// than a re-use of `list.rs`'s richer types. Keeping it local avoids a
/// cross-module type dependency for one entry point.
#[derive(Debug, Deserialize)]
struct AllCatalogV1 {
    schema_version: u32,
    #[serde(default)]
    components: Vec<AllCatalogEntry>,
}

#[derive(Debug, Deserialize)]
struct AllCatalogEntry {
    name: String,
    #[serde(default)]
    status: Option<String>,
}

/// Wire shape for a batch entry.  `status` is one of:
/// `installed` | `planned` (dry-run) | `adopted` | `adopt-planned` (dry-run) |
/// `failed` | `skipped`.
#[derive(Serialize)]
struct AllSummaryItem {
    component: String,
    status: &'static str,
    reason: Option<String>,
}

#[derive(Serialize)]
struct AllSummaryPayload {
    total: usize,
    installed: usize,
    planned: usize,
    /// Existing system RPMs recorded as rpm-observed (§7.5).
    adopted: usize,
    /// Dry-run adopt previews.
    adopt_planned: usize,
    failed: usize,
    skipped: usize,
    dry_run: bool,
    items: Vec<AllSummaryItem>,
}

fn handle_all(args: InstallArgs, ctx: &CliContext) -> Result<(), CliError> {
    let names = resolve_all_components(ctx)?;
    if names.is_empty() {
        if !ctx.quiet && !ctx.json {
            let color = Palette::new(ctx.no_color);
            println!(
                "{}",
                color.muted("no available components in catalog; nothing to install")
            );
        }
        if ctx.json {
            return render_json(
                "install --all",
                AllSummaryPayload {
                    total: 0,
                    installed: 0,
                    planned: 0,
                    adopted: 0,
                    adopt_planned: 0,
                    failed: 0,
                    skipped: 0,
                    dry_run: ctx.dry_run,
                    items: Vec::new(),
                },
            );
        }
        return Ok(());
    }

    // Suppress per-component rendering: handle_all owns the final output.
    // Each handle_one call runs in quiet mode so it doesn't print individual
    // JSON envelopes or human-mode messages — only the batch summary at the
    // end goes to stdout.
    let suppressed_ctx = CliContext {
        json: false,
        quiet: true,
        ..ctx.clone()
    };

    let mut items: Vec<AllSummaryItem> = Vec::with_capacity(names.len());
    let mut first_error: Option<CliError> = None;
    let mut last_processed = 0usize;

    for (idx, name) in names.iter().enumerate() {
        last_processed = idx;
        if !ctx.quiet && !ctx.json {
            let color = Palette::new(ctx.no_color);
            println!("{} {name}", color.label("==>"));
        }
        let per_args = InstallArgs {
            component: Some(name.clone()),
            all: false,
            fail_fast: false,
            version: None,
            backend: args.backend.clone(),
            repo: args.repo.clone(),
            package: None,
        };
        match handle_one(name.clone(), per_args, &suppressed_ctx) {
            // Map (outcome, dry-run) to a batch status string so the summary
            // distinguishes a fresh install from an RPM adopt (§7.5). Dry-run
            // successes are "planned"/"adopt-planned": nothing was written.
            Ok(outcome) => items.push(AllSummaryItem {
                component: name.clone(),
                status: batch_status(outcome, ctx.dry_run),
                reason: None,
            }),
            Err(err) => {
                let reason = err.reason().to_string();
                items.push(AllSummaryItem {
                    component: name.clone(),
                    status: "failed",
                    reason: Some(reason),
                });
                if first_error.is_none() {
                    first_error = Some(err);
                }
                if args.fail_fast {
                    break;
                }
            }
        }
    }

    // --fail-fast may have left components unprocessed.  Mark them as
    // skipped so `total` always equals the full target set.
    for name in &names[last_processed + 1..] {
        items.push(AllSummaryItem {
            component: name.clone(),
            status: "skipped",
            reason: Some("--fail-fast: not attempted".to_string()),
        });
    }

    let installed = items.iter().filter(|i| i.status == "installed").count();
    let planned = items.iter().filter(|i| i.status == "planned").count();
    let adopted = items.iter().filter(|i| i.status == "adopted").count();
    let adopt_planned = items.iter().filter(|i| i.status == "adopt-planned").count();
    let failed = items.iter().filter(|i| i.status == "failed").count();
    let skipped = items.iter().filter(|i| i.status == "skipped").count();

    if ctx.json {
        // The batch summary is the single, complete JSON response.  We
        // return BatchPartial (not Ok) so that main's render_error still
        // sets a non-zero exit code — but render_error recognises
        // BatchPartial and skips the second JSON render.
        render_json_with_status(
            "install --all",
            failed == 0,
            AllSummaryPayload {
                total: names.len(),
                installed,
                planned,
                adopted,
                adopt_planned,
                failed,
                skipped,
                dry_run: ctx.dry_run,
                items,
            },
        )?;
        return match first_error {
            Some(_) => Err(CliError::BatchPartial {
                command: "install --all".to_string(),
            }),
            None => Ok(()),
        };
    }

    if !ctx.quiet {
        let color = Palette::new(ctx.no_color);
        println!();
        let failed_names: Vec<&str> = items
            .iter()
            .filter(|i| i.status == "failed")
            .map(|i| i.component.as_str())
            .collect();
        let ok_word = if ctx.dry_run { "planned" } else { "installed" };
        let ok_count = if ctx.dry_run { planned } else { installed };
        // Adopts are a distinct outcome from installs; show them as their own
        // segment (and only when non-zero) so the count isn't lost (§7.5).
        let adopt_word = if ctx.dry_run {
            "adopt-planned"
        } else {
            "adopted"
        };
        let adopt_count = if ctx.dry_run { adopt_planned } else { adopted };
        let adopt_segment = if adopt_count > 0 {
            format!("  {adopt_word}={adopt_count}")
        } else {
            String::new()
        };
        if failed_names.is_empty() {
            println!(
                "{} total={}  {ok_word}={}{adopt_segment}  skipped={}",
                color.label("summary:"),
                names.len(),
                ok_count,
                skipped,
            );
        } else {
            println!(
                "{} total={}  {ok_word}={}{adopt_segment}  failed={} ({})  skipped={}",
                color.label("summary:"),
                names.len(),
                ok_count,
                failed,
                failed_names.join(", "),
                skipped,
            );
            for item in items.iter().filter(|i| i.status == "failed") {
                if let Some(reason) = &item.reason {
                    eprintln!("{} {}: {reason}", color.err("failed:"), item.component);
                }
            }
        }
        // List adopted components explicitly so `--all` shows which were
        // taken over rather than freshly installed.
        for item in items
            .iter()
            .filter(|i| i.status == "adopted" || i.status == "adopt-planned")
        {
            println!(
                "{} {}",
                color.label("adopted rpm-observed:"),
                item.component
            );
        }
    }

    // Human mode: preserve non-zero exit code on failure.
    match first_error {
        Some(_) => Err(CliError::BatchPartial {
            command: "install --all".to_string(),
        }),
        None => Ok(()),
    }
}

/// Batch status string for a successful `handle_one`, combining the outcome
/// with dry-run. Kept aligned with the `filter`-by-string counting in
/// [`handle_all`] (§7.5): a new string here must be matched there too.
fn batch_status(outcome: InstallOutcome, dry_run: bool) -> &'static str {
    match (outcome, dry_run) {
        (InstallOutcome::Installed, false) => "installed",
        (InstallOutcome::Installed, true) => "planned",
        (InstallOutcome::Adopted, false) => "adopted",
        (InstallOutcome::Adopted, true) => "adopt-planned",
    }
}

/// Fetch and parse the component catalog, returning the names of components
/// whose `status` is `available`. Returns an error if the catalog URL is not
/// configured or the catalog cannot be fetched/parsed.
fn resolve_all_components(ctx: &CliContext) -> Result<Vec<String>, CliError> {
    let url = common::resolve_catalog_url(ctx, "install --all")?.ok_or_else(|| {
        CliError::InvalidArgument {
            command: "install --all".to_string(),
            reason: "component catalog is not configured; set ANOLISA_CATALOG_URL or \
                 configure [backends.raw].base_url in repo.toml"
                .to_string(),
        }
    })?;
    let bytes = common::fetch_catalog_bytes(&url, "install --all")?;
    let catalog: AllCatalogV1 =
        serde_json::from_slice(&bytes).map_err(|err| CliError::InvalidArgument {
            command: "install --all".to_string(),
            reason: format!("failed to parse component catalog JSON: {err}"),
        })?;
    if catalog.schema_version != 1 {
        return Err(CliError::InvalidArgument {
            command: "install --all".to_string(),
            reason: format!(
                "unsupported component catalog schema_version {}; expected 1",
                catalog.schema_version
            ),
        });
    }
    let mut names: Vec<String> = Vec::new();
    for entry in catalog.components {
        if entry.name.trim().is_empty() {
            if !ctx.quiet {
                eprintln!("warning: catalog contains an entry with an empty name; skipping");
            }
            continue;
        }
        if entry.status.as_deref() == Some("available") {
            names.push(entry.name);
        }
    }
    Ok(names)
}

/// Caller-side inputs to [`resolve_raw`], grouped to keep the signature flat.
struct ResolveInputs<'a> {
    component: String,
    package: String,
    backend: String,
    base_url: String,
    version: Option<&'a str>,
    warnings: Vec<String>,
}

/// Resolve raw backend metadata without fetching the artifact.
///
/// This fetches the distribution index into the download cache, selects a
/// supported artifact, and derives the artifact URL. Execution later
/// downloads the artifact and reads its install contract; dry-run may read
/// lightweight `meta.toml` metadata for a richer preview.
fn resolve_raw(
    ctx: &CliContext,
    layout: &FsLayout,
    env: &anolisa_env::EnvFacts,
    inputs: ResolveInputs<'_>,
) -> Result<RawResolution, CliError> {
    let ResolveInputs {
        component,
        package,
        backend,
        base_url,
        version,
        warnings,
    } = inputs;

    // The index is always re-fetched (DownloadCache overwrites on conflict),
    // so a republished repo is picked up without a cache flush.
    let index_url = raw_index_url(&base_url);
    let cache = DownloadCache::new(layout.cache_dir.clone());
    let downloaded_index = cache
        .fetch(&index_url, None)
        .map_err(|err| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!("failed to fetch distribution index {index_url}: {err}"),
        })?;
    let index = DistributionIndex::load(&downloaded_index.cached_path).map_err(|err| {
        CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!("failed to parse distribution index {index_url}: {err}"),
        }
    })?;

    // The index is keyed by the backend-native package name so that
    // `package_map` / `--package` select between alternate publications.
    let query = ResolveQuery {
        component: &package,
        version,
        channel: None,
        install_mode: ctx.install_mode.as_str(),
        os: &env.os,
        arch: &env.arch,
        libc: env.libc.as_deref(),
        pkg_base: env.pkg_base.as_deref(),
        preferred_types: &[],
    };
    let entry = index.resolve(&query).map_err(|err| CliError::InvalidArgument {
        command: COMMAND.to_string(),
        reason: format!(
            "cannot resolve package '{package}' (component '{component}', version {}, {}/{}, {} mode) from {index_url}: {err}",
            version.unwrap_or("latest"),
            env.os,
            env.arch,
            ctx.install_mode.as_str(),
        ),
    })?;

    let wire_type = artifact_type_wire(&entry.artifact_type);
    if !SUPPORTED_ARTIFACT_TYPES.contains(&wire_type) {
        return Err(CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!(
                "resolved artifact type '{wire_type}' is not installable by the raw backend (supported: {})",
                SUPPORTED_ARTIFACT_TYPES.join(", ")
            ),
        });
    }
    // Three URL forms, most-mirror-friendly first: an omitted url uses the
    // code-owned raw layout, a repo-relative url resolves against the index
    // directory (self-contained mirrors), and an absolute url is used as-is
    // (escape hatch for off-repo artifacts).
    let artifact_url = if entry.url.is_empty() {
        let values = std::collections::BTreeMap::from([
            ("component", Some(entry.component.clone())),
            ("version", Some(entry.version.clone())),
            ("os", Some(entry.os.clone())),
            ("arch", Some(entry.arch.clone())),
            ("libc", entry.libc.clone()),
            ("ext", Some(artifact_ext(&entry.artifact_type).to_string())),
        ]);
        raw_artifact_url(&backend, &base_url, &values).map_err(|err| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "cannot derive artifact URL for '{package}' {} from raw repository layout: {err}",
                entry.version
            ),
        })?
    } else if entry.url.contains("://") {
        entry.url.clone()
    } else {
        format!(
            "{}/{}",
            raw_relative_root(&base_url),
            entry.url.trim_start_matches('/')
        )
    };

    Ok(RawResolution {
        component,
        package,
        backend,
        base_url,
        artifact_url,
        entry,
        warnings,
    })
}

impl InstallContractSource {
    fn label(self) -> &'static str {
        match self {
            Self::EmbeddedArtifact => "embedded artifact manifest",
            Self::SidecarMeta => "sidecar meta.toml",
            Self::LocalCatalog => "local catalog manifest",
        }
    }
}

fn build_install_preview(
    ctx: &CliContext,
    layout: &FsLayout,
    mut resolution: RawResolution,
) -> Result<InstallPreview, CliError> {
    if resolution.entry.sha256.is_none() {
        resolution.warnings.push(format!(
            "distribution entry for '{}' {} has no sha256; execute will refuse to install it",
            resolution.package, resolution.entry.version
        ));
    }

    let Some(contract) = load_lightweight_install_contract(ctx, layout, &resolution)? else {
        resolution.warnings.push(format!(
            "dry-run did not download artifact {}; file and service details are unavailable",
            resolution.artifact_url
        ));
        return Ok(InstallPreview {
            resolution,
            files: Vec::new(),
            services: Vec::new(),
        });
    };

    let (files, services) = match resolve_manifest_contract(
        &contract.manifest,
        layout,
        &resolution,
        ctx.install_mode.as_str(),
        contract.source,
    ) {
        Ok(contract_files) => contract_files,
        Err(err) if contract.source == InstallContractSource::LocalCatalog => {
            resolution.warnings.push(format!(
                "local catalog manifest does not match resolved artifact; file and service details are unavailable: {}",
                err.reason()
            ));
            (Vec::new(), Vec::new())
        }
        Err(err) => return Err(err),
    };

    Ok(InstallPreview {
        resolution,
        files,
        services,
    })
}

fn prepare_raw_execution(
    ctx: &CliContext,
    layout: &FsLayout,
    resolution: RawResolution,
) -> Result<PreparedInstall, CliError> {
    let sha256 = resolution.entry.sha256.as_deref().ok_or_else(|| {
        CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "distribution entry for '{}' {} has no sha256 — refusing to install an unverifiable artifact",
                resolution.package, resolution.entry.version
            ),
        }
    })?;

    let cache = DownloadCache::new(layout.cache_dir.clone());
    let artifact = cache
        .fetch(&resolution.artifact_url, Some(sha256))
        .map_err(|err| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "failed to download artifact {}: {err}",
                resolution.artifact_url
            ),
        })?;

    let contract =
        load_execution_install_contract(ctx, layout, &resolution, &artifact.cached_path)?;
    let (files, services) = resolve_manifest_contract(
        &contract.manifest,
        layout,
        &resolution,
        ctx.install_mode.as_str(),
        contract.source,
    )?;

    Ok(PreparedInstall {
        resolution,
        artifact_path: artifact.cached_path,
        files,
        services,
        manifest_toml: contract.toml,
    })
}

fn load_execution_install_contract(
    ctx: &CliContext,
    layout: &FsLayout,
    resolution: &RawResolution,
    artifact_path: &Path,
) -> Result<LoadedInstallContract, CliError> {
    match resolution.entry.artifact_type {
        ArtifactType::TarGz => {
            let toml = read_embedded_component_manifest_text(artifact_path)
                .map_err(|err| CliError::Runtime {
                    command: COMMAND.to_string(),
                    reason: format!(
                        "failed to read embedded component manifest from {}: {err}",
                        resolution.artifact_url
                    ),
                })?
                .ok_or_else(|| CliError::Runtime {
                    command: COMMAND.to_string(),
                    reason: format!(
                        "published artifact for package '{}' has no embedded .anolisa/component.toml",
                        resolution.package
                    ),
                })?;
            let manifest = ComponentManifest::from_toml_str(&toml).map_err(|err| {
                CliError::Runtime {
                    command: COMMAND.to_string(),
                    reason: format!(
                        "failed to parse embedded component manifest from {}: {err}",
                        resolution.artifact_url
                    ),
                }
            })?;
            Ok(LoadedInstallContract {
                manifest,
                source: InstallContractSource::EmbeddedArtifact,
                toml,
            })
        }
        ArtifactType::Binary => {
            load_lightweight_install_contract(ctx, layout, resolution)?.ok_or_else(|| {
                CliError::Runtime {
                    command: COMMAND.to_string(),
                    reason: format!(
                        "binary artifact for package '{}' {} requires sidecar meta.toml or a matching local component manifest",
                        resolution.package, resolution.entry.version
                    ),
                }
            })
        }
        other => Err(CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!(
                "resolved artifact type '{}' is not installable by the raw backend (supported: {})",
                artifact_type_wire(&other),
                SUPPORTED_ARTIFACT_TYPES.join(", ")
            ),
        }),
    }
}

fn load_lightweight_install_contract(
    ctx: &CliContext,
    layout: &FsLayout,
    resolution: &RawResolution,
) -> Result<Option<LoadedInstallContract>, CliError> {
    if let Some(contract) = fetch_sidecar_meta_manifest(layout, resolution)? {
        return Ok(Some(contract));
    }

    load_catalog_manifest(ctx, &resolution.component)
}

fn fetch_sidecar_meta_manifest(
    layout: &FsLayout,
    resolution: &RawResolution,
) -> Result<Option<LoadedInstallContract>, CliError> {
    let Some(meta_url) = sidecar_meta_url(
        &resolution.artifact_url,
        &resolution.entry.component,
        &resolution.entry.version,
    ) else {
        return Ok(None);
    };
    let expected_sha = manifest_digest_sha256(resolution.entry.manifest_digest.as_deref())?;
    let cache = DownloadCache::new(layout.cache_dir.clone());
    let downloaded = match cache.fetch(&meta_url, expected_sha) {
        Ok(downloaded) => downloaded,
        Err(DownloadError::HttpStatus { status: 404, .. }) => return Ok(None),
        Err(DownloadError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(err) => {
            return Err(CliError::Runtime {
                command: COMMAND.to_string(),
                reason: format!("failed to fetch sidecar metadata {meta_url}: {err}"),
            });
        }
    };
    let toml =
        std::fs::read_to_string(&downloaded.cached_path).map_err(|err| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "failed to read sidecar metadata {} from cache: {err}",
                downloaded.cached_path.display()
            ),
        })?;
    let manifest = ComponentManifest::from_toml_str(&toml).map_err(|err| CliError::Runtime {
        command: COMMAND.to_string(),
        reason: format!("failed to parse sidecar metadata {meta_url}: {err}"),
    })?;
    Ok(Some(LoadedInstallContract {
        manifest,
        source: InstallContractSource::SidecarMeta,
        toml,
    }))
}

fn load_catalog_manifest(
    ctx: &CliContext,
    component: &str,
) -> Result<Option<LoadedInstallContract>, CliError> {
    let catalog = common::load_bundled_catalog(ctx, COMMAND)?;
    let Some(manifest) = catalog.component(component).cloned() else {
        return Ok(None);
    };
    let toml = serialize_manifest_toml(&manifest, InstallContractSource::LocalCatalog)?;
    Ok(Some(LoadedInstallContract {
        manifest,
        source: InstallContractSource::LocalCatalog,
        toml,
    }))
}

fn serialize_manifest_toml(
    manifest: &ComponentManifest,
    source: InstallContractSource,
) -> Result<String, CliError> {
    toml::to_string_pretty(manifest).map_err(|err| CliError::Runtime {
        command: COMMAND.to_string(),
        reason: format!(
            "failed to serialize {} for local install metadata: {err}",
            source.label()
        ),
    })
}

fn manifest_digest_sha256(digest: Option<&str>) -> Result<Option<&str>, CliError> {
    match digest {
        None => Ok(None),
        Some(value) => value
            .strip_prefix("sha256:")
            .map(Some)
            .ok_or_else(|| CliError::Runtime {
                command: COMMAND.to_string(),
                reason: format!(
                    "unsupported manifest_digest '{value}' for sidecar metadata verification"
                ),
            }),
    }
}

fn sidecar_meta_url(artifact_url: &str, component: &str, version: &str) -> Option<String> {
    let version_marker = format!("/{component}/{version}/");
    if let Some(idx) = artifact_url.rfind(&version_marker) {
        return Some(format!(
            "{}meta.toml",
            &artifact_url[..idx + version_marker.len()]
        ));
    }

    artifact_url
        .rfind('/')
        .map(|idx| format!("{}/meta.toml", &artifact_url[..idx]))
}

fn resolve_manifest_contract(
    manifest: &ComponentManifest,
    layout: &FsLayout,
    resolution: &RawResolution,
    mode: &str,
    source: InstallContractSource,
) -> Result<(Vec<ResolvedInstallFile>, Vec<String>), CliError> {
    if manifest.component.name.as_str() != resolution.component {
        return Err(CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "{} for package '{}' declares component '{}', expected '{}'",
                source.label(),
                resolution.package,
                manifest.component.name,
                resolution.component
            ),
        });
    }
    if manifest.component.version.as_str() != resolution.entry.version.as_str() {
        return Err(CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "{} for component '{}' declares version {}, but the distribution index resolved {}",
                source.label(),
                resolution.component,
                manifest.component.version,
                resolution.entry.version
            ),
        });
    }

    if !manifest.install.modes.iter().any(|m| m == mode) {
        return Err(CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!(
                "{} for component '{}' is inconsistent with the distribution index: index resolved {mode}-mode support, but manifest declares modes: {}",
                source.label(),
                resolution.component,
                manifest.install.modes.join(", ")
            ),
        });
    }

    let mut files = resolve_manifest_files(manifest, layout, &resolution.component)?;
    if files.is_empty() {
        return Err(CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "component '{}' declares no [install.files] — nothing to install",
                resolution.component
            ),
        });
    }
    // Adapter resources are laid alongside the component's own files, from
    // the same artifact. Install only *places* them under the standard
    // `{datadir}/adapters/<component>/<framework>/` tree — enabling them
    // against a framework is the separate `anolisa adapter enable` step.
    files.extend(resolve_adapter_files(
        manifest,
        layout,
        &resolution.component,
    )?);

    Ok((files, manifest.install.services.clone()))
}

/// Render the manifest's `[install.files]` against the layout: expand
/// `{bindir}`-style placeholders and reject any destination escaping the
/// ANOLISA-owned roots before a single byte is written.
fn resolve_manifest_files(
    manifest: &ComponentManifest,
    layout: &FsLayout,
    component: &str,
) -> Result<Vec<ResolvedInstallFile>, CliError> {
    let mut files = Vec::with_capacity(manifest.install.files.len());
    for spec in &manifest.install.files {
        let template = spec.install_path().ok_or_else(|| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "component '{component}' has an [install.files] entry with neither source nor dest"
            ),
        })?;
        let dest = expand_layout_placeholders(template, layout, &[("component", component)])
            .map_err(|err| CliError::Runtime {
                command: COMMAND.to_string(),
                reason: format!("failed to expand install path '{template}': {err}"),
            })?;
        validate_owned_path(layout, &dest).map_err(|err| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "install destination '{}' failed path safety check: {err}",
                dest.display()
            ),
        })?;
        // A symlink's source is its referent — a layout template like the
        // dest, not an archive path. Expand and bound-check it the same way.
        let source = match (spec.kind, spec.source.as_deref()) {
            (FileKind::Symlink, Some(template)) => {
                let referent =
                    expand_layout_placeholders(template, layout, &[("component", component)])
                        .map_err(|err| CliError::Runtime {
                            command: COMMAND.to_string(),
                            reason: format!(
                                "failed to expand symlink referent '{template}': {err}"
                            ),
                        })?;
                validate_owned_path(layout, &referent).map_err(|err| CliError::Runtime {
                    command: COMMAND.to_string(),
                    reason: format!(
                        "symlink referent '{}' failed path safety check: {err}",
                        referent.display()
                    ),
                })?;
                Some(referent.to_string_lossy().into_owned())
            }
            _ => spec.source.clone(),
        };
        files.push(ResolvedInstallFile {
            source,
            dest,
            mode: spec.mode.clone(),
            kind: spec.kind,
        });
    }
    Ok(files)
}

/// Render the manifest's `[[adapters]]` entries into install file mappings.
///
/// Install only *places* adapter resources under the standard
/// `{datadir}/adapters/<component>/<framework>/` tree; it never runs a
/// framework CLI or touches user framework state — that is
/// `anolisa adapter enable`.
///
/// Each entry is linted up front for the fields install needs: a framework,
/// a source, and a destination. The framework does not have to be supported
/// by this ANOLISA build; install only lays data down, while
/// `anolisa adapter enable` decides whether a built-in driver exists.
fn resolve_adapter_files(
    manifest: &ComponentManifest,
    layout: &FsLayout,
    component: &str,
) -> Result<Vec<ResolvedInstallFile>, CliError> {
    if manifest.adapters.is_empty() {
        return Ok(Vec::new());
    }
    let mut files = Vec::with_capacity(manifest.adapters.len());
    for adapter in &manifest.adapters {
        let framework = adapter
            .framework
            .as_deref()
            .ok_or_else(|| CliError::InvalidArgument {
                command: COMMAND.to_string(),
                reason: format!(
                    "component '{component}' has an [[adapters]] entry with no framework"
                ),
            })?;
        let source = adapter
            .source
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CliError::InvalidArgument {
                command: COMMAND.to_string(),
                reason: format!(
                    "component '{component}' adapter for '{framework}' declares no source"
                ),
            })?;
        let dest_template = adapter
            .dest
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CliError::InvalidArgument {
                command: COMMAND.to_string(),
                reason: format!(
                    "component '{component}' adapter for '{framework}' declares no dest"
                ),
            })?;
        let dest = expand_layout_placeholders(dest_template, layout, &[("component", component)])
            .map_err(|err| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!("failed to expand adapter dest '{dest_template}': {err}"),
        })?;
        validate_owned_path(layout, &dest).map_err(|err| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "adapter destination '{}' failed path safety check: {err}",
                dest.display()
            ),
        })?;
        // The runner lays an entire archive subtree only when the source key
        // ends with '/'. An adapter bundle is always a directory, so force
        // directory-prefix semantics regardless of how the manifest wrote it.
        let source = if source.ends_with('/') {
            source.to_string()
        } else {
            format!("{source}/")
        };
        files.push(ResolvedInstallFile {
            source: Some(source),
            dest,
            // Bundle contents are framework-loaded data, not directly
            // executed by ANOLISA; lay them 0644. Per-file modes inside a
            // bundle are not expressible in `[[adapters]]` in the MVP.
            mode: Some("0644".to_string()),
            kind: FileKind::Data,
        });
    }
    Ok(files)
}

/// Execute the resolved install: download+verify, copy files under the
/// install lock, persist state, and append the audit record. Files already
/// on disk are rolled back when a later step fails, so no phantom install
/// survives an error.
fn execute_raw(
    ctx: &CliContext,
    layout: &FsLayout,
    command: &str,
    prepared: PreparedInstall,
) -> Result<(), CliError> {
    let PreparedInstall {
        mut resolution,
        artifact_path,
        files,
        services,
        manifest_toml,
    } = prepared;
    let started_at = now_iso8601();

    // Acquire lock, then load state inside the lock so a concurrent writer
    // cannot be overwritten and state-load failures precede any file copy.
    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;
    let mut state =
        common::load_installed_state(ctx, command).map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!("failed to load installed state: {err}"),
        })?;
    ensure_component_backend_compatible(
        &state,
        &resolution.component,
        &resolution.backend,
        command,
    )?;

    // Nanosecond suffix avoids collisions between near-simultaneous
    // processes that serialize on the lock within the same second.
    let lock_ts = Utc::now();
    let operation_id = format!(
        "op-install-{}-{}",
        lock_ts.format("%Y%m%d%H%M%S"),
        lock_ts.timestamp_subsec_nanos()
    );

    let runner = InstallRunner::new(layout);
    let outcome = runner
        .install_files(
            artifact_type_wire(&resolution.entry.artifact_type),
            &artifact_path,
            &files,
        )
        .map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!("install failed: {err}"),
        })?;

    // From this point files are on disk — failures must roll them back.
    let manifest_path =
        match write_installed_component_manifest(layout, &resolution.component, &manifest_toml) {
            Ok(path) => path,
            Err(err) => {
                rollback_installed_files(&outcome.files);
                return Err(err);
            }
        };

    let mut owned_files: Vec<OwnedFile> = outcome
        .files
        .iter()
        .map(|f| OwnedFile {
            path: f.path.clone(),
            owner: FileOwner::Anolisa,
            sha256: Some(f.sha256.clone()),
        })
        .collect();
    owned_files.push(OwnedFile {
        path: manifest_path.clone(),
        owner: FileOwner::Anolisa,
        sha256: None,
    });
    let mut installed_paths: Vec<String> = outcome
        .files
        .iter()
        .map(|f| f.path.display().to_string())
        .collect();
    installed_paths.push(manifest_path.display().to_string());

    let service_manager = match ctx.install_mode {
        crate::context::InstallMode::System => "systemd",
        crate::context::InstallMode::User => "systemd-user",
    };

    // Migrate away legacy capability rows on this state write; surfaced
    // in the result warnings and audited in the central log below. A
    // state-save failure rolls the prune back with the rest of the write.
    let pruned_legacy = state.prune_legacy_capabilities();
    if !pruned_legacy.is_empty() {
        resolution.warnings.push(format!(
            "pruned legacy capability state object(s) written by an older release: {}",
            pruned_legacy.join(", ")
        ));
    }

    state.install_mode = match ctx.install_mode {
        crate::context::InstallMode::System => StateInstallMode::System,
        crate::context::InstallMode::User => StateInstallMode::User,
    };
    state.prefix = layout.prefix.clone();
    state.upsert_object(InstalledObject {
        kind: ObjectKind::Component,
        name: resolution.component.clone(),
        version: resolution.entry.version.clone(),
        status: ObjectStatus::Installed,
        // Embedded-manifest digest verification is future work; recording
        // an unverified digest would overstate what install checked.
        manifest_digest: None,
        distribution_source: Some(resolution.artifact_url.clone()),
        install_backend: Some(resolution.backend.clone()),
        ownership: Some(Ownership::RawManaged),
        rpm_metadata: None,
        installed_at: started_at.clone(),
        last_operation_id: Some(operation_id.clone()),
        managed: true,
        adopted: false,
        subscription_scope: Default::default(),
        enabled_features: Vec::new(),
        component_refs: Vec::new(),
        files: owned_files,
        external_modified_files: Vec::new(),
        services: services
            .iter()
            .map(|svc| ServiceRef {
                name: svc.clone(),
                manager: service_manager.to_string(),
                restartable: true,
                // Service enablement is deferred to a later milestone.
                enabled: false,
            })
            .collect(),
        health: Vec::new(),
    });
    state.operations.push(OperationRecord {
        id: operation_id.clone(),
        command: command.to_string(),
        status: "ok".to_string(),
        started_at: started_at.clone(),
        finished_at: Some(now_iso8601()),
    });

    let state_path = layout.state_dir.join("installed.toml");
    if let Err(err) = state.save(&state_path) {
        rollback_installed_files(&outcome.files);
        rollback_installed_manifest(&manifest_path);
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "failed to save state; attempted best-effort rollback of installed files (some may remain on disk): {err}"
            ),
        });
    }

    // Audit log is best-effort: the install already succeeded and state is
    // saved, so a log failure downgrades to a warning instead of unwinding.
    let log = CentralLog::open(layout.central_log.clone());
    if !pruned_legacy.is_empty() {
        // Warn-severity so `logs --level warn` surfaces the migration.
        let prune_record = LogRecord {
            kind: LogKind::Operation,
            operation_id: Some(operation_id.clone()),
            command: command.to_string(),
            source: "anolisa-cli".to_string(),
            component: None,
            severity: Severity::Warn,
            message: format!(
                "pruned legacy capability state object(s) written by an older release: {}",
                pruned_legacy.join(", ")
            ),
            actor: "cli".to_string(),
            install_mode: Some(ctx.install_mode.as_str().to_string()),
            started_at: started_at.clone(),
            finished_at: Some(now_iso8601()),
            status: None,
            objects: pruned_legacy.clone(),
            backup_ids: Vec::new(),
            warnings: Vec::new(),
            details: serde_json::Value::Null,
        };
        if let Err(err) = log.append(&prune_record) {
            eprintln!("warning: failed to write central log: {err}");
        }
    }
    let record = LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.clone()),
        command: command.to_string(),
        source: "anolisa-cli".to_string(),
        component: Some(resolution.component.clone()),
        severity: Severity::Info,
        message: format!(
            "component {} {} installed via {} backend",
            resolution.component, resolution.entry.version, resolution.backend
        ),
        actor: "cli".to_string(),
        install_mode: Some(ctx.install_mode.as_str().to_string()),
        started_at,
        finished_at: Some(now_iso8601()),
        status: Some(LogStatus::Ok),
        objects: vec![resolution.component.clone()],
        backup_ids: Vec::new(),
        warnings: resolution.warnings.clone(),
        details: serde_json::Value::Null,
    };
    if let Err(err) = log.append(&record) {
        eprintln!("warning: failed to write central log: {err}");
    }

    let payload = InstallResultPayload {
        component: resolution.component,
        package: resolution.package,
        version: resolution.entry.version,
        backend: resolution.backend,
        base_url: resolution.base_url,
        install_mode: ctx.install_mode.as_str().to_string(),
        operation_id,
        artifact_url: resolution.artifact_url,
        files_installed: installed_paths,
        services,
        warnings: resolution.warnings,
    };
    if ctx.json {
        return render_json(command, &payload);
    }
    if !ctx.quiet {
        render_result(&payload, ctx.no_color);
    }
    Ok(())
}

fn ensure_component_backend_compatible(
    state: &InstalledState,
    component: &str,
    requested_backend: &str,
    command: &str,
) -> Result<(), CliError> {
    let Some(obj) = state.find_object(ObjectKind::Component, component) else {
        return Ok(());
    };

    match installed_backend_label(obj) {
        Some(installed_backend) if installed_backend == requested_backend => Ok(()),
        Some(installed_backend) => Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "component '{component}' is already installed via backend '{installed_backend}'; reinstalling it via backend '{requested_backend}' is not allowed — uninstall it first or use backend '{installed_backend}'",
            ),
        }),
        None => Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "component '{component}' is already installed but its install backend is unknown; uninstall it before installing via backend '{requested_backend}'",
            ),
        }),
    }
}

fn installed_backend_label(obj: &InstalledObject) -> Option<&str> {
    obj.install_backend
        .as_deref()
        .or_else(|| infer_backend_from_distribution_source(obj.distribution_source.as_deref()))
}

fn infer_backend_from_distribution_source(source: Option<&str>) -> Option<&'static str> {
    let source = source?;
    if source.starts_with("http://")
        || source.starts_with("https://")
        || source.starts_with("file://")
    {
        Some("raw")
    } else {
        None
    }
}

fn render_plan(ctx: &CliContext, preview: &InstallPreview) -> Result<(), CliError> {
    let resolved = &preview.resolution;
    let payload = InstallPlanPayload {
        component: resolved.component.clone(),
        package: resolved.package.clone(),
        version: resolved.entry.version.clone(),
        backend: resolved.backend.clone(),
        base_url: resolved.base_url.clone(),
        install_mode: ctx.install_mode.as_str().to_string(),
        artifact: ArtifactInfo {
            r#type: artifact_type_wire(&resolved.entry.artifact_type).to_string(),
            url: resolved.artifact_url.clone(),
            sha256: resolved.entry.sha256.clone(),
        },
        files: preview
            .files
            .iter()
            .map(|f| f.dest.display().to_string())
            .collect(),
        services: preview.services.clone(),
        dry_run: true,
        warnings: resolved.warnings.clone(),
    };

    if ctx.json {
        return render_json(COMMAND, &payload);
    }
    if ctx.quiet {
        return Ok(());
    }
    let color = Palette::new(ctx.no_color);
    println!(
        "{} {} v{} {}",
        color.command("install"),
        payload.component,
        payload.version,
        color.muted("(dry-run — nothing installed)"),
    );
    println!("{} {}", color.label("backend:"), payload.backend);
    println!(
        "{} {}",
        color.label("base_url:"),
        color.path(&payload.base_url)
    );
    println!("{} {}", color.label("package:"), payload.package);
    println!("{} {}", color.label("install_mode:"), payload.install_mode);
    println!(
        "{} {} ({})",
        color.label("artifact:"),
        color.path(&payload.artifact.url),
        payload.artifact.r#type
    );
    println!("{}", color.header("files:"));
    for f in &payload.files {
        println!("  - {}", color.path(f));
    }
    if !payload.services.is_empty() {
        println!("{}", color.header("services (recorded, not started):"));
        for s in &payload.services {
            println!("  - {s}");
        }
    }
    render_warnings(&payload.warnings, &color);
    Ok(())
}

fn render_result(payload: &InstallResultPayload, no_color: bool) {
    let color = Palette::new(no_color);
    println!(
        "{} {} v{} {}",
        color.command("install"),
        payload.component,
        payload.version,
        color.ok("succeeded"),
    );
    println!("{} {}", color.label("backend:"), payload.backend);
    println!("{} {}", color.label("package:"), payload.package);
    println!(
        "{} {}",
        color.label("operation_id:"),
        color.id(&payload.operation_id)
    );
    println!(
        "{} {}",
        color.label("files installed:"),
        payload.files_installed.len()
    );
    for p in &payload.files_installed {
        println!("  - {}", color.path(p));
    }
    if !payload.services.is_empty() {
        println!("{}", color.header("services (recorded, not started):"));
        for s in &payload.services {
            println!("  - {s}");
        }
    }
    render_warnings(&payload.warnings, &color);
}

fn render_warnings(warnings: &[String], color: &Palette) {
    if warnings.is_empty() {
        return;
    }
    println!("{}", color.warn("warnings:"));
    for w in warnings {
        println!("  - {w}");
    }
}

/// Route a [`RepoConfigError`] to the CLI error surface.
///
/// `caller_fixable` decides the bucket: selection/substitution/override
/// errors are actionable by the caller (pass a different `--backend`,
/// fix `[vars]`, fix the `--repo` URL) → INVALID_ARGUMENT (exit 2);
/// discovery/IO/parse failures mean the config asset itself is broken →
/// EXECUTION_FAILED (exit 1), mirroring the execution-policy split.
fn repo_config_err(err: RepoConfigError, caller_fixable: bool) -> CliError {
    if caller_fixable {
        CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: err.to_string(),
        }
    } else {
        CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!("failed to load repo config: {err}"),
        }
    }
}

/// `{ext}` placeholder value for the conventional file name. Single-file
/// artifacts ship bare; OCI rows are references, not downloadable files,
/// and never resolve through URL derivation.
fn artifact_ext(t: &ArtifactType) -> &'static str {
    match t {
        ArtifactType::TarGz => ".tar.gz",
        ArtifactType::Zip => ".zip",
        ArtifactType::Rpm => ".rpm",
        ArtifactType::Deb => ".deb",
        ArtifactType::Binary | ArtifactType::File | ArtifactType::Oci => "",
    }
}

/// Wire-form artifact type string for the install runner.
fn artifact_type_wire(t: &ArtifactType) -> &'static str {
    match t {
        ArtifactType::TarGz => "tar_gz",
        ArtifactType::Binary => "binary",
        ArtifactType::Rpm => "rpm",
        ArtifactType::Deb => "deb",
        ArtifactType::Zip => "zip",
        ArtifactType::Oci => "oci",
        ArtifactType::File => "file",
    }
}

/// Best-effort cleanup of installed files after a state-save failure.
fn rollback_installed_files(files: &[anolisa_core::InstalledFile]) {
    for f in files {
        let _ = std::fs::remove_file(&f.path);
    }
}

fn write_installed_component_manifest(
    layout: &FsLayout,
    component: &str,
    toml: &str,
) -> Result<PathBuf, CliError> {
    let path = common::installed_component_manifest_path(layout, component, COMMAND)?;
    write_atomic_text(&path, toml).map_err(|err| CliError::Runtime {
        command: COMMAND.to_string(),
        reason: format!(
            "failed to write installed component manifest at {}: {err}",
            path.display()
        ),
    })?;
    Ok(path)
}

fn write_atomic_text(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("component.toml");
    let nanos = Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let tmp = parent.join(format!(".{name}.tmp-{}-{nanos}", std::process::id()));

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o644);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(content.as_bytes())?;
    drop(file);
    if let Err(err) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

fn rollback_installed_manifest(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// ISO 8601 UTC timestamp with second precision.
fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::context::InstallMode;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use sha2::{Digest, Sha256};
    use std::path::{Path, PathBuf};
    use tar::{Builder, Header};
    use tempfile::tempdir;

    // `--prefix` only rebases system-mode layouts (user mode resolves from
    // $HOME), so isolation tests run in System mode under a tempdir to keep
    // every filesystem probe (repo.toml, state, cache) away from the host.
    fn ctx_with_prefix(json: bool, prefix: Option<PathBuf>) -> CliContext {
        CliContext {
            install_mode: if prefix.is_some() {
                InstallMode::System
            } else {
                InstallMode::User
            },
            prefix,
            json,
            dry_run: false,
            verbose: false,
            quiet: true, // suppress stdout during tests
            no_color: true,
        }
    }

    fn args(component: &str) -> InstallArgs {
        InstallArgs {
            component: Some(component.to_string()),
            all: false,
            fail_fast: false,
            version: None,
            backend: None,
            repo: None,
            package: None,
        }
    }

    fn toml_string_array(values: &[&str]) -> String {
        let quoted: Vec<String> = values.iter().map(|value| format!("\"{value}\"")).collect();
        format!("[{}]", quoted.join(", "))
    }

    fn component_manifest_toml(component: &str, version: &str, modes: &[&str]) -> String {
        let modes = toml_string_array(modes);
        format!(
            r#"[component]
name = "{component}"
version = "{version}"

[component.layout]
modes = {modes}

[[component.layout.files]]
source = "bin/{component}"
target = "{{bindir}}/{component}"
mode = "0755"
type = "executable"
"#
        )
    }

    fn build_tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let buf = Vec::new();
        let enc = GzEncoder::new(buf, Compression::default());
        let mut tar = Builder::new(enc);
        for (path, data) in entries {
            let mut header = Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, *path, *data)
                .expect("append tar entry");
        }
        let enc = tar.into_inner().expect("finish tar");
        enc.finish().expect("finish gzip")
    }

    fn build_component_artifact(component: &str, version: &str, modes: &[&str]) -> Vec<u8> {
        let manifest = component_manifest_toml(component, version, modes);
        let bin_path = format!("bin/{component}");
        let payload = format!("#!/bin/sh\necho {component}\n");
        build_tar_gz(&[
            (".anolisa/component.toml", manifest.as_bytes()),
            (bin_path.as_str(), payload.as_bytes()),
        ])
    }

    /// Build a manifest with a single `[[adapters]]` entry, optionally
    /// overriding the framework and whether source/dest are present.
    fn adapter_manifest(framework: &str, source: Option<&str>, dest: Option<&str>) -> String {
        let mut toml = String::from(
            "[component]\nname = \"tokenless\"\nversion = \"0.1.0\"\n\n\
             [component.layout]\nmodes = [\"system\"]\n\n\
             [[adapters]]\n",
        );
        toml.push_str(&format!("framework = \"{framework}\"\n"));
        if let Some(s) = source {
            toml.push_str(&format!("source = \"{s}\"\n"));
        }
        if let Some(d) = dest {
            toml.push_str(&format!("dest = \"{d}\"\n"));
        }
        toml
    }

    #[test]
    fn resolve_adapter_files_lays_bundle_under_datadir() {
        let prefix = tempdir().unwrap();
        let layout = FsLayout::system(Some(prefix.path().to_path_buf()));
        let toml = adapter_manifest(
            "openclaw",
            Some("adapters/tokenless/openclaw"),
            Some("{datadir}/adapters/{component}/openclaw/"),
        );
        let manifest = ComponentManifest::from_toml_str(&toml).expect("parse manifest");
        let files = resolve_adapter_files(&manifest, &layout, "tokenless").expect("resolve");

        assert_eq!(files.len(), 1);
        let f = &files[0];
        // Source is normalized to a directory prefix so the whole bundle
        // tree is laid down by the runner.
        assert_eq!(f.source.as_deref(), Some("adapters/tokenless/openclaw/"));
        assert_eq!(f.dest, layout.datadir.join("adapters/tokenless/openclaw"));
        assert_eq!(f.kind, FileKind::Data);
        assert_eq!(f.mode.as_deref(), Some("0644"));
    }

    #[test]
    fn resolve_adapter_files_allows_unknown_framework() {
        let prefix = tempdir().unwrap();
        let layout = FsLayout::system(Some(prefix.path().to_path_buf()));
        let toml = adapter_manifest(
            "hermes",
            Some("adapters/tokenless/hermes"),
            Some("{datadir}/adapters/{component}/hermes/"),
        );
        let manifest = ComponentManifest::from_toml_str(&toml).expect("parse manifest");
        let files = resolve_adapter_files(&manifest, &layout, "tokenless").expect("resolve");

        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].source.as_deref(),
            Some("adapters/tokenless/hermes/")
        );
        assert_eq!(
            files[0].dest,
            layout.datadir.join("adapters/tokenless/hermes")
        );
    }

    #[test]
    fn resolve_adapter_files_rejects_missing_source() {
        let prefix = tempdir().unwrap();
        let layout = FsLayout::system(Some(prefix.path().to_path_buf()));
        let toml = adapter_manifest(
            "openclaw",
            None,
            Some("{datadir}/adapters/{component}/openclaw/"),
        );
        let manifest = ComponentManifest::from_toml_str(&toml).expect("parse manifest");
        let err = resolve_adapter_files(&manifest, &layout, "tokenless")
            .expect_err("missing source must be rejected");
        assert!(
            matches!(err, CliError::InvalidArgument { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn resolve_adapter_files_empty_when_no_adapters() {
        let prefix = tempdir().unwrap();
        let layout = FsLayout::system(Some(prefix.path().to_path_buf()));
        let toml = component_manifest_toml("tokenless", "0.1.0", &["system"]);
        let manifest = ComponentManifest::from_toml_str(&toml).expect("parse manifest");
        let files = resolve_adapter_files(&manifest, &layout, "tokenless").expect("resolve");
        assert!(files.is_empty());
    }

    fn write_empty_repo(root: &Path) -> String {
        let v1 = root.join("v1");
        std::fs::create_dir_all(&v1).expect("create repo dirs");
        std::fs::write(
            v1.join("index.toml"),
            r#"schema_version = 1
channel = "stable"
publisher = "test"
"#,
        )
        .expect("write index");
        format!("file://{}", v1.display())
    }

    /// Lay out a local file:// raw repo containing one tar.gz artifact for
    /// `agentsight` targeting the *detected* host os/arch, and return the
    /// repo's raw v1 root. Uses a repo-relative artifact URL to also exercise
    /// the relative-URL join.
    fn write_local_repo(root: &Path) -> String {
        write_local_repo_component(root, "agentsight", "0.2.0", &["system"])
    }

    fn write_local_repo_component(
        root: &Path,
        component: &str,
        version: &str,
        modes: &[&str],
    ) -> String {
        write_local_repo_component_with_modes(root, component, version, modes, modes)
    }

    fn write_local_repo_component_with_modes(
        root: &Path,
        component: &str,
        version: &str,
        index_modes: &[&str],
        manifest_modes: &[&str],
    ) -> String {
        let v1 = root.join("v1");
        std::fs::create_dir_all(&v1).expect("create repo dirs");

        let artifact = build_component_artifact(component, version, manifest_modes);
        let artifact_name = format!("{component}.tar.gz");
        std::fs::write(v1.join(&artifact_name), &artifact).expect("write artifact");
        let sha = format!("{:x}", Sha256::digest(&artifact));
        let modes = toml_string_array(index_modes);

        let env = anolisa_env::EnvService::detect();
        let index = format!(
            r#"schema_version = 1
channel = "stable"
publisher = "test"

[[entries]]
component = "{component}"
version = "{version}"
channel = "stable"
artifact_type = "tar_gz"
backend = "raw"
url = "{artifact_name}"
os = "{os}"
arch = "{arch}"
install_modes = {modes}
sha256 = "{sha}"
"#,
            os = env.os,
            arch = env.arch,
        );
        std::fs::write(v1.join("index.toml"), index).expect("write index");
        format!("file://{}", v1.display())
    }

    fn write_published_layout_repo_with_meta(
        root: &Path,
        component: &str,
        version: &str,
        modes: &[&str],
    ) -> String {
        let env = anolisa_env::EnvService::detect();
        let version_dir = root.join("v1").join(component).join(version);
        let artifact_dir = version_dir.join(&env.os).join(&env.arch);
        std::fs::create_dir_all(&artifact_dir).expect("create artifact dirs");

        let manifest = component_manifest_toml(component, version, modes);
        std::fs::write(version_dir.join("meta.toml"), &manifest).expect("write meta");

        let artifact = build_component_artifact(component, version, modes);
        let artifact_name = format!(
            "{component}-{version}-{os}-{arch}.tar.gz",
            os = env.os,
            arch = env.arch
        );
        std::fs::write(artifact_dir.join(&artifact_name), &artifact).expect("write artifact");
        let sha = format!("{:x}", Sha256::digest(&artifact));
        let modes = toml_string_array(modes);
        let url = format!(
            "{component}/{version}/{os}/{arch}/{artifact_name}",
            os = env.os,
            arch = env.arch
        );

        let index = format!(
            r#"schema_version = 1
channel = "stable"
publisher = "test"

[[entries]]
component = "{component}"
version = "{version}"
channel = "stable"
artifact_type = "tar_gz"
backend = "raw"
url = "{url}"
os = "{os}"
arch = "{arch}"
install_modes = {modes}
sha256 = "{sha}"
"#,
            os = env.os,
            arch = env.arch,
        );
        std::fs::write(root.join("v1/index.toml"), index).expect("write index");
        format!("file://{}", root.join("v1").display())
    }

    fn write_binary_repo_component(
        root: &Path,
        component: &str,
        version: &str,
        modes: &[&str],
    ) -> String {
        let v1 = root.join("v1");
        std::fs::create_dir_all(&v1).expect("create repo dirs");

        let artifact = format!("#!/bin/sh\necho {component}\n").into_bytes();
        let artifact_name = component.to_string();
        std::fs::write(v1.join(&artifact_name), &artifact).expect("write artifact");
        let sha = format!("{:x}", Sha256::digest(&artifact));
        let modes = toml_string_array(modes);

        let env = anolisa_env::EnvService::detect();
        let index = format!(
            r#"schema_version = 1
channel = "stable"
publisher = "test"

[[entries]]
component = "{component}"
version = "{version}"
channel = "stable"
artifact_type = "binary"
backend = "raw"
url = "{artifact_name}"
os = "{os}"
arch = "{arch}"
install_modes = {modes}
sha256 = "{sha}"
"#,
            os = env.os,
            arch = env.arch,
        );
        std::fs::write(v1.join("index.toml"), index).expect("write index");
        format!("file://{}", v1.display())
    }

    fn write_overlay_manifest(layout: &FsLayout, component: &str, version: &str, modes: &[&str]) {
        let runtime_dir = layout.manifests_overlay.join("runtime");
        std::fs::create_dir_all(&runtime_dir).expect("create overlay runtime dir");
        std::fs::write(
            runtime_dir.join(format!("{component}.toml")),
            component_manifest_toml(component, version, modes),
        )
        .expect("write overlay manifest");
    }

    #[test]
    fn sidecar_meta_url_uses_version_directory_for_published_layout() {
        let artifact_url = "https://example.test/anolisa/v1/tokenless/0.5.0/linux/x86_64/tokenless-0.5.0-linux-x86_64.tar.gz";

        assert_eq!(
            sidecar_meta_url(artifact_url, "tokenless", "0.5.0").as_deref(),
            Some("https://example.test/anolisa/v1/tokenless/0.5.0/meta.toml")
        );
    }

    #[test]
    fn sidecar_meta_url_keeps_flat_layout_fallback() {
        let artifact_url = "file:///tmp/repo/v1/legacy-bin";

        assert_eq!(
            sidecar_meta_url(artifact_url, "legacy-bin", "1.0.0").as_deref(),
            Some("file:///tmp/repo/v1/meta.toml")
        );
    }

    /// Like [`write_local_repo`], but the index row omits `url` and the
    /// artifact sits at the conventional publish path
    /// `{component}/{version}/{os}/{arch}/{component}-{version}-{os}-{arch}.tar.gz`
    /// under the raw v1 root.
    fn write_conventional_repo(root: &Path) -> String {
        let env = anolisa_env::EnvService::detect();
        let artifact_dir = root
            .join("v1/agentsight/0.2.0")
            .join(&env.os)
            .join(&env.arch);
        std::fs::create_dir_all(&artifact_dir).expect("create repo dirs");

        let artifact = build_component_artifact("agentsight", "0.2.0", &["system"]);
        let file_name = format!("agentsight-0.2.0-{}-{}.tar.gz", env.os, env.arch);
        std::fs::write(artifact_dir.join(file_name), &artifact).expect("write artifact");
        let sha = format!("{:x}", Sha256::digest(&artifact));

        let index = format!(
            r#"schema_version = 1
channel = "stable"
publisher = "test"

[[entries]]
component = "agentsight"
version = "0.2.0"
channel = "stable"
artifact_type = "tar_gz"
backend = "raw"
os = "{os}"
arch = "{arch}"
install_modes = ["system"]
sha256 = "{sha}"
"#,
            os = env.os,
            arch = env.arch,
        );
        std::fs::write(root.join("v1/index.toml"), index).expect("write index");
        format!("file://{}", root.join("v1").display())
    }

    #[test]
    fn install_cli_rejects_multiple_components() {
        let err = InstallArgs::try_parse_from(["install", "agentsight", "tokenless"])
            .expect_err("must reject extra positional arguments");
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn install_unknown_component_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let mut a = args("no-such-component");
        a.repo = Some(write_empty_repo(&tmp.path().join("repo")));

        let err = handle(a, &ctx_with_prefix(false, Some(prefix))).expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("no-such-component"));
    }

    /// Install mode support comes from the remote distribution index before
    /// any artifact is downloaded.
    #[test]
    fn install_unsupported_mode_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let mut a = args("agentsight");
        a.repo = Some(write_local_repo_component(
            &tmp.path().join("repo"),
            "agentsight",
            "0.2.0",
            &["user"],
        ));

        let err = handle(a, &ctx_with_prefix(false, Some(prefix))).expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("install mode is not supported"),
            "got: {}",
            err.reason()
        );
    }

    /// The embedded manifest is a publisher consistency check after index
    /// resolution, but it should use the same caller-visible error bucket as
    /// the index-level mode filter.
    #[test]
    fn install_manifest_mode_mismatch_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let mut a = args("agentsight");
        a.repo = Some(write_local_repo_component_with_modes(
            &tmp.path().join("repo"),
            "agentsight",
            "0.2.0",
            &["system"],
            &["user"],
        ));

        let err = handle(a, &ctx_with_prefix(false, Some(prefix))).expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason()
                .contains("inconsistent with the distribution index")
                && err.reason().contains("system-mode support"),
            "got: {}",
            err.reason()
        );
    }

    /// `--backend` naming a known-but-unconfigured backend is caller
    /// input → INVALID_ARGUMENT, with the hint naming repo.toml.
    #[test]
    fn install_unconfigured_backend_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let mut a = args("agentsight");
        a.backend = Some("npm".to_string());
        let err = handle(a, &ctx_with_prefix(false, Some(tmp.path().to_path_buf())))
            .expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("npm"), "got: {}", err.reason());
        assert!(
            err.reason().contains("repo.toml"),
            "reason must point at repo.toml: {}",
            err.reason()
        );
    }

    #[test]
    fn install_unknown_backend_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let mut a = args("agentsight");
        a.backend = Some("pip".to_string());
        let err = handle(a, &ctx_with_prefix(false, Some(tmp.path().to_path_buf())))
            .expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("pip"));
    }

    /// A configured non-raw backend selects fine but has no executor yet.
    /// `npm` is the stand-in: it is in `KNOWN_BACKENDS` and configurable, but
    /// has no installer (unlike `rpm`, which routes to the adopt path).
    #[test]
    fn install_configured_npm_backend_is_not_implemented() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().to_path_buf();
        let layout = FsLayout::system(Some(prefix.clone()));
        std::fs::create_dir_all(&layout.etc_dir).expect("etc dir");
        std::fs::write(
            layout.etc_dir.join("repo.toml"),
            r#"schema_version = 1
default_backend = "raw"

[backends.raw]
base_url = "https://example.com/anolisa"

[backends.npm]
base_url = "https://registry.npmjs.org"
scope = "@anolisa"
"#,
        )
        .expect("write repo.toml");

        let mut a = args("agentsight");
        a.backend = Some("npm".to_string());
        let err = handle(a, &ctx_with_prefix(false, Some(prefix))).expect_err("must error");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
        assert!(err.reason().contains("npm"), "got: {}", err.reason());
    }

    /// A malformed `--repo` URL fails the same shape rules as configured
    /// base_urls and routes to INVALID_ARGUMENT.
    #[test]
    fn install_invalid_repo_override_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let mut a = args("agentsight");
        a.repo = Some("ftp://example.com/repo".to_string());
        let err = handle(a, &ctx_with_prefix(false, Some(tmp.path().to_path_buf())))
            .expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("ftp"), "got: {}", err.reason());
    }

    /// Dry-run resolves through the real index (fetch + ResolveQuery +
    /// file rendering) but must not install anything or create state.
    #[test]
    fn install_dry_run_resolves_without_writing_files() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let repo_url = write_local_repo(&tmp.path().join("repo"));

        let mut a = args("agentsight");
        a.repo = Some(repo_url);
        let mut ctx = ctx_with_prefix(false, Some(prefix.clone()));
        ctx.dry_run = true;
        handle(a, &ctx).expect("dry-run must succeed");

        let layout = FsLayout::system(Some(prefix));
        assert!(
            !layout.bin_dir.join("agentsight").exists(),
            "dry-run must not install the binary"
        );
        assert!(
            !layout.state_dir.join("installed.toml").exists(),
            "dry-run must not write state"
        );
        let cached_names: Vec<String> = std::fs::read_dir(layout.cache_dir.join("downloads"))
            .expect("downloads cache exists")
            .map(|entry| {
                entry
                    .expect("cache entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert!(
            cached_names
                .iter()
                .all(|name| !name.ends_with("agentsight.tar.gz")),
            "dry-run must not download the install artifact; cache entries: {cached_names:?}"
        );
    }

    #[test]
    fn install_dry_run_reads_version_meta_without_downloading_artifact() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let repo_url = write_published_layout_repo_with_meta(
            &tmp.path().join("repo"),
            "remote-only",
            "1.0.0",
            &["system"],
        );
        let mut ctx = ctx_with_prefix(false, Some(prefix.clone()));
        ctx.dry_run = true;
        let layout = FsLayout::system(Some(prefix));
        let env = anolisa_env::EnvService::detect();

        let resolution = resolve_raw(
            &ctx,
            &layout,
            &env,
            ResolveInputs {
                component: "remote-only".to_string(),
                package: "remote-only".to_string(),
                backend: "raw".to_string(),
                base_url: repo_url,
                version: None,
                warnings: Vec::new(),
            },
        )
        .expect("resolve");
        let preview = build_install_preview(&ctx, &layout, resolution).expect("preview");

        assert_eq!(preview.files.len(), 1);
        assert_eq!(preview.files[0].dest, layout.bin_dir.join("remote-only"));
        assert!(
            preview
                .resolution
                .warnings
                .iter()
                .all(|warning| !warning.contains("file and service details are unavailable")),
            "version-level meta.toml should provide file details: {:?}",
            preview.resolution.warnings
        );

        let cached_names: Vec<String> = std::fs::read_dir(layout.cache_dir.join("downloads"))
            .expect("downloads cache exists")
            .map(|entry| {
                entry
                    .expect("cache entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert!(
            cached_names
                .iter()
                .all(|name| !name.ends_with("remote-only-1.0.0-linux-x86_64.tar.gz")),
            "dry-run must not download the install artifact; cache entries: {cached_names:?}"
        );
    }

    /// Legacy distribution indexes may still publish a raw `binary` entry.
    /// Keep installing those when a local component manifest supplies the
    /// destination contract.
    #[test]
    fn install_binary_artifact_uses_local_catalog_contract() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let layout = FsLayout::system(Some(prefix.clone()));
        write_overlay_manifest(&layout, "legacy-bin", "1.0.0", &["system"]);

        let mut a = args("legacy-bin");
        a.repo = Some(write_binary_repo_component(
            &tmp.path().join("repo"),
            "legacy-bin",
            "1.0.0",
            &["system"],
        ));

        handle(a, &ctx_with_prefix(false, Some(prefix.clone()))).expect("install must succeed");

        let bin = FsLayout::system(Some(prefix)).bin_dir.join("legacy-bin");
        assert!(bin.exists(), "binary artifact must be installed");
        assert_eq!(
            std::fs::read_to_string(&bin).expect("read installed binary"),
            "#!/bin/sh\necho legacy-bin\n"
        );
    }

    /// End-to-end raw install from a local file:// repo: resolve via the
    /// repo-relative artifact URL, verify sha256, install the binary to
    /// {bindir}, and persist component state.
    #[test]
    fn install_raw_end_to_end_from_local_repo() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let repo_url = write_local_repo(&tmp.path().join("repo"));

        let mut a = args("agentsight");
        a.repo = Some(repo_url.clone());
        handle(a, &ctx_with_prefix(false, Some(prefix.clone()))).expect("install must succeed");

        let layout = FsLayout::system(Some(prefix));
        let bin = layout.bin_dir.join("agentsight");
        assert!(bin.exists(), "binary must be installed at {{bindir}}");
        let manifest_path =
            common::installed_component_manifest_path(&layout, "agentsight", COMMAND)
                .expect("manifest path");
        assert!(
            manifest_path.exists(),
            "installed component manifest must be persisted"
        );
        let saved_manifest =
            ComponentManifest::from_file(&manifest_path).expect("saved manifest parses");
        assert_eq!(saved_manifest.component.name, "agentsight");
        assert_eq!(saved_manifest.component.version, "0.2.0");

        let state = anolisa_core::InstalledState::load(&layout.state_dir.join("installed.toml"))
            .expect("state must load");
        let obj = state
            .find_object(ObjectKind::Component, "agentsight")
            .expect("component object must be recorded");
        assert_eq!(obj.version, "0.2.0");
        assert_eq!(obj.status, ObjectStatus::Installed);
        assert_eq!(obj.files.len(), 2);
        assert!(
            obj.files.iter().any(|file| file.path == manifest_path),
            "installed manifest must be tracked as an owned file"
        );
        assert!(
            obj.distribution_source
                .as_deref()
                .is_some_and(|u| u.starts_with(&repo_url)),
            "distribution_source must record the resolved artifact URL"
        );
        assert_eq!(
            obj.install_backend.as_deref(),
            Some("raw"),
            "install_backend must record the selected backend"
        );
        assert!(
            obj.services.iter().all(|s| !s.enabled),
            "install must not mark services enabled"
        );
        assert_eq!(state.operations.len(), 1);
        assert!(state.operations[0].id.starts_with("op-install-"));
    }

    #[test]
    fn install_raw_uses_embedded_manifest_without_local_catalog() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let repo_url = write_local_repo_component(
            &tmp.path().join("repo"),
            "remote-only",
            "1.0.0",
            &["system"],
        );

        let mut a = args("remote-only");
        a.repo = Some(repo_url);
        handle(a, &ctx_with_prefix(false, Some(prefix.clone()))).expect("install must succeed");

        let layout = FsLayout::system(Some(prefix));
        assert!(
            layout.bin_dir.join("remote-only").exists(),
            "component absent from local manifests must install from embedded artifact contract"
        );
        let state = anolisa_core::InstalledState::load(&layout.state_dir.join("installed.toml"))
            .expect("state must load");
        assert!(
            state
                .find_object(ObjectKind::Component, "remote-only")
                .is_some(),
            "remote-only component must be recorded"
        );
    }

    #[test]
    fn install_existing_component_with_different_backend_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let layout = FsLayout::system(Some(prefix.clone()));
        std::fs::create_dir_all(&layout.etc_dir).expect("etc dir");
        std::fs::create_dir_all(&layout.state_dir).expect("state dir");
        std::fs::write(
            layout.etc_dir.join("repo.toml"),
            r#"schema_version = 1
default_backend = "raw"

[backends.raw]
base_url = "https://example.com/anolisa"

[backends.npm]
base_url = "https://registry.npmjs.org"
scope = "@anolisa"
"#,
        )
        .expect("write repo.toml");

        let mut state = anolisa_core::InstalledState {
            install_mode: StateInstallMode::System,
            prefix: layout.prefix.clone(),
            ..Default::default()
        };
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Component,
            name: "agentsight".to_string(),
            version: "0.2.0".to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: None,
            distribution_source: Some("file:///repo/v1/agentsight-bin".to_string()),
            install_backend: Some("raw".to_string()),
            ownership: None,
            rpm_metadata: None,
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: Some("op-prior".to_string()),
            managed: true,
            adopted: false,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        });
        state
            .save(&layout.state_dir.join("installed.toml"))
            .expect("save state");

        let mut a = args("agentsight");
        a.backend = Some("npm".to_string());
        let err = handle(a, &ctx_with_prefix(false, Some(prefix))).expect_err("must error");

        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("already installed via backend 'raw'")
                && err.reason().contains("backend 'npm'"),
            "reason must explain backend conflict: {}",
            err.reason()
        );
    }

    /// An index row without `url` installs from the code-owned raw layout
    /// under the raw v1 root.
    #[test]
    fn install_derives_artifact_url_from_convention_when_index_omits_url() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let repo_url = write_conventional_repo(&tmp.path().join("repo"));

        let mut a = args("agentsight");
        a.repo = Some(repo_url.clone());
        handle(a, &ctx_with_prefix(false, Some(prefix.clone()))).expect("install must succeed");

        let layout = FsLayout::system(Some(prefix));
        assert!(layout.bin_dir.join("agentsight").exists());

        let state = anolisa_core::InstalledState::load(&layout.state_dir.join("installed.toml"))
            .expect("state must load");
        let obj = state
            .find_object(ObjectKind::Component, "agentsight")
            .expect("component object must be recorded");
        let env = anolisa_env::EnvService::detect();
        assert_eq!(
            obj.distribution_source.as_deref(),
            Some(
                format!(
                    "{repo_url}/agentsight/0.2.0/{os}/{arch}/agentsight-0.2.0-{os}-{arch}.tar.gz",
                    os = env.os,
                    arch = env.arch
                )
                .as_str()
            ),
            "distribution_source must record the convention-derived URL"
        );
    }

    /// A legacy template-form repo URL still resolves by taking the static
    /// prefix before `{component}` as the raw v1 root.
    #[test]
    fn install_resolves_legacy_template_form_repo_url() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let repo_root = tmp.path().join("repo");
        // write_conventional_repo puts the tree under <root>/v1/; point the
        // template's static prefix at that same directory.
        let _ = write_conventional_repo(&repo_root);
        let template_url = format!(
            "file://{}/v1/{{component}}/{{version}}/{{os}}/{{arch}}/",
            repo_root.display()
        );

        let mut a = args("agentsight");
        a.repo = Some(template_url);
        handle(a, &ctx_with_prefix(false, Some(prefix.clone()))).expect("install must succeed");

        let layout = FsLayout::system(Some(prefix));
        assert!(layout.bin_dir.join("agentsight").exists());

        let state = anolisa_core::InstalledState::load(&layout.state_dir.join("installed.toml"))
            .expect("state must load");
        let obj = state
            .find_object(ObjectKind::Component, "agentsight")
            .expect("component object must be recorded");
        let env = anolisa_env::EnvService::detect();
        assert_eq!(
            obj.distribution_source.as_deref(),
            Some(
                format!(
                    "file://{}/v1/agentsight/0.2.0/{os}/{arch}/agentsight-0.2.0-{os}-{arch}.tar.gz",
                    repo_root.display(),
                    os = env.os,
                    arch = env.arch
                )
                .as_str()
            ),
            "distribution_source must record the convention-derived URL"
        );
    }

    /// Requesting a version the index does not publish is caller input.
    #[test]
    fn install_unpublished_version_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let repo_url = write_local_repo(&tmp.path().join("repo"));

        let mut a = args("agentsight");
        a.repo = Some(repo_url);
        a.version = Some("9.9.9".to_string());
        let err =
            handle(a, &ctx_with_prefix(false, Some(prefix))).expect_err("must fail to resolve");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("9.9.9"), "got: {}", err.reason());
    }

    // ── --all / --fail-fast clap validation tests ─────────────────────

    #[test]
    fn install_all_and_component_are_mutually_exclusive() {
        let err = InstallArgs::try_parse_from(["install", "--all", "tokenless"])
            .expect_err("must reject --all with positional");
        assert!(
            err.kind() == clap::error::ErrorKind::ArgumentConflict
                || err.to_string().contains("cannot be used with")
        );
    }

    #[test]
    fn install_all_conflicts_with_package() {
        let err = InstallArgs::try_parse_from(["install", "--all", "--package", "foo"])
            .expect_err("must reject --all with --package");
        assert!(
            err.kind() == clap::error::ErrorKind::ArgumentConflict
                || err.to_string().contains("cannot be used with")
        );
    }

    #[test]
    fn install_all_conflicts_with_version() {
        let err = InstallArgs::try_parse_from(["install", "--all", "--version", "1.0.0"])
            .expect_err("must reject --all with --version");
        assert!(
            err.kind() == clap::error::ErrorKind::ArgumentConflict
                || err.to_string().contains("cannot be used with")
        );
    }

    #[test]
    fn install_fail_fast_without_all_is_rejected() {
        // clap still parses it (ArgGroup + requires limitation), but
        // handle() now rejects at runtime.
        let a = InstallArgs::try_parse_from(["install", "tokenless", "--fail-fast"])
            .expect("clap allows this parse");
        assert!(!a.all);
        assert!(a.fail_fast);

        let ctx = ctx_with_prefix(false, None);
        let err = handle(a, &ctx).expect_err("handle should reject --fail-fast without --all");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
    }

    #[test]
    fn install_all_parses_successfully() {
        let a = InstallArgs::try_parse_from(["install", "--all"]).expect("should parse");
        assert!(a.all);
        assert!(a.component.is_none());
    }

    #[test]
    fn install_all_with_fail_fast_parses_successfully() {
        let a =
            InstallArgs::try_parse_from(["install", "--all", "--fail-fast"]).expect("should parse");
        assert!(a.all);
        assert!(a.fail_fast);
    }

    // ── catalog parsing tests ────────────────────────────────────────

    #[test]
    fn all_catalog_filters_available_components() {
        let json = r#"{
            "schema_version": 1,
            "components": [
                {"name": "a", "status": "available"},
                {"name": "b", "status": "planned"},
                {"name": "c", "status": "available"},
                {"name": "d"}
            ]
        }"#;
        let catalog: AllCatalogV1 = serde_json::from_str(json).expect("parse");
        assert_eq!(catalog.schema_version, 1);
        let names: Vec<String> = catalog
            .components
            .into_iter()
            .filter(|c| c.status.as_deref() == Some("available"))
            .map(|c| c.name)
            .filter(|n| !n.trim().is_empty())
            .collect();
        assert_eq!(names, vec!["a", "c"]);
    }

    #[test]
    fn all_catalog_rejects_unsupported_schema_version() {
        let json = r#"{"schema_version": 2, "components": []}"#;
        let catalog: AllCatalogV1 = serde_json::from_str(json).expect("parse");
        assert_ne!(catalog.schema_version, 1);
    }

    #[test]
    fn all_catalog_skips_empty_names() {
        let json = r#"{
            "schema_version": 1,
            "components": [
                {"name": "", "status": "available"},
                {"name": "  ", "status": "available"},
                {"name": "valid", "status": "available"}
            ]
        }"#;
        let catalog: AllCatalogV1 = serde_json::from_str(json).expect("parse");
        let names: Vec<String> = catalog
            .components
            .into_iter()
            .filter(|c| c.status.as_deref() == Some("available"))
            .map(|c| c.name)
            .filter(|n| !n.trim().is_empty())
            .collect();
        assert_eq!(names, vec!["valid"]);
    }

    // ── rpm adopt path (#958) ───────────────────────────────────────

    use anolisa_platform::pkg_query::PackageVersion;

    /// Configurable in-memory [`PackageQuery`] so adopt tests run without a
    /// live rpmdb.
    #[derive(Default)]
    struct FakeQuery {
        installed: Vec<(String, PackageInfo)>,
        origins: Vec<(String, String)>,
        provides: Vec<(String, Vec<String>)>,
        multi_version: Vec<String>,
        origin_fails: bool,
    }

    impl PackageQuery for FakeQuery {
        fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
            if self.multi_version.iter().any(|p| p == package) {
                return Err(PackageQueryError::UnexpectedOutput {
                    command: "rpm".to_string(),
                    detail: "2 installed versions".to_string(),
                });
            }
            Ok(self
                .installed
                .iter()
                .find(|(n, _)| n == package)
                .map(|(_, info)| info.clone()))
        }

        fn query_available(&self, _package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
            Ok(Vec::new())
        }

        fn installed_origin(&self, package: &str) -> Result<Option<String>, PackageQueryError> {
            if self.origin_fails {
                return Err(PackageQueryError::QueryFailed {
                    command: "dnf".to_string(),
                    code: Some(1),
                    stderr: "boom".to_string(),
                });
            }
            Ok(self
                .origins
                .iter()
                .find(|(n, _)| n == package)
                .map(|(_, repo)| repo.clone()))
        }

        fn what_provides_installed(
            &self,
            capability: &str,
        ) -> Result<Vec<String>, PackageQueryError> {
            Ok(self
                .provides
                .iter()
                .find(|(cap, _)| cap == capability)
                .map(|(_, names)| names.clone())
                .unwrap_or_default())
        }
    }

    fn pkg_info(name: &str, version: &str, release: Option<&str>, arch: &str) -> PackageInfo {
        PackageInfo {
            name: name.to_string(),
            version: PackageVersion {
                epoch: None,
                version: version.to_string(),
                release: release.map(str::to_string),
            },
            arch: arch.to_string(),
            origin: None,
        }
    }

    /// System-mode ctx over a tempdir with a raw-only `repo.toml` (the AC1
    /// shape: no `[backends.rpm]` table). Returns the temp guard so callers
    /// keep the directory alive.
    fn system_ctx_with_raw_repo(dry_run: bool) -> (tempfile::TempDir, CliContext) {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().to_path_buf();
        let layout = FsLayout::system(Some(prefix.clone()));
        std::fs::create_dir_all(&layout.etc_dir).expect("etc dir");
        std::fs::create_dir_all(&layout.state_dir).expect("state dir");
        std::fs::write(
            layout.etc_dir.join("repo.toml"),
            "schema_version = 1\ndefault_backend = \"raw\"\n\n[backends.raw]\nbase_url = \"https://example.com/anolisa\"\n",
        )
        .expect("write repo.toml");
        let mut ctx = ctx_with_prefix(false, Some(prefix));
        ctx.dry_run = dry_run;
        (tmp, ctx)
    }

    fn load_state(ctx: &CliContext) -> InstalledState {
        let layout = common::resolve_layout(ctx);
        InstalledState::load(&layout.state_dir.join("installed.toml")).expect("load state")
    }

    fn repo_with_rpm_map(pairs: &[(&str, &str)]) -> RepoConfig {
        let mut map = String::new();
        for (k, v) in pairs {
            map.push_str(&format!("{k} = \"{v}\"\n"));
        }
        RepoConfig::from_toml_str(&format!(
            "schema_version = 1\ndefault_backend = \"rpm\"\n[backends.rpm]\nbase_url = \"https://e/x\"\n[backends.rpm.package_map]\n{map}"
        ))
        .expect("parse repo")
    }

    // ── §5 package-name mapping ──

    #[test]
    fn candidates_cli_override_wins() {
        let q = FakeQuery::default();
        let got =
            rpm_package_candidates(Some("explicit-pkg"), None, None, &q, "copilot-shell").unwrap();
        assert_eq!(got, vec!["explicit-pkg".to_string()]);
    }

    #[test]
    fn candidates_manifest_package_wins_over_default() {
        let manifest = ComponentManifest::from_toml_str(
            "[component]\nname = \"copilot-shell\"\nversion = \"1.0.0\"\nlayer = \"runtime\"\n\n[backends.rpm]\npackage = \"vendor-copilot\"\n",
        )
        .expect("parse manifest");
        let q = FakeQuery::default();
        let got = rpm_package_candidates(None, Some(&manifest), None, &q, "copilot-shell").unwrap();
        assert_eq!(got, vec!["vendor-copilot".to_string()]);
    }

    #[test]
    fn candidates_package_map_wins_over_default() {
        let repo = repo_with_rpm_map(&[("copilot-shell", "site-copilot")]);
        let backend = repo.backends.get("rpm");
        let q = FakeQuery::default();
        let got = rpm_package_candidates(None, None, backend, &q, "copilot-shell").unwrap();
        assert_eq!(got, vec!["site-copilot".to_string()]);
    }

    #[test]
    fn candidates_provides_single_match() {
        let q = FakeQuery {
            provides: vec![(
                "anolisa-component(copilot-shell)".to_string(),
                vec!["anolisa-copilot-shell".to_string()],
            )],
            ..Default::default()
        };
        let got = rpm_package_candidates(None, None, None, &q, "copilot-shell").unwrap();
        assert_eq!(got, vec!["anolisa-copilot-shell".to_string()]);
    }

    #[test]
    fn candidates_provides_multiple_is_ambiguous() {
        let q = FakeQuery {
            provides: vec![(
                "anolisa-component(copilot-shell)".to_string(),
                vec!["pkg-a".to_string(), "pkg-b".to_string()],
            )],
            ..Default::default()
        };
        let got = rpm_package_candidates(None, None, None, &q, "copilot-shell").unwrap();
        assert_eq!(got, vec!["pkg-a".to_string(), "pkg-b".to_string()]);
    }

    #[test]
    fn candidates_falls_back_to_default_naming() {
        let q = FakeQuery::default();
        let got = rpm_package_candidates(None, None, None, &q, "copilot-shell").unwrap();
        assert_eq!(got, vec!["anolisa-copilot-shell".to_string()]);
    }

    // ── §5/§7.1 situation probe ──

    #[test]
    fn probe_reports_adoptable_for_installed_default_name() {
        let (_tmp, ctx) = system_ctx_with_raw_repo(false);
        let repo = RepoConfig::load(&common::resolve_layout(&ctx)).expect("repo");
        let q = FakeQuery {
            installed: vec![(
                "anolisa-copilot-shell".to_string(),
                pkg_info("anolisa-copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
            )],
            ..Default::default()
        };
        let situation = probe_rpm_situation(
            "copilot-shell",
            &args("copilot-shell"),
            &repo,
            &ctx,
            &q,
            "install",
        )
        .expect("probe");
        match situation {
            RpmSituation::Adoptable { package, info } => {
                assert_eq!(package, "anolisa-copilot-shell");
                assert_eq!(info.version.to_string(), "2.3.0-1.al8");
            }
            other => panic!(
                "expected Adoptable, got {other:?}",
                other = situation_label(&other)
            ),
        }
    }

    #[test]
    fn probe_reports_absent_when_not_installed() {
        let (_tmp, ctx) = system_ctx_with_raw_repo(false);
        let repo = RepoConfig::load(&common::resolve_layout(&ctx)).expect("repo");
        let q = FakeQuery::default();
        let situation = probe_rpm_situation(
            "copilot-shell",
            &args("copilot-shell"),
            &repo,
            &ctx,
            &q,
            "install",
        )
        .expect("probe");
        assert!(matches!(situation, RpmSituation::Absent));
    }

    #[test]
    fn probe_reports_ambiguous_for_multiple_providers() {
        let (_tmp, ctx) = system_ctx_with_raw_repo(false);
        let repo = RepoConfig::load(&common::resolve_layout(&ctx)).expect("repo");
        let q = FakeQuery {
            provides: vec![(
                "anolisa-component(copilot-shell)".to_string(),
                vec!["pkg-a".to_string(), "pkg-b".to_string()],
            )],
            ..Default::default()
        };
        let situation = probe_rpm_situation(
            "copilot-shell",
            &args("copilot-shell"),
            &repo,
            &ctx,
            &q,
            "install",
        )
        .expect("probe");
        assert!(matches!(situation, RpmSituation::Ambiguous(_)));
    }

    #[test]
    fn probe_reports_multi_version_drift() {
        let (_tmp, ctx) = system_ctx_with_raw_repo(false);
        let repo = RepoConfig::load(&common::resolve_layout(&ctx)).expect("repo");
        let q = FakeQuery {
            multi_version: vec!["anolisa-copilot-shell".to_string()],
            ..Default::default()
        };
        let situation = probe_rpm_situation(
            "copilot-shell",
            &args("copilot-shell"),
            &repo,
            &ctx,
            &q,
            "install",
        )
        .expect("probe");
        assert!(matches!(situation, RpmSituation::MultiVersion(_)));
    }

    fn situation_label(s: &RpmSituation) -> &'static str {
        match s {
            RpmSituation::Adoptable { .. } => "Adoptable",
            RpmSituation::Absent => "Absent",
            RpmSituation::Ambiguous(_) => "Ambiguous",
            RpmSituation::MultiVersion(_) => "MultiVersion",
        }
    }

    // ── §7.2 adopt state write ──

    #[test]
    fn adopt_writes_rpm_observed_state() {
        let (_tmp, ctx) = system_ctx_with_raw_repo(false);
        let q = FakeQuery {
            installed: vec![(
                "anolisa-copilot-shell".to_string(),
                pkg_info("anolisa-copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
            )],
            origins: vec![("anolisa-copilot-shell".to_string(), "@System".to_string())],
            ..Default::default()
        };
        let outcome =
            handle_one_with_query("copilot-shell".to_string(), args("copilot-shell"), &ctx, &q)
                .expect("adopt ok");
        assert_eq!(outcome, InstallOutcome::Adopted);

        let state = load_state(&ctx);
        let obj = state
            .find_object(ObjectKind::Component, "copilot-shell")
            .expect("component recorded");
        assert_eq!(obj.status, ObjectStatus::Adopted);
        assert_eq!(obj.ownership, Some(Ownership::RpmObserved));
        assert_eq!(obj.install_backend.as_deref(), Some("rpm"));
        assert!(!obj.managed, "rpm-observed must not be ANOLISA-managed");
        assert!(obj.adopted);
        assert!(obj.files.is_empty(), "RPM-owned files stay out of state");
        assert_eq!(obj.version, "2.3.0-1.al8");
        let meta = obj.rpm_metadata.as_ref().expect("rpm metadata");
        assert_eq!(meta.package_name, "anolisa-copilot-shell");
        assert_eq!(meta.evr.as_deref(), Some("2.3.0-1.al8"));
        assert_eq!(meta.arch.as_deref(), Some("x86_64"));
        assert_eq!(meta.source_repo.as_deref(), Some("@System"));
    }

    #[test]
    fn adopt_dry_run_does_not_write_state() {
        let (_tmp, ctx) = system_ctx_with_raw_repo(true);
        let q = FakeQuery {
            installed: vec![(
                "anolisa-copilot-shell".to_string(),
                pkg_info("anolisa-copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
            )],
            ..Default::default()
        };
        let outcome =
            handle_one_with_query("copilot-shell".to_string(), args("copilot-shell"), &ctx, &q)
                .expect("adopt plan ok");
        assert_eq!(outcome, InstallOutcome::Adopted);
        let state = load_state(&ctx);
        assert!(
            state
                .find_object(ObjectKind::Component, "copilot-shell")
                .is_none(),
            "dry-run must not persist adopt state"
        );
    }

    #[test]
    fn adopt_refresh_overwrites_evr() {
        let (_tmp, ctx) = system_ctx_with_raw_repo(false);
        // Pre-seed an older rpm-observed record.
        let mut state = InstalledState {
            install_mode: StateInstallMode::System,
            prefix: common::resolve_layout(&ctx).prefix.clone(),
            ..Default::default()
        };
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Component,
            name: "copilot-shell".to_string(),
            version: "2.2.0-1.al8".to_string(),
            status: ObjectStatus::Adopted,
            manifest_digest: None,
            distribution_source: None,
            install_backend: Some("rpm".to_string()),
            ownership: Some(Ownership::RpmObserved),
            rpm_metadata: Some(RpmMetadata {
                package_name: "anolisa-copilot-shell".to_string(),
                evr: Some("2.2.0-1.al8".to_string()),
                arch: Some("x86_64".to_string()),
                source_repo: Some("@System".to_string()),
            }),
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: Some("op-prior".to_string()),
            managed: false,
            adopted: true,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        });
        state
            .save(
                &common::resolve_layout(&ctx)
                    .state_dir
                    .join("installed.toml"),
            )
            .expect("seed state");

        // rpmdb now reports a newer EVR.
        let q = FakeQuery {
            installed: vec![(
                "anolisa-copilot-shell".to_string(),
                pkg_info("anolisa-copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
            )],
            origins: vec![("anolisa-copilot-shell".to_string(), "@System".to_string())],
            ..Default::default()
        };
        // No --backend: existing rpm-observed state must route to adopt-refresh,
        // not be blocked by the raw trunk.
        let outcome =
            handle_one_with_query("copilot-shell".to_string(), args("copilot-shell"), &ctx, &q)
                .expect("refresh ok");
        assert_eq!(outcome, InstallOutcome::Adopted);
        let state = load_state(&ctx);
        let obj = state
            .find_object(ObjectKind::Component, "copilot-shell")
            .expect("still recorded");
        assert_eq!(obj.version, "2.3.0-1.al8");
        assert_eq!(
            obj.rpm_metadata.as_ref().and_then(|m| m.evr.as_deref()),
            Some("2.3.0-1.al8")
        );
    }

    #[test]
    fn adopt_origin_failure_degrades_to_none() {
        let (_tmp, ctx) = system_ctx_with_raw_repo(false);
        let q = FakeQuery {
            installed: vec![(
                "anolisa-copilot-shell".to_string(),
                pkg_info("anolisa-copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
            )],
            origin_fails: true,
            ..Default::default()
        };
        let outcome =
            handle_one_with_query("copilot-shell".to_string(), args("copilot-shell"), &ctx, &q)
                .expect("adopt still succeeds");
        assert_eq!(outcome, InstallOutcome::Adopted);
        let state = load_state(&ctx);
        let obj = state
            .find_object(ObjectKind::Component, "copilot-shell")
            .expect("recorded");
        assert_eq!(
            obj.rpm_metadata
                .as_ref()
                .and_then(|m| m.source_repo.as_deref()),
            None,
            "origin lookup failure must degrade source_repo to None, not fail the adopt"
        );
    }

    #[test]
    fn explicit_rpm_not_installed_is_not_implemented() {
        let (_tmp, ctx) = system_ctx_with_raw_repo(false);
        let q = FakeQuery::default();
        let mut a = args("copilot-shell");
        a.backend = Some("rpm".to_string());
        let err = handle_one_with_query("copilot-shell".to_string(), a, &ctx, &q)
            .expect_err("not installed → not implemented");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
        // The #959 delegation hint rides on the NotImplemented variant; reason()
        // surfaces the command, which still names the rpm backend.
        assert!(err.reason().contains("rpm"), "got: {}", err.reason());
    }

    #[test]
    fn adopt_ambiguous_is_invalid_argument() {
        let (_tmp, ctx) = system_ctx_with_raw_repo(false);
        let q = FakeQuery {
            provides: vec![(
                "anolisa-component(copilot-shell)".to_string(),
                vec!["pkg-a".to_string(), "pkg-b".to_string()],
            )],
            ..Default::default()
        };
        let err =
            handle_one_with_query("copilot-shell".to_string(), args("copilot-shell"), &ctx, &q)
                .expect_err("ambiguous → refuse");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("pkg-a") && err.reason().contains("pkg-b"));
    }

    #[test]
    fn explicit_rpm_in_user_mode_is_rejected() {
        // route_rpm_adopt rejects user scope before touching rpmdb; call it
        // directly so the test needs no $HOME isolation.
        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        let repo =
            RepoConfig::from_toml_str("schema_version = 1\ndefault_backend = \"raw\"\n[backends.raw]\nbase_url = \"https://e/x\"\n")
                .expect("repo");
        let installed = InstalledState::default();
        let q = FakeQuery::default();
        let mut user_ctx = ctx_with_prefix(false, Some(tmp.path().to_path_buf()));
        user_ctx.install_mode = InstallMode::User;

        let mut a = args("copilot-shell");
        a.backend = Some("rpm".to_string());
        let err = route_rpm_adopt(
            "copilot-shell",
            &a,
            &user_ctx,
            "install copilot-shell",
            &layout,
            &repo,
            &installed,
            BackendSource::Explicit,
            None,
            &q,
        )
        .expect_err("user mode must be rejected");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("system"),
            "rejection must point at --install-mode system: {}",
            err.reason()
        );
    }

    #[test]
    fn explicit_rpm_on_raw_installed_component_is_rejected() {
        let (_tmp, ctx) = system_ctx_with_raw_repo(false);
        // Component already installed via raw.
        let mut state = InstalledState {
            install_mode: StateInstallMode::System,
            prefix: common::resolve_layout(&ctx).prefix.clone(),
            ..Default::default()
        };
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Component,
            name: "copilot-shell".to_string(),
            version: "1.0.0".to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: None,
            distribution_source: Some("https://example.com/raw".to_string()),
            install_backend: Some("raw".to_string()),
            ownership: Some(Ownership::RawManaged),
            rpm_metadata: None,
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: Some("op-prior".to_string()),
            managed: true,
            adopted: false,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        });
        state
            .save(
                &common::resolve_layout(&ctx)
                    .state_dir
                    .join("installed.toml"),
            )
            .expect("seed state");

        let q = FakeQuery {
            installed: vec![(
                "anolisa-copilot-shell".to_string(),
                pkg_info("anolisa-copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
            )],
            ..Default::default()
        };
        let mut a = args("copilot-shell");
        a.backend = Some("rpm".to_string());
        let err = handle_one_with_query("copilot-shell".to_string(), a, &ctx, &q)
            .expect_err("backend switch must be rejected");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("raw") && err.reason().contains("rpm"));
    }

    #[test]
    fn batch_status_maps_outcome_and_dry_run() {
        assert_eq!(batch_status(InstallOutcome::Installed, false), "installed");
        assert_eq!(batch_status(InstallOutcome::Installed, true), "planned");
        assert_eq!(batch_status(InstallOutcome::Adopted, false), "adopted");
        assert_eq!(batch_status(InstallOutcome::Adopted, true), "adopt-planned");
    }
}
