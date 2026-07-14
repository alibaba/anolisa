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
    InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectKind, ObjectStatus,
    OperationRecord, Ownership, RpmMetadata,
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
use crate::commands::tier1::rpm_install;
use crate::context::{CliContext, InstallMode};
use crate::progress::{self, Activity, ProgressReporter};
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

    // A single activity spinner spans both planning and the apply transactions,
    // so a slow repo query or `dnf` transaction never looks like a hung process.
    // It is dropped (clearing the line) before `render_result` prints the final
    // structured output, and on every early-return path via RAII.
    let feedback = progress::feedback_for_stderr(ctx.json, ctx.quiet);
    let activity = Activity::start(feedback, "Planning upgrade...");

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
        &activity,
    )?;

    // Clear the spinner before the final result is rendered to stdout.
    drop(activity);
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
    /// Whether a missing state row should be recorded as an observed RPM default.
    adopt_if_missing: bool,
}

/// A missing default component the upgrade will `dnf install`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedInstall {
    name: String,
    package: String,
}

/// A target default already present in rpmdb but absent from ANOLISA state, with
/// no dnf transaction needed. Upgrade records it as observed state.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedObservedDefault {
    name: String,
    package: String,
    installed: String,
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
    observed_defaults: Vec<PlannedObservedDefault>,
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
            ACTION_NOOP => {
                if component.absent_from_state {
                    match observed_default(component) {
                        Ok(observed) => plan.observed_defaults.push(observed),
                        Err(reason) => plan.errors.push(PlanError {
                            name: component.component.clone(),
                            reason,
                        }),
                    }
                }
            }
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
        adopt_if_missing: false,
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
        adopt_if_missing: component.absent_from_state,
    })
}

/// Build a no-transaction observed-default record plan, or an error string when
/// the check row is missing the fields needed to refresh rpmdb/state.
fn observed_default(component: &ComponentCheck) -> Result<PlannedObservedDefault, String> {
    let package = component
        .package
        .clone()
        .ok_or_else(|| "observed default reported without an RPM package".to_string())?;
    let installed = component
        .installed
        .clone()
        .ok_or_else(|| "observed default reported without an installed version".to_string())?;
    Ok(PlannedObservedDefault {
        name: component.component.clone(),
        package,
        installed,
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
    recorded: Vec<RecordedItem>,
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
struct RecordedItem {
    name: String,
    package: String,
    /// Installed EVR recorded into ANOLISA state; `None` on a dry-run preview.
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

/// A component state refresh awaiting the state save. Most rows follow a
/// successful dnf update; already-current observed defaults use the same shape
/// after a read-only rpmdb refresh. The CLI update is intentionally not modelled
/// here because the binary is not an ANOLISA component object.
struct PendingUpdate {
    name: String,
    package: String,
    /// EVR before the upgrade, carried through so the result can show `from → to`.
    from: String,
    /// Post-transaction rpmdb truth used to refresh recorded version/EVR/arch.
    refreshed: PackageInfo,
    /// Installed package source repository, if rpm/dnf can report it separately.
    source_repo: Option<String>,
    /// True for a target default that was present in rpmdb but absent from state.
    adopt_if_missing: bool,
    /// True when no package update occurred and the item should be reported as a
    /// state recording rather than an update.
    record_only: bool,
}

/// A newly installed default whose dnf transaction and rpmdb re-read both
/// succeeded, awaiting the state save.
struct PendingInstall {
    name: String,
    package: String,
    refreshed: PackageInfo,
    source_repo: Option<String>,
}

/// Outcome of the single state save: the items it actually recorded (reported as
/// updated/installed) plus per-item drift errors for changes it refused to make.
#[derive(Default)]
struct PersistOutcome {
    updated: Vec<UpdatedItem>,
    installed: Vec<InstalledItem>,
    recorded: Vec<RecordedItem>,
    errors: Vec<ErrorResult>,
}

#[derive(Default)]
struct AuthorizedPlan<'a> {
    updates: Vec<&'a PlannedUpdate>,
    installs: Vec<&'a PlannedInstall>,
    observed_defaults: Vec<&'a PlannedObservedDefault>,
    errors: Vec<ErrorResult>,
}

struct UpgradeAudit {
    started_at: String,
    operation_id: String,
}

struct FinalizeUpgrade<'a> {
    ctx: &'a CliContext,
    layout: &'a FsLayout,
    command: &'a str,
    state: &'a mut InstalledState,
    audit: &'a UpgradeAudit,
    cli_updated: Option<&'a UpdatedItem>,
    updates: &'a [PendingUpdate],
    installs: &'a [PendingInstall],
    prior_errors: usize,
    warnings: &'a [String],
}

fn new_upgrade_audit() -> UpgradeAudit {
    let lock_ts = Utc::now();
    UpgradeAudit {
        started_at: now_iso8601(),
        operation_id: format!(
            "op-upgrade-{}-{}",
            lock_ts.format("%Y%m%d%H%M%S"),
            lock_ts.timestamp_subsec_nanos()
        ),
    }
}

/// Revalidate the plan against the locked ANOLISA state before any RPM
/// transaction runs. Only authorized items are handed to dnf.
fn authorize_plan<'a>(state: &InstalledState, plan: &'a UpgradePlan) -> AuthorizedPlan<'a> {
    let mut authorized = AuthorizedPlan::default();

    for update in &plan.updates {
        match state.find_object(ObjectKind::Component, &update.name) {
            Some(obj) if is_matching_rpm_object(obj, &update.package) => {
                authorized.updates.push(update);
            }
            Some(obj) => authorized.errors.push(ErrorResult {
                name: update.name.clone(),
                reason: format!(
                    "component '{}' is now {} in ANOLISA state; refusing to run dnf update for '{}'",
                    update.name,
                    obj.effective_ownership().label(),
                    update.package
                ),
            }),
            None if update.adopt_if_missing => authorized.updates.push(update),
            None => authorized.errors.push(ErrorResult {
                name: update.name.clone(),
                reason: format!(
                    "component '{}' is no longer present in ANOLISA state; refusing to run dnf update for '{}'",
                    update.name, update.package
                ),
            }),
        }
    }

    for install in &plan.installs {
        match classify_install_slot(state, &install.name, &install.package) {
            InstallSlot::Absent | InstallSlot::MatchingRpm => authorized.installs.push(install),
            InstallSlot::Conflict(existing_ownership) => authorized.errors.push(ErrorResult {
                name: install.name.clone(),
                reason: format!(
                    "component '{}' already exists as {existing_ownership} in ANOLISA state; refusing to run dnf install for '{}'",
                    install.name, install.package
                ),
            }),
        }
    }

    for observed in &plan.observed_defaults {
        match state.find_object(ObjectKind::Component, &observed.name) {
            Some(obj) if is_matching_rpm_object(obj, &observed.package) => {
                authorized.observed_defaults.push(observed);
            }
            Some(obj) => authorized.errors.push(ErrorResult {
                name: observed.name.clone(),
                reason: format!(
                    "default component '{}' already exists as {} in ANOLISA state; refusing to record '{}' as rpm-observed",
                    observed.name,
                    obj.effective_ownership().label(),
                    observed.package
                ),
            }),
            None => authorized.observed_defaults.push(observed),
        }
    }

    authorized
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
    reporter: &dyn ProgressReporter,
) -> Result<UpgradeResult, CliError> {
    let preview_state = common::load_installed_state(ctx, command)?;
    reject_upgrade_pending_claims(layout, &preview_state, plan, command)?;

    if dry_run {
        // Dry-run only planned; it never applies a transaction, so it reports no
        // apply-phase progress (planning feedback is handled by the caller).
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
    let mut warnings: Vec<String> = Vec::new();
    let mut updated: Vec<UpdatedItem> = Vec::new();
    let mut installed: Vec<InstalledItem> = Vec::new();
    let mut recorded: Vec<RecordedItem> = Vec::new();
    let needs_finalize = plan.cli.is_some()
        || !plan.updates.is_empty()
        || !plan.installs.is_empty()
        || !plan.observed_defaults.is_empty();
    if needs_finalize {
        let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!("failed to acquire install lock: {err}"),
        })?;
        let mut state = common::load_installed_state(ctx, command)?;
        reject_upgrade_pending_claims(layout, &state, plan, command)?;
        let audit = new_upgrade_audit();

        // Upgrade only runs in system mode; keep the state scope consistent with
        // install/adopt so a fresh state file records the right mode/prefix.
        state.install_mode = StateInstallMode::System;
        state.prefix = layout.prefix.clone();

        let authorized = authorize_plan(&state, plan);
        errors.extend(authorized.errors);

        // Reliable total for the `i/total` counter: only the items that actually
        // run a `dnf` transaction (CLI update + authorized component updates and
        // installs). Observed-default recording touches no transaction and is
        // folded into the finalize phase, so it is deliberately excluded — the
        // counter never advertises work that has no `dnf` step.
        let transaction_total =
            plan.cli.is_some() as usize + authorized.updates.len() + authorized.installs.len();
        let mut transaction_step = 0usize;

        // The CLI update is applied first and reported, but is never an ANOLISA
        // component object (the binary is rpm-owned). Held separately so the
        // audit step can count it toward the outcome even on a CLI-only upgrade.
        let mut cli_updated: Option<UpdatedItem> = None;
        // Component updates / installs whose dnf transaction and rpmdb re-read
        // both succeeded, held back until the single state save records them.
        let mut pending_updates: Vec<PendingUpdate> = Vec::new();
        let mut pending_installs: Vec<PendingInstall> = Vec::new();

        // 1. CLI package (rpm-owned binary). Reported but never recorded as a
        //    component object, so it is not gated on component state.
        if let Some(cli) = &plan.cli {
            transaction_step += 1;
            reporter.report(&format!(
                "Upgrading {} ({transaction_step}/{transaction_total})...",
                cli.name
            ));
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

        // 2. Already-installed RPM-backed components authorized by locked state.
        for update in authorized.updates {
            transaction_step += 1;
            reporter.report(&format!(
                "Upgrading {} ({transaction_step}/{transaction_total})...",
                update.name
            ));
            match txn.update(&update.package) {
                Ok(()) => match query.query_installed(&update.package) {
                    Ok(Some(info)) => {
                        let source_repo =
                            installed_origin_or_warn(query, &update.package, &mut warnings);
                        pending_updates.push(PendingUpdate {
                            name: update.name.clone(),
                            package: update.package.clone(),
                            from: update.from.clone(),
                            refreshed: info,
                            source_repo,
                            adopt_if_missing: update.adopt_if_missing,
                            record_only: false,
                        });
                    }
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

        // 3. Missing default components authorized by locked state.
        for install in authorized.installs {
            transaction_step += 1;
            reporter.report(&format!(
                "Installing {} ({transaction_step}/{transaction_total})...",
                install.name
            ));
            match txn.install(&install.package) {
                Ok(()) => match query.query_installed(&install.package) {
                    Ok(Some(info)) => {
                        let source_repo =
                            installed_origin_or_warn(query, &install.package, &mut warnings);
                        pending_installs.push(PendingInstall {
                            name: install.name.clone(),
                            package: install.package.clone(),
                            refreshed: info,
                            source_repo,
                        });
                    }
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

        // 4. Already-current target defaults authorized by locked state. No dnf
        //    transaction is needed; refresh rpmdb truth and record as observed.
        for observed in authorized.observed_defaults {
            match query.query_installed(&observed.package) {
                Ok(Some(info)) => {
                    let source_repo =
                        installed_origin_or_warn(query, &observed.package, &mut warnings);
                    pending_updates.push(PendingUpdate {
                        name: observed.name.clone(),
                        package: observed.package.clone(),
                        from: observed.installed.clone(),
                        refreshed: info,
                        source_repo,
                        adopt_if_missing: true,
                        record_only: true,
                    });
                }
                Ok(None) => errors.push(ErrorResult {
                    name: observed.name.clone(),
                    reason: format!(
                        "default component '{}' resolved to package '{}' but it is absent from rpmdb; state was not recorded",
                        observed.name, observed.package
                    ),
                }),
                Err(err) => errors.push(ErrorResult {
                    name: observed.name.clone(),
                    reason: format!(
                        "default component '{}' resolved to package '{}' but reading its version failed ({err}); state was not recorded",
                        observed.name, observed.package
                    ),
                }),
            }
        }

        // Refresh ANOLISA state and write the durable audit while the same lock
        // that authorized the RPM work is still held.
        reporter.report("Finalizing ANOLISA state...");
        let outcome = finalize_upgrade(FinalizeUpgrade {
            ctx,
            layout,
            command,
            state: &mut state,
            audit: &audit,
            cli_updated: cli_updated.as_ref(),
            updates: &pending_updates,
            installs: &pending_installs,
            prior_errors: errors.len(),
            warnings: &warnings,
        })?;
        if let Some(item) = cli_updated {
            updated.push(item);
        }
        updated.extend(outcome.updated);
        installed.extend(outcome.installed);
        recorded.extend(outcome.recorded);
        errors.extend(outcome.errors);
    }

    let succeeded = updated.len() + installed.len() + recorded.len();
    let status = apply_status(succeeded, errors.len());

    Ok(UpgradeResult {
        target: plan.target.clone(),
        backend: BACKEND,
        status,
        dry_run: false,
        updated,
        installed,
        recorded,
        skipped: skipped_results(plan),
        errors,
        warnings,
    })
}

fn reject_upgrade_pending_claims(
    layout: &FsLayout,
    state: &InstalledState,
    plan: &UpgradePlan,
    command: &str,
) -> Result<(), CliError> {
    for update in &plan.updates {
        rpm_install::reject_pending_claim(
            layout,
            state,
            &[update.name.as_str(), update.package.as_str()],
            command,
        )?;
    }
    for install in &plan.installs {
        rpm_install::reject_pending_claim(
            layout,
            state,
            &[install.name.as_str(), install.package.as_str()],
            command,
        )?;
    }
    for observed in &plan.observed_defaults {
        rpm_install::reject_pending_claim(
            layout,
            state,
            &[observed.name.as_str(), observed.package.as_str()],
            command,
        )?;
    }
    Ok(())
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

fn installed_origin_or_warn(
    query: &dyn PackageQuery,
    package: &str,
    warnings: &mut Vec<String>,
) -> Option<String> {
    match query.installed_origin(package) {
        Ok(origin) => origin,
        Err(err) => {
            warnings.push(format!(
                "could not determine source repo for '{package}': {err}"
            ));
            None
        }
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
    let recorded = plan
        .observed_defaults
        .iter()
        .map(|observed| RecordedItem {
            name: observed.name.clone(),
            package: observed.package.clone(),
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
        recorded,
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
        recorded: Vec::new(),
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

/// Refresh ANOLISA state for successful component work and write the durable
/// audit (operation record + central log) while the caller holds the install
/// lock used to authorize the RPM transactions.
///
/// Audit is decoupled from component-state changes: because a real dnf
/// transaction may have run even when nothing lands in `installed.toml` (a
/// CLI-only upgrade, or a run where every component drifted/failed), and some
/// already-current defaults only need a state record, this appends an operation
/// record and central-log entry whenever real work was attempted. The recorded
/// status is the true `ok` / `partial` / `failed` outcome of the whole command
/// (`cli_updated` and `prior_errors` from the transaction phase are folded in),
/// not just the component-persistence result.
///
/// Each pending change is still re-validated against the locked state before it
/// is recorded:
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
fn finalize_upgrade(req: FinalizeUpgrade<'_>) -> Result<PersistOutcome, CliError> {
    let FinalizeUpgrade {
        ctx,
        layout,
        command,
        state,
        audit,
        cli_updated,
        updates,
        installs,
        prior_errors,
        warnings,
    } = req;
    let mut outcome = PersistOutcome::default();

    for update in updates {
        let evr = update.refreshed.version.to_string();
        let Some(obj) = state.find_object_mut(ObjectKind::Component, &update.name) else {
            if update.adopt_if_missing {
                state.upsert_object(new_observed_rpm_component(
                    &update.name,
                    &update.package,
                    &evr,
                    &update.refreshed,
                    update.source_repo.as_deref(),
                    &audit.started_at,
                    &audit.operation_id,
                ));
                if update.record_only {
                    outcome.recorded.push(RecordedItem {
                        name: update.name.clone(),
                        package: update.package.clone(),
                        version: Some(evr),
                    });
                } else {
                    outcome.updated.push(UpdatedItem {
                        name: update.name.clone(),
                        package: update.package.clone(),
                        from: update.from.clone(),
                        to: evr,
                    });
                }
                continue;
            }
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
        if !is_matching_rpm_object(obj, &update.package) {
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
        obj.last_operation_id = Some(audit.operation_id.clone());
        if let Some(meta) = obj.rpm_metadata.as_mut() {
            meta.evr = Some(evr.clone());
            meta.arch = Some(update.refreshed.arch.clone());
            if let Some(source_repo) = &update.source_repo {
                meta.source_repo = Some(source_repo.clone());
            }
        }
        if update.record_only {
            outcome.recorded.push(RecordedItem {
                name: update.name.clone(),
                package: update.package.clone(),
                version: Some(evr),
            });
        } else {
            outcome.updated.push(UpdatedItem {
                name: update.name.clone(),
                package: update.package.clone(),
                from: update.from.clone(),
                to: evr,
            });
        }
    }

    for install in installs {
        let evr = install.refreshed.version.to_string();
        match classify_install_slot(state, &install.name, &install.package) {
            InstallSlot::Absent => {
                state.upsert_object(new_rpm_component(
                    &install.name,
                    &install.package,
                    &evr,
                    &install.refreshed,
                    install.source_repo.as_deref(),
                    &audit.started_at,
                    &audit.operation_id,
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
                    obj.last_operation_id = Some(audit.operation_id.clone());
                    if let Some(meta) = obj.rpm_metadata.as_mut() {
                        meta.evr = Some(evr.clone());
                        meta.arch = Some(install.refreshed.arch.clone());
                        if let Some(source_repo) = &install.source_repo {
                            meta.source_repo = Some(source_repo.clone());
                        }
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
    let total_success = cli_updated.is_some() as usize
        + outcome.updated.len()
        + outcome.installed.len()
        + outcome.recorded.len();
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
        id: audit.operation_id.clone(),
        command: command.to_string(),
        status: status.to_string(),
        started_at: audit.started_at.clone(),
        finished_at: Some(now_iso8601()),
    });

    let state_path = layout.state_dir.join("installed.toml");
    state.save(&state_path).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to save state: {err}"),
    })?;

    // Audit log is best-effort: state already persisted, so a log failure
    // downgrades to a stderr warning rather than unwinding.
    let recorded = outcome.updated.len() + outcome.installed.len() + outcome.recorded.len();
    let log = CentralLog::open(layout.central_log.clone());
    let mut objects: Vec<String> = cli_updated.map(|c| c.package.clone()).into_iter().collect();
    objects.extend(outcome.updated.iter().map(|u| u.name.clone()));
    objects.extend(outcome.installed.iter().map(|i| i.name.clone()));
    objects.extend(outcome.recorded.iter().map(|i| i.name.clone()));
    let record = LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(audit.operation_id.clone()),
        command: command.to_string(),
        source: "anolisa-cli".to_string(),
        component: None,
        severity,
        message: format!(
            "applied RPM image upgrade ({total_success} succeeded, {recorded} component state change(s) recorded, {total_errors} error(s))"
        ),
        actor: "cli".to_string(),
        install_mode: Some(ctx.install_mode.as_str().to_string()),
        started_at: audit.started_at.clone(),
        finished_at: Some(now_iso8601()),
        status: Some(log_status),
        objects,
        backup_ids: Vec::new(),
        warnings: warnings.to_vec(),
        details: serde_json::Value::Null,
    };
    if let Err(err) = log.append(&record)
        && !ctx.quiet
    {
        // Routed through `suspend_output` so this warning never collides with
        // the "Finalizing ANOLISA state..." spinner frame (issue #1452).
        progress::suspend_output(|| {
            eprintln!("warning: failed to write central log: {err}");
        });
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

fn classify_install_slot(state: &InstalledState, name: &str, package: &str) -> InstallSlot {
    match state.find_object(ObjectKind::Component, name) {
        None => InstallSlot::Absent,
        Some(existing) if is_matching_rpm_object(existing, package) => InstallSlot::MatchingRpm,
        Some(existing) => InstallSlot::Conflict(existing.effective_ownership().label()),
    }
}

fn is_matching_rpm_object(obj: &InstalledObject, package: &str) -> bool {
    obj.rpm_metadata
        .as_ref()
        .is_some_and(|m| m.package_name == package)
        && obj.effective_ownership().is_rpm()
}

/// Build an rpm-observed component object for a target default that was already
/// installed on the host but absent from ANOLISA state. `upgrade` updated the RPM
/// package, but ANOLISA still does not own its removal.
fn new_observed_rpm_component(
    name: &str,
    package: &str,
    evr: &str,
    refreshed: &PackageInfo,
    source_repo: Option<&str>,
    installed_at: &str,
    operation_id: &str,
) -> InstalledObject {
    InstalledObject {
        kind: ObjectKind::Component,
        name: name.to_string(),
        version: evr.to_string(),
        status: ObjectStatus::Adopted,
        manifest_digest: None,
        distribution_source: None,
        raw_package: None,
        install_backend: Some("rpm".to_string()),
        ownership: Some(Ownership::RpmObserved),
        rpm_metadata: Some(RpmMetadata {
            package_name: package.to_string(),
            evr: Some(evr.to_string()),
            arch: Some(refreshed.arch.clone()),
            source_repo: source_repo.map(str::to_string),
        }),
        installed_at: installed_at.to_string(),
        last_operation_id: Some(operation_id.to_string()),
        managed: false,
        adopted: true,
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

/// Build a fresh rpm-managed component object for a newly installed default.
/// Mirrors the delegated-install path: `managed = true`, `adopted = false`,
/// ownership [`Ownership::RpmManaged`], backend `rpm`, and no owned files (dnf
/// owns the file transaction).
fn new_rpm_component(
    name: &str,
    package: &str,
    evr: &str,
    refreshed: &PackageInfo,
    source_repo: Option<&str>,
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
            source_repo: source_repo.map(str::to_string),
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
    for item in &result.recorded {
        let verb = if result.dry_run {
            "would record".to_string()
        } else {
            format!("recorded {}", item.version.as_deref().unwrap_or("-"))
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
        "{} {} updated, {} installed, {} recorded, {} skipped, {} error(s) [{}]",
        color.label("summary:"),
        result.updated.len(),
        result.installed.len(),
        result.recorded.len(),
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
