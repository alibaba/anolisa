//! Command dispatch for `install`: provider-family selection, target
//! resolution, and the observe → plan → execute pipeline for one component.
//!
//! The handler resolves the requested component to a [`ProviderTarget`],
//! assembles host facts, asks the planner for a step sequence (decision
//! table I1–I11), and hands it to the matching executor: an owned plan runs
//! through [`RawInstallOps`], a delegated plan re-uses the native-transaction
//! executor with a [`StoreRecordSink`]. No lifecycle policy lives here.

use std::path::Path;

use anolisa_core::executor::{DelegatedExecutionTarget, execute_delegated_steps};
use anolisa_core::facts::{
    FactsError, JournalEvidence, ObserveRequest, assemble_facts, pending_journal_for,
};
use anolisa_core::lock::InstallLock;
use anolisa_core::owned_executor::{OwnedExecutionError, execute_owned_steps};
use anolisa_core::planner::{
    HookKind, InstallRequest, Intent, NativeProbe, Plan, PlanError, ProviderTarget, Step, plan,
};
use anolisa_core::providers::{DelegatedProvider, ProviderError};
use anolisa_core::record_sink::{DelegatedIdentity, RecordContext, StoreRecordSink};
use anolisa_core::state::{ObjectKind, OperationRecord};
use anolisa_core::state_store::StateStore;
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::pkg_query::{PackageQuery, PackageQueryError};
use anolisa_platform::pkg_transaction::PackageTransaction;
use anolisa_platform::privilege;
use anolisa_platform::rpm_query::RpmPackageQuery;
use anolisa_platform::rpm_repo::DnfRepoSource;
use anolisa_platform::rpm_transaction::RpmTransaction;
use chrono::{SecondsFormat, Utc};
use serde::Serialize;

use anolisa_core::domain::{InstallationScope, NativePm, ProviderBinding};

use crate::commands::common;
use crate::commands::common::RepoPersistPolicy;
use crate::commands::tier1::recovery::LockedJournalGate;
use crate::commands::tier1::rpm_install;
use crate::context::{CliContext, InstallMode};
use crate::repo_config::{
    BackendConfig, HostVars, RepoConfig, RepoConfigError, normalize_override_url,
};
use crate::resolution::{
    BackendKind, ComponentResolver, ResolutionSet, ResolutionUse, ResolveOptions,
    load_optional_component_index,
};
use crate::response::{CliError, render_json};

use super::owned_ops::{
    RawInstallOps, ValidatedInstall, installed_version_label, validate_owned_install,
};
use super::raw::resolve_raw;
use super::render::repo_config_err;
use super::rpm::{
    PinError, RpmTarget, resolve_pinned_candidate, rpm_package_candidates_with_index,
};
use super::types::{InstallOutcome, RawResolution, ResolveInputs};
use super::{ANOLISA_RPM_REPO_ID, COMMAND, InstallArgs};

/// Dispatch `install <component>` against the live host.
pub(crate) fn handle_one(
    component: String,
    args: InstallArgs,
    ctx: &CliContext,
) -> Result<InstallOutcome, CliError> {
    let (query, txn) = host_backends(&component, &args, ctx)?;
    install_component_with_deps(&component, &args, ctx, &query, &txn, privilege::is_root())
}

/// Real host backends for one component invocation.
///
/// The backends receive the repo.toml RPM source when the rpm family will
/// execute, so availability probes and install transactions do not silently
/// fall back to the host's enabled system repos.
pub(crate) fn host_backends(
    component: &str,
    args: &InstallArgs,
    ctx: &CliContext,
) -> Result<(RpmPackageQuery, RpmTransaction), CliError> {
    let command = format!("{COMMAND} {component}");
    let layout = common::resolve_layout(ctx);
    let env = anolisa_env::EnvService::detect();
    let repo_config = common::load_repo_config(ctx, &layout, COMMAND, RepoPersistPolicy::Require)?;
    let (resolved, view, _) = common::resolve_install_target(component, ctx, &command)?;
    let store = &view.writable.state;

    let rpm_repo = if install_family(args, store, &resolved, &repo_config) == "rpm" {
        configured_rpm_repo_source(&repo_config, &env)?
    } else {
        None
    };
    let query = match rpm_repo.clone() {
        Some(repo) => RpmPackageQuery::system_with_repo(repo),
        None => RpmPackageQuery::system(),
    };
    let txn = match rpm_repo {
        Some(repo) => RpmTransaction::system_with_repo(repo),
        None => RpmTransaction::system(),
    };
    Ok((query, txn))
}

/// Provider family for this invocation: explicit `--backend` (canonical
/// spelling) wins, then the recorded provenance of an existing installation
/// (owned → raw, delegated → rpm), then repo.toml `default_backend`.
///
/// System-RPM presence deliberately plays no part here: it is a host fact
/// the planner rules on (I3), not a routing input.
fn install_family(
    args: &InstallArgs,
    store: &StateStore,
    component: &str,
    repo_config: &RepoConfig,
) -> String {
    if let Some(explicit) = args.backend.as_deref() {
        return RepoConfig::canonical_backend_name(explicit).to_string();
    }
    if let Some(installation) = store.find(ObjectKind::Component, component) {
        return match installation.binding {
            ProviderBinding::Owned { .. } => "raw".to_string(),
            ProviderBinding::Delegated { .. } => "rpm".to_string(),
        };
    }
    repo_config.default_backend.clone()
}

/// What the planning prefix decided for one component, before any side
/// effect ran: the resolved identity, the provider family, and the planned
/// route. The single-component path executes it directly; batch
/// orchestration classifies on the route to group fresh delegated installs
/// into one merged native transaction.
pub(crate) struct PlannedComponent {
    pub(crate) command: String,
    pub(crate) component: String,
    /// Lifecycle resolution pinned the literal input, so backend-level
    /// package aliases must not rewrite it to another component. This holds
    /// for an exact visible identity and for incomplete cross-scope
    /// visibility where aliasing would be unsafe.
    pub(crate) component_identity_pinned: bool,
    pub(crate) family: String,
    pub(crate) native_package: Option<String>,
    /// Resolved candidate for a `--version`-pinned delegated install, carried
    /// so the dry-run preview and JSON envelope report the real artifact.
    /// `None` for unpinned installs and every owned install.
    pub(crate) delegated_pin: Option<DelegatedPin>,
    pub(crate) scope: InstallationScope,
    pub(crate) now: String,
    pub(crate) store: StateStore,
    pub(crate) route: PlannedRoute,
}

/// Resolved metadata for a version-pinned delegated install, surfaced to the
/// dry-run preview and the JSON envelope. Built from the repository candidate
/// the pin selected, never from the raw `--version` argument alone.
#[derive(Debug, Clone)]
pub(crate) struct DelegatedPin {
    /// The `--version` value the caller requested.
    pub(crate) requested_version: String,
    /// Upstream VERSION field of the selected candidate (equals
    /// `requested_version`, restated for an unambiguous JSON contract).
    pub(crate) resolved_version: String,
    /// Full resolved EVR of the selected candidate.
    pub(crate) resolved_evr: String,
    /// Architecture of the selected candidate, checked against the freshly
    /// installed package before the record commits.
    pub(crate) resolved_arch: String,
    /// Exact NEVRA handed to the native transaction.
    pub(crate) artifact: String,
    /// Source repository the candidate came from, when reported.
    pub(crate) source_repo: Option<String>,
}

struct ResolvedInstallIdentity {
    component: String,
    pinned: bool,
}

/// Which executor family the plan routed to, or the idempotent NoOp.
pub(crate) enum PlannedRoute {
    /// I4/I8: the record already covers the request; nothing to execute.
    AlreadyInstalled { version: Option<String> },
    /// Delegated step family (I2 for a fresh install): one native
    /// transaction, a fresh observation, and a record commit.
    Delegated { steps: Vec<Step> },
    /// Owned step family; raw artifact resolution (network) is deferred to
    /// execution.
    Owned { steps: Vec<Step> },
}

impl PlannedRoute {
    /// The planned steps, empty for the NoOp route.
    pub(crate) fn steps(&self) -> &[Step] {
        match self {
            Self::AlreadyInstalled { .. } => &[],
            Self::Delegated { steps } | Self::Owned { steps } => steps,
        }
    }
}

/// Core of [`handle_one`] with the package backends injected so tests drive
/// every branch without a live rpmdb/dnf or real privileges.
pub(crate) fn install_component_with_deps(
    input: &str,
    args: &InstallArgs,
    ctx: &CliContext,
    query: &dyn PackageQuery,
    txn: &dyn PackageTransaction,
    is_root: bool,
) -> Result<InstallOutcome, CliError> {
    let planned = plan_component(input, args, ctx, query, txn)?;
    execute_planned(planned, args, ctx, query, txn, is_root)
}

/// Planning prefix of an install: resolve the component and its provider
/// target, assemble host facts, and ask the planner for the step sequence.
/// Read-only against the host — every side effect belongs to
/// [`execute_planned`].
pub(crate) fn plan_component(
    input: &str,
    args: &InstallArgs,
    ctx: &CliContext,
    query: &dyn PackageQuery,
    txn: &dyn PackageTransaction,
) -> Result<PlannedComponent, CliError> {
    let command = format!("{COMMAND} {input}");
    let layout = common::resolve_layout(ctx);
    let journal_dir = rpm_install::journal_dir(&layout);
    let uid = privilege::effective_uid();
    let scope = match ctx.install_mode {
        InstallMode::System => InstallationScope::System,
        InstallMode::User => InstallationScope::User { uid },
    };
    let now = now_iso8601();
    let env = anolisa_env::EnvService::detect();
    let repo_config = common::load_repo_config(ctx, &layout, COMMAND, RepoPersistPolicy::Require)?;

    // Resolve identity across all visible roots, but bind install planning to
    // the writable scope only. A user install may therefore shadow an
    // existing system installation without mutating or inheriting it.
    let (mut component, view, component_identity_pinned) =
        common::resolve_install_target(input, ctx, &command)?;
    let store = view.writable.state;

    if let Some(explicit) = args.backend.as_deref()
        && let Some(warning) = RepoConfig::backend_name_deprecation_warning(explicit)
    {
        eprintln!("warning: {warning}");
    }

    let family = install_family(args, &store, &component, &repo_config);

    // Backend gate: only the raw and rpm families have executors. The
    // selection call validates the name and its configuration first, so an
    // unconfigured or unknown backend stays INVALID_ARGUMENT. Over an
    // existing record the provenance conflict outranks the missing executor:
    // the request would be refused even if the backend could install.
    if family != "raw" && family != "rpm" {
        let (backend_name, _) = repo_config
            .select_backend(Some(family.as_str()))
            .map_err(|err| repo_config_err(err, true))?;
        if let Some(installation) = store.find(ObjectKind::Component, &component) {
            let installed_backend = match installation.binding {
                ProviderBinding::Owned { .. } => "raw",
                ProviderBinding::Delegated { .. } => "rpm",
            };
            return Err(CliError::InvalidArgument {
                command,
                reason: format!(
                    "component '{component}' is already installed via backend '{installed_backend}'; reinstalling it via backend '{backend_name}' is not allowed — uninstall it first or use backend '{installed_backend}'"
                ),
            });
        }
        return Err(CliError::not_implemented_with_hint(
            format!("install --backend {backend_name}"),
            format!(
                "the '{backend_name}' backend is configured but its executor is not implemented yet — only 'raw' and 'rpm' can install today",
            ),
        ));
    }

    // I10 short-circuit: a quarantined record decides the outcome before any
    // network or rpmdb resolution has to run.
    if quarantined(&store, &component) {
        return Err(plan_error_to_cli(
            PlanError::NeedsAttention,
            &component,
            &command,
        ));
    }

    // Same for a pending operation journal: it blocks any new mutation, and
    // the refusal must not depend on the rpm candidate chain or the raw repo
    // resolving.
    let pending = pending_journal_for(
        JournalEvidence::new(&journal_dir, &store.operations),
        &component,
    )
    .map_err(|err| CliError::Runtime {
        command: command.clone(),
        reason: err.to_string(),
    })?;
    if pending.is_some() {
        return Err(plan_error_to_cli(
            PlanError::PendingOperation,
            &component,
            &command,
        ));
    }

    // Resolve the provider target and the native probe package. Nothing here
    // touches the raw repository: an active record's arms all end in a NoOp
    // or a refusal (I4–I9, I11), and a fresh owned target's plan does not
    // depend on the resolved version — only the probe answer (I3) matters,
    // and that answer must not depend on the repo being reachable.
    let active_binding = store
        .find(ObjectKind::Component, &component)
        .map(|installation| installation.binding.clone());
    let mut delegated_pin: Option<DelegatedPin> = None;
    let (target, native_package): (ProviderTarget, Option<String>) = match &active_binding {
        Some(binding) => target_for_active_record(binding, &family, args, &component),
        None if family == "raw" => {
            let native_package = match scope {
                InstallationScope::System => Some(system_probe_package(
                    args,
                    &layout,
                    &env,
                    &repo_config,
                    &component,
                    query,
                    &command,
                )?),
                InstallationScope::User { .. } => None,
            };
            (
                ProviderTarget::Owned {
                    version: args.version.clone().unwrap_or_default(),
                },
                native_package,
            )
        }
        None => {
            // Delegated targets need system scope; in user scope the planner
            // refuses (its first guard), so nothing touches the rpmdb here.
            if !matches!(scope, InstallationScope::System) {
                let package = args.package.clone().unwrap_or_else(|| component.clone());
                (
                    ProviderTarget::Delegated {
                        pm: NativePm::Rpm,
                        package,
                        artifact: None,
                    },
                    None,
                )
            } else {
                let fresh = resolve_fresh_delegated(
                    args,
                    &layout,
                    &env,
                    &repo_config,
                    &component,
                    query,
                    &command,
                )?;
                component = fresh.component;
                delegated_pin = fresh.pin;
                (fresh.target, Some(fresh.package))
            }
        }
    };

    let provider = DelegatedProvider::new(query, txn);
    let observe_request = ObserveRequest {
        kind: ObjectKind::Component,
        name: &component,
        scope,
        native_package: native_package.as_deref(),
        observed_at: &now,
        verify_owned_files: false,
    };
    // A missing rpm/dnf binary is a hard error whenever a probe was needed:
    // without it the host cannot prove the component is not an unobserved
    // system RPM, and a raw install over one could corrupt it (I3).
    let facts = assemble_facts(
        &observe_request,
        &store,
        Some(&provider),
        &layout,
        &journal_dir,
    )
    .map_err(|err| match err {
        FactsError::Probe(ProviderError::Query(PackageQueryError::CommandMissing {
            command: bin,
        })) => rpm_tooling_missing_error(&command, &bin, &component),
        err => CliError::Runtime {
            command: command.clone(),
            reason: err.to_string(),
        },
    })?;

    let request = InstallRequest {
        target,
        requested_version: args.version.clone(),
    };
    let route = match plan(&Intent::Install(request), &facts) {
        Ok(Plan::Execute { steps, .. }) => {
            // Route by step family: a delegated plan requests one native
            // transaction, an owned plan places the resolved artifact through
            // the raw backend.
            let is_delegated_plan = steps.iter().all(|step| {
                matches!(
                    step,
                    Step::NativeTransaction { .. }
                        | Step::Observe { .. }
                        | Step::WriteRecord(_)
                        | Step::DropRecord
                )
            });
            if is_delegated_plan {
                PlannedRoute::Delegated { steps }
            } else {
                PlannedRoute::Owned { steps }
            }
        }
        Ok(Plan::NoOp { .. }) => {
            // I4/I8: install is idempotent over a healthy record.
            let version = store
                .find(ObjectKind::Component, &component)
                .map(installed_version_label);
            PlannedRoute::AlreadyInstalled { version }
        }
        Err(err) => return Err(plan_error_to_cli(err, &component, &command)),
    };

    Ok(PlannedComponent {
        command,
        component,
        component_identity_pinned,
        family,
        native_package,
        delegated_pin,
        scope,
        now,
        store,
        route,
    })
}

/// Execution half of [`install_component_with_deps`]: render the idempotent
/// NoOp, place a resolved owned artifact, or run the delegated native
/// transaction. Dry-run renders the plan and stops before any side effect.
fn execute_planned(
    planned: PlannedComponent,
    args: &InstallArgs,
    ctx: &CliContext,
    query: &dyn PackageQuery,
    txn: &dyn PackageTransaction,
    is_root: bool,
) -> Result<InstallOutcome, CliError> {
    let PlannedComponent {
        command,
        mut component,
        component_identity_pinned,
        family,
        native_package,
        delegated_pin,
        scope,
        now,
        store,
        route,
    } = planned;
    let layout = common::resolve_layout(ctx);
    let env = anolisa_env::EnvService::detect();
    let repo_config = common::load_repo_config(ctx, &layout, COMMAND, RepoPersistPolicy::Require)?;
    let state_path = layout.state_dir.join("installed.toml");
    let journal_dir = rpm_install::journal_dir(&layout);

    // Only a settled owned plan resolves the raw artifact (network) — every
    // planning refusal is independent of the raw repo being reachable.
    let (steps, resolution) = match route {
        PlannedRoute::AlreadyInstalled { version } => {
            render_result(
                ctx,
                &InstallResultPayload {
                    component: component.clone(),
                    package: native_package,
                    version,
                    backend: family,
                    action: "already-installed",
                    operation_id: None,
                    requested_version: None,
                    resolved_version: None,
                    source_repo: None,
                    artifact: None,
                    dry_run: ctx.dry_run,
                    plan: Vec::new(),
                },
            )?;
            return Ok(InstallOutcome::AlreadyInstalled);
        }
        PlannedRoute::Delegated { steps } => (steps, None),
        PlannedRoute::Owned { steps } => {
            let resolution = resolve_owned_artifact(
                args,
                ctx,
                &layout,
                &env,
                &repo_config,
                ResolvedInstallIdentity {
                    component: component.clone(),
                    pinned: component_identity_pinned,
                },
                &command,
            )?;
            component = resolution.component.clone();
            (steps, Some(resolution))
        }
    };

    let plan_labels: Vec<String> = steps.iter().map(step_label).collect();

    if ctx.dry_run {
        for warning in resolution.iter().flat_map(|r| r.warnings.iter()) {
            eprintln!("warning: {warning}");
        }
        // A pinned delegated dry-run reports the version it resolved against
        // the repository, not the raw `--version` echo — the pin fields carry
        // the exact candidate so the preview proves what would be installed.
        let base_version = resolution
            .as_ref()
            .map(|r| r.entry.version.clone())
            .or_else(|| args.version.clone());
        let mut payload = InstallResultPayload {
            component,
            package: native_package,
            version: base_version,
            backend: family,
            action: "planned",
            operation_id: None,
            requested_version: None,
            resolved_version: None,
            source_repo: None,
            artifact: None,
            dry_run: true,
            plan: plan_labels,
        };
        if let Some(pin) = &delegated_pin {
            payload = payload.with_pin(pin);
        }
        render_result(ctx, &payload)?;
        return Ok(InstallOutcome::Installed);
    }

    if let Some(resolution) = resolution {
        // Download, digest check, and contract validation run before the
        // lock: they are side-effect free outside the download cache, and a
        // contract refusal (mode mismatch, component conflict, malformed
        // hooks) is an argument error, not a failed transaction.
        let validated = validate_owned_install(ctx, &layout, &store, resolution, &command)?;
        let provider = DelegatedProvider::new(query, txn);
        return install_owned(
            &component,
            ctx,
            &layout,
            &state_path,
            &journal_dir,
            scope,
            &now,
            &steps,
            &plan_labels,
            validated,
            native_package.as_deref(),
            &provider,
            &command,
        );
    }

    let provider = DelegatedProvider::new(query, txn);
    let package = native_package.unwrap_or_else(|| component.clone());
    install_delegated(
        &component,
        &package,
        ctx,
        &layout,
        &state_path,
        &journal_dir,
        scope,
        &now,
        &steps,
        &plan_labels,
        delegated_pin.as_ref(),
        &provider,
        &repo_config,
        is_root,
        &command,
    )
}

/// Target shape for an existing active record. No remote resolution: the
/// planner's active-record arms only compare identities and versions.
fn target_for_active_record(
    binding: &ProviderBinding,
    family: &str,
    args: &InstallArgs,
    component: &str,
) -> (ProviderTarget, Option<String>) {
    match binding {
        ProviderBinding::Owned { artifact } => {
            let target = if family == "raw" {
                ProviderTarget::Owned {
                    version: artifact.version.clone(),
                }
            } else {
                // Family switch over an owned record: the planner refuses
                // with ProvenanceConflict (I11) before probing anything.
                ProviderTarget::Delegated {
                    pm: NativePm::Rpm,
                    package: args
                        .package
                        .clone()
                        .unwrap_or_else(|| component.to_string()),
                    artifact: None,
                }
            };
            (target, None)
        }
        ProviderBinding::Delegated { package, .. } => {
            let recorded = package.resolved_name().map(str::to_string);
            let package = args
                .package
                .clone()
                .or(recorded)
                .unwrap_or_else(|| component.to_string());
            let target = if family == "raw" {
                ProviderTarget::Owned {
                    version: args.version.clone().unwrap_or_default(),
                }
            } else {
                // Version pins over an existing record are out of scope: the
                // planner's active-record arms decide the outcome (I6–I9) and
                // never re-resolve a pinned artifact.
                ProviderTarget::Delegated {
                    pm: NativePm::Rpm,
                    package: package.clone(),
                    artifact: None,
                }
            };
            (target, Some(package))
        }
    }
}

/// Owned artifact resolution for a settled I1 plan: repo.toml → base_url →
/// package → distribution index entry.
fn resolve_owned_artifact(
    args: &InstallArgs,
    ctx: &CliContext,
    layout: &FsLayout,
    env: &anolisa_env::EnvFacts,
    repo_config: &RepoConfig,
    identity: ResolvedInstallIdentity,
    command: &str,
) -> Result<RawResolution, CliError> {
    let ResolvedInstallIdentity {
        component,
        pinned: component_identity_pinned,
    } = identity;
    let (backend_name, backend) = repo_config
        .select_backend(Some("raw"))
        .map_err(|err| repo_config_err(err, true))?;

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
    let (component, package) = resolve_raw_identity(
        layout,
        env,
        repo_config,
        backend,
        component,
        args.package.as_deref(),
        component_identity_pinned,
    );
    resolve_raw(
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
    )
    .map_err(|err| err.with_command(command))
}

/// RPM package name a raw install probes for the planner's I3 rule.
fn system_probe_package(
    args: &InstallArgs,
    layout: &FsLayout,
    env: &anolisa_env::EnvFacts,
    repo_config: &RepoConfig,
    component: &str,
    query: &dyn PackageQuery,
    command: &str,
) -> Result<String, CliError> {
    let component_index = load_optional_component_index(layout, env, repo_config);
    let candidates = rpm_package_candidates_with_index(
        args.package.as_deref(),
        repo_config.backends.get("rpm"),
        component_index.as_ref(),
        query,
        component,
        ResolutionUse::Install,
    )
    .map_err(|err| match err {
        PackageQueryError::CommandMissing { command: bin } => {
            rpm_tooling_missing_error(command, &bin, component)
        }
        err => pkg_query_err(err, command),
    })?;
    Ok(match candidates.as_slice() {
        [single] => single.package.clone(),
        _ => component.to_string(),
    })
}

/// Fresh delegated resolution result: the provider target, its bare package,
/// the resolved component name (aliases may re-map the input), and — when a
/// `--version` was pinned — the resolved candidate metadata for reporting.
struct FreshDelegated {
    target: ProviderTarget,
    package: String,
    component: String,
    pin: Option<DelegatedPin>,
}

/// Fresh delegated resolution: the component must resolve to exactly one
/// ANOLISA RPM package. When `--version` is set, the bare package is further
/// resolved to an exact repository candidate for the host architecture, and
/// the resulting NEVRA becomes the native transaction's artifact spec while
/// the bare package stays the observation/record identity.
fn resolve_fresh_delegated(
    args: &InstallArgs,
    layout: &FsLayout,
    env: &anolisa_env::EnvFacts,
    repo_config: &RepoConfig,
    component: &str,
    query: &dyn PackageQuery,
    command: &str,
) -> Result<FreshDelegated, CliError> {
    let component_index = load_optional_component_index(layout, env, repo_config);
    let candidates = rpm_package_candidates_with_index(
        args.package.as_deref(),
        repo_config.backends.get("rpm"),
        component_index.as_ref(),
        query,
        component,
        ResolutionUse::Install,
    )
    .map_err(|err| match err {
        PackageQueryError::CommandMissing { command: bin } => {
            rpm_tooling_missing_error(command, &bin, component)
        }
        err => pkg_query_err(err, command),
    })?;
    match candidates.as_slice() {
        [] => Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "component '{component}' is not an ANOLISA RPM component; use the ANOLISA component name and configure the repo-side component index or publish Provides: anolisa-component({component})"
            ),
        }),
        [single] => {
            let package = single.package.clone();
            let resolved_component = single.component.clone();
            // A `--version` pin resolves the bare package to an exact
            // repository candidate before any mutation; the NEVRA is the only
            // value that reaches the native transaction. An unpinned install
            // keeps `artifact` as `None` (repository default).
            let (artifact, pin) = match args.version.as_deref() {
                Some(version) => {
                    let pinned = resolve_pinned_candidate(query, &package, version, &env.arch)
                        .map_err(|err| {
                            pin_error_to_cli(
                                err,
                                command,
                                &resolved_component,
                                &package,
                                version,
                                &env.arch,
                            )
                        })?;
                    let pin = DelegatedPin {
                        requested_version: version.to_string(),
                        resolved_version: pinned.version.clone(),
                        resolved_evr: pinned.evr.clone(),
                        resolved_arch: pinned.arch.clone(),
                        artifact: pinned.artifact.clone(),
                        source_repo: pinned.source_repo.clone(),
                    };
                    (Some(pinned.artifact), Some(pin))
                }
                None => (None, None),
            };
            Ok(FreshDelegated {
                target: ProviderTarget::Delegated {
                    pm: NativePm::Rpm,
                    package: package.clone(),
                    artifact,
                },
                package,
                component: resolved_component,
                pin,
            })
        }
        many => Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "multiple RPM candidates match '{component}': {}; cannot resolve unambiguously — pin one with `--package <name>` or fix the component index / package metadata",
                many.iter()
                    .map(RpmTarget::label)
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        }),
    }
}

/// Map a version-pin resolution failure to a CLI error. Runs during read-only
/// planning, so every arm reports that nothing was changed and never widens
/// the version constraint.
fn pin_error_to_cli(
    err: PinError,
    command: &str,
    component: &str,
    package: &str,
    version: &str,
    arch: &str,
) -> CliError {
    match err {
        PinError::Query(PackageQueryError::CommandMissing { command: bin }) => {
            rpm_tooling_missing_error(command, &bin, component)
        }
        PinError::Query(err) => pkg_query_err(err, command),
        PinError::VersionAbsent => CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "version '{version}' of component '{component}' (package '{package}') is not available in the configured ANOLISA RPM repository; nothing was changed — check `anolisa list` / the published versions and retry with an available `--version`"
            ),
        },
        PinError::ArchUnsupported { offered } => CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "version '{version}' of component '{component}' (package '{package}') is not available for this host architecture '{arch}' (repository offers: {}); nothing was changed",
                offered.join(", ")
            ),
        },
    }
}

/// Execute an owned install plan (I1) through the raw backend.
///
/// The state and relevant system package are re-read under the install lock,
/// preventing a stale fresh-install plan from claiming either authority.
#[expect(clippy::too_many_arguments)]
fn install_owned(
    target: &str,
    ctx: &CliContext,
    layout: &FsLayout,
    state_path: &Path,
    journal_dir: &Path,
    scope: InstallationScope,
    now: &str,
    steps: &[Step],
    plan_labels: &[String],
    validated: ValidatedInstall,
    native_package: Option<&str>,
    provider: &DelegatedProvider,
    command: &str,
) -> Result<InstallOutcome, CliError> {
    // No root pre-check for an owned install: `--prefix` may point at a
    // user-writable tree, and a genuine permission problem fails the exact
    // step and unwinds honestly instead of a blanket refusal.
    let package = validated.package().to_string();
    let version = validated.version().to_string();
    let resolve_warnings = validated.warnings().to_vec();

    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;
    let mut store = StateStore::load_for_layout(state_path, privilege::effective_uid(), layout)
        .map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!("failed to load installed state: {err}"),
        })?;
    if store.find(ObjectKind::Component, target).is_some() || quarantined(&store, target) {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "a record for '{target}' appeared while this install was resolving; nothing was changed — re-run `anolisa install {target}`"
            ),
        });
    }
    revalidate_native_absence(native_package, provider, now, target, command)?;

    let evidence = JournalEvidence::new(journal_dir, &store.operations);
    let mut journal_gate = LockedJournalGate::load(&_lock, evidence, command)?;
    let mut journal = journal_gate.begin(COMMAND, target, state_path.to_path_buf(), command)?;
    let operation_id = journal.operation_id.clone();

    let (result, retained_note) = {
        let mut ops = RawInstallOps::new(
            ctx,
            layout,
            target.to_string(),
            scope,
            now.to_string(),
            operation_id.clone(),
            validated,
            &mut store,
            state_path,
        );
        let result = execute_owned_steps(steps, &mut ops, &mut journal);
        // Auto-provisioned system packages are retained on failure; the note
        // must be read out before the ops (and their borrow) are dropped.
        let note = ops.retained_packages_note();
        (result, note)
    };
    let outcome =
        result.map_err(|err| owned_error_to_cli(err, target, scope, command, &retained_note))?;

    // Operation history is best-effort bookkeeping on top of the committed
    // record: the install already succeeded, so a history-write failure
    // degrades to a warning instead of unwinding anything.
    store.operations.push(OperationRecord {
        id: operation_id.clone(),
        command: command.to_string(),
        status: "ok".to_string(),
        started_at: now.to_string(),
        finished_at: Some(now_iso8601()),
        parent_operation_id: None,
    });
    if let Err(err) = store.save(state_path) {
        eprintln!("warning: failed to record operation history: {err}");
    }

    for warning in resolve_warnings.iter().chain(outcome.warnings.iter()) {
        eprintln!("warning: {warning}");
    }

    render_result(
        ctx,
        &InstallResultPayload {
            component: target.to_string(),
            package: Some(package),
            version: Some(version),
            backend: "raw".to_string(),
            action: "installed",
            operation_id: Some(operation_id),
            requested_version: None,
            resolved_version: None,
            source_repo: None,
            artifact: None,
            dry_run: false,
            plan: plan_labels.to_vec(),
        },
    )?;
    Ok(InstallOutcome::Installed)
}

/// Execute a delegated install plan (I2): one native transaction, a fresh
/// observation, and a managed record.
#[expect(clippy::too_many_arguments)]
fn install_delegated(
    target: &str,
    package: &str,
    ctx: &CliContext,
    layout: &FsLayout,
    state_path: &Path,
    journal_dir: &Path,
    scope: InstallationScope,
    now: &str,
    steps: &[Step],
    plan_labels: &[String],
    delegated_pin: Option<&DelegatedPin>,
    provider: &DelegatedProvider,
    repo_config: &RepoConfig,
    is_root: bool,
    command: &str,
) -> Result<InstallOutcome, CliError> {
    // A fresh delegated install pulls from the configured ANOLISA RPM
    // repository; without one, dnf would resolve against arbitrary host
    // repos.
    require_configured_rpm_backend(repo_config, command)?;

    if !is_root {
        return Err(CliError::PermissionDenied {
            command: command.to_string(),
            reason: "installing an RPM-backed component runs dnf and requires root".to_string(),
            hint: Some(format!("sudo anolisa install {target}")),
        });
    }

    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;
    let mut store = StateStore::load_for_layout(state_path, privilege::effective_uid(), layout)
        .map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!("failed to load installed state: {err}"),
        })?;
    if store.find(ObjectKind::Component, target).is_some() || quarantined(&store, target) {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "a record for '{target}' appeared while this install was resolving; nothing was changed — re-run `anolisa install {target}`"
            ),
        });
    }
    revalidate_native_absence(Some(package), provider, now, target, command)?;

    let evidence = JournalEvidence::new(journal_dir, &store.operations);
    let mut journal_gate = LockedJournalGate::load(&_lock, evidence, command)?;
    let mut journal = journal_gate.begin(COMMAND, target, state_path.to_path_buf(), command)?;
    let operation_id = journal.operation_id.clone();

    let context = RecordContext {
        kind: ObjectKind::Component,
        name: target.to_string(),
        scope,
        now: now.to_string(),
        operation_id: Some(operation_id.clone()),
        delegated: Some(DelegatedIdentity {
            pm: NativePm::Rpm,
            package: package.to_string(),
        }),
        owned_artifact: None,
    };
    // Pin the execution target to the resolved artifact so the executor both
    // constrains the native transaction to the exact NEVRA and verifies the
    // freshly installed EVR/arch before committing the record.
    let mut exec_target = DelegatedExecutionTarget::new(NativePm::Rpm, Some(package));
    if let Some(pin) = delegated_pin {
        exec_target =
            exec_target.with_pinned_artifact(&pin.artifact, &pin.resolved_evr, &pin.resolved_arch);
    }
    let outcome = {
        let mut sink = StoreRecordSink::new(&mut store, state_path, context);
        execute_delegated_steps(steps, exec_target, provider, &mut sink, &mut journal, now)
    }
    .map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!(
            "install of '{target}' failed: {err}; the native transaction is never undone automatically — run `anolisa repair {target}` to reconcile"
        ),
    })?;

    // Operation history is best-effort bookkeeping on top of the committed
    // record, exactly like the owned path.
    store.operations.push(OperationRecord {
        id: operation_id.clone(),
        command: command.to_string(),
        status: "ok".to_string(),
        started_at: now.to_string(),
        finished_at: Some(now_iso8601()),
        parent_operation_id: None,
    });
    if let Err(err) = store.save(state_path) {
        eprintln!("warning: failed to record operation history: {err}");
    }

    // Best-effort: snapshot the datadir component contract so adapter
    // commands can discover declared adapters. Missing or unwritable
    // contracts produce warnings, never failures.
    for warning in super::io_util::snapshot_datadir_contract(
        layout,
        target,
        command,
        ctx.packaged_data_probe(),
    ) {
        eprintln!("warning: {warning}");
    }

    let version = outcome.observation.as_ref().map(|o| o.version.clone());
    let mut payload = InstallResultPayload {
        component: target.to_string(),
        package: Some(package.to_string()),
        version,
        backend: "rpm".to_string(),
        action: "installed",
        operation_id: Some(operation_id),
        requested_version: None,
        resolved_version: None,
        source_repo: None,
        artifact: None,
        dry_run: false,
        plan: plan_labels.to_vec(),
    };
    if let Some(pin) = delegated_pin {
        payload = payload.with_pin(pin);
    }
    render_result(ctx, &payload)?;
    Ok(InstallOutcome::Installed)
}

pub(crate) fn revalidate_native_absence(
    package: Option<&str>,
    provider: &DelegatedProvider,
    now: &str,
    target: &str,
    command: &str,
) -> Result<(), CliError> {
    let Some(package) = package else {
        return Ok(());
    };
    match provider.observe(package, now) {
        Ok(NativeProbe::Absent) => Ok(()),
        Ok(NativeProbe::Present { .. } | NativeProbe::MultipleVersions { .. }) => {
            Err(CliError::InvalidArgument {
                command: command.to_string(),
                reason: format!(
                    "system RPM '{package}' appeared while '{target}' was being resolved; nothing was changed — run `sudo anolisa --install-mode system adopt {target}` or retry after removing the external package"
                ),
            })
        }
        Ok(NativeProbe::NotProbed) => Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!("locked system-RPM probe for '{package}' did not run"),
        }),
        Err(ProviderError::Query(PackageQueryError::CommandMissing { command: bin })) => {
            Err(rpm_tooling_missing_error(command, &bin, target))
        }
        Err(err) => Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!("locked rpm query failed for '{package}': {err}"),
        }),
    }
}

pub(crate) fn configured_rpm_repo_source(
    repo_config: &RepoConfig,
    env: &anolisa_env::EnvFacts,
) -> Result<Option<DnfRepoSource>, CliError> {
    let Some(backend) = repo_config.backends.get("rpm") else {
        return Ok(None);
    };
    let host = HostVars {
        os: env.os.clone(),
        arch: env.arch.clone(),
    };
    let base_url = repo_config
        .resolved_base_url("rpm", backend, &host)
        .map_err(|err| repo_config_err(err, true))?;
    Ok(Some(DnfRepoSource::new(
        ANOLISA_RPM_REPO_ID,
        base_url,
        backend.gpgcheck,
    )))
}

pub(crate) fn require_configured_rpm_backend(
    repo_config: &RepoConfig,
    command: &str,
) -> Result<(), CliError> {
    if repo_config.backends.contains_key("rpm") {
        Ok(())
    } else {
        Err(repo_config_err(
            RepoConfigError::BackendNotConfigured {
                name: "rpm".to_string(),
            },
            true,
        )
        .with_command(command))
    }
}

/// Resolve the raw package while preserving a literal lifecycle identity.
///
/// `component_identity_pinned` prevents the backend-specific alias pass from
/// changing the component selected by lifecycle resolution. Pinning applies
/// both to exact visible identities and to incomplete cross-scope visibility;
/// explicit package overrides and package maps may still choose the
/// distribution package.
pub(crate) fn resolve_raw_identity(
    layout: &FsLayout,
    env: &anolisa_env::EnvFacts,
    repo_config: &RepoConfig,
    backend: &BackendConfig,
    component: String,
    cli_override: Option<&str>,
    component_identity_pinned: bool,
) -> (String, String) {
    if cli_override.is_some() || backend.package_map.contains_key(&component) {
        let package = repo_config.package_name(backend, &component, cli_override);
        return (component, package);
    }

    let component_index = load_optional_component_index(layout, env, repo_config);
    let resolver = ComponentResolver::new(component_index.as_ref(), None, None);
    match resolver.resolve(
        &component,
        BackendKind::Raw,
        ResolutionUse::Install,
        ResolveOptions::default(),
    ) {
        Ok(ResolutionSet::Unique(target))
            if !component_identity_pinned || target.component == component =>
        {
            (target.component, target.package)
        }
        _ => {
            let package = repo_config.package_name(backend, &component, cli_override);
            (component, package)
        }
    }
}

/// True when the store holds a quarantined record for this component.
pub(crate) fn quarantined(store: &StateStore, component: &str) -> bool {
    store
        .quarantined
        .iter()
        .any(|q| q.record.kind == ObjectKind::Component && q.record.name == component)
}

/// Map an owned-executor failure to a CLI error that reports honestly what
/// happened to the host: cleanly unwound, partially unwound, or untouched.
fn owned_error_to_cli(
    err: OwnedExecutionError,
    target: &str,
    scope: InstallationScope,
    command: &str,
    retained_note: &str,
) -> CliError {
    let repair = common::scoped_component_command(scope, "repair", target);
    let reason = match err {
        OwnedExecutionError::StepFailed {
            step,
            source,
            rolled_back,
            rollback_warnings,
            ..
        } => {
            let at = step_label(&step);
            if !rolled_back {
                format!(
                    "install of '{target}' failed at '{at}': {source}; the host was not changed{retained_note}"
                )
            } else if rollback_warnings.is_empty() {
                format!(
                    "install of '{target}' failed at '{at}': {source}; this run's changes were undone{retained_note}"
                )
            } else {
                format!(
                    "install of '{target}' failed at '{at}': {source}; undoing this run's changes reported problems ({}) — run `{repair}`{retained_note}",
                    rollback_warnings.join("; ")
                )
            }
        }
        OwnedExecutionError::RecoveryUncertain { detail, .. } => {
            format!("install of '{target}' failed: {detail}; run `{repair}`{retained_note}")
        }
        other => format!("install of '{target}' failed: {other}{retained_note}"),
    };
    CliError::Runtime {
        command: command.to_string(),
        reason,
    }
}

/// Actionable "rpm/dnf tooling missing" error. The system-RPM presence check
/// needs the native tooling; installing without it could place raw files
/// over an unobserved system RPM.
fn rpm_tooling_missing_error(command: &str, bin: &str, target: &str) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: format!(
            "cannot install '{target}': {bin} not found on PATH — the system-RPM presence check needs rpm/dnf; install rpm/dnf and retry"
        ),
    }
}

fn pkg_query_err(err: PackageQueryError, command: &str) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: format!("rpm query failed: {err}"),
    }
}

/// Map a planning refusal to an actionable CLI error. The planner names the
/// way out; this mapping only renders it.
fn plan_error_to_cli(err: PlanError, target: &str, command: &str) -> CliError {
    let command = command.to_string();
    match err {
        PlanError::AlreadyPresentOnSystem => CliError::InvalidArgument {
            command,
            reason: format!(
                "'{target}' is already installed as a system RPM that ANOLISA does not manage; run `anolisa adopt {target}` to start tracking it"
            ),
        },
        PlanError::UseUpdate => CliError::InvalidArgument {
            command,
            reason: format!(
                "component '{target}' is already installed at a different version; run `anolisa update {target} --version <version>` to change versions"
            ),
        },
        PlanError::AlreadyManaged => CliError::InvalidArgument {
            command,
            reason: format!(
                "component '{target}' is already managed through the native package manager; run `anolisa update {target}` to move versions"
            ),
        },
        PlanError::ExternallyRemoved => CliError::InvalidArgument {
            command,
            reason: format!(
                "the package backing '{target}' was removed outside ANOLISA; run `anolisa repair {target}` to reconcile or `anolisa forget {target}` to drop the record"
            ),
        },
        PlanError::TrackedButAbsent => CliError::InvalidArgument {
            command,
            reason: format!(
                "'{target}' is tracked but its package is no longer installed; run `anolisa forget {target}` to drop the record, then install again"
            ),
        },
        PlanError::NeedsAttention => CliError::InvalidArgument {
            command,
            reason: format!(
                "the record for '{target}' was quarantined by the state migration; run `anolisa repair {target}` to resolve it"
            ),
        },
        PlanError::ProvenanceConflict => CliError::InvalidArgument {
            command,
            reason: format!(
                "the requested backend/package conflicts with the recorded provenance of '{target}'; uninstall it first or re-run without the conflicting override"
            ),
        },
        PlanError::DelegatedRequiresSystemScope => CliError::InvalidArgument {
            command,
            reason: format!(
                "installing '{target}' through the RPM backend requires system mode; re-run with sudo or use `--backend raw`"
            ),
        },
        PlanError::PendingOperation => CliError::Runtime {
            command,
            reason: format!(
                "a previous operation on '{target}' is pending recovery; run `anolisa repair {target}` before retrying"
            ),
        },
        other => CliError::InvalidArgument {
            command,
            reason: format!("cannot install '{target}': {other:?}"),
        },
    }
}

/// Human-facing label for a plan step (preview rendering).
pub(crate) fn step_label(step: &Step) -> String {
    match step {
        Step::NativeTransaction {
            action, packages, ..
        } => format!("dnf {} {}", action.verb(), packages.join(" ")),
        Step::Observe { packages } => format!("observe {}", packages.join(" ")),
        Step::WriteRecord(write) => format!("record: {}", write.label()),
        Step::DropRecord => "record: drop".to_string(),
        Step::DownloadVerify => "download and verify artifact".to_string(),
        Step::ProvisionRuntimeDeps => "provision runtime dependencies".to_string(),
        Step::RunHook(kind) => format!(
            "run {} hooks",
            match kind {
                HookKind::PreInstall => "pre-install",
                HookKind::PostInstall => "post-install",
                HookKind::PreUninstall => "pre-uninstall",
                HookKind::PostUninstall => "post-uninstall",
            }
        ),
        Step::BackupFiles => "back up current files".to_string(),
        Step::PlaceFiles => "place files".to_string(),
        Step::SetCapabilities => "apply file capabilities".to_string(),
        Step::EnableServices => "enable services".to_string(),
        Step::RestartServices => "restart services".to_string(),
        Step::StopServices => "stop services".to_string(),
        Step::RemoveOwnedFiles => "remove owned files".to_string(),
        other => format!("{other:?}"),
    }
}

/// JSON payload for a completed (or previewed, or idempotent) install.
///
/// The pin fields (`requested_version`, `resolved_version`, `source_repo`,
/// `artifact`) are additive and only present for a version-pinned delegated
/// install; `version` keeps its existing meaning (the effective/installed
/// version) across every route, so the wire contract stays backward
/// compatible.
#[derive(Debug, Serialize)]
struct InstallResultPayload {
    component: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    package: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    backend: String,
    /// `installed` | `planned` (dry-run) | `already-installed`.
    action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    /// `--version` value the caller requested (version-pinned installs only).
    #[serde(skip_serializing_if = "Option::is_none")]
    requested_version: Option<String>,
    /// Full resolved EVR of the pinned candidate (version-pinned only).
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_version: Option<String>,
    /// Source repository the pinned candidate came from (version-pinned only).
    #[serde(skip_serializing_if = "Option::is_none")]
    source_repo: Option<String>,
    /// Exact NEVRA handed to dnf (version-pinned only).
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact: Option<String>,
    dry_run: bool,
    plan: Vec<String>,
}

impl InstallResultPayload {
    /// Copy the resolved-candidate fields from a delegated version pin. The
    /// pin's resolved EVR becomes `resolved_version`; `version` is set to the
    /// upstream version so the existing field stays a clear, compatible
    /// answer for pinned installs too.
    fn with_pin(mut self, pin: &DelegatedPin) -> Self {
        self.requested_version = Some(pin.requested_version.clone());
        self.resolved_version = Some(pin.resolved_evr.clone());
        self.source_repo = pin.source_repo.clone();
        self.artifact = Some(pin.artifact.clone());
        if self.version.is_none() {
            self.version = Some(pin.resolved_version.clone());
        }
        self
    }
}

/// Detail lines shown above the plan in a dry-run preview.
///
/// For a version-pinned delegated install this makes the resolved candidate
/// explicit — bare package, requested version, resolved EVR, exact artifact,
/// and source repository — so the preview proves what would be installed
/// rather than echoing the request. Unpinned/owned installs contribute no pin
/// fields and yield an empty list (the plan alone is shown).
fn dry_run_detail_lines(payload: &InstallResultPayload) -> Vec<String> {
    let mut lines = Vec::new();
    // Only a version pin populates these; guard on `artifact` so unpinned and
    // owned dry-runs render exactly as before (plan only).
    if payload.artifact.is_some() {
        if let Some(package) = &payload.package {
            lines.push(format!("package: {package}"));
        }
        if let Some(requested) = &payload.requested_version {
            lines.push(format!("requested version: {requested}"));
        }
        if let Some(resolved) = &payload.resolved_version {
            lines.push(format!("resolved version: {resolved}"));
        }
        if let Some(artifact) = &payload.artifact {
            lines.push(format!("artifact: {artifact}"));
        }
        if let Some(repo) = &payload.source_repo {
            lines.push(format!("repository: {repo}"));
        }
    }
    lines
}

fn render_result(ctx: &CliContext, payload: &InstallResultPayload) -> Result<(), CliError> {
    if ctx.json {
        return render_json(COMMAND, payload);
    }
    if ctx.quiet {
        return Ok(());
    }
    if payload.dry_run {
        println!("install {} (dry-run):", payload.component);
        for line in dry_run_detail_lines(payload) {
            println!("  {line}");
        }
        for label in &payload.plan {
            println!("  - {label}");
        }
        return Ok(());
    }
    match (payload.action, &payload.version) {
        ("already-installed", Some(version)) => {
            println!("{} {version} is already installed", payload.component);
        }
        ("already-installed", None) => println!("{} is already installed", payload.component),
        (_, Some(version)) => println!("installed {} {version}", payload.component),
        (_, None) => println!("installed {}", payload.component),
    }
    Ok(())
}

fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::super::handle;
    use super::super::tests::*;
    use super::*;
    use crate::repo_config::RepoConfig;
    use anolisa_platform::fs_layout::FsLayout;
    use tempfile::tempdir;

    #[test]
    fn pinned_payload_json_exposes_requested_resolved_repo_and_artifact() {
        // The JSON envelope for a version-pinned install must carry the
        // requested/resolved version, the source repo, the exact artifact, and
        // the bare package — while keeping `version` as a compatible answer.
        let pin = DelegatedPin {
            requested_version: "0.6.2".to_string(),
            resolved_version: "0.6.2".to_string(),
            resolved_evr: "0.6.2-1.alnx4".to_string(),
            resolved_arch: "x86_64".to_string(),
            artifact: "agentsight-0.6.2-1.alnx4.x86_64".to_string(),
            source_repo: Some("anolisa-configured".to_string()),
        };
        let payload = InstallResultPayload {
            component: "agentsight".to_string(),
            package: Some("agentsight".to_string()),
            version: None,
            backend: "rpm".to_string(),
            action: "planned",
            operation_id: None,
            requested_version: None,
            resolved_version: None,
            source_repo: None,
            artifact: None,
            dry_run: true,
            plan: vec!["dnf install agentsight-0.6.2-1.alnx4.x86_64".to_string()],
        }
        .with_pin(&pin);

        let json = serde_json::to_value(&payload).expect("serialize payload");
        assert_eq!(json["component"], "agentsight");
        assert_eq!(json["package"], "agentsight");
        assert_eq!(json["requested_version"], "0.6.2");
        assert_eq!(json["resolved_version"], "0.6.2-1.alnx4");
        assert_eq!(json["source_repo"], "anolisa-configured");
        assert_eq!(json["artifact"], "agentsight-0.6.2-1.alnx4.x86_64");
        // `version` stays present as the upstream version (compatible field).
        assert_eq!(json["version"], "0.6.2");
    }

    #[test]
    fn pinned_dry_run_detail_lines_show_package_resolved_version_and_artifact() {
        // The human dry-run contract: bare package, requested version, resolved
        // EVR, exact artifact, and source repository, each on its own line.
        let pin = DelegatedPin {
            requested_version: "0.6.2".to_string(),
            resolved_version: "0.6.2".to_string(),
            resolved_evr: "0.6.2-1.alnx4".to_string(),
            resolved_arch: "x86_64".to_string(),
            artifact: "agentsight-0.6.2-1.alnx4.x86_64".to_string(),
            source_repo: Some("anolisa-configured".to_string()),
        };
        let payload = InstallResultPayload {
            component: "agentsight".to_string(),
            package: Some("agentsight".to_string()),
            version: None,
            backend: "rpm".to_string(),
            action: "planned",
            operation_id: None,
            requested_version: None,
            resolved_version: None,
            source_repo: None,
            artifact: None,
            dry_run: true,
            plan: Vec::new(),
        }
        .with_pin(&pin);

        assert_eq!(
            dry_run_detail_lines(&payload),
            vec![
                "package: agentsight".to_string(),
                "requested version: 0.6.2".to_string(),
                "resolved version: 0.6.2-1.alnx4".to_string(),
                "artifact: agentsight-0.6.2-1.alnx4.x86_64".to_string(),
                "repository: anolisa-configured".to_string(),
            ]
        );
    }

    #[test]
    fn unpinned_dry_run_detail_lines_are_empty() {
        // No pin → no detail lines; the unpinned/owned preview is unchanged.
        let payload = InstallResultPayload {
            component: "agentsight".to_string(),
            package: Some("agentsight".to_string()),
            version: Some("0.6.2".to_string()),
            backend: "rpm".to_string(),
            action: "planned",
            operation_id: None,
            requested_version: None,
            resolved_version: None,
            source_repo: None,
            artifact: None,
            dry_run: true,
            plan: vec!["dnf install agentsight".to_string()],
        };
        assert!(dry_run_detail_lines(&payload).is_empty());
    }

    #[test]
    fn unpinned_payload_json_omits_pin_fields() {
        // Without a pin the additive fields must not appear, preserving the
        // pre-existing wire contract for plain installs.
        let payload = InstallResultPayload {
            component: "agentsight".to_string(),
            package: Some("agentsight".to_string()),
            version: Some("0.6.2".to_string()),
            backend: "rpm".to_string(),
            action: "installed",
            operation_id: None,
            requested_version: None,
            resolved_version: None,
            source_repo: None,
            artifact: None,
            dry_run: false,
            plan: Vec::new(),
        };
        let json = serde_json::to_value(&payload).expect("serialize payload");
        assert!(json.get("requested_version").is_none());
        assert!(json.get("resolved_version").is_none());
        assert!(json.get("source_repo").is_none());
        assert!(json.get("artifact").is_none());
    }

    #[test]
    fn raw_resolution_does_not_rewrite_exact_state_identity() {
        let tmp = tempdir().expect("tempdir");
        let repo_root = tmp.path().join("repo");
        let repo_v1 = repo_root.join("v1");
        std::fs::create_dir_all(&repo_v1).expect("repo dir");
        std::fs::write(
            repo_v1.join("components.toml"),
            r#"
schema_version = 1

[[components]]
name = "cosh"

[[components.backends]]
kind = "raw"
package = "cosh"

[[components.aliases]]
kind = "raw-package"
name = "legacy-name"

[[components]]
name = "sec-core"

[[components.backends]]
kind = "raw"
package = "agent-sec-core"
"#,
        )
        .expect("component index");
        let repo_config = RepoConfig::from_toml_str(&format!(
            "schema_version = 1\ndefault_backend = \"raw\"\n[backends.raw]\nbase_url = \"file://{}\"\n",
            repo_root.display()
        ))
        .expect("repo config");
        let backend = repo_config.backends.get("raw").expect("raw backend");
        let layout = FsLayout::system(Some(tmp.path().join("root")));
        let env = anolisa_env::EnvService::detect();

        let exact = resolve_raw_identity(
            &layout,
            &env,
            &repo_config,
            backend,
            "legacy-name".to_string(),
            None,
            true,
        );
        let alias = resolve_raw_identity(
            &layout,
            &env,
            &repo_config,
            backend,
            "legacy-name".to_string(),
            None,
            false,
        );
        let exact_with_mapped_package = resolve_raw_identity(
            &layout,
            &env,
            &repo_config,
            backend,
            "sec-core".to_string(),
            None,
            true,
        );

        assert_eq!(
            exact,
            ("legacy-name".to_string(), "legacy-name".to_string())
        );
        assert_eq!(alias, ("cosh".to_string(), "cosh".to_string()));
        assert_eq!(
            exact_with_mapped_package,
            ("sec-core".to_string(), "agent-sec-core".to_string())
        );
    }

    #[test]
    fn install_unknown_component_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let mut a = args("no-such-component");
        a.repo = Some(write_empty_repo(&tmp.path().join("repo")));

        let err =
            handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix))).expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("no-such-component"));
    }

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

        let err =
            handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix))).expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("install mode is not supported"),
            "got: {}",
            err.reason()
        );
    }

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

        let err =
            handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix))).expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason()
                .contains("inconsistent with the distribution index")
                && err.reason().contains("system-mode support"),
            "got: {}",
            err.reason()
        );
    }

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

    #[test]
    fn install_invalid_repo_override_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let mut a = args("agentsight");
        a.repo = Some("ftp://example.com/repo".to_string());
        let err = handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(tmp.path().to_path_buf())))
            .expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("ftp"), "got: {}", err.reason());
    }

    #[test]
    fn configured_rpm_repo_source_uses_repo_toml_backend() {
        let repo = RepoConfig::from_toml_str(
            r#"schema_version = 1
default_backend = "rpm"
[vars]
releasever = "4"
[backends.rpm]
base_url = "http://repo.example/alinux/$releasever/agentic-os/$basearch/os/"
insecure = true
gpgcheck = false
"#,
        )
        .expect("parse repo");
        let source = configured_rpm_repo_source(&repo, &linux_env())
            .expect("resolve rpm repo")
            .expect("rpm repo exists");
        assert_eq!(source.id(), ANOLISA_RPM_REPO_ID);
        assert_eq!(
            source.base_url(),
            "http://repo.example/alinux/4/agentic-os/x86_64/os"
        );
        assert_eq!(source.gpgcheck(), Some(false));
    }

    #[test]
    fn install_family_follows_flag_then_record_then_default() {
        use anolisa_core::domain::{
            Installation, LifecycleStatus, ManagementRelation, PackageIdentity,
        };

        let repo = RepoConfig::from_toml_str(
            r#"schema_version = 1
default_backend = "raw"
[backends.raw]
base_url = "https://example.com/anolisa"
[backends.rpm]
base_url = "https://repo.example/anolisa"
"#,
        )
        .expect("parse repo");

        // Explicit --backend wins, canonicalized.
        let mut a = args("cosh");
        a.backend = Some("rpm".to_string());
        let store = StateStore::empty();
        assert_eq!(install_family(&a, &store, "cosh", &repo), "rpm");

        // Recorded provenance is sticky: a delegated record routes to rpm
        // even though the default backend is raw.
        let a = args("cosh");
        let mut store = StateStore::empty();
        store.upsert(Installation {
            kind: ObjectKind::Component,
            name: "cosh".to_string(),
            scope: InstallationScope::System,
            binding: ProviderBinding::Delegated {
                pm: NativePm::Rpm,
                package: PackageIdentity::Resolved {
                    name: "copilot-shell".to_string(),
                },
                relation: ManagementRelation::Managed {
                    since: "2026-07-01T00:00:00Z".to_string(),
                },
                last_observed: None,
            },
            status: LifecycleStatus::Installed,
            installed_at: "2026-07-01T00:00:00Z".to_string(),
            last_operation_id: None,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            health: Vec::new(),
        });
        assert_eq!(install_family(&a, &store, "cosh", &repo), "rpm");

        // No flag, no record: the default backend decides.
        let store = StateStore::empty();
        assert_eq!(install_family(&a, &store, "cosh", &repo), "raw");
    }
}
