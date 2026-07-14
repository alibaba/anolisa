//! `anolisa repair <component>` — reconcile ANOLISA state with rpmdb reality.
//!
//! When a user runs `dnf update`/`downgrade` outside ANOLISA, the recorded EVR
//! drifts from rpmdb (surfaced as `drifted` by `anolisa status`). `repair`
//! reads rpmdb, confirms the package identity is still valid, and refreshes the
//! ANOLISA state record (version, EVR, arch, source repo). It runs **no**
//! dnf/rpm transaction and never switches backend — only rpmdb reads plus a
//! state write.
//!
//! A package that has been `rpm -e`'d cannot be repaired: there is nothing to
//! refresh from, so `repair` refuses and points at `anolisa forget`. Raw
//! components have no rpmdb to reconcile against and are not handled yet.

use chrono::{SecondsFormat, Utc};
use clap::Parser;
use serde::Serialize;

use anolisa_core::central_log::{CentralLog, LogKind, LogRecord, LogStatus, Severity};
use anolisa_core::lock::InstallLock;
use anolisa_core::state::{InstalledState, ObjectKind, OperationRecord, Ownership, RpmMetadata};
use anolisa_platform::pkg_query::{PackageInfo, PackageQuery, PackageQueryError};
use anolisa_platform::rpm_query::RpmPackageQuery;

use crate::color::Palette;
use crate::commands::common;
use crate::commands::common::RepoPersistPolicy;
use crate::commands::tier1::install::{
    rpm_package_candidates_with_index, snapshot_datadir_contract,
};
use crate::commands::tier1::rpm_install::{self, PendingRpmInstall};
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
/// Returns [`CliError`] when the component is absent, raw-backed (unsupported),
/// the package is gone from rpmdb, the rpmdb read is ambiguous, or the state
/// write fails.
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

    let layout = common::resolve_layout(ctx);
    if let Some(pending) = rpm_install::find_pending_claim(
        &layout,
        &installed,
        &[target, component.as_str()],
        &command,
    )? {
        return repair_pending_rpm(ctx, &layout, &installed, pending, query, &command);
    }

    let obj = installed
        .find_object(ObjectKind::Component, &component)
        .ok_or_else(|| CliError::InvalidArgument {
            command: command.clone(),
            reason: format!(
                "component '{target}' is not installed — nothing to repair (run `anolisa status` to see what is installed)"
            ),
        })?;

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

fn repair_pending_rpm(
    ctx: &CliContext,
    layout: &anolisa_platform::fs_layout::FsLayout,
    preview_state: &InstalledState,
    pending: PendingRpmInstall,
    query: &dyn PackageQuery,
    command: &str,
) -> Result<(), CliError> {
    // Preview must reject the same ownership conflicts as execution; otherwise
    // dry-run could promise a recovery that the locked path will refuse.
    reject_pending_state_owner(preview_state, &pending, command)?;

    if ctx.dry_run {
        let info = query_pending_package(query, &pending.package, command)?
            .ok_or_else(|| CliError::Runtime {
                command: command.to_string(),
                reason: format!(
                    "pending RPM package '{}' for component '{}' is not installed; a real repair would mark the recovery journal Failed and clear its pending claim, then return an error so installation can be retried — run `anolisa repair {}` without --dry-run to perform that cleanup",
                    pending.package, pending.component, pending.component
                ),
            })?;
        let warnings = Vec::new();
        let payload = RepairPayload {
            component: pending.component,
            package: pending.package,
            backend: "rpm",
            ownership: "rpm-managed",
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
        return Ok(());
    }

    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;
    let mut state = common::load_installed_state(ctx, command)?;
    let mut pending = rpm_install::find_pending_claim(
        layout,
        &state,
        &[pending.component.as_str(), pending.package.as_str()],
        command,
    )?
    .ok_or_else(|| pending_claim_changed_error(&pending, command))?;

    // The unlocked check is only a fast-fail. State may change before lock
    // acquisition, so ownership must be validated again before any write.
    reject_pending_state_owner(&state, &pending, command)?;

    let info = match query_pending_package(query, &pending.package, command) {
        Ok(info) => info,
        Err(err) => {
            let journal_error = pending
                .finish_partial(pending.state_step, &err.reason(), command)
                .err();
            return Err(pending_repair_error(
                &pending,
                command,
                &err.reason(),
                journal_error.as_ref(),
            ));
        }
    };
    let Some(info) = info else {
        let reason = format!("RPM package '{}' is not installed", pending.package);
        if let Err(journal_err) = pending.finish_failed(pending.state_step, &reason, command) {
            return Err(pending_repair_error(
                &pending,
                command,
                &reason,
                Some(&journal_err),
            ));
        }
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "pending RPM install for component '{}' was terminated because package '{}' is not installed; its journal is now Failed and no longer participates in recovery, and installed.toml was left unchanged — retry `anolisa install {}`",
                pending.component, pending.package, pending.component
            ),
        });
    };

    let mut warnings = Vec::new();
    let source_repo = match query.installed_origin(&pending.package) {
        Ok(origin) => origin,
        Err(err) => {
            warnings.push(format!(
                "could not determine source repo for '{}': {err}",
                pending.package
            ));
            None
        }
    };
    let operation_id = pending.transaction.operation_id.clone();
    let started_at = pending.transaction.started_at.clone();
    state.install_mode = anolisa_core::state::InstallMode::System;
    state.prefix = layout.prefix.clone();
    state.upsert_object(rpm_install::fresh_rpm_object(
        &pending.component,
        &info,
        source_repo.as_deref(),
        &operation_id,
        &started_at,
    ));
    let install_command = format!("install {}", pending.component);
    state.operations.push(rpm_install::install_operation(
        &operation_id,
        &install_command,
        &started_at,
        now_iso8601(),
    ));
    let state_path = layout.state_dir.join("installed.toml");
    if let Err(err) = state.save(&state_path) {
        let reason = format!("failed to save recovered RPM state: {err}");
        let journal_error = pending
            .finish_partial(pending.state_step, &reason, command)
            .err();
        return Err(pending_repair_error(
            &pending,
            command,
            &reason,
            journal_error.as_ref(),
        ));
    }

    if let Err(err) = pending
        .mark_install_done(command)
        .and_then(|()| pending.mark_state_done(command))
        .and_then(|()| pending.finish_ok(command))
    {
        warnings.push(format!(
            "recovered state was saved, but the RPM recovery journal could not be finalised: {}",
            err.reason()
        ));
    }
    warnings.extend(snapshot_datadir_contract(
        layout,
        &pending.component,
        command,
    ));
    if let Err(warning) = append_pending_repair_log(
        ctx,
        layout,
        &pending,
        &info,
        &operation_id,
        &started_at,
        &warnings,
    ) {
        warnings.push(warning);
    }

    let payload = RepairPayload {
        component: pending.component,
        package: pending.package,
        backend: "rpm",
        ownership: "rpm-managed",
        install_mode: ctx.install_mode.as_str().to_string(),
        from_version: None,
        to_version: info.version.to_string(),
        refreshed: true,
        changed: true,
        dry_run: false,
        operation_id: Some(operation_id),
        warnings,
    };
    render_repair(ctx, &payload);
    Ok(())
}

fn pending_claim_changed_error(pending: &PendingRpmInstall, command: &str) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: format!(
            "pending RPM install '{}' for component '{}' (package '{}', journal {}) no longer matches after the install lock was acquired; it may have completed, moved, or changed — reload installed.toml and cross-check the journal with rpmdb before retrying repair",
            pending.transaction.operation_id,
            pending.component,
            pending.package,
            pending.transaction.journal_path.display()
        ),
    }
}

fn pending_repair_error(
    pending: &PendingRpmInstall,
    command: &str,
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
            "{detail}; {journal_detail} — {write_guidance}run `anolisa repair {}` again",
            pending.component,
        ),
    }
}

fn reject_pending_state_owner(
    state: &InstalledState,
    pending: &PendingRpmInstall,
    command: &str,
) -> Result<(), CliError> {
    let Some(owner) = rpm_install::state_claim_owner(state, &pending.component, &pending.package)
    else {
        return Ok(());
    };
    Err(CliError::Runtime {
        command: command.to_string(),
        reason: format!(
            "pending RPM install for component '{}' (package '{}') conflicts with existing state component '{}'; refusing to overwrite either owner",
            pending.component, pending.package, owner.name
        ),
    })
}

fn query_pending_package(
    query: &dyn PackageQuery,
    package: &str,
    command: &str,
) -> Result<Option<PackageInfo>, CliError> {
    match query.query_installed(package) {
        Ok(info) => Ok(info),
        Err(PackageQueryError::CommandMissing { .. }) => Err(rpm_tooling_missing_error(command)),
        Err(PackageQueryError::UnexpectedOutput { detail, .. }) => Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "rpm returned unexpected output for pending package '{package}': {detail}; recovery marker was preserved"
            ),
        }),
        Err(err) => Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "failed to query pending RPM package '{package}': {err}; recovery marker was preserved"
            ),
        }),
    }
}

fn append_pending_repair_log(
    ctx: &CliContext,
    layout: &anolisa_platform::fs_layout::FsLayout,
    pending: &PendingRpmInstall,
    info: &PackageInfo,
    operation_id: &str,
    started_at: &str,
    warnings: &[String],
) -> Result<(), String> {
    let record = LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.to_string()),
        command: format!("repair {}", pending.component),
        source: "anolisa-cli".to_string(),
        component: Some(pending.component.clone()),
        severity: Severity::Info,
        message: format!(
            "recovered pending RPM package {} ({}) as rpm-managed for component {}",
            pending.package, info.version, pending.component
        ),
        actor: "cli".to_string(),
        install_mode: Some(ctx.install_mode.as_str().to_string()),
        started_at: started_at.to_string(),
        finished_at: Some(now_iso8601()),
        status: Some(LogStatus::Ok),
        objects: vec![pending.component.clone()],
        backup_ids: Vec::new(),
        warnings: warnings.to_vec(),
        details: serde_json::Value::Null,
    };
    CentralLog::open(layout.central_log.clone())
        .append(&record)
        .map_err(|err| {
            format!("recovered state was saved, but the central log was not written: {err}")
        })
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
    use crate::context::InstallMode;

    use std::{cell::Cell, fs, path::PathBuf};

    use anolisa_core::state::{
        InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectStatus,
    };
    use anolisa_core::transaction::{Transaction, TransactionOutcomeStatus};
    use anolisa_platform::pkg_query::PackageVersion;

    /// Configurable in-memory [`PackageQuery`] for the repair tests. Repair runs
    /// no transaction, so a query alone drives every path.
    struct FakeQuery {
        package: String,
        installed: Option<PackageInfo>,
        origin: Option<String>,
        multi_version: bool,
        command_missing: bool,
        block_journal_update: Option<PathBuf>,
        journal_was_blocked: Cell<bool>,
    }

    impl FakeQuery {
        fn new(package: &str, installed: Option<PackageInfo>) -> Self {
            Self {
                package: package.to_string(),
                installed,
                origin: None,
                multi_version: false,
                command_missing: false,
                block_journal_update: None,
                journal_was_blocked: Cell::new(false),
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
        fn failing_journal_update(mut self, path: PathBuf) -> Self {
            self.block_journal_update = Some(path);
            self
        }
    }

    impl PackageQuery for FakeQuery {
        fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
            if let Some(path) = &self.block_journal_update
                && !self.journal_was_blocked.replace(true)
            {
                let backup = path.with_extension("before-write-failure");
                fs::rename(path, &backup).expect("move journal directory");
                fs::write(path, b"block journal updates")
                    .expect("replace journal directory with file");
            }
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

    #[test]
    fn repair_recovers_fresh_rpm_install_by_package_alias() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let layout = common::resolve_layout(&c);
        let mut pending =
            rpm_install::begin_fresh_install(&layout, "cosh", "copilot-shell", "install cosh")
                .expect("begin pending install");
        pending
            .mark_install_done("install cosh")
            .expect("record dnf success");
        pending
            .finish_partial(
                pending.state_step,
                "simulated crash before state commit",
                "install cosh",
            )
            .expect("mark partial");
        let operation_id = pending.transaction.operation_id.clone();
        let journal_path = pending.transaction.journal_path.clone();
        let rpm = FakeQuery::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64")),
        )
        .with_origin("anolisa");

        repair_with_query("copilot-shell", &c, &rpm).expect("recover pending install");

        let state = load_state(&c);
        let object = state
            .find_object(ObjectKind::Component, "cosh")
            .expect("recovered component");
        assert_eq!(object.ownership, Some(Ownership::RpmManaged));
        assert_eq!(
            object.last_operation_id.as_deref(),
            Some(operation_id.as_str())
        );
        assert!(state.operations.iter().any(|op| op.id == operation_id));
        let journal = Transaction::load_journal(&journal_path).expect("load journal");
        assert_eq!(journal.status, TransactionOutcomeStatus::Ok);
        assert!(
            journal.steps.iter().all(|step| {
                step.status == anolisa_core::transaction::TransactionStepStatus::Done
            })
        );
    }

    #[test]
    fn pending_repair_log_failure_returns_warning() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let layout = common::resolve_layout(&c);
        let pending =
            rpm_install::begin_fresh_install(&layout, "cosh", "copilot-shell", "install cosh")
                .expect("begin pending install");
        let log_parent = layout.central_log.parent().expect("central log parent");
        fs::create_dir_all(log_parent.parent().expect("log root")).expect("create log root");
        fs::write(log_parent, b"block central log directory").expect("block central log");
        let info = pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64");

        let warning = append_pending_repair_log(
            &c,
            &layout,
            &pending,
            &info,
            &pending.transaction.operation_id,
            &pending.transaction.started_at,
            &[],
        )
        .expect_err("central log failure must be returned to the caller");

        assert!(warning.contains("recovered state was saved"));
        assert!(warning.contains("central log was not written"));
        assert!(warning.contains(&log_parent.display().to_string()));
    }

    #[test]
    fn repair_clears_pending_install_when_package_is_absent() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let layout = common::resolve_layout(&c);
        let pending =
            rpm_install::begin_fresh_install(&layout, "cosh", "copilot-shell", "install cosh")
                .expect("begin pending install");
        let journal_path = pending.transaction.journal_path.clone();
        let rpm = FakeQuery::new("copilot-shell", None);

        let err = repair_with_query("cosh", &c, &rpm)
            .expect_err("absent package must clear pending marker and report retry");
        assert!(err.reason().contains("retry `anolisa install cosh`"));
        assert!(err.reason().contains("journal is now Failed"));
        assert!(err.reason().contains("no longer participates in recovery"));
        assert!(err.reason().contains("installed.toml was left unchanged"));
        let journal = Transaction::load_journal(&journal_path).expect("load journal");
        assert_eq!(journal.status, TransactionOutcomeStatus::Failed);
        assert!(
            rpm_install::find_pending_claim(&layout, &InstalledState::default(), &["cosh"], "test")
                .expect("scan journals")
                .is_none()
        );
    }

    #[test]
    fn repair_pending_absent_dry_run_previews_cleanup_without_mutation() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, true);
        let layout = common::resolve_layout(&c);
        let pending =
            rpm_install::begin_fresh_install(&layout, "cosh", "copilot-shell", "install cosh")
                .expect("begin pending install");
        let journal_path = pending.transaction.journal_path.clone();
        let rpm = FakeQuery::new("copilot-shell", None);

        let err = repair_with_query("cosh", &c, &rpm)
            .expect_err("dry-run must preview cleanup without reporting recovery success");
        assert!(
            err.reason()
                .contains("would mark the recovery journal Failed")
        );
        assert!(err.reason().contains("clear its pending claim"));
        let journal = Transaction::load_journal(&journal_path).expect("load journal");
        assert_eq!(journal.status, TransactionOutcomeStatus::InFlight);
    }

    #[test]
    fn repair_query_failure_marks_pending_install_partial() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let layout = common::resolve_layout(&c);
        let pending =
            rpm_install::begin_fresh_install(&layout, "cosh", "copilot-shell", "install cosh")
                .expect("begin pending install");
        let journal_path = pending.transaction.journal_path.clone();
        let rpm = FakeQuery::new("copilot-shell", None).command_missing();

        repair_with_query("cosh", &c, &rpm).expect_err("query failure must preserve recovery");

        let journal = Transaction::load_journal(&journal_path).expect("load journal");
        assert_eq!(journal.status, TransactionOutcomeStatus::Partial);
        assert!(
            rpm_install::find_pending_claim(&layout, &InstalledState::default(), &["cosh"], "test")
                .expect("scan pending")
                .is_some()
        );
    }

    #[test]
    fn repair_journal_failure_preserves_retry_guidance() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let layout = common::resolve_layout(&c);
        let pending =
            rpm_install::begin_fresh_install(&layout, "cosh", "copilot-shell", "install cosh")
                .expect("begin pending install");
        let operation_id = pending.transaction.operation_id.clone();
        let journal_path = pending.transaction.journal_path.clone();
        let rpm = FakeQuery::new("copilot-shell", None)
            .failing_journal_update(layout.state_dir.join("journal"));

        let err = repair_with_query("cosh", &c, &rpm)
            .expect_err("journal failure must not hide repair retry guidance");
        assert!(
            err.reason().contains("not installed"),
            "got: {}",
            err.reason()
        );
        assert!(err.reason().contains("could not be updated"));
        assert!(err.reason().contains("may remain live"));
        assert!(err.reason().contains(&operation_id));
        assert!(err.reason().contains(&journal_path.display().to_string()));
        assert!(err.reason().contains("repair cosh` again"));
    }

    #[test]
    fn repair_pending_install_refuses_existing_state_owner() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let layout = common::resolve_layout(&c);
        rpm_install::begin_fresh_install(&layout, "cosh", "copilot-shell", "install cosh")
            .expect("begin pending install");
        seed(&c, raw_object("copilot-shell", "1.0.0"));
        let rpm = FakeQuery::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64")),
        );

        let err = repair_with_query("cosh", &c, &rpm)
            .expect_err("pending recovery must not overwrite a state owner");
        assert!(
            err.reason()
                .contains("conflicts with existing state component")
        );
        assert_eq!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .expect("raw owner remains")
                .ownership,
            Some(Ownership::RawManaged)
        );
    }

    #[test]
    fn repair_pending_dry_run_refuses_existing_state_owner() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, true);
        let layout = common::resolve_layout(&c);
        rpm_install::begin_fresh_install(&layout, "cosh", "copilot-shell", "install cosh")
            .expect("begin pending install");
        seed(&c, raw_object("copilot-shell", "1.0.0"));
        let rpm = FakeQuery::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64")),
        );

        let err = repair_with_query("cosh", &c, &rpm)
            .expect_err("dry-run must report the same ownership conflict");
        assert!(
            err.reason()
                .contains("conflicts with existing state component")
        );
    }

    #[test]
    fn changed_pending_claim_error_identifies_original_transaction() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let layout = common::resolve_layout(&c);
        let pending =
            rpm_install::begin_fresh_install(&layout, "cosh", "copilot-shell", "install cosh")
                .expect("begin pending install");

        let err = pending_claim_changed_error(&pending, "repair cosh");
        assert!(err.reason().contains(&pending.transaction.operation_id));
        assert!(
            err.reason()
                .contains(&pending.transaction.journal_path.display().to_string())
        );
        assert!(err.reason().contains("installed.toml"));
        assert!(err.reason().contains("rpmdb"));
    }

    /// CLI surface: `repair <component>` parses to the positional.
    #[test]
    fn repair_parses_positional_component() {
        use clap::Parser as _;
        let a = RepairArgs::try_parse_from(["repair", "copilot-shell"]).expect("parse");
        assert_eq!(a.component, "copilot-shell");
    }
}
