//! `anolisa upgrade` — first-phase RPM/system-image upgrade orchestration
//! (issue #1411).
//!
//! `upgrade` is an orchestration layer over RPM-backed operations, not a generic
//! backend-migration mechanism. It consumes the exact read-only plan produced by
//! [`anolisa update --check`](super::update::check::compute_update_check_report)
//! and, unless `--dry-run` is given, applies it through `dnf`:
//!
//! 1. Update the RPM-owned `anolisa` CLI package, if a newer candidate exists.
//! 2. Update already-installed RPM-backed components.
//! 3. Install target-profile default components that are missing.
//! 4. Re-read rpmdb and refresh ANOLISA state so `anolisa status` reflects the
//!    result.
//!
//! Scope is deliberately narrow (first phase):
//! - RPM / system-image scenario only; user mode is rejected. `--dry-run` waives
//!   the root requirement but not the system-mode one, so a non-root preview is
//!   `sudo anolisa upgrade --dry-run` or
//!   `anolisa --install-mode system upgrade --dry-run` (matching `repair`).
//! - Raw-managed components are **skipped** with a clear reason — never updated,
//!   migrated, or overwritten.
//! - npm/raw upgrade and broad adapter/framework reconciliation are out of
//!   scope; adapter needs would be surfaced as warnings only if discovered.
//!
//! The command never re-derives the plan: the planner lives in `update::check`
//! and both commands share it, so `upgrade` and `update --check` can never
//! disagree about what is upgradable.

use chrono::{SecondsFormat, Utc};
use clap::Parser;
use serde::Serialize;

use anolisa_core::central_log::{CentralLog, LogKind, LogRecord, LogStatus, Severity};
use anolisa_core::lock::InstallLock;
use anolisa_core::state::{
    InstallMode as StateInstallMode, InstalledObject, ObjectKind, ObjectStatus, OperationRecord,
    Ownership, RpmMetadata,
};
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::pkg_query::{PackageInfo, PackageQuery};
use anolisa_platform::pkg_transaction::{PackageTransaction, PackageTransactionError};
use anolisa_platform::privilege;
use anolisa_platform::rpm_query::RpmPackageQuery;
use anolisa_platform::rpm_transaction::RpmTransaction;

use super::update::check::{
    self, ACTION_ERROR, ACTION_INSTALL, ACTION_NOOP, ACTION_UNSUPPORTED, ACTION_UNSUPPORTED_RPM,
    ACTION_UPDATE, CliCheck, ComponentCheck,
};
use crate::color::Palette;
use crate::commands::common;
use crate::commands::common::RepoPersistPolicy;
use crate::context::{CliContext, InstallMode};
use crate::response::{self, CliError};

/// Command label for JSON envelopes and error routing.
const COMMAND: &str = "upgrade";

/// Upgrade backend this command covers. Always `rpm` in the first phase.
const BACKEND: &str = "rpm";

/// Reason attached to raw-managed components skipped by the RPM upgrade path.
/// Matches the wording called out in issue #1411 so the boundary reads the same
/// in the human summary and the JSON envelope.
const RAW_SKIP_REASON: &str = "raw-managed components are out of scope for RPM image upgrade";

// Stable status vocabulary for the result envelope.
const STATUS_OK: &str = "ok";
const STATUS_PARTIAL: &str = "partial";
const STATUS_FAILED: &str = "failed";
const STATUS_BLOCKED: &str = "blocked";

/// Arguments for `anolisa upgrade`.
///
/// `--dry-run` and `--json` are global flags read from [`CliContext`]; they are
/// intentionally not redefined here.
#[derive(Debug, Parser)]
pub struct UpgradeArgs {
    /// Target image/toolchain profile (defaults to the repository-declared
    /// default profile).
    #[arg(long, value_name = "TARGET")]
    pub target: Option<String>,

    /// Apply the upgrade non-interactively once the plan is known.
    ///
    /// The first phase is already non-interactive (it never prompts), so this
    /// flag is accepted for forward compatibility and to make the intent
    /// explicit in scripts; it does not currently change behaviour.
    #[arg(short = 'y', long = "assume-yes")]
    pub assume_yes: bool,
}

/// Dispatch `anolisa upgrade`.
///
/// # Errors
///
/// Returns [`CliError`] when the install mode is not `system`, real execution
/// is attempted without root, the configured RPM repository cannot be resolved,
/// state cannot be persisted, or the applied upgrade finished in a non-`ok`
/// status (partial / failed / blocked) — in the latter case the result has
/// already been rendered and only the exit code is propagated.
pub fn handle(args: UpgradeArgs, ctx: &CliContext) -> Result<(), CliError> {
    // System-only: `upgrade` reasons about rpm-owned packages and the configured
    // RPM repository, which do not exist in user mode. The dispatcher already
    // gates this, but a direct caller (or a test) must be refused here too.
    if ctx.install_mode != InstallMode::System {
        return Err(CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: "`anolisa upgrade` supports only system/RPM image scenarios; run `sudo anolisa upgrade` without `--install-mode user`".to_string(),
        });
    }

    let layout = common::resolve_layout(ctx);

    // Reuse the read-only planner behind `update --check`. This performs only
    // rpm/dnf *queries* — no transaction, no state write.
    let report = check::compute_update_check_report(args.target.as_deref(), ctx, &layout)?;

    // Resolve the configured RPM repository for the apply transactions. The
    // planner already proved a `[backends.rpm]` table exists, so this load
    // succeeds; it is repeated here (rather than threaded out of the planner) to
    // keep the planner's signature read-only and repo-agnostic.
    let repo_config = common::load_repo_config(ctx, &layout, COMMAND, RepoPersistPolicy::Require)?;
    let env = anolisa_env::EnvService::detect();
    let repo = super::update::rpm_repo_source_for_update(&repo_config, &env, COMMAND)?
        .ok_or_else(|| CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: "repo.toml has no [backends.rpm] table; `anolisa upgrade` needs the configured ANOLISA RPM repository".to_string(),
        })?;
    let query = RpmPackageQuery::system_with_repo(repo.clone());
    let txn = RpmTransaction::system_with_repo(repo);

    let plan = build_plan(report.target.clone(), &report.cli, &report.components);
    let result = run_upgrade_with_deps(
        ctx,
        &layout,
        &plan,
        &query,
        &txn,
        privilege::is_root(),
        ctx.dry_run,
        COMMAND,
    )?;

    render_result(ctx, &result);

    // A non-`ok` status has already been rendered (human or JSON); propagate a
    // non-zero exit without emitting a second envelope, mirroring the batch
    // install path.
    if result.status == STATUS_OK {
        Ok(())
    } else {
        Err(CliError::BatchPartial {
            command: COMMAND.to_string(),
        })
    }
}

// ── plan conversion ────────────────────────────────────────────────────────

/// A single RPM package the upgrade will `dnf upgrade`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedUpdate {
    /// ANOLISA component name (or the CLI package name for the CLI update).
    name: String,
    /// RPM package to upgrade.
    package: String,
    /// EVR recorded before the upgrade (rpmdb truth from the check).
    from: String,
    /// Newest repo candidate EVR the check reported.
    to: String,
}

/// A missing default component the upgrade will `dnf install`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedInstall {
    name: String,
    package: String,
}

/// An item deliberately left untouched (raw-managed, non-RPM CLI).
#[derive(Debug, Clone, PartialEq, Eq)]
struct SkippedItem {
    name: String,
    reason: String,
}

/// An item that cannot be planned/applied (ambiguous mapping, missing package,
/// upstream query failure recorded by the check).
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlanError {
    name: String,
    reason: String,
}

/// The RPM transaction plan converted from an [`update --check`] report.
#[derive(Debug, Default, PartialEq, Eq)]
struct UpgradePlan {
    target: Option<String>,
    /// CLI package update, kept separate because it is applied first and is not
    /// recorded as an ANOLISA component object (the binary is owned by rpm).
    cli: Option<PlannedUpdate>,
    updates: Vec<PlannedUpdate>,
    installs: Vec<PlannedInstall>,
    skipped: Vec<SkippedItem>,
    errors: Vec<PlanError>,
}

impl UpgradePlan {
    /// Whether the plan carries an item error. Real execution aborts before any
    /// `dnf` transaction when this is true.
    fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// Convert a check report's CLI + component rows into an [`UpgradePlan`].
///
/// This is a pure classification with no host access: it maps the check's action
/// vocabulary onto upgrade intents (see issue #1411 §"Plan conversion"). An
/// `install` row that carries no resolved RPM package becomes an item **error**
/// (it cannot be applied) rather than a silent no-op; unsupported/raw-managed
/// rows become skips and never abort.
fn build_plan(
    target: Option<String>,
    cli: &CliCheck,
    components: &[ComponentCheck],
) -> UpgradePlan {
    let mut plan = UpgradePlan {
        target,
        ..UpgradePlan::default()
    };

    // ── CLI ──
    match cli.action.as_str() {
        ACTION_UPDATE => match cli_update(cli) {
            Ok(update) => plan.cli = Some(update),
            Err(reason) => plan.errors.push(PlanError {
                name: cli_name(cli),
                reason,
            }),
        },
        ACTION_NOOP => {}
        ACTION_UNSUPPORTED => plan.skipped.push(SkippedItem {
            name: cli_name(cli),
            reason: cli.error.clone().unwrap_or_else(|| {
                "anolisa CLI is not RPM-managed; use `anolisa update self`".to_string()
            }),
        }),
        ACTION_ERROR => plan.errors.push(PlanError {
            name: cli_name(cli),
            reason: cli
                .error
                .clone()
                .unwrap_or_else(|| "CLI upgrade check failed".to_string()),
        }),
        // Any other/unknown action is treated as nothing to do.
        _ => {}
    }

    // ── components ──
    for component in components {
        match component.action.as_str() {
            ACTION_UPDATE => match component_update(component) {
                Ok(update) => plan.updates.push(update),
                Err(reason) => plan.errors.push(PlanError {
                    name: component.component.clone(),
                    reason,
                }),
            },
            ACTION_INSTALL => match component.package.clone() {
                Some(package) => plan.installs.push(PlannedInstall {
                    name: component.component.clone(),
                    package,
                }),
                // A default with no resolved RPM package cannot be installed.
                // The check leaves `package = None` when the component index has
                // no unambiguous mapping; refuse rather than guess a name.
                None => plan.errors.push(PlanError {
                    name: component.component.clone(),
                    reason: format!(
                        "no RPM package could be resolved for default component '{}'; cannot install it (repo component index is missing or ambiguous)",
                        component.component
                    ),
                }),
            },
            ACTION_UNSUPPORTED_RPM => plan.skipped.push(SkippedItem {
                name: component.component.clone(),
                reason: RAW_SKIP_REASON.to_string(),
            }),
            ACTION_NOOP => {}
            ACTION_ERROR => plan.errors.push(PlanError {
                name: component.component.clone(),
                reason: component
                    .error
                    .clone()
                    .unwrap_or_else(|| "component upgrade check failed".to_string()),
            }),
            _ => {}
        }
    }

    plan
}

/// Human/JSON name for the CLI row (its RPM package, falling back to `anolisa`).
fn cli_name(cli: &CliCheck) -> String {
    cli.package.clone().unwrap_or_else(|| "anolisa".to_string())
}

/// Build the CLI [`PlannedUpdate`], or an error string when the check row is
/// missing the fields an update needs.
fn cli_update(cli: &CliCheck) -> Result<PlannedUpdate, String> {
    let package = cli
        .package
        .clone()
        .ok_or_else(|| "CLI update reported without an RPM package".to_string())?;
    let from = cli
        .installed
        .clone()
        .ok_or_else(|| "CLI update reported without an installed version".to_string())?;
    let to = cli
        .available
        .clone()
        .ok_or_else(|| "CLI update reported without a candidate version".to_string())?;
    Ok(PlannedUpdate {
        name: package.clone(),
        package,
        from,
        to,
    })
}

/// Build a component [`PlannedUpdate`], or an error string when the check row is
/// missing the fields an update needs.
fn component_update(component: &ComponentCheck) -> Result<PlannedUpdate, String> {
    let package = component
        .package
        .clone()
        .ok_or_else(|| "update reported without an RPM package".to_string())?;
    let from = component
        .installed
        .clone()
        .ok_or_else(|| "update reported without an installed version".to_string())?;
    let to = component
        .available
        .clone()
        .ok_or_else(|| "update reported without a candidate version".to_string())?;
    Ok(PlannedUpdate {
        name: component.component.clone(),
        package,
        from,
        to,
    })
}

// ── result envelope ──────────────────────────────────────────────────────────

/// Wire shape for `anolisa upgrade` (`--json`) and the human summary source.
#[derive(Debug, Serialize)]
struct UpgradeResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    backend: &'static str,
    status: &'static str,
    dry_run: bool,
    updated: Vec<UpdatedItem>,
    installed: Vec<InstalledItem>,
    skipped: Vec<SkippedResult>,
    errors: Vec<ErrorResult>,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct UpdatedItem {
    name: String,
    package: String,
    from: String,
    to: String,
}

#[derive(Debug, Serialize)]
struct InstalledItem {
    name: String,
    package: String,
    /// Installed EVR after `dnf install`; `None` on a dry-run preview.
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
}

#[derive(Debug, Serialize)]
struct SkippedResult {
    name: String,
    reason: String,
}

#[derive(Debug, Serialize)]
struct ErrorResult {
    name: String,
    reason: String,
}

// ── apply ──────────────────────────────────────────────────────────────────

/// A component update whose dnf transaction and rpmdb re-read both succeeded,
/// awaiting the state save. The CLI update is intentionally not modelled here:
/// the CLI binary is rpm-owned and not tracked as an ANOLISA component object,
/// so it is reported but never written to `installed.toml` (self-update owns
/// that concern).
struct PendingUpdate {
    name: String,
    package: String,
    /// EVR before the upgrade, carried through so the result can show `from → to`.
    from: String,
    /// Post-transaction rpmdb truth used to refresh recorded version/EVR/arch.
    refreshed: PackageInfo,
}

/// A newly installed default whose dnf transaction and rpmdb re-read both
/// succeeded, awaiting the state save.
struct PendingInstall {
    name: String,
    package: String,
    refreshed: PackageInfo,
}

/// Outcome of the single state save: the items it actually recorded (reported as
/// updated/installed) plus per-item drift errors for changes it refused to make.
#[derive(Default)]
struct PersistOutcome {
    updated: Vec<UpdatedItem>,
    installed: Vec<InstalledItem>,
    errors: Vec<ErrorResult>,
}

/// Core of [`handle`] with the package query, transaction, root status, and
/// dry-run flag injected so tests drive the whole apply path without a live
/// rpmdb/dnf or real privileges.
///
/// Dry-run renders the plan (and any item errors) without touching the host.
/// Real execution requires root, aborts before any `dnf` transaction if the plan
/// carries errors, then applies CLI update → component updates → missing-default
/// installs, refreshing ANOLISA state from rpmdb once at the end.
#[allow(clippy::too_many_arguments)]
fn run_upgrade_with_deps(
    ctx: &CliContext,
    layout: &FsLayout,
    plan: &UpgradePlan,
    query: &dyn PackageQuery,
    txn: &dyn PackageTransaction,
    is_root: bool,
    dry_run: bool,
    command: &str,
) -> Result<UpgradeResult, CliError> {
    if dry_run {
        return Ok(render_plan_preview(plan));
    }

    // Real execution needs root for the dnf transactions. Check up front so the
    // operator gets an actionable message rather than dnf's raw refusal.
    if !is_root {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: "applying an RPM image upgrade requires root privileges; re-run with sudo: `sudo anolisa upgrade`".to_string(),
        });
    }

    // Any planning error blocks the whole run before a single dnf transaction.
    if plan.has_errors() {
        return Ok(blocked_result(plan));
    }

    let mut errors: Vec<ErrorResult> = Vec::new();
    // The CLI update is applied first and reported, but is never an ANOLISA
    // component object (the binary is rpm-owned). Held separately so the audit
    // step can count it toward the outcome even on a CLI-only upgrade.
    let mut cli_updated: Option<UpdatedItem> = None;
    // Component updates / installs whose dnf transaction and rpmdb re-read both
    // succeeded, held back until the single state save can confirm they landed.
    // Only items the state write actually records are reported as
    // updated/installed — a transaction that mutated rpmdb but could not be
    // reflected in ANOLISA state must surface as an error, never a silent `ok`.
    let mut pending_updates: Vec<PendingUpdate> = Vec::new();
    let mut pending_installs: Vec<PendingInstall> = Vec::new();

    // 1. CLI package (rpm-owned binary). Reported but never recorded as a
    //    component object, so it is not gated on the state save.
    if let Some(cli) = &plan.cli {
        match txn.update(&cli.package) {
            Ok(()) => match refresh_evr(query, &cli.package) {
                Ok(to) => {
                    cli_updated = Some(UpdatedItem {
                        name: cli.name.clone(),
                        package: cli.package.clone(),
                        from: cli.from.clone(),
                        to,
                    })
                }
                Err(reason) => errors.push(ErrorResult {
                    name: cli.name.clone(),
                    reason,
                }),
            },
            Err(err) => errors.push(ErrorResult {
                name: cli.name.clone(),
                reason: txn_error_reason(err),
            }),
        }
    }

    // 2. Already-installed RPM-backed components.
    for update in &plan.updates {
        match txn.update(&update.package) {
            Ok(()) => match query.query_installed(&update.package) {
                Ok(Some(info)) => pending_updates.push(PendingUpdate {
                    name: update.name.clone(),
                    package: update.package.clone(),
                    from: update.from.clone(),
                    refreshed: info,
                }),
                Ok(None) => errors.push(ErrorResult {
                    name: update.name.clone(),
                    reason: format!(
                        "dnf upgraded '{}' but it is no longer in rpmdb under that name; run `anolisa repair {}`",
                        update.package, update.name
                    ),
                }),
                Err(err) => errors.push(ErrorResult {
                    name: update.name.clone(),
                    reason: format!(
                        "dnf upgraded '{}' but reading the new version failed ({err}); run `anolisa repair {}`",
                        update.package, update.name
                    ),
                }),
            },
            Err(err) => errors.push(ErrorResult {
                name: update.name.clone(),
                reason: txn_error_reason(err),
            }),
        }
    }

    // 3. Missing default components.
    for install in &plan.installs {
        match txn.install(&install.package) {
            Ok(()) => match query.query_installed(&install.package) {
                Ok(Some(info)) => pending_installs.push(PendingInstall {
                    name: install.name.clone(),
                    package: install.package.clone(),
                    refreshed: info,
                }),
                Ok(None) => errors.push(ErrorResult {
                    name: install.name.clone(),
                    reason: format!(
                        "dnf installed '{}' but it is not present in rpmdb; state was not recorded",
                        install.package
                    ),
                }),
                Err(err) => errors.push(ErrorResult {
                    name: install.name.clone(),
                    reason: format!(
                        "dnf installed '{}' but reading its version failed ({err}); state was not recorded",
                        install.package
                    ),
                }),
            },
            Err(err) => errors.push(ErrorResult {
                name: install.name.clone(),
                reason: txn_error_reason(err),
            }),
        }
    }

    // 4. Refresh ANOLISA state and write the durable audit under the install
    //    lock. This runs whenever any dnf transaction was attempted — including
    //    a CLI-only upgrade with no component changes — so a real system
    //    mutation always leaves an operation record and central-log entry, never
    //    a silent `ok`. Persistence reloads state and validates each component
    //    against it, so a drifted item (vanished, changed RPM identity, or
    //    already present under a different backend) is reported as an error
    //    rather than reported updated/installed or silently overwriting a
    //    non-RPM record. Only items the save actually recorded come back as
    //    updated/installed.
    let mut updated: Vec<UpdatedItem> = Vec::new();
    let mut installed: Vec<InstalledItem> = Vec::new();
    let attempted_transaction =
        plan.cli.is_some() || !plan.updates.is_empty() || !plan.installs.is_empty();
    if attempted_transaction {
        // The transaction-phase errors (dnf/refresh failures) and the CLI
        // success are passed in so the persisted operation/audit record reflects
        // the true `ok`/`partial`/`failed` outcome of the whole command.
        let outcome = finalize_upgrade(
            ctx,
            layout,
            command,
            cli_updated.as_ref(),
            &pending_updates,
            &pending_installs,
            errors.len(),
        )?;
        if let Some(item) = cli_updated {
            updated.push(item);
        }
        updated.extend(outcome.updated);
        installed.extend(outcome.installed);
        errors.extend(outcome.errors);
    }

    let succeeded = updated.len() + installed.len();
    let status = apply_status(succeeded, errors.len());

    Ok(UpgradeResult {
        target: plan.target.clone(),
        backend: BACKEND,
        status,
        dry_run: false,
        updated,
        installed,
        skipped: skipped_results(plan),
        errors,
        warnings: Vec::new(),
    })
}

/// Status for a completed real run: `ok` when nothing errored, `partial` when
/// some items succeeded and some failed, `failed` when nothing succeeded.
fn apply_status(succeeded: usize, error_count: usize) -> &'static str {
    if error_count == 0 {
        STATUS_OK
    } else if succeeded > 0 {
        STATUS_PARTIAL
    } else {
        STATUS_FAILED
    }
}

/// Re-read the installed EVR of `package` after a transaction, mapping the
/// "gone/duplicate/query-failed" branches to a human reason string.
fn refresh_evr(query: &dyn PackageQuery, package: &str) -> Result<String, String> {
    match query.query_installed(package) {
        Ok(Some(info)) => Ok(info.version.to_string()),
        Ok(None) => Err(format!(
            "dnf upgraded '{package}' but it is no longer in rpmdb under that name"
        )),
        Err(err) => Err(format!(
            "dnf upgraded '{package}' but reading the new version failed: {err}"
        )),
    }
}

/// Human-readable reason for a failed `dnf` transaction.
fn txn_error_reason(err: PackageTransactionError) -> String {
    match err {
        PackageTransactionError::CommandMissing { command } => {
            format!("{command} not found; cannot run the RPM transaction")
        }
        PackageTransactionError::PermissionDenied { command } => {
            format!("permission denied running {command}; re-run with sudo")
        }
        PackageTransactionError::TransactionFailed {
            operation,
            code,
            stderr,
            ..
        } => format!(
            "dnf {operation} failed (exit {}): {}",
            code.map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string()),
            stderr.trim(),
        ),
    }
}

/// Build the dry-run preview result from the plan (no host access).
fn render_plan_preview(plan: &UpgradePlan) -> UpgradeResult {
    let mut updated: Vec<UpdatedItem> = Vec::new();
    if let Some(cli) = &plan.cli {
        updated.push(planned_to_updated(cli));
    }
    updated.extend(plan.updates.iter().map(planned_to_updated));

    let installed = plan
        .installs
        .iter()
        .map(|install| InstalledItem {
            name: install.name.clone(),
            package: install.package.clone(),
            version: None,
        })
        .collect();

    // A dry-run whose plan carries errors would be blocked if applied for real;
    // report that so automation can gate on it, otherwise `ok`.
    let status = if plan.has_errors() {
        STATUS_BLOCKED
    } else {
        STATUS_OK
    };

    UpgradeResult {
        target: plan.target.clone(),
        backend: BACKEND,
        status,
        dry_run: true,
        updated,
        installed,
        skipped: skipped_results(plan),
        errors: plan_errors(plan),
        warnings: Vec::new(),
    }
}

/// Build a `blocked` result: the plan carried errors, so no dnf ran.
fn blocked_result(plan: &UpgradePlan) -> UpgradeResult {
    UpgradeResult {
        target: plan.target.clone(),
        backend: BACKEND,
        status: STATUS_BLOCKED,
        dry_run: false,
        updated: Vec::new(),
        installed: Vec::new(),
        skipped: skipped_results(plan),
        errors: plan_errors(plan),
        warnings: Vec::new(),
    }
}

fn planned_to_updated(update: &PlannedUpdate) -> UpdatedItem {
    UpdatedItem {
        name: update.name.clone(),
        package: update.package.clone(),
        from: update.from.clone(),
        to: update.to.clone(),
    }
}

fn skipped_results(plan: &UpgradePlan) -> Vec<SkippedResult> {
    plan.skipped
        .iter()
        .map(|item| SkippedResult {
            name: item.name.clone(),
            reason: item.reason.clone(),
        })
        .collect()
}

fn plan_errors(plan: &UpgradePlan) -> Vec<ErrorResult> {
    plan.errors
        .iter()
        .map(|item| ErrorResult {
            name: item.name.clone(),
            reason: item.reason.clone(),
        })
        .collect()
}

// ── state persistence + audit ────────────────────────────────────────────────

/// Refresh ANOLISA state for the successful component transactions and write the
/// durable audit (operation record + central log) under the install lock.
///
/// Audit is decoupled from component-state changes: because a real dnf
/// transaction may have run even when nothing lands in `installed.toml` (a
/// CLI-only upgrade, or a run where every component drifted/failed), this always
/// appends an operation record and central-log entry whenever any transaction
/// was attempted — so a real system mutation is never left unaudited. The
/// recorded status is the true `ok` / `partial` / `failed` outcome of the whole
/// command (`cli_updated` and `prior_errors` from the transaction phase are
/// folded in), not just the component-persistence result.
///
/// State is reloaded under the lock and each pending change is re-validated
/// against it, because the plan was computed before the lock was held and before
/// the dnf transactions ran — the on-disk state may have drifted in between:
/// - **updates**: the component must still exist and still be the same RPM
///   package (mirrors the single-component update guard); otherwise the new EVR
///   is not grafted on and the item is reported as an error.
/// - **installs**: a name absent from state is inserted as rpm-managed; a name
///   already present under the *same* RPM package is refreshed idempotently; a
///   name present under a **different** backend/package (e.g. a raw-managed
///   component, or a different RPM) is left untouched and reported as an error,
///   so a concurrent install can never be silently overwritten with an
///   rpm-managed record.
///
/// Only the changes actually recorded are returned as updated/installed; every
/// refused change is returned as an error so the caller downgrades the overall
/// status rather than claiming a clean `ok` while state is stale. RPM-owned
/// files are never recorded — dnf owns the file transaction.
fn finalize_upgrade(
    ctx: &CliContext,
    layout: &FsLayout,
    command: &str,
    cli_updated: Option<&UpdatedItem>,
    updates: &[PendingUpdate],
    installs: &[PendingInstall],
    prior_errors: usize,
) -> Result<PersistOutcome, CliError> {
    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;
    let mut state = common::load_installed_state(ctx, command)?;

    let started_at = now_iso8601();
    let lock_ts = Utc::now();
    let operation_id = format!(
        "op-upgrade-{}-{}",
        lock_ts.format("%Y%m%d%H%M%S"),
        lock_ts.timestamp_subsec_nanos()
    );

    // Upgrade only runs in system mode; keep the state scope consistent with the
    // install/adopt paths so a fresh state file records the right mode/prefix.
    state.install_mode = StateInstallMode::System;
    state.prefix = layout.prefix.clone();

    let mut outcome = PersistOutcome::default();

    for update in updates {
        let evr = update.refreshed.version.to_string();
        let Some(obj) = state.find_object_mut(ObjectKind::Component, &update.name) else {
            outcome.errors.push(ErrorResult {
                name: update.name.clone(),
                reason: format!(
                    "dnf upgraded '{}' but component '{}' vanished from ANOLISA state during the upgrade; run `anolisa repair {}` to refresh it",
                    update.package, update.name, update.name
                ),
            });
            continue;
        };
        // Refuse to graft the new EVR onto a row that is no longer this RPM
        // package (a concurrent backend change), mirroring the single-component
        // update guard.
        let matches = obj
            .rpm_metadata
            .as_ref()
            .is_some_and(|m| m.package_name == update.package)
            && obj.effective_ownership().is_rpm();
        if !matches {
            outcome.errors.push(ErrorResult {
                name: update.name.clone(),
                reason: format!(
                    "dnf upgraded '{}' but component '{}' changed ownership/package in ANOLISA state during the upgrade; state was not refreshed — run `anolisa repair {}`",
                    update.package, update.name, update.name
                ),
            });
            continue;
        }
        obj.version = evr.clone();
        obj.last_operation_id = Some(operation_id.clone());
        if let Some(meta) = obj.rpm_metadata.as_mut() {
            meta.evr = Some(evr.clone());
            meta.arch = Some(update.refreshed.arch.clone());
        }
        outcome.updated.push(UpdatedItem {
            name: update.name.clone(),
            package: update.package.clone(),
            from: update.from.clone(),
            to: evr,
        });
    }

    for install in installs {
        let evr = install.refreshed.version.to_string();
        // Classify the current state slot with an immutable borrow that is
        // dropped before any mutation below.
        let slot = match state.find_object(ObjectKind::Component, &install.name) {
            None => InstallSlot::Absent,
            Some(existing) => {
                let same_rpm = existing
                    .rpm_metadata
                    .as_ref()
                    .is_some_and(|m| m.package_name == install.package)
                    && existing.effective_ownership().is_rpm();
                if same_rpm {
                    InstallSlot::MatchingRpm
                } else {
                    InstallSlot::Conflict(existing.effective_ownership().label())
                }
            }
        };

        match slot {
            InstallSlot::Absent => {
                state.upsert_object(new_rpm_component(
                    &install.name,
                    &install.package,
                    &evr,
                    &install.refreshed,
                    &started_at,
                    &operation_id,
                ));
                outcome.installed.push(InstalledItem {
                    name: install.name.clone(),
                    package: install.package.clone(),
                    version: Some(evr),
                });
            }
            // A row already present under the same RPM package: treat the
            // install as an idempotent refresh rather than a duplicate insert.
            InstallSlot::MatchingRpm => {
                if let Some(obj) = state.find_object_mut(ObjectKind::Component, &install.name) {
                    obj.version = evr.clone();
                    obj.last_operation_id = Some(operation_id.clone());
                    if let Some(meta) = obj.rpm_metadata.as_mut() {
                        meta.evr = Some(evr.clone());
                        meta.arch = Some(install.refreshed.arch.clone());
                    }
                }
                outcome.installed.push(InstalledItem {
                    name: install.name.clone(),
                    package: install.package.clone(),
                    version: Some(evr),
                });
            }
            // A row present under a different backend/package: never overwrite
            // it (that would erase raw-managed or foreign-RPM provenance).
            InstallSlot::Conflict(existing_ownership) => {
                outcome.errors.push(ErrorResult {
                    name: install.name.clone(),
                    reason: format!(
                        "dnf installed '{}' but a {existing_ownership} component named '{}' already exists in ANOLISA state; refusing to overwrite it with an rpm-managed record — run `anolisa status {}`",
                        install.package, install.name, install.name
                    ),
                });
            }
        }
    }

    // Count the whole command's outcome, not just component state: the CLI
    // update (not an ANOLISA object) and the transaction-phase errors are folded
    // in so the durable record shows the true `ok` / `partial` / `failed`.
    let total_success =
        cli_updated.is_some() as usize + outcome.updated.len() + outcome.installed.len();
    let total_errors = prior_errors + outcome.errors.len();

    if total_success == 0 && total_errors == 0 {
        // No transaction actually happened (e.g. only skips/noops reached here);
        // nothing to audit and nothing to persist.
        return Ok(outcome);
    }

    let status = apply_status(total_success, total_errors);
    let (log_status, severity) = match status {
        STATUS_OK => (LogStatus::Ok, Severity::Info),
        STATUS_PARTIAL => (LogStatus::Partial, Severity::Warn),
        _ => (LogStatus::Failed, Severity::Error),
    };

    // Always append the operation record and save, even when no component object
    // changed (a CLI-only upgrade, or a run where every component drifted): the
    // record is what makes a real system mutation auditable via `anolisa logs`.
    state.operations.push(OperationRecord {
        id: operation_id.clone(),
        command: command.to_string(),
        status: status.to_string(),
        started_at: started_at.clone(),
        finished_at: Some(now_iso8601()),
    });

    let state_path = layout.state_dir.join("installed.toml");
    state.save(&state_path).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to save state: {err}"),
    })?;

    // Audit log is best-effort: state already persisted, so a log failure
    // downgrades to a stderr warning rather than unwinding.
    let recorded = outcome.updated.len() + outcome.installed.len();
    let log = CentralLog::open(layout.central_log.clone());
    let mut objects: Vec<String> = cli_updated.map(|c| c.package.clone()).into_iter().collect();
    objects.extend(outcome.updated.iter().map(|u| u.name.clone()));
    objects.extend(outcome.installed.iter().map(|i| i.name.clone()));
    let record = LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id),
        command: command.to_string(),
        source: "anolisa-cli".to_string(),
        component: None,
        severity,
        message: format!(
            "applied RPM image upgrade ({total_success} succeeded, {recorded} component state change(s) recorded, {total_errors} error(s))"
        ),
        actor: "cli".to_string(),
        install_mode: Some(ctx.install_mode.as_str().to_string()),
        started_at,
        finished_at: Some(now_iso8601()),
        status: Some(log_status),
        objects,
        backup_ids: Vec::new(),
        warnings: Vec::new(),
        details: serde_json::Value::Null,
    };
    if let Err(err) = log.append(&record)
        && !ctx.quiet
    {
        eprintln!("warning: failed to write central log: {err}");
    }

    Ok(outcome)
}

/// The current ANOLISA-state slot for a to-be-installed default, classified
/// under the install lock so the install decision cannot race a concurrent write.
enum InstallSlot {
    /// No object with this name exists — a clean insert.
    Absent,
    /// An object exists under the same RPM package — an idempotent refresh.
    MatchingRpm,
    /// An object exists under a different backend/package (carries its ownership
    /// label for the error) — must not be overwritten.
    Conflict(&'static str),
}

/// Build a fresh rpm-managed component object for a newly installed default.
/// Mirrors the delegated-install path: `managed = true`, `adopted = false`,
/// ownership [`Ownership::RpmManaged`], backend `rpm`, and no owned files (dnf
/// owns the file transaction).
fn new_rpm_component(
    name: &str,
    package: &str,
    evr: &str,
    refreshed: &PackageInfo,
    installed_at: &str,
    operation_id: &str,
) -> InstalledObject {
    InstalledObject {
        kind: ObjectKind::Component,
        name: name.to_string(),
        version: evr.to_string(),
        status: ObjectStatus::Installed,
        manifest_digest: None,
        distribution_source: None,
        raw_package: None,
        install_backend: Some("rpm".to_string()),
        ownership: Some(Ownership::RpmManaged),
        rpm_metadata: Some(RpmMetadata {
            package_name: package.to_string(),
            evr: Some(evr.to_string()),
            arch: Some(refreshed.arch.clone()),
            source_repo: refreshed.origin.clone(),
        }),
        installed_at: installed_at.to_string(),
        last_operation_id: Some(operation_id.to_string()),
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

/// RFC3339 UTC timestamp, seconds precision (matches the install/update paths).
fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

// ── rendering ────────────────────────────────────────────────────────────────

/// Render the upgrade result as JSON (envelope) or a human summary.
fn render_result(ctx: &CliContext, result: &UpgradeResult) {
    if ctx.json {
        // `ok` reflects the status so machine callers can gate on the envelope
        // flag as well as the inner `status` field; a serialize failure is
        // ignored so an already-applied upgrade is not reported as failed.
        let _ = response::render_json_with_status(COMMAND, result.status == STATUS_OK, result);
        return;
    }
    if ctx.quiet {
        return;
    }
    let color = Palette::new(ctx.no_color);

    let mut header = format!("upgrade (backend: {}", result.backend);
    if let Some(target) = &result.target {
        header.push_str(&format!(", target: {target}"));
    }
    header.push_str(if result.dry_run {
        ") (dry-run — nothing applied)"
    } else {
        ")"
    });
    println!("{}", color.command(header));

    for item in &result.updated {
        let verb = if result.dry_run {
            "would update"
        } else {
            "updated"
        };
        println!(
            "  {} {} {} → {} {}",
            color.ok(format!("✓ {verb}")),
            item.name,
            item.from,
            item.to,
            color.muted(format!("({})", item.package)),
        );
    }
    for item in &result.installed {
        let verb = if result.dry_run {
            "would install".to_string()
        } else {
            format!("installed {}", item.version.as_deref().unwrap_or("-"))
        };
        println!(
            "  {} {} {}",
            color.ok(format!("✓ {verb}")),
            item.name,
            color.muted(format!("({})", item.package)),
        );
    }
    for item in &result.skipped {
        println!(
            "  {} {} {}",
            color.muted("• skipped"),
            item.name,
            color.muted(format!("({})", item.reason)),
        );
    }
    for item in &result.errors {
        println!(
            "  {} {} {}",
            color.err("✗ error"),
            item.name,
            color.warn(format!("({})", item.reason)),
        );
    }

    println!(
        "{} {} updated, {} installed, {} skipped, {} error(s) [{}]",
        color.label("summary:"),
        result.updated.len(),
        result.installed.len(),
        result.skipped.len(),
        result.errors.len(),
        result.status,
    );

    for warning in &result.warnings {
        eprintln!("{} {warning}", color.warn("warning:"));
    }
}

#[cfg(test)]
mod tests;
