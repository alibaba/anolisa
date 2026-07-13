//! `anolisa repair <component>` — reconcile ANOLISA state with rpmdb reality.
//!
//! When a user runs `dnf update`/`downgrade` outside ANOLISA, the recorded EVR
//! drifts from rpmdb (surfaced as `drifted` by `anolisa status`). `repair`
//! reads rpmdb, confirms the package identity is still valid, and refreshes the
//! ANOLISA state record (version, EVR, arch, source repo). It runs **no**
//! dnf/rpm transaction and never switches backend — only rpmdb reads plus a
//! state write.
//!
//! A pending delegated install can also be completed from its transaction
//! journal when dnf committed the RPM but `installed.toml` was not saved. A
//! tracked package that has since been `rpm -e`'d still cannot be repaired;
//! raw components have no rpmdb to reconcile against and are not handled yet.

use chrono::{SecondsFormat, Utc};
use clap::Parser;
use serde::Serialize;

use anolisa_core::central_log::{CentralLog, LogKind, LogRecord, LogStatus, Severity};
use anolisa_core::lock::InstallLock;
use anolisa_core::state::{ObjectKind, OperationRecord, Ownership, RpmMetadata};
use anolisa_core::transaction::TransactionOutcomeStatus;
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::pkg_query::{PackageInfo, PackageQuery, PackageQueryError};
use anolisa_platform::rpm_query::RpmPackageQuery;

use crate::color::Palette;
use crate::commands::common;
use crate::commands::common::RepoPersistPolicy;
use crate::commands::tier1::install::{
    rpm_package_candidates_with_index, snapshot_datadir_contract,
};
use crate::commands::tier1::rpm_recovery::{
    RpmInstallRecovery, find_pending_rpm_install, load_pending_rpm_install,
    record_rpm_managed_install,
};
use crate::context::CliContext;
use crate::resolution::{ResolutionUse, load_optional_component_index};
use crate::response::{CliError, render_json};

/// Command label for JSON envelopes and error routing.
const COMMAND: &str = "repair";

/// Arguments for `anolisa repair <component>`.
#[derive(Debug, Parser)]
pub struct RepairArgs {
    /// Component whose ANOLISA state should be refreshed from rpmdb
    #[arg(value_name = "COMPONENT")]
    pub component: String,
}

/// Wire shape for a `repair <component>` result (`--json`) and its dry-run
/// preview.
#[derive(Serialize)]
struct RepairPayload {
    component: String,
    package: String,
    /// Always `rpm`: repair never switches backend.
    backend: &'static str,
    /// `rpm-observed` or `rpm-managed`; preserved across the repair.
    ownership: &'static str,
    install_mode: String,
    /// EVR ANOLISA had recorded; `None` for a legacy row with no metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    from_version: Option<String>,
    /// EVR read back from rpmdb (the value state is reconciled to).
    to_version: String,
    /// Whether state was actually written (false on dry-run).
    refreshed: bool,
    /// Whether the rpmdb EVR differed from what ANOLISA had recorded.
    changed: bool,
    dry_run: bool,
    /// `None` on dry-run (nothing recorded).
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    warnings: Vec<String>,
}

/// Dispatch `repair <component>`: build the real rpm-backed query and reconcile.
///
/// # Errors
///
/// Returns [`CliError`] when neither state nor a recovery journal identifies
/// the component, the component is raw-backed (unsupported), the package is
/// gone from rpmdb, the rpmdb read is ambiguous, or the state write fails.
pub fn handle(args: RepairArgs, ctx: &CliContext) -> Result<(), CliError> {
    let query = RpmPackageQuery::system();
    repair_with_query(&args.component, ctx, &query)
}

/// Core of [`handle`] with the package query injected so tests drive the
/// reconcile path without a live rpmdb. Repair runs no dnf transaction, so only
/// a [`PackageQuery`] is required.
fn repair_with_query(
    target: &str,
    ctx: &CliContext,
    query: &dyn PackageQuery,
) -> Result<(), CliError> {
    let command = format!("repair {target}");
    common::require_system_mode(
        ctx,
        &command,
        "repair reconciles system RPM state and requires system scope",
        &format!("sudo anolisa repair {target}"),
    )?;

    let installed = common::load_installed_state(ctx, COMMAND)?;

    let component = common::lookup_component_name(target, &installed, ctx, COMMAND);

    let obj = match installed.find_object(ObjectKind::Component, &component) {
        Some(obj) => obj,
        None => {
            let layout = common::resolve_layout(ctx);
            let state_path = layout.state_dir.join("installed.toml");
            let journal_dir = layout.state_dir.join("journal");
            let pending = find_pending_rpm_install(
                &journal_dir,
                &state_path,
                &installed,
                &component,
            )
                .map_err(|err| rpm_recovery_error(&command, err))?
                .ok_or_else(|| CliError::InvalidArgument {
                    command: command.clone(),
                    reason: format!(
                        "component '{target}' is not installed — nothing to repair (run `anolisa status` to see what is installed)"
                    ),
                })?;
            return repair_pending_rpm_install(ctx, query, &command, pending);
        }
    };

    let ownership = obj.effective_ownership();
    // Raw components have no rpmdb to reconcile against. Keep them on the same
    // not-implemented boundary the update path uses for raw.
    if !ownership.is_rpm() {
        return Err(CliError::not_implemented_with_hint(
            command,
            "raw component repair is not implemented yet; only RPM-backed components can be repaired today",
        ));
    }

    // Resolve the package to reconcile against. A recorded package name is the
    // identity to confirm; when absent (a legacy row with no rpm_metadata), fall
    // back to the adopt candidate chain so repair can backfill the metadata the
    // update path refuses to run without.
    let package =
        resolve_repair_package(&component, obj.rpm_metadata.as_ref(), ctx, query, &command)?;
    let recorded_evr = obj.rpm_metadata.as_ref().and_then(|m| m.evr.clone());
    let ownership_label = ownership.label();

    // rpmdb query — the truth repair reconciles to.
    let info = match query.query_installed(&package) {
        Ok(Some(info)) => info,
        // rpm -e: nothing to refresh from. repair cannot conjure the package
        // back, so point at forget (or reinstall) rather than fabricating state.
        Ok(None) => {
            return Err(CliError::Runtime {
                command,
                reason: format!(
                    "RPM package '{package}' for component '{component}' is recorded in ANOLISA state but is not present in rpmdb — it may have been removed with `rpm -e`; run `anolisa forget {component}` to drop the stale state, or reinstall"
                ),
            });
        }
        // rpm could not be reduced to a single installed version (duplicates, a
        // malformed `--qf` row, or none on a zero exit): an ambiguous reconcile
        // target. Refuse with the backend's own detail rather than asserting one
        // specific cause.
        Err(PackageQueryError::UnexpectedOutput { detail, .. }) => {
            return Err(CliError::Runtime {
                command,
                reason: format!(
                    "rpm returned unexpected output for package '{package}': {detail}; refusing to refresh until it resolves to a single installed version"
                ),
            });
        }
        Err(PackageQueryError::CommandMissing { .. }) => {
            return Err(rpm_tooling_missing_error(&command));
        }
        Err(err) => return Err(rpm_query_err(err, &command)),
    };

    let to_evr = info.version.to_string();
    let changed = recorded_evr.as_deref() != Some(to_evr.as_str());

    // source_repo is supplementary metadata: a failed origin lookup degrades to
    // `None` with a warning and never fails the repair (mirrors adopt).
    let mut warnings: Vec<String> = Vec::new();
    let source_repo = match query.installed_origin(&package) {
        Ok(origin) => origin,
        Err(err) => {
            warnings.push(format!(
                "could not determine source repo for '{package}': {err}"
            ));
            None
        }
    };

    if ctx.dry_run {
        let payload = RepairPayload {
            component,
            package,
            backend: "rpm",
            ownership: ownership_label,
            install_mode: ctx.install_mode.as_str().to_string(),
            from_version: recorded_evr,
            to_version: to_evr,
            refreshed: false,
            changed,
            dry_run: true,
            operation_id: None,
            warnings,
        };
        render_repair(ctx, &payload);
        return Ok(());
    }

    let operation_id = persist_repair(
        ctx,
        &component,
        &package,
        ownership,
        &info,
        &to_evr,
        source_repo.as_deref(),
        &command,
        &warnings,
    )?;

    let payload = RepairPayload {
        component,
        package,
        backend: "rpm",
        ownership: ownership_label,
        install_mode: ctx.install_mode.as_str().to_string(),
        from_version: recorded_evr,
        to_version: to_evr,
        refreshed: true,
        changed,
        dry_run: false,
        operation_id: Some(operation_id),
        warnings,
    };
    render_repair(ctx, &payload);
    Ok(())
}

/// Recover an RPM install whose dnf transaction outlived its state commit.
fn repair_pending_rpm_install(
    ctx: &CliContext,
    query: &dyn PackageQuery,
    command: &str,
    pending: RpmInstallRecovery,
) -> Result<(), CliError> {
    if ctx.dry_run {
        return preview_pending_rpm_repair(ctx, query, command, pending);
    }

    let layout = common::resolve_layout(ctx);
    let state_path = layout.state_dir.join("installed.toml");
    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;
    let mut state = common::load_installed_state(ctx, command)?;
    if let Some(existing) = state.find_object(ObjectKind::Component, pending.component()) {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "component '{}' appeared in state while RPM recovery was being prepared and is now tracked as {}; refusing to overwrite it",
                pending.component(),
                existing.effective_ownership().label()
            ),
        });
    }

    // Reload the exact marker and rescan under the lock. The path check guards
    // against replacement, while the scan also rejects a newly-created second
    // marker for the same component.
    let expected_path = pending.journal_path().to_path_buf();
    let expected_operation_id = pending.operation_id().to_string();
    let expected_component = pending.component().to_string();
    let expected_package = pending.package().to_string();
    let exact = load_pending_rpm_install(&expected_path, &state_path)
        .map_err(|err| rpm_recovery_error(command, err))?
        .ok_or_else(|| CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "RPM recovery journal {} is no longer pending; no state was changed",
                expected_path.display()
            ),
        })?;
    validate_pending_identity(
        command,
        &exact,
        &expected_operation_id,
        &expected_component,
        &expected_package,
    )?;
    let mut recovery = find_pending_rpm_install(
        &layout.state_dir.join("journal"),
        &state_path,
        &state,
        &expected_component,
    )
    .map_err(|err| rpm_recovery_error(command, err))?
    .ok_or_else(|| CliError::Runtime {
        command: command.to_string(),
        reason: format!(
            "RPM recovery journal for component '{expected_component}' is no longer pending; no state was changed"
        ),
    })?;
    validate_pending_identity(
        command,
        &recovery,
        &expected_operation_id,
        &expected_component,
        &expected_package,
    )?;

    let info = match query.query_installed(&expected_package) {
        Ok(Some(info)) => info,
        Ok(None) => {
            let reason = format!(
                "rpmdb does not contain pending package '{expected_package}'; no ANOLISA state was committed"
            );
            recovery
                .mark_install_failed(&reason)
                .map_err(|err| pending_journal_update_error(command, &expected_component, err))?;
            recovery
                .mark_persist_failed(&reason)
                .map_err(|err| pending_journal_update_error(command, &expected_component, err))?;
            recovery
                .finish(TransactionOutcomeStatus::Failed)
                .map_err(|err| pending_journal_update_error(command, &expected_component, err))?;
            return Err(CliError::Runtime {
                command: command.to_string(),
                reason: format!(
                    "pending RPM install for component '{expected_component}' was cleared because package '{expected_package}' is not installed; re-run `anolisa install --backend rpm {expected_component}`"
                ),
            });
        }
        Err(PackageQueryError::UnexpectedOutput { detail, .. }) => {
            let reason = format!("rpm returned unexpected output: {detail}");
            mark_pending_query_partial(&mut recovery, &reason);
            return Err(pending_rpm_query_error(
                command,
                &expected_component,
                &expected_package,
                &reason,
            ));
        }
        Err(PackageQueryError::CommandMissing { .. }) => {
            let reason = "rpm/dnf was not found";
            mark_pending_query_partial(&mut recovery, reason);
            return Err(pending_rpm_query_error(
                command,
                &expected_component,
                &expected_package,
                reason,
            ));
        }
        Err(err) => {
            let reason = format!("rpm query failed: {err}");
            mark_pending_query_partial(&mut recovery, &reason);
            return Err(pending_rpm_query_error(
                command,
                &expected_component,
                &expected_package,
                &reason,
            ));
        }
    };

    let mut warnings = Vec::new();
    let source_repo = match query.installed_origin(&expected_package) {
        Ok(origin) => origin,
        Err(err) => {
            warnings.push(format!(
                "could not determine source repo for '{expected_package}': {err}"
            ));
            None
        }
    };
    recovery
        .mark_install_done()
        .map_err(|err| pending_journal_update_error(command, &expected_component, err))?;

    let finished_at = now_iso8601();
    let install_command = format!("install {expected_component}");
    record_rpm_managed_install(
        &mut state,
        &layout,
        &expected_component,
        &info,
        source_repo.as_deref(),
        &expected_operation_id,
        recovery.started_at(),
        &install_command,
        &finished_at,
    );
    if let Err(err) = state.save(&state_path) {
        let reason = format!("failed to save recovered RPM state: {err}");
        let _ = recovery.mark_persist_failed(&reason);
        let _ = recovery.finish(TransactionOutcomeStatus::Partial);
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "{reason}; pending install remains recoverable — run `anolisa repair {expected_component}` after fixing the state path"
            ),
        });
    }

    warnings.extend(snapshot_datadir_contract(
        &layout,
        &expected_component,
        command,
    ));
    if let Err(err) = recovery.mark_persist_done() {
        warnings.push(format!(
            "recovered state was saved but the RPM recovery journal could not mark persistence complete: {err}"
        ));
    } else if let Err(err) = recovery.finish(TransactionOutcomeStatus::Ok) {
        warnings.push(format!(
            "recovered state was saved but the RPM recovery journal could not be finalized: {err}"
        ));
    }

    append_pending_repair_log(
        ctx,
        &layout,
        command,
        &expected_component,
        &expected_package,
        &info,
        &expected_operation_id,
        recovery.started_at(),
        &finished_at,
        &warnings,
    );
    let payload = RepairPayload {
        component: expected_component,
        package: expected_package,
        backend: "rpm",
        ownership: Ownership::RpmManaged.label(),
        install_mode: ctx.install_mode.as_str().to_string(),
        from_version: None,
        to_version: info.version.to_string(),
        refreshed: true,
        changed: true,
        dry_run: false,
        operation_id: Some(expected_operation_id),
        warnings,
    };
    render_repair(ctx, &payload);
    Ok(())
}

fn preview_pending_rpm_repair(
    ctx: &CliContext,
    query: &dyn PackageQuery,
    command: &str,
    pending: RpmInstallRecovery,
) -> Result<(), CliError> {
    let package = pending.package().to_string();
    let component = pending.component().to_string();
    let info = match query.query_installed(&package) {
        Ok(Some(info)) => info,
        Ok(None) => {
            return Err(CliError::Runtime {
                command: command.to_string(),
                reason: format!(
                    "pending RPM package '{package}' is not installed; dry-run left the recovery marker unchanged — run `anolisa repair {component}` without --dry-run to clear it, then reinstall"
                ),
            });
        }
        Err(err) => {
            return Err(pending_rpm_query_error(
                command,
                &component,
                &package,
                &err.to_string(),
            ));
        }
    };
    let mut warnings = Vec::new();
    match query.installed_origin(&package) {
        Ok(_) => {}
        Err(err) => {
            warnings.push(format!(
                "could not determine source repo for '{package}': {err}"
            ));
        }
    }
    let payload = RepairPayload {
        component,
        package,
        backend: "rpm",
        ownership: Ownership::RpmManaged.label(),
        install_mode: ctx.install_mode.as_str().to_string(),
        from_version: None,
        to_version: info.version.to_string(),
        refreshed: false,
        changed: true,
        dry_run: true,
        operation_id: None,
        warnings,
    };
    render_repair(ctx, &payload);
    Ok(())
}

fn validate_pending_identity(
    command: &str,
    pending: &RpmInstallRecovery,
    operation_id: &str,
    component: &str,
    package: &str,
) -> Result<(), CliError> {
    if pending.operation_id() == operation_id
        && pending.component() == component
        && pending.package() == package
    {
        return Ok(());
    }
    Err(CliError::Runtime {
        command: command.to_string(),
        reason: format!(
            "RPM recovery journal changed while repair was being prepared; expected operation '{operation_id}' for component '{component}' and package '{package}', so no state was changed"
        ),
    })
}

fn mark_pending_query_partial(recovery: &mut RpmInstallRecovery, reason: &str) {
    let _ = recovery.mark_persist_failed(reason);
    let _ = recovery.finish(TransactionOutcomeStatus::Partial);
}

fn rpm_recovery_error(
    command: &str,
    err: crate::commands::tier1::rpm_recovery::RpmRecoveryError,
) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: format!("RPM install recovery journal failure: {err}"),
    }
}

fn pending_journal_update_error(
    command: &str,
    component: &str,
    err: anolisa_core::transaction::TransactionError,
) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: format!(
            "failed to update the RPM recovery journal for component '{component}': {err}; no ANOLISA state was committed"
        ),
    }
}

fn pending_rpm_query_error(
    command: &str,
    component: &str,
    package: &str,
    reason: &str,
) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: format!(
            "could not determine whether pending RPM package '{package}' for component '{component}' is installed ({reason}); recovery marker was retained and ANOLISA state was not changed"
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn append_pending_repair_log(
    ctx: &CliContext,
    layout: &FsLayout,
    command: &str,
    component: &str,
    package: &str,
    info: &PackageInfo,
    operation_id: &str,
    started_at: &str,
    finished_at: &str,
    warnings: &[String],
) {
    let evr = info.version.to_string();
    let record = LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.to_string()),
        command: command.to_string(),
        source: "anolisa-cli".to_string(),
        component: Some(component.to_string()),
        severity: Severity::Info,
        message: format!(
            "recovered rpm-managed state for component {component} from installed RPM package {package} ({evr})"
        ),
        actor: "cli".to_string(),
        install_mode: Some(ctx.install_mode.as_str().to_string()),
        started_at: started_at.to_string(),
        finished_at: Some(finished_at.to_string()),
        status: Some(LogStatus::Ok),
        objects: vec![component.to_string()],
        backup_ids: Vec::new(),
        warnings: warnings.to_vec(),
        details: serde_json::Value::Null,
    };
    if let Err(err) = CentralLog::open(layout.central_log.clone()).append(&record) {
        eprintln!("warning: failed to write central log: {err}");
    }
}

/// Resolve the RPM package name `repair` should reconcile against.
///
/// A recorded, non-empty package name is the identity to confirm. When it is
/// absent — a legacy row written before `rpm_metadata` existed — fall back to
/// the shared component resolver so repair can backfill the metadata that
/// `update` refuses to run without.
fn resolve_repair_package(
    component: &str,
    meta: Option<&RpmMetadata>,
    ctx: &CliContext,
    query: &dyn PackageQuery,
    command: &str,
) -> Result<String, CliError> {
    if let Some(name) = meta
        .map(|m| m.package_name.as_str())
        .filter(|n| !n.is_empty())
    {
        return Ok(name.to_string());
    }

    // Legacy row with no recorded package name: resolve via the same component
    // identity resolver adopt uses. repo.toml / components.toml are best-effort
    // inputs (mirrors status::observed_record): a load failure just drops that
    // precedence tier.
    let layout = common::resolve_layout(ctx);
    let repo_config =
        common::load_repo_config(ctx, &layout, COMMAND, RepoPersistPolicy::BestEffort).ok();
    let rpm_backend = repo_config.as_ref().and_then(|c| c.backends.get("rpm"));
    let env = anolisa_env::EnvService::detect();
    let component_index = repo_config
        .as_ref()
        .and_then(|cfg| load_optional_component_index(&layout, &env, cfg));

    let candidates = match rpm_package_candidates_with_index(
        None,
        rpm_backend,
        component_index.as_ref(),
        query,
        component,
        ResolutionUse::RepairLegacy,
    ) {
        Ok(candidates) => candidates,
        Err(PackageQueryError::CommandMissing { .. }) => {
            return Err(rpm_tooling_missing_error(command));
        }
        Err(err) => return Err(rpm_query_err(err, command)),
    };
    if candidates.len() >= 2 {
        return Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "multiple candidate RPMs for component '{component}': {}; cannot repair unambiguously — reinstall to pin one, or fix the component index / package metadata",
                candidates
                    .iter()
                    .map(|target| target.package.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        });
    }
    candidates
        .into_iter()
        .next()
        .map(|target| target.package)
        .ok_or_else(|| CliError::Runtime {
            command: command.to_string(),
            reason: format!("could not resolve an RPM package name for component '{component}'"),
        })
}

/// Persist the reconciled RPM metadata under the install lock, then append an
/// audit record. Ownership and `install_backend` are left untouched — repair
/// never switches backend. Returns the operation id.
#[allow(clippy::too_many_arguments)]
fn persist_repair(
    ctx: &CliContext,
    component: &str,
    package: &str,
    ownership: Ownership,
    info: &PackageInfo,
    to_evr: &str,
    source_repo: Option<&str>,
    command: &str,
    warnings: &[String],
) -> Result<String, CliError> {
    let layout = common::resolve_layout(ctx);
    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;
    let mut state = common::load_installed_state(ctx, command)?;

    // Re-validate under the lock: the component must still exist and still be
    // RPM-owned. A concurrent uninstall/forget or backend change between the
    // pre-lock read and here must not be clobbered by a stale repair record.
    let obj = state
        .find_object_mut(ObjectKind::Component, component)
        .ok_or_else(|| CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "component '{component}' disappeared from state during repair; no changes recorded"
            ),
        })?;
    if !obj.effective_ownership().is_rpm() {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "component '{component}' is no longer an RPM component in state; refusing to record an RPM repair"
            ),
        });
    }
    // A recorded package name must be unchanged under the lock: `query_installed`
    // ran against `package` (snapshotted before the lock), so a concurrent
    // re-point to a different RPM would graft this EVR onto the wrong package.
    // An empty/absent prior name is a legacy backfill and allowed.
    if let Some(recorded) = obj
        .rpm_metadata
        .as_ref()
        .map(|m| m.package_name.as_str())
        .filter(|n| !n.is_empty())
        && recorded != package
    {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "component '{component}' RPM package identity changed during repair (expected package '{package}'); refusing to record an EVR against a different package — run `anolisa status {component}`"
            ),
        });
    }

    let now = now_iso8601();
    let lock_ts = Utc::now();
    let operation_id = format!(
        "op-repair-{}-{}",
        lock_ts.format("%Y%m%d%H%M%S"),
        lock_ts.timestamp_subsec_nanos()
    );

    // Reconcile the recorded version to rpmdb truth. ownership / install_backend
    // / status are deliberately untouched: repair refreshes facts, it does not
    // re-decide the lifecycle.
    obj.version = to_evr.to_string();
    obj.last_operation_id = Some(operation_id.clone());
    match obj.rpm_metadata.as_mut() {
        Some(meta) => {
            // Backfill the name for a legacy row; a no-op when already set.
            meta.package_name = package.to_string();
            meta.evr = Some(to_evr.to_string());
            meta.arch = Some(info.arch.clone());
            // Only overwrite source_repo when freshly determined — a failed
            // origin lookup must not erase a previously-good value.
            if let Some(repo) = source_repo {
                meta.source_repo = Some(repo.to_string());
            }
        }
        None => {
            obj.rpm_metadata = Some(RpmMetadata {
                package_name: package.to_string(),
                evr: Some(to_evr.to_string()),
                arch: Some(info.arch.clone()),
                source_repo: source_repo.map(str::to_string),
            });
        }
    }

    state.operations.push(OperationRecord {
        id: operation_id.clone(),
        command: command.to_string(),
        status: "ok".to_string(),
        started_at: now.clone(),
        finished_at: Some(now.clone()),
    });

    let state_path = layout.state_dir.join("installed.toml");
    state.save(&state_path).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to save state: {err}"),
    })?;

    // Audit log is best-effort: the repair already persisted, so a log failure
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
            "refreshed ANOLISA state for component {component} to {to_evr} from rpmdb package {package} ({ownership_label})",
            ownership_label = ownership.label(),
        ),
        actor: "cli".to_string(),
        install_mode: Some(ctx.install_mode.as_str().to_string()),
        started_at: now.clone(),
        finished_at: Some(now),
        status: Some(LogStatus::Ok),
        objects: vec![component.to_string()],
        backup_ids: Vec::new(),
        warnings: warnings.to_vec(),
        details: serde_json::Value::Null,
    };
    if let Err(err) = log.append(&record) {
        eprintln!("warning: failed to write central log: {err}");
    }

    Ok(operation_id)
}

/// Human/JSON renderer for a repair result.
fn render_repair(ctx: &CliContext, payload: &RepairPayload) {
    if ctx.json {
        // Errors here are unreachable for a plain Serialize struct; ignore the
        // Result so an (already-persisted) repair is not reported as failed.
        let _ = render_json(COMMAND, payload);
        return;
    }
    if ctx.quiet {
        return;
    }
    let color = Palette::new(ctx.no_color);
    let from = payload.from_version.as_deref().unwrap_or("(none recorded)");
    if payload.dry_run {
        println!(
            "{} {} {} {}",
            color.command("repair"),
            payload.component,
            color.muted(format!("({}, {})", payload.ownership, payload.package)),
            color.muted("(dry-run — nothing written)"),
        );
        if payload.changed {
            println!(
                "{} {} → {}",
                color.label("would refresh:"),
                from,
                payload.to_version
            );
        } else {
            println!(
                "{} state already matches rpmdb ({})",
                color.label("would refresh:"),
                payload.to_version,
            );
        }
    } else if payload.changed {
        println!(
            "{} {} {} → {}",
            color.ok("✓ repaired"),
            payload.component,
            from,
            payload.to_version,
        );
    } else {
        println!(
            "{} {} already matches rpmdb ({})",
            color.ok("✓"),
            payload.component,
            payload.to_version,
        );
    }
    // Remind the operator that an observed row is a pre-existing system RPM.
    if payload.ownership == "rpm-observed" {
        println!(
            "    {} {} is a system RPM observed by ANOLISA; dnf owns the file transaction",
            color.label("note:"),
            payload.package,
        );
    }
    render_warnings(&payload.warnings, &color);
}

/// Map a [`PackageQueryError`] onto a CLI runtime error (the benign
/// not-installed / multi-version branches are split off by the caller).
fn rpm_query_err(err: PackageQueryError, command: &str) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: format!("rpm query failed: {err}"),
    }
}

/// Warn-and-exit error when `rpm`/`dnf` is absent: an RPM component cannot be
/// reconciled without the package manager.
fn rpm_tooling_missing_error(command: &str) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: "rpm/dnf not found: cannot reconcile an RPM-backed component without the package manager. Install rpm/dnf and retry".to_string(),
    }
}

/// Render any accumulated warnings to stderr, one per line.
fn render_warnings(warnings: &[String], color: &Palette) {
    for w in warnings {
        eprintln!("{} {w}", color.warn("warning:"));
    }
}

/// RFC3339 UTC timestamp, seconds precision (matches the install/update paths).
fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::tier1::rpm_recovery::begin_rpm_install;
    use crate::context::InstallMode;

    use std::{fs, path::PathBuf};

    use anolisa_core::state::{
        InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectStatus,
    };
    use anolisa_core::transaction::{Transaction, TransactionOutcomeStatus, TransactionStepStatus};
    use anolisa_platform::pkg_query::PackageVersion;

    /// Configurable in-memory [`PackageQuery`] for the repair tests. Repair runs
    /// no transaction, so a query alone drives every path.
    struct FakeQuery {
        package: String,
        installed: Option<PackageInfo>,
        origin: Option<String>,
        multi_version: bool,
        command_missing: bool,
    }

    impl FakeQuery {
        fn new(package: &str, installed: Option<PackageInfo>) -> Self {
            Self {
                package: package.to_string(),
                installed,
                origin: None,
                multi_version: false,
                command_missing: false,
            }
        }
        fn with_origin(mut self, origin: &str) -> Self {
            self.origin = Some(origin.to_string());
            self
        }
        fn multi_version(mut self) -> Self {
            self.multi_version = true;
            self
        }
        fn command_missing(mut self) -> Self {
            self.command_missing = true;
            self
        }
    }

    impl PackageQuery for FakeQuery {
        fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
            if self.command_missing {
                return Err(PackageQueryError::CommandMissing {
                    command: "rpm".to_string(),
                });
            }
            if package != self.package {
                return Ok(None);
            }
            if self.multi_version {
                return Err(PackageQueryError::UnexpectedOutput {
                    command: "rpm".to_string(),
                    detail: "2 installed versions".to_string(),
                });
            }
            Ok(self.installed.clone())
        }
        fn query_available(&self, _package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
            Ok(Vec::new())
        }
        fn installed_origin(&self, package: &str) -> Result<Option<String>, PackageQueryError> {
            if package == self.package {
                Ok(self.origin.clone())
            } else {
                Ok(None)
            }
        }
        fn provided_capabilities_installed(
            &self,
            package: &str,
        ) -> Result<Vec<String>, PackageQueryError> {
            if package == self.package && self.installed.is_some() {
                Ok(vec![format!("anolisa-component({package})")])
            } else {
                Ok(Vec::new())
            }
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

    fn ctx(prefix: PathBuf, install_mode: InstallMode, dry_run: bool) -> CliContext {
        CliContext {
            install_mode,
            prefix: Some(prefix),
            json: false,
            dry_run,
            verbose: false,
            quiet: true,
            no_color: true,
        }
    }

    fn seed_component_index(ctx: &CliContext, index: &str) {
        let layout = common::resolve_layout(ctx);
        let repo_v1 = layout.prefix.join("repo").join("v1");
        fs::create_dir_all(&repo_v1).expect("mkdir repo");
        fs::write(repo_v1.join("components.toml"), index).expect("write components.toml");
        fs::create_dir_all(&layout.etc_dir).expect("mkdir etc");
        fs::write(
            layout.etc_dir.join("repo.toml"),
            format!(
                "schema_version = 1\n\
                 default_backend = \"raw\"\n\
                 \n\
                 [backends.raw]\n\
                 base_url = \"file://{}\"\n",
                repo_v1.display()
            ),
        )
        .expect("write repo.toml");
    }

    /// An RPM-backed component object (observed or managed).
    fn rpm_object(
        component: &str,
        package: &str,
        evr: &str,
        ownership: Ownership,
        status: ObjectStatus,
    ) -> InstalledObject {
        InstalledObject {
            kind: ObjectKind::Component,
            name: component.to_string(),
            version: evr.to_string(),
            status,
            manifest_digest: None,
            distribution_source: None,
            raw_package: None,
            install_backend: Some("rpm".to_string()),
            ownership: Some(ownership),
            rpm_metadata: Some(RpmMetadata {
                package_name: package.to_string(),
                evr: Some(evr.to_string()),
                arch: Some("x86_64".to_string()),
                source_repo: Some("@System".to_string()),
            }),
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: Some("op-prior".to_string()),
            managed: !matches!(ownership, Ownership::RpmObserved),
            adopted: matches!(ownership, Ownership::RpmObserved),
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
            provisioned_packages: Vec::new(),
        }
    }

    /// A raw-managed component object (no rpm metadata).
    fn raw_object(component: &str, version: &str) -> InstalledObject {
        InstalledObject {
            kind: ObjectKind::Component,
            name: component.to_string(),
            version: version.to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: None,
            distribution_source: Some("https://example.com/x".to_string()),
            raw_package: None,
            install_backend: Some("raw".to_string()),
            ownership: Some(Ownership::RawManaged),
            rpm_metadata: None,
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: None,
            managed: true,
            adopted: false,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
            provisioned_packages: Vec::new(),
        }
    }

    fn seed(ctx: &CliContext, obj: InstalledObject) {
        let layout = common::resolve_layout(ctx);
        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");
        let mut state = InstalledState {
            install_mode: match ctx.install_mode {
                InstallMode::System => StateInstallMode::System,
                InstallMode::User => StateInstallMode::User,
            },
            prefix: layout.prefix.clone(),
            ..Default::default()
        };
        state.upsert_object(obj);
        state
            .save(&layout.state_dir.join("installed.toml"))
            .expect("seed state");
    }

    fn load_state(ctx: &CliContext) -> InstalledState {
        let layout = common::resolve_layout(ctx);
        InstalledState::load(&layout.state_dir.join("installed.toml")).expect("load state")
    }

    fn seed_pending_rpm_install(
        ctx: &CliContext,
        component: &str,
        package: &str,
    ) -> (String, PathBuf) {
        let layout = common::resolve_layout(ctx);
        let mut recovery = begin_rpm_install(
            layout.state_dir.join("installed.toml"),
            &layout.state_dir.join("journal"),
            component,
            package,
        )
        .expect("begin pending install");
        let operation_id = recovery.operation_id().to_string();
        let journal_path = recovery.journal_path().to_path_buf();
        recovery.mark_install_done().expect("dnf done marker");
        recovery
            .mark_persist_failed("simulated state commit failure")
            .expect("persist failure marker");
        recovery
            .finish(TransactionOutcomeStatus::Partial)
            .expect("partial marker");
        (operation_id, journal_path)
    }

    #[test]
    fn repair_recovers_installed_rpm_from_pending_journal() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let layout = common::resolve_layout(&c);
        let contract_source = FsLayout::component_contract_path(&layout.datadir, "cosh");
        fs::create_dir_all(contract_source.parent().expect("contract parent"))
            .expect("create contract dir");
        fs::write(
            &contract_source,
            "[component]\nname = \"cosh\"\nversion = \"2.3.0\"\n",
        )
        .expect("write contract");
        let (operation_id, journal_path) = seed_pending_rpm_install(&c, "cosh", "copilot-shell");
        let rpm = FakeQuery::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "aarch64")),
        )
        .with_origin("anolisa");

        repair_with_query("cosh", &c, &rpm).expect("recover pending install");

        let state = load_state(&c);
        let obj = state
            .find_object(ObjectKind::Component, "cosh")
            .expect("recovered component");
        assert_eq!(obj.version, "2.3.0-1.al8");
        assert_eq!(obj.status, ObjectStatus::Installed);
        assert_eq!(obj.ownership, Some(Ownership::RpmManaged));
        assert_eq!(obj.install_backend.as_deref(), Some("rpm"));
        assert!(obj.managed);
        assert!(!obj.adopted);
        assert_eq!(
            obj.last_operation_id.as_deref(),
            Some(operation_id.as_str())
        );
        let metadata = obj.rpm_metadata.as_ref().expect("rpm metadata");
        assert_eq!(metadata.package_name, "copilot-shell");
        assert_eq!(metadata.evr.as_deref(), Some("2.3.0-1.al8"));
        assert_eq!(metadata.arch.as_deref(), Some("aarch64"));
        assert_eq!(metadata.source_repo.as_deref(), Some("anolisa"));
        let operation = state
            .operations
            .iter()
            .find(|operation| operation.id == operation_id)
            .expect("original install operation committed");
        assert_eq!(operation.command, "install cosh");

        let journal = Transaction::load_journal(&journal_path).expect("load finalized journal");
        assert_eq!(journal.status, TransactionOutcomeStatus::Ok);
        assert!(
            journal
                .steps
                .iter()
                .all(|step| step.status == TransactionStepStatus::Done)
        );
        let snapshot = common::installed_component_manifest_path(&layout, "cosh", "repair cosh")
            .expect("snapshot path");
        assert_eq!(
            fs::read_to_string(snapshot).expect("read snapshot"),
            fs::read_to_string(contract_source).expect("read source contract")
        );
    }

    #[test]
    fn repair_clears_pending_marker_when_rpm_is_absent() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let (_operation_id, journal_path) = seed_pending_rpm_install(&c, "cosh", "copilot-shell");
        let rpm = FakeQuery::new("copilot-shell", None);

        let err = repair_with_query("cosh", &c, &rpm)
            .expect_err("absent pending package must clear marker and error");
        assert!(err.reason().contains("was cleared"));
        assert!(err.reason().contains("install --backend rpm cosh"));
        let journal = Transaction::load_journal(&journal_path).expect("load failed journal");
        assert_eq!(journal.status, TransactionOutcomeStatus::Failed);
        assert!(
            journal
                .steps
                .iter()
                .all(|step| step.status == TransactionStepStatus::Failed)
        );
        assert!(
            load_state(&c)
                .find_object(ObjectKind::Component, "cosh")
                .is_none()
        );
        let layout = common::resolve_layout(&c);
        assert!(
            find_pending_rpm_install(
                &layout.state_dir.join("journal"),
                &layout.state_dir.join("installed.toml"),
                &load_state(&c),
                "cosh",
            )
            .expect("scan markers")
            .is_none()
        );
    }

    #[test]
    fn repair_keeps_pending_marker_partial_when_rpmdb_is_ambiguous() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let (_operation_id, journal_path) = seed_pending_rpm_install(&c, "cosh", "copilot-shell");
        let rpm = FakeQuery::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.3.0", Some("1"), "x86_64")),
        )
        .multi_version();

        let err = repair_with_query("cosh", &c, &rpm)
            .expect_err("ambiguous rpmdb must preserve pending recovery");
        assert!(err.reason().contains("marker was retained"));
        assert_eq!(
            Transaction::load_journal(&journal_path)
                .expect("load partial journal")
                .status,
            TransactionOutcomeStatus::Partial
        );
        assert!(
            load_state(&c)
                .find_object(ObjectKind::Component, "cosh")
                .is_none()
        );
    }

    #[test]
    fn repair_pending_dry_run_writes_nothing() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, true);
        let (_operation_id, journal_path) = seed_pending_rpm_install(&c, "cosh", "copilot-shell");
        let before = fs::read(&journal_path).expect("read marker before dry-run");
        let rpm = FakeQuery::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.3.0", Some("1"), "x86_64")),
        );

        repair_with_query("cosh", &c, &rpm).expect("dry-run preview");
        assert_eq!(
            fs::read(&journal_path).expect("read marker after dry-run"),
            before
        );
        assert!(
            load_state(&c)
                .find_object(ObjectKind::Component, "cosh")
                .is_none()
        );
    }

    /// A drifted rpm-observed component refreshes its EVR/arch/source from rpmdb
    /// while ownership, backend, and lifecycle status are preserved.
    #[test]
    fn repair_refreshes_drifted_evr_and_keeps_ownership() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        // rpmdb has moved on to 2.3.0 via a manual dnf update.
        let rpm = FakeQuery::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64")),
        )
        .with_origin("alinux-updates");

        repair_with_query("copilot-shell", &c, &rpm).expect("repair ok");

        let obj = load_state(&c)
            .find_object(ObjectKind::Component, "copilot-shell")
            .cloned()
            .expect("present");
        assert_eq!(obj.version, "2.3.0-1.al8", "version reconciled to rpmdb");
        assert_eq!(
            obj.ownership,
            Some(Ownership::RpmObserved),
            "ownership kept"
        );
        assert_eq!(obj.install_backend.as_deref(), Some("rpm"), "backend kept");
        assert_eq!(obj.status, ObjectStatus::Adopted, "status unchanged");
        let meta = obj.rpm_metadata.expect("metadata");
        assert_eq!(meta.evr.as_deref(), Some("2.3.0-1.al8"));
        assert_eq!(meta.source_repo.as_deref(), Some("alinux-updates"));
        assert_ne!(obj.last_operation_id.as_deref(), Some("op-prior"));
    }

    #[test]
    fn repair_resolves_package_alias_to_canonical_state_component() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed_component_index(
            &c,
            r#"
schema_version = 1

[[components]]
name = "cosh"

[[components.backends]]
kind = "rpm"
package = "copilot-shell"
legacy_adopt = true

[[components.aliases]]
kind = "rpm-package"
name = "copilot-shell"
"#,
        );
        seed(
            &c,
            rpm_object(
                "cosh",
                "copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        let rpm = FakeQuery::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64")),
        )
        .with_origin("alinux-updates");

        repair_with_query("copilot-shell", &c, &rpm).expect("repair via package alias");

        let state = load_state(&c);
        let obj = state
            .find_object(ObjectKind::Component, "cosh")
            .cloned()
            .expect("canonical component repaired");
        assert_eq!(obj.version, "2.3.0-1.al8");
        assert_eq!(
            obj.rpm_metadata
                .as_ref()
                .map(|meta| meta.package_name.as_str()),
            Some("copilot-shell")
        );
        assert!(
            state
                .find_object(ObjectKind::Component, "copilot-shell")
                .is_none(),
            "repair must refresh the canonical state row, not create a package-name row"
        );
    }

    /// The "keeping ownership / does not switch backend" criterion holds for the
    /// rpm-managed lifecycle too, not just observed: a drifted rpm-managed
    /// component refreshes its EVR while ownership stays `rpm-managed`.
    #[test]
    fn repair_refreshes_rpm_managed_keeping_ownership() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmManaged,
                ObjectStatus::Installed,
            ),
        );
        let rpm = FakeQuery::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64")),
        );
        repair_with_query("copilot-shell", &c, &rpm).expect("repair ok");

        let obj = load_state(&c)
            .find_object(ObjectKind::Component, "copilot-shell")
            .cloned()
            .expect("present");
        assert_eq!(obj.version, "2.3.0-1.al8", "version reconciled to rpmdb");
        assert_eq!(
            obj.ownership,
            Some(Ownership::RpmManaged),
            "rpm-managed ownership kept across refresh",
        );
        assert_eq!(obj.install_backend.as_deref(), Some("rpm"), "backend kept");
        assert_eq!(obj.status, ObjectStatus::Installed, "status unchanged");
    }

    /// A failed origin lookup must not erase a previously-good source_repo.
    #[test]
    fn repair_keeps_prior_source_repo_when_origin_unknown() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        // No origin configured on the fake -> installed_origin yields None.
        let rpm = FakeQuery::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64")),
        );
        repair_with_query("copilot-shell", &c, &rpm).expect("repair ok");
        let obj = load_state(&c)
            .find_object(ObjectKind::Component, "copilot-shell")
            .cloned()
            .expect("present");
        assert_eq!(
            obj.rpm_metadata.expect("meta").source_repo.as_deref(),
            Some("@System"),
            "prior source_repo preserved when origin re-lookup is empty",
        );
    }

    /// `rpm -e`'d package: repair refuses and points at forget; state untouched.
    #[test]
    fn repair_on_missing_package_points_at_forget() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        let rpm = FakeQuery::new("copilot-shell", None);
        let err =
            repair_with_query("copilot-shell", &c, &rpm).expect_err("removed package must error");
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("forget"),
            "reason must point at forget: {}",
            err.reason()
        );
        assert_eq!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .map(|o| o.version.clone())
                .as_deref(),
            Some("2.2.0-1.al8"),
            "state must be untouched",
        );
    }

    /// A same-name multi-version rpmdb is an ambiguous reconcile target.
    #[test]
    fn repair_multi_version_is_refused() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmManaged,
                ObjectStatus::Installed,
            ),
        );
        let rpm = FakeQuery::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.2.0", Some("1.al8"), "x86_64")),
        )
        .multi_version();
        let err =
            repair_with_query("copilot-shell", &c, &rpm).expect_err("multi-version must error");
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(err.reason().contains("unexpected output"));
        assert!(err.reason().contains("2 installed versions"));
    }

    /// Missing rpm/dnf tooling surfaces as an actionable runtime error.
    #[test]
    fn repair_without_rpm_tooling_errors() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        let rpm = FakeQuery::new("copilot-shell", None).command_missing();
        let err =
            repair_with_query("copilot-shell", &c, &rpm).expect_err("missing tooling must error");
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(err.reason().contains("rpm/dnf not found"));
    }

    /// Raw components are not repairable yet -> NOT_IMPLEMENTED.
    #[test]
    fn repair_raw_component_is_not_implemented() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        // User mode ignores `prefix` and resolves from the process home, so
        // this test uses system mode to keep the seeded state under `tmp`.
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(&c, raw_object("copilot-shell", "9.9.9"));
        let rpm = FakeQuery::new("copilot-shell", None);
        let err = repair_with_query("copilot-shell", &c, &rpm)
            .expect_err("raw repair is not implemented");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
    }

    /// An absent component routes to INVALID_ARGUMENT (exit 2).
    #[test]
    fn repair_unknown_component_routes_to_invalid_argument() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let rpm = FakeQuery::new("copilot-shell", None);
        let err =
            repair_with_query("copilot-shell", &c, &rpm).expect_err("absent component must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert_eq!(err.exit_code(), 2);
        assert!(err.reason().contains("not installed"));
    }

    /// Dry-run previews the reconcile without writing state.
    #[test]
    fn repair_dry_run_writes_nothing() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, true);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        let rpm = FakeQuery::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64")),
        );
        repair_with_query("copilot-shell", &c, &rpm).expect("dry-run ok");
        assert_eq!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .map(|o| o.version.clone())
                .as_deref(),
            Some("2.2.0-1.al8"),
            "dry-run must not refresh the recorded version",
        );
    }

    /// Repair on an already-matching component is a no-op refresh: it succeeds,
    /// records an operation, and leaves the version in place.
    #[test]
    fn repair_no_op_when_already_matches() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "copilot-shell",
                "2.3.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        let rpm = FakeQuery::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64")),
        );
        repair_with_query("copilot-shell", &c, &rpm).expect("repair ok");
        let obj = load_state(&c)
            .find_object(ObjectKind::Component, "copilot-shell")
            .cloned()
            .expect("present");
        assert_eq!(obj.version, "2.3.0-1.al8");
        assert_ne!(obj.last_operation_id.as_deref(), Some("op-prior"));
    }

    /// A legacy RPM row with no recorded metadata is repaired by resolving the
    /// default package name and backfilling `rpm_metadata` from rpmdb.
    #[test]
    fn repair_backfills_metadata_for_legacy_row() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        // RPM-owned (managed) row, but rpm_metadata absent (pre-v3 shape).
        let mut obj = rpm_object(
            "legacy-rpm",
            "",
            "0.0.0",
            Ownership::RpmManaged,
            ObjectStatus::Installed,
        );
        obj.rpm_metadata = None;
        seed(&c, obj);
        // No recorded package_name, so the shared resolver recovers it from the
        // installed package's ANOLISA provides metadata.
        let rpm = FakeQuery::new(
            "legacy-rpm",
            Some(pkg_info("legacy-rpm", "1.0.0", Some("1.al8"), "x86_64")),
        )
        .with_origin("@System");
        repair_with_query("legacy-rpm", &c, &rpm).expect("repair ok");
        let obj = load_state(&c)
            .find_object(ObjectKind::Component, "legacy-rpm")
            .cloned()
            .expect("present");
        let meta = obj.rpm_metadata.expect("metadata backfilled");
        assert_eq!(meta.package_name, "legacy-rpm");
        assert_eq!(meta.evr.as_deref(), Some("1.0.0-1.al8"));
        assert_eq!(obj.version, "1.0.0-1.al8");
    }

    /// CLI surface: `repair <component>` parses to the positional.
    #[test]
    fn repair_parses_positional_component() {
        use clap::Parser as _;
        let a = RepairArgs::try_parse_from(["repair", "copilot-shell"]).expect("parse");
        assert_eq!(a.component, "copilot-shell");
    }
}
