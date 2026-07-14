//! RPM backend paths for `install`: adopt (#958) and delegated install (#959).

use serde::Serialize;

use anolisa_core::central_log::{CentralLog, LogKind, LogRecord, LogStatus, Severity};
use anolisa_core::lock::InstallLock;
use anolisa_core::state::{
    InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectKind, ObjectStatus,
    OperationRecord, Ownership, RpmMetadata,
};
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::pkg_query::{PackageInfo, PackageQuery, PackageQueryError};
use anolisa_platform::pkg_transaction::{PackageTransaction, PackageTransactionError};
use chrono::Utc;

use crate::color::Palette;
use crate::commands::common;
use crate::commands::tier1::rpm_install;
use crate::context::CliContext;
use crate::repo_config::{BackendConfig, RepoConfig};
use crate::resolution::{
    BackendKind, ComponentIndex, ComponentResolver, ResolutionSet, ResolutionSource, ResolutionUse,
    ResolveOptions, ResolvedTarget, rpm_component_provide,
};
use crate::response::{CliError, render_json};

use super::InstallArgs;
use super::io_util::{now_iso8601, snapshot_datadir_contract};
use super::render::render_warnings;
use super::types::*;

// Re-definitions that live in the parent module but are accessed by RPM code.
// These come from dispatch.rs (not yet extracted) via the parent's glob
// re-export, so `super::*` covers them.
use super::{COMMAND, ensure_component_backend_compatible, require_configured_rpm_backend};

pub(crate) struct RpmExec<'a> {
    pub(crate) query: &'a dyn PackageQuery,
    pub(crate) txn: &'a dyn PackageTransaction,
    pub(crate) is_root: bool,
}

impl<'a> RpmExec<'a> {
    pub(crate) fn new(
        query: &'a dyn PackageQuery,
        txn: &'a dyn PackageTransaction,
        is_root: bool,
    ) -> Self {
        Self {
            query,
            txn,
            is_root,
        }
    }
}
// ── rpm adopt path (#958) ───────────────────────────────────────────

/// Resolved RPM component/package pair.
#[derive(Debug, Clone)]
pub(crate) struct RpmTarget {
    pub(crate) component: String,
    pub(crate) package: String,
    source: ResolutionSource,
    legacy_adopt: bool,
}

impl RpmTarget {
    pub(crate) fn new(component: impl Into<String>, package: impl Into<String>) -> Self {
        Self {
            component: component.into(),
            package: package.into(),
            source: ResolutionSource::InstalledRpmProvides,
            legacy_adopt: true,
        }
    }

    pub(crate) fn from_resolved(target: ResolvedTarget) -> Self {
        Self {
            component: target.component,
            package: target.package,
            source: target.source,
            legacy_adopt: target.legacy_adopt,
        }
    }

    fn from_installed_state(component: &str, package: &str) -> Self {
        Self {
            component: component.to_string(),
            package: package.to_string(),
            source: ResolutionSource::InstalledState,
            legacy_adopt: false,
        }
    }

    pub(crate) fn label(&self) -> String {
        if self.component == self.package {
            self.package.clone()
        } else {
            format!("{} -> {}", self.component, self.package)
        }
    }
}

impl PartialEq for RpmTarget {
    fn eq(&self, other: &Self) -> bool {
        self.component == other.component && self.package == other.package
    }
}

/// Result of probing whether a target is present as a system RPM (§5/§7.1).
pub(crate) enum RpmSituation {
    /// Exactly one candidate package name, installed once — ready to adopt.
    Adoptable {
        /// Resolved component/package identity.
        target: RpmTarget,
        /// rpmdb query result carrying EVR/arch for the state record.
        info: PackageInfo,
    },
    /// Not present as a system RPM: the single candidate is not installed
    /// (rpm tooling ran and returned nothing). Auto-detect falls through to the
    /// default backend; an explicit `--backend rpm` (or `default_backend =
    /// "rpm"`) delegates a `dnf install` of this package and records it as
    /// `rpm-managed`. A *missing* rpm/dnf binary is a different case —
    /// it is a hard warn-and-exit, not `Absent`.
    Absent {
        /// Resolved component/package identity to hand to `dnf install`.
        target: RpmTarget,
    },
    /// No ANOLISA component identity could be proven for the input.
    NotAnolisaComponent,
    /// `provides` reverse-lookup matched several distinct installed packages
    /// (§5.5). Reported, never silently adopted.
    Ambiguous(Vec<RpmTarget>),
    /// The candidate resolved but rpmdb holds several installed versions of it
    /// (`UnexpectedOutput`, §5.5) — a drift state, not a clean adopt target.
    MultiVersion(RpmTarget),
}

/// Resolve the candidate RPM component/package pair(s) and probe rpmdb.
///
/// Errors when a query hard-fails. A missing `rpm`/`dnf` binary is a
/// warn-and-exit ([`rpm_tooling_missing_error`]): the probe cannot prove the
/// component is *not* an unobserved system RPM, so we refuse to silently fall
/// back to raw rather than treat it as [`Absent`].
///
/// [`Absent`]: RpmSituation::Absent
pub(crate) fn probe_rpm_situation(
    component: &str,
    cli_override: Option<&str>,
    rpm_backend: Option<&BackendConfig>,
    component_index: Option<&ComponentIndex>,
    use_case: ResolutionUse,
    query: &dyn PackageQuery,
    command: &str,
) -> Result<RpmSituation, CliError> {
    let candidates = match rpm_package_candidates_with_index(
        cli_override,
        rpm_backend,
        component_index,
        query,
        component,
        use_case,
    ) {
        Ok(candidates) => candidates,
        // No rpm/dnf on this host: refuse to silently fall back to raw (§7.1).
        Err(PackageQueryError::CommandMissing { .. }) => {
            return Err(rpm_tooling_missing_error(command));
        }
        Err(err) => return Err(pkg_query_err(err, command)),
    };

    if candidates.is_empty() {
        return Ok(RpmSituation::NotAnolisaComponent);
    }
    if candidates.len() >= 2 {
        return Ok(RpmSituation::Ambiguous(candidates));
    }
    // Empty and ambiguous candidate sets were handled above, so exactly one
    // package remains here.
    let target = candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| RpmTarget::new(component, component));

    match query.query_installed(&target.package) {
        Ok(Some(info)) => {
            if rpm_installed_target_allowed(&target, query)
                .map_err(|err| pkg_query_err(err, command))?
            {
                Ok(RpmSituation::Adoptable { target, info })
            } else {
                Ok(RpmSituation::NotAnolisaComponent)
            }
        }
        Ok(None) => Ok(RpmSituation::Absent { target }),
        // Same name, several installed versions: a drift the caller reports.
        Err(PackageQueryError::UnexpectedOutput { .. }) => Ok(RpmSituation::MultiVersion(target)),
        // No rpm/dnf on this host: refuse to silently fall back to raw (§7.1).
        Err(PackageQueryError::CommandMissing { .. }) => Err(rpm_tooling_missing_error(command)),
        Err(err) => Err(pkg_query_err(err, command)),
    }
}

/// Resolve candidate RPM component/package pairs for `input`.
///
/// Precedence, in order: CLI `--package`, repo-side component index,
/// repo.toml `package_map`, installed/available
/// `anolisa-component(<name>)` providers, then the input package's own
/// `Provides: anolisa-component(<component>)` metadata.
///
/// Ordinary RPM packages without ANOLISA metadata return an empty vector:
/// `install --backend rpm <arg>` installs ANOLISA components, not arbitrary
/// `dnf install <arg>` targets.
///
/// # Errors
/// Propagates a hard [`PackageQueryError`] from the package query; empty
/// query results are the normal "no explicit component identity" branch.
#[cfg(test)]
pub(crate) fn rpm_package_candidates(
    cli_override: Option<&str>,
    rpm_backend: Option<&BackendConfig>,
    query: &dyn PackageQuery,
    input: &str,
) -> Result<Vec<RpmTarget>, PackageQueryError> {
    rpm_package_candidates_with_index(
        cli_override,
        rpm_backend,
        None,
        query,
        input,
        ResolutionUse::Install,
    )
}

pub(crate) fn rpm_package_candidates_with_index(
    cli_override: Option<&str>,
    rpm_backend: Option<&BackendConfig>,
    component_index: Option<&ComponentIndex>,
    query: &dyn PackageQuery,
    input: &str,
    use_case: ResolutionUse,
) -> Result<Vec<RpmTarget>, PackageQueryError> {
    let resolver = ComponentResolver::new(component_index, rpm_backend, Some(query));
    let resolved = resolver.resolve(
        input,
        BackendKind::Rpm,
        use_case,
        ResolveOptions {
            package_override: cli_override,
        },
    )?;
    Ok(match resolved {
        ResolutionSet::None => Vec::new(),
        ResolutionSet::Unique(target) => vec![RpmTarget::from_resolved(target)],
        ResolutionSet::Ambiguous(targets) => {
            targets.into_iter().map(RpmTarget::from_resolved).collect()
        }
    })
}

fn rpm_installed_target_allowed(
    target: &RpmTarget,
    query: &dyn PackageQuery,
) -> Result<bool, PackageQueryError> {
    if matches!(
        target.source,
        ResolutionSource::RepoPackageMap
            | ResolutionSource::InstalledState
            | ResolutionSource::InstalledRpmProvides
            | ResolutionSource::AvailableRpmProvides
    ) || target.legacy_adopt
    {
        return Ok(true);
    }
    let expected = rpm_component_provide(&target.component);
    Ok(query
        .provided_capabilities_installed(&target.package)?
        .iter()
        .any(|capability| rpm_capability_matches_component(capability, &expected)))
}

enum TrackedRpmSituation {
    ManagedAbsent(RpmTarget),
    ManagedPresent(RpmTarget),
    ObservedAbsent(RpmTarget),
    ObservedPresent {
        target: RpmTarget,
        info: PackageInfo,
    },
}

fn probe_tracked_rpm(
    state: &InstalledState,
    component: &str,
    package_override: Option<&str>,
    query: &dyn PackageQuery,
    command: &str,
) -> Result<Option<TrackedRpmSituation>, CliError> {
    let Some(existing) = state.find_object(ObjectKind::Component, component) else {
        return Ok(None);
    };
    let ownership = existing.effective_ownership();
    let observed = match ownership {
        Ownership::RpmManaged => false,
        Ownership::RpmObserved => true,
        Ownership::RawManaged => return Ok(None),
    };
    let Some(recorded_package) = existing
        .rpm_metadata
        .as_ref()
        .map(|metadata| metadata.package_name.trim())
        .filter(|package| !package.is_empty())
    else {
        return Ok(None);
    };

    if let Some(requested_package) = package_override {
        if observed {
            refuse_observed_repoint(state, component, requested_package, command)?;
        } else if requested_package != recorded_package {
            return Err(CliError::InvalidArgument {
                command: command.to_string(),
                reason: format!(
                    "component '{component}' is managed through RPM package '{recorded_package}', not '{requested_package}'; refusing to reinstall a different package"
                ),
            });
        }
    }

    let target = RpmTarget::from_installed_state(component, recorded_package);
    match query.query_installed(recorded_package) {
        Ok(Some(info)) if observed => {
            Ok(Some(TrackedRpmSituation::ObservedPresent { target, info }))
        }
        Ok(Some(_)) => Ok(Some(TrackedRpmSituation::ManagedPresent(target))),
        Ok(None) if observed => Ok(Some(TrackedRpmSituation::ObservedAbsent(target))),
        Ok(None) => Ok(Some(TrackedRpmSituation::ManagedAbsent(target))),
        Err(PackageQueryError::CommandMissing { .. }) => Err(rpm_tooling_missing_error(command)),
        Err(err) => Err(pkg_query_err(err, command)),
    }
}

pub(crate) fn rpm_capability_matches_component(capability: &str, expected: &str) -> bool {
    let capability = capability.trim();
    if capability == expected {
        return true;
    }
    capability
        .strip_prefix(expected)
        .is_some_and(|rest| rest.trim_start().starts_with('='))
}

/// Layer 2 for the `rpm` backend: reject in user mode, otherwise adopt an
/// installed package, delegate a `dnf install` for an absent one, or surface
/// the ambiguous / drift cases. `situation` is reused from layer 1's probe when
/// present (the `SystemRpm` source), and computed here otherwise
/// (`Explicit` rpm).
#[allow(clippy::too_many_arguments)]
pub(crate) fn route_rpm_adopt(
    component: &str,
    args: &InstallArgs,
    ctx: &CliContext,
    command: &str,
    layout: &FsLayout,
    repo_config: &RepoConfig,
    installed: &InstalledState,
    source: BackendSource,
    situation: Option<RpmSituation>,
    component_index: Option<&ComponentIndex>,
    exec: &RpmExec,
) -> Result<InstallOutcome, CliError> {
    common::require_system_mode(
        ctx,
        command,
        "--backend rpm adopts a system RPM and requires system scope",
        &format!("sudo anolisa install --backend rpm {component}"),
    )?;

    // Explicit `--backend rpm` may switch an already-installed component's
    // provenance; reuse the same guard the raw path uses.
    if source == BackendSource::Explicit {
        ensure_component_backend_compatible(installed, component, "rpm", command)?;
    }

    let situation = match situation {
        Some(s) => s,
        None => match probe_tracked_rpm(
            installed,
            component,
            args.package.as_deref(),
            exec.query,
            command,
        )? {
            Some(TrackedRpmSituation::ManagedAbsent(target)) => RpmSituation::Absent { target },
            Some(TrackedRpmSituation::ManagedPresent(target)) => {
                return Err(CliError::InvalidArgument {
                    command: command.to_string(),
                    reason: format!(
                        "component '{}' is already tracked as rpm-managed and RPM package '{}' is installed; use `anolisa status {}` to inspect it or `anolisa repair {}` to reconcile its metadata",
                        target.component, target.package, target.component, target.component
                    ),
                });
            }
            Some(TrackedRpmSituation::ObservedAbsent(target)) => {
                return Err(CliError::InvalidArgument {
                    command: command.to_string(),
                    reason: format!(
                        "component '{}' is rpm-observed and package '{}' is missing; ANOLISA does not own its installation — run `anolisa forget {}` first, then install it as rpm-managed",
                        target.component, target.package, target.component
                    ),
                });
            }
            Some(TrackedRpmSituation::ObservedPresent { target, info }) => {
                RpmSituation::Adoptable { target, info }
            }
            None => probe_rpm_situation(
                component,
                args.package.as_deref(),
                repo_config.backends.get("rpm"),
                component_index,
                ResolutionUse::Install,
                exec.query,
                command,
            )?,
        },
    };

    match situation {
        RpmSituation::Adoptable { target, info } => {
            if source == BackendSource::Explicit {
                ensure_component_backend_compatible(installed, &target.component, "rpm", command)?;
            }
            execute_adopt(
                ctx,
                layout,
                command,
                &target.component,
                target.package,
                info,
                exec.query,
            )
        }
        RpmSituation::Absent { target } => {
            require_configured_rpm_backend(repo_config, command)?;
            if source == BackendSource::Explicit {
                ensure_component_backend_compatible(installed, &target.component, "rpm", command)?;
            }
            let expectation = DelegatedInstallExpectation::capture(installed, &target.component);
            execute_delegated_install(exec, ctx, layout, command, &target.package, expectation)
        }
        RpmSituation::NotAnolisaComponent => Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "component '{component}' is not an ANOLISA RPM component; use the ANOLISA component name and configure the repo-side component index or publish Provides: anolisa-component({component})"
            ),
        }),
        RpmSituation::Ambiguous(targets) => Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "multiple RPM candidates match '{component}': {}; cannot resolve unambiguously — pin one with `--package <name>` or fix the component index / package metadata",
                targets
                    .iter()
                    .map(RpmTarget::label)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }),
        RpmSituation::MultiVersion(target) => Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "RPM package '{}' has multiple installed versions; refusing to adopt a single version automatically — resolve the duplicate first",
                target.package
            ),
        }),
    }
}

/// Wire shape for an adopt result (`--json`) and its dry-run preview.
#[derive(Serialize)]
pub(crate) struct AdoptResultPayload {
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

/// Refuse re-adopting an existing `rpm-observed` component under a *different*
/// RPM package — a package-identity migration, not an idempotent refresh.
///
/// Shared by the dry-run preview (non-locked) and the locked write path so the
/// preview can never promise an adopt the real run would reject. Returns
/// `Ok(())` when there is no existing record, it is not rpm-observed, its
/// recorded package name is empty, or the package is unchanged.
fn refuse_observed_repoint(
    state: &InstalledState,
    component: &str,
    new_package: &str,
    command: &str,
) -> Result<(), CliError> {
    let Some(existing) = state.find_object(ObjectKind::Component, component) else {
        return Ok(());
    };
    if !matches!(existing.effective_ownership(), Ownership::RpmObserved) {
        return Ok(());
    }
    if let Some(prev) = existing
        .rpm_metadata
        .as_ref()
        .map(|m| m.package_name.trim())
        && !prev.is_empty()
        && prev != new_package
    {
        return Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "component '{component}' is already adopted from RPM package '{prev}', not '{new_package}'; adopt will not silently repoint it to a different package — run `anolisa forget {component}` first, then adopt the new package"
            ),
        });
    }
    Ok(())
}

/// Record an installed system RPM as `rpm-observed` state (§7.2). Fetches
/// nothing, writes no owned files, touches no RPM-owned paths — only rpmdb
/// reads plus a state write. On `--dry-run` it renders the plan without
/// writing.
pub(crate) fn execute_adopt(
    ctx: &CliContext,
    layout: &FsLayout,
    command: &str,
    component: &str,
    package: String,
    info: PackageInfo,
    query: &dyn PackageQuery,
) -> Result<InstallOutcome, CliError> {
    let mut warnings: Vec<String> = Vec::new();
    let preview_state =
        common::load_installed_state(ctx, command).map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!("failed to load installed state: {err}"),
        })?;
    rpm_install::reject_pending_claim(
        layout,
        &preview_state,
        &[component, package.as_str()],
        command,
    )?;
    refuse_observed_repoint(&preview_state, component, &info.name, command)?;

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

    // Package-identity guard, evaluated *before* the dry-run return so the preview
    // never promises an adopt the real run would reject. A re-adopt of an existing
    // rpm-observed component must target the same RPM; `--package` pointing at a
    // different one is a migration, not a refresh. This non-locked read is the
    // preview / pre-lock fast-fail; the locked path below re-checks for TOCTOU.
    if ctx.dry_run {
        render_adopt(ctx, command, &payload);
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
    rpm_install::reject_pending_claim(layout, &state, &[component, package.as_str()], command)?;

    // Re-validate against the freshly-reloaded state, mirroring execute_raw's
    // post-lock guard. Layer 1 may have decided "adopt" from a pre-lock read
    // where the component was absent, but a concurrent raw install can win the
    // lock and record it first. Without this check the adopt would clobber the
    // raw provenance; with it, the loser is rejected rather than overwriting.
    ensure_component_backend_compatible(&state, component, "rpm", command)?;

    // Backend compatibility is necessary but not sufficient: rpm-managed and
    // rpm-observed share the "rpm" backend label, so the check above passes for
    // a component ANOLISA actively manages. Adopt may only create a new record
    // or refresh an existing rpm-observed one — it must never downgrade a managed
    // component to observed and silently drop ANOLISA's removal authority
    // (`owns_removal`). `adopt`'s pre-lock gate refuses this for the common case;
    // re-checking here under the lock closes the window where a concurrent
    // managed install lands between that read and this acquisition.
    if let Some(existing) = state.find_object(ObjectKind::Component, component)
        && !matches!(existing.effective_ownership(), Ownership::RpmObserved)
    {
        return Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "component '{component}' is already tracked as {} and will not be downgraded to rpm-observed; run `anolisa repair {component}` to refresh a managed RPM component, or `anolisa uninstall {component}` first",
                existing.effective_ownership().label()
            ),
        });
    }
    // Re-check the package-identity guard under the lock (TOCTOU): a concurrent
    // re-adopt could have repointed the recorded package between the pre-lock
    // preview above and this acquisition.
    refuse_observed_repoint(&state, component, &info.name, command)?;

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
        raw_package: None,
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
        provisioned_packages: Vec::new(),
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

    // Best-effort: snapshot the datadir component contract so adapter commands
    // can discover declared adapters. Missing or unwritable contracts produce
    // warnings, never failures.
    let snapshot_warnings = snapshot_datadir_contract(layout, component, command);
    warnings.extend(snapshot_warnings);

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
    payload.warnings = warnings;
    render_adopt(ctx, command, &payload);
    Ok(InstallOutcome::Adopted)
}

/// Render an adopt result (JSON envelope or the proposal §6.1 human text).
/// Silent in quiet mode; the `--all` batch path drives its own summary.
/// Bare verb for an adopt JSON envelope. `command` is the rich
/// `"<verb> <component>"` form, so the envelope takes its first token (matching
/// repair/forget's bare-verb envelopes). Because `execute_adopt` is shared, this
/// is "install" through the install trunk and "adopt" through the explicit
/// command — not a hardcoded "install".
pub(crate) fn adopt_envelope_verb(command: &str) -> &str {
    match command.split(' ').next() {
        Some(verb) if !verb.is_empty() => verb,
        _ => COMMAND,
    }
}

fn render_adopt(ctx: &CliContext, command: &str, payload: &AdoptResultPayload) {
    if ctx.json {
        // Errors here are unreachable for a plain Serialize struct; ignore the
        // Result so the (already-persisted) adopt is not reported as failed.
        let _ = render_json(adopt_envelope_verb(command), payload);
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
    // Dry-run records nothing, so the action line must read as conditional —
    // "Adopted" here would contradict the "nothing recorded" suffix above.
    let action_line = if payload.dry_run {
        "Would adopt as rpm-observed. ANOLISA will not replace it with raw."
    } else {
        "Adopted as rpm-observed. ANOLISA will not replace it with raw."
    };
    println!("{}", color.ok(action_line));
    render_warnings(&payload.warnings, &color);
}

// ── rpm delegated install path (#959) ───────────────────────────────

/// Wire shape for a delegated `dnf install` result (`--json`) and its dry-run
/// preview.
#[derive(Serialize)]
pub(crate) struct DelegatedInstallPayload {
    component: String,
    package: String,
    /// Always `rpm`: delegated install routes through the rpm backend.
    backend: &'static str,
    /// Always `rpm-managed`: ANOLISA drove the install and owns the removal.
    ownership: &'static str,
    install_mode: String,
    /// EVR recorded after install (rpmdb truth); `None` on dry-run.
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    arch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_repo: Option<String>,
    /// Repo candidate EVRs surfaced in the dry-run preview (best-effort).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    available_candidates: Vec<String>,
    /// `None` on dry-run (nothing recorded).
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    dry_run: bool,
    warnings: Vec<String>,
}

/// State authority used when an absent RPM package is installed through dnf.
#[derive(Debug, Clone, Copy)]
enum DelegatedInstallDisposition {
    /// No ANOLISA object exists; installation creates a new managed record.
    Fresh,
    /// A managed object exists but its RPM is missing; preserve and refresh it.
    ReinstallManaged,
}

/// Canonical component identity and object snapshot selected by RPM routing.
///
/// Capturing both together prevents callers from pairing a package alias with
/// an object from a different component. The locked execution path compares
/// the complete object because any concurrent metadata write invalidates the
/// route decision; the command fails before dnf instead of guessing whether a
/// changed field is benign.
#[derive(Debug, Clone)]
pub(crate) struct DelegatedInstallExpectation {
    component: String,
    object: Option<InstalledObject>,
}

impl DelegatedInstallExpectation {
    /// Captures the canonical component from the state snapshot used to route
    /// the delegated install.
    pub(crate) fn capture(state: &InstalledState, component: &str) -> Self {
        Self {
            component: component.to_string(),
            object: state.find_object(ObjectKind::Component, component).cloned(),
        }
    }

    fn verify_locked(&self, state: &InstalledState, command: &str) -> Result<(), CliError> {
        if state.find_object(ObjectKind::Component, &self.component) == self.object.as_ref() {
            return Ok(());
        }
        Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "component '{}' changed while this RPM install was being prepared; dnf was not run — inspect it with `anolisa status {}` and retry",
                self.component, self.component
            ),
        })
    }
}

fn classify_delegated_install(
    existing: Option<&InstalledObject>,
    component: &str,
    package: &str,
    command: &str,
) -> Result<DelegatedInstallDisposition, CliError> {
    let Some(existing) = existing else {
        return Ok(DelegatedInstallDisposition::Fresh);
    };
    match existing.effective_ownership() {
        Ownership::RpmManaged => {
            if let Some(recorded_package) = existing
                .rpm_metadata
                .as_ref()
                .map(|metadata| metadata.package_name.trim())
                && !recorded_package.is_empty()
                && recorded_package != package
            {
                return Err(CliError::InvalidArgument {
                    command: command.to_string(),
                    reason: format!(
                        "component '{component}' is managed through RPM package '{recorded_package}', not '{package}'; refusing to reinstall a different package"
                    ),
                });
            }
            Ok(DelegatedInstallDisposition::ReinstallManaged)
        }
        Ownership::RpmObserved => Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "component '{component}' is rpm-observed and package '{package}' is missing; ANOLISA does not own its installation — run `anolisa forget {component}` first, then install it as rpm-managed"
            ),
        }),
        ownership => Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "component '{component}' is tracked as {} and cannot be reinstalled through RPM",
                ownership.label()
            ),
        }),
    }
}

/// Install a missing RPM by delegating to dnf, then create or refresh its
/// ANOLISA-managed state.
///
/// This is the write-side mirror of [`execute_adopt`]: where adopt only
/// observes an already-installed package, delegated install drives the package
/// manager to place it and records ANOLISA ownership of the removal
/// (`owns_removal=true`). ANOLISA never fetches bytes itself — dnf owns the
/// file transaction. Gated on root for the real run; `--dry-run` previews the
/// `dnf install` without touching the host.
pub(crate) fn execute_delegated_install(
    exec: &RpmExec,
    ctx: &CliContext,
    layout: &FsLayout,
    command: &str,
    package: &str,
    expectation: DelegatedInstallExpectation,
) -> Result<InstallOutcome, CliError> {
    let component = expectation.component.as_str();
    let mut warnings: Vec<String> = Vec::new();
    let disposition =
        classify_delegated_install(expectation.object.as_ref(), component, package, command)?;

    // Dry-run: preview the dnf transaction with best-effort repo candidates.
    // Never needs root, never writes state.
    if ctx.dry_run {
        let preview_state = common::load_installed_state(ctx, command)?;
        rpm_install::reject_pending_claim(layout, &preview_state, &[component, package], command)?;
        let candidates = match exec.query.query_available(package) {
            Ok(infos) => {
                let mut evrs: Vec<String> =
                    infos.into_iter().map(|i| i.version.to_string()).collect();
                // Display list, not a version ranking — rpmvercmp is dnf's job.
                evrs.sort();
                evrs.dedup();
                evrs
            }
            Err(err) => {
                warnings.push(format!(
                    "could not query available versions for '{package}': {err}; dnf will still resolve candidates at install time"
                ));
                Vec::new()
            }
        };
        let payload = DelegatedInstallPayload {
            component: component.to_string(),
            package: package.to_string(),
            backend: "rpm",
            ownership: "rpm-managed",
            install_mode: ctx.install_mode.as_str().to_string(),
            version: None,
            arch: None,
            source_repo: None,
            available_candidates: candidates,
            operation_id: None,
            dry_run: true,
            warnings,
        };
        render_delegated_install(ctx, &payload);
        return Ok(InstallOutcome::Installed);
    }

    // Privilege gate: dnf transactions need root. Check up front so the user
    // gets an actionable message instead of dnf's raw mid-transaction refusal.
    if !exec.is_root {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "installing system RPM '{package}' requires root privileges; re-run with sudo: `sudo anolisa install --backend rpm {component}`"
            ),
        });
    }

    // The package transaction and state commit share one lock. The route into
    // this function used an unlocked state snapshot, so reload and compare it
    // before dnf can mutate rpmdb.
    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;
    let state = common::load_installed_state(ctx, command).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: err.reason(),
    })?;
    expectation.verify_locked(&state, command)?;
    ensure_component_backend_compatible(&state, component, "rpm", command)?;
    rpm_install::reject_pending_claim(layout, &state, &[component, package], command)?;

    let mut pending = if matches!(disposition, DelegatedInstallDisposition::Fresh) {
        Some(rpm_install::begin_fresh_install(
            layout, component, package, command,
        )?)
    } else {
        None
    };
    let audit = match pending.as_ref() {
        Some(pending) => InstallAudit {
            operation_id: pending.transaction.operation_id.clone(),
            started_at: pending.transaction.started_at.clone(),
        },
        None => new_install_audit(),
    };

    // dnf install — delegate the file transaction while retaining the lock.
    if let Err(err) = exec.txn.install(package) {
        let install_error = txn_install_err(err, command);
        if let Some(pending) = pending.as_mut() {
            match exec.query.query_installed(package) {
                Ok(None) => {
                    if let Err(journal_err) = pending.finish_failed(
                        pending.install_step,
                        &install_error.reason(),
                        command,
                    ) {
                        return Err(pending_retry_blocked_error(
                            command,
                            pending,
                            &install_error.reason(),
                            &journal_err,
                        ));
                    }
                    return Err(install_error);
                }
                Ok(Some(_)) | Err(_) => {
                    return Err(finish_partial_recovery_error(
                        pending,
                        pending.install_step,
                        &install_error.reason(),
                        command,
                        &format!(
                            "{}; RPM installation state is not safely known",
                            install_error.reason()
                        ),
                    ));
                }
            }
        }
        return Err(install_error);
    }
    if let Some(pending) = pending.as_mut() {
        if let Err(err) = pending.mark_install_done(command) {
            return Err(pending_recovery_error(
                command,
                pending,
                "dnf completed, but ANOLISA could not record the completed RPM install",
                Some(&err),
            ));
        }
    }

    // Refresh from rpmdb: the authoritative installed EVR/arch.
    let info = match exec.query.query_installed(package) {
        Ok(Some(info)) => info,
        // dnf reported success, so the package should be present; a miss here is
        // anomalous (a no-op transaction?). Refuse rather than record a phantom.
        Ok(None) => {
            let reason = format!(
                "dnf install of '{package}' reported success but rpmdb has no such package"
            );
            if let Some(pending) = pending.as_mut() {
                return Err(finish_partial_recovery_error(
                    pending,
                    pending.state_step,
                    &reason,
                    command,
                    &reason,
                ));
            }
            return Err(CliError::Runtime {
                command: command.to_string(),
                reason,
            });
        }
        Err(PackageQueryError::UnexpectedOutput { .. }) => {
            let reason = format!(
                "RPM package '{package}' has multiple installed versions after install; refusing to record an ambiguous version"
            );
            if let Some(pending) = pending.as_mut() {
                return Err(finish_partial_recovery_error(
                    pending,
                    pending.state_step,
                    &reason,
                    command,
                    &reason,
                ));
            }
            return Err(CliError::Runtime {
                command: command.to_string(),
                reason,
            });
        }
        Err(err) => {
            let query_error = pkg_query_err(err, command);
            if let Some(pending) = pending.as_mut() {
                return Err(finish_partial_recovery_error(
                    pending,
                    pending.state_step,
                    &query_error.reason(),
                    command,
                    &query_error.reason(),
                ));
            }
            return Err(query_error);
        }
    };

    // source_repo is supplementary metadata: a failed origin lookup degrades to
    // `None` with a warning and never fails the install (mirrors adopt).
    let source_repo = match exec.query.installed_origin(package) {
        Ok(origin) => origin,
        Err(err) => {
            warnings.push(format!(
                "could not determine source repo for '{package}': {err}"
            ));
            None
        }
    };

    let (operation_id, snapshot_warnings) = match persist_delegated_install_locked(
        ctx,
        layout,
        state,
        command,
        component,
        package,
        disposition,
        &audit,
        &info,
        source_repo.as_deref(),
        &warnings,
    ) {
        Ok(result) => result,
        Err(err) => {
            if let Some(pending) = pending.as_mut() {
                return Err(finish_partial_recovery_error(
                    pending,
                    pending.state_step,
                    &err.reason(),
                    command,
                    &err.reason(),
                ));
            }
            return Err(err);
        }
    };
    warnings.extend(snapshot_warnings);

    // State and its successful operation record are authoritative once saved.
    // Journal finalisation failures are warnings: the scanner ignores the stale
    // journal by operation ID, so reporting the completed install as failed
    // would incorrectly invite a retry.
    if let Some(pending) = pending.as_mut() {
        if let Err(err) = pending
            .mark_state_done(command)
            .and_then(|()| pending.finish_ok(command))
        {
            warnings.push(format!(
                "installed state was saved, but the RPM recovery journal could not be finalised: {}",
                err.reason()
            ));
        }
    }

    let payload = DelegatedInstallPayload {
        component: component.to_string(),
        package: package.to_string(),
        backend: "rpm",
        ownership: "rpm-managed",
        install_mode: ctx.install_mode.as_str().to_string(),
        version: Some(info.version.to_string()),
        arch: Some(info.arch.clone()),
        source_repo,
        available_candidates: Vec::new(),
        operation_id: Some(operation_id),
        dry_run: false,
        warnings,
    };
    render_delegated_install(ctx, &payload);
    Ok(InstallOutcome::Installed)
}

struct InstallAudit {
    operation_id: String,
    started_at: String,
}

fn new_install_audit() -> InstallAudit {
    let lock_ts = Utc::now();
    InstallAudit {
        operation_id: format!(
            "op-install-{}-{}",
            lock_ts.format("%Y%m%d%H%M%S"),
            lock_ts.timestamp_subsec_nanos()
        ),
        started_at: now_iso8601(),
    }
}

fn finish_partial_recovery_error(
    pending: &mut rpm_install::PendingRpmInstall,
    failed_step: usize,
    journal_reason: &str,
    command: &str,
    detail: &str,
) -> CliError {
    let journal_error = pending
        .finish_partial(failed_step, journal_reason, command)
        .err();
    pending_recovery_error(command, pending, detail, journal_error.as_ref())
}

fn pending_recovery_error(
    command: &str,
    pending: &rpm_install::PendingRpmInstall,
    detail: &str,
    journal_error: Option<&CliError>,
) -> CliError {
    let (journal_detail, write_guidance) = match journal_error {
        Some(err) => (
            pending.journal_update_failure_detail(err),
            "restore write access to ANOLISA state storage, then ",
        ),
        None => (
            format!(
                "recovery journal operation '{}' is at '{}'",
                pending.transaction.operation_id,
                pending.transaction.journal_path.display()
            ),
            "",
        ),
    };
    CliError::Runtime {
        command: command.to_string(),
        reason: format!(
            "{detail}; RPM package '{}' may be installed while ANOLISA state is incomplete; {journal_detail} — {write_guidance}run `anolisa repair {}` before retrying",
            pending.package, pending.component
        ),
    }
}

fn pending_retry_blocked_error(
    command: &str,
    pending: &rpm_install::PendingRpmInstall,
    detail: &str,
    journal_error: &CliError,
) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: format!(
            "{detail}; the RPM package was not observed as installed, but {}; restore write access to ANOLISA state storage and run `anolisa repair {}` to clear the pending claim before retrying",
            pending.journal_update_failure_detail(journal_error),
            pending.component
        ),
    }
}

/// Persist a delegated install as `rpm-managed` state under the install lock,
/// then append an audit record. Returns the operation id.
///
/// Mirrors [`execute_adopt`]'s state write but records ANOLISA ownership
/// (`managed=true`, `adopted=false`, [`Ownership::RpmManaged`]) — the file
/// transaction was ANOLISA-driven, so a later uninstall delegates back to dnf.
#[allow(clippy::too_many_arguments)]
fn persist_delegated_install_locked(
    ctx: &CliContext,
    layout: &FsLayout,
    mut state: InstalledState,
    command: &str,
    component: &str,
    package: &str,
    disposition: DelegatedInstallDisposition,
    audit: &InstallAudit,
    info: &PackageInfo,
    source_repo: Option<&str>,
    warnings: &[String],
) -> Result<(String, Vec<String>), CliError> {
    let evr = info.version.to_string();
    let started_at = audit.started_at.clone();
    let operation_id = audit.operation_id.clone();

    state.install_mode = StateInstallMode::System;
    state.prefix = layout.prefix.clone();
    match disposition {
        DelegatedInstallDisposition::Fresh => state.upsert_object(rpm_install::fresh_rpm_object(
            component,
            info,
            source_repo,
            &operation_id,
            &started_at,
        )),
        DelegatedInstallDisposition::ReinstallManaged => {
            let object = state
                .find_object_mut(ObjectKind::Component, component)
                .ok_or_else(|| CliError::Runtime {
                    command: command.to_string(),
                    reason: format!(
                        "component '{component}' vanished while its RPM was being reinstalled"
                    ),
                })?;
            object.version = evr.clone();
            object.status = ObjectStatus::Installed;
            object.install_backend = Some("rpm".to_string());
            object.ownership = Some(Ownership::RpmManaged);
            object.managed = true;
            object.adopted = false;
            object.last_operation_id = Some(operation_id.clone());
            let metadata = object.rpm_metadata.get_or_insert_with(|| RpmMetadata {
                package_name: info.name.clone(),
                evr: None,
                arch: None,
                source_repo: None,
            });
            metadata.package_name = info.name.clone();
            metadata.evr = Some(evr.clone());
            metadata.arch = Some(info.arch.clone());
            if source_repo.is_some() {
                metadata.source_repo = source_repo.map(str::to_string);
            }
        }
    }
    state.operations.push(rpm_install::install_operation(
        &operation_id,
        command,
        &started_at,
        now_iso8601(),
    ));

    let state_path = layout.state_dir.join("installed.toml");
    state.save(&state_path).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to save state: {err}"),
    })?;

    // Best-effort: snapshot the datadir component contract so adapter commands
    // can discover declared adapters. Missing or unwritable contracts produce
    // warnings, never failures.
    let snapshot_warnings = snapshot_datadir_contract(layout, component, command);
    let mut all_warnings = warnings.to_vec();
    all_warnings.extend(snapshot_warnings.clone());

    // Audit log is best-effort: the install already persisted, so a log failure
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
            "installed RPM package {package} ({evr}) as rpm-managed for component {component} via dnf"
        ),
        actor: "cli".to_string(),
        install_mode: Some(ctx.install_mode.as_str().to_string()),
        started_at,
        finished_at: Some(now_iso8601()),
        status: Some(LogStatus::Ok),
        objects: vec![component.to_string()],
        backup_ids: Vec::new(),
        warnings: all_warnings,
        details: serde_json::Value::Null,
    };
    if let Err(err) = log.append(&record) {
        eprintln!("warning: failed to write central log: {err}");
    }

    Ok((operation_id, snapshot_warnings))
}

/// Render a delegated-install result (JSON envelope or human text). Silent in
/// quiet mode; the `--all` batch path drives its own summary.
fn render_delegated_install(ctx: &CliContext, payload: &DelegatedInstallPayload) {
    if ctx.json {
        // Errors here are unreachable for a plain Serialize struct; ignore the
        // Result so the (already-persisted) install is not reported as failed.
        let _ = render_json(COMMAND, payload);
        return;
    }
    if ctx.quiet {
        return;
    }
    let color = Palette::new(ctx.no_color);
    if payload.dry_run {
        println!(
            "{} {} {} {}",
            color.command("install"),
            payload.component,
            color.muted(format!("(rpm-managed, {})", payload.package)),
            color.muted("(dry-run — nothing installed)"),
        );
        if payload.available_candidates.is_empty() {
            println!(
                "{} {}",
                color.label("available:"),
                color.muted("no repo candidates reported"),
            );
        } else {
            println!(
                "{} {}",
                color.label("available:"),
                payload.available_candidates.join(", "),
            );
        }
        println!("  would run: dnf install -y {}", payload.package);
    } else {
        println!(
            "{} {} {} {}",
            color.command("install"),
            payload.component,
            color.muted(format!("(rpm-managed, {})", payload.package)),
            color.ok("installed via dnf"),
        );
        if let Some(v) = &payload.version {
            println!("{} {}", color.label("version:"), v);
        }
    }
    render_warnings(&payload.warnings, &color);
}

/// Map a [`PackageTransactionError`] from `dnf install` onto a CLI runtime
/// error with an actionable hint.
fn txn_install_err(err: PackageTransactionError, command: &str) -> CliError {
    match err {
        PackageTransactionError::CommandMissing { .. } => rpm_tooling_missing_error(command),
        PackageTransactionError::PermissionDenied { command: bin } => {
            common::package_permission_error(command, &bin, "install")
        }
        PackageTransactionError::TransactionFailed { code, stderr, .. } => {
            common::package_transaction_failed_error(command, "install", code, &stderr)
        }
    }
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

/// Warn-and-exit error raised when the system-RPM probe cannot run because
/// `rpm`/`dnf` is absent (§7.1).
///
/// Without rpm tooling the probe cannot tell whether the component is already
/// installed as a system RPM. We deliberately refuse to silently fall back to
/// a raw install here: a raw install over an unobserved system RPM could
/// clobber or duplicate it. The caller may still force a raw install with an
/// explicit `--backend raw`, which bypasses the probe entirely.
fn rpm_tooling_missing_error(command: &str) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: "rpm/dnf not found: cannot detect whether this component is already installed as a system RPM. Install rpm/dnf, or pass `--backend raw` to install without RPM adoption".to_string(),
    }
}
