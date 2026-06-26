//! `anolisa uninstall <COMPONENT>` (with optional `--purge` /
//! `--remove-system-package`).
//!
//! Teardown is **ownership-driven** (`raw_rpm_lifecycle_proposal.md` §11):
//!
//!   * `raw-managed` — the CLI face of [`anolisa_core::execute_plan`]:
//!     only ANOLISA-owned files are removed, external residue is kept.
//!     Unchanged by the RPM lifecycle work.
//!   * `rpm-managed` — delegates `dnf remove` then drops ANOLISA state
//!     (`Ownership::owns_removal()` is `true`).
//!   * `rpm-observed` — a preinstalled system RPM: the default uninstall
//!     drops **only** the ANOLISA state record and never runs dnf
//!     (`owns_removal()` is `false`). Removing the system package
//!     requires the explicit `--remove-system-package` override.
//!
//! The removal decision collapses to
//! `ownership.owns_removal() || --remove-system-package`. The RPM path
//! mirrors `update`'s dnf delegation (injected query/transaction, root
//! gate) and `forget`'s state drop; the raw path is untouched.
//!
//! Two surfaces apply to every ownership:
//!
//!   * `--dry-run` — render the plan (human or JSON), touching nothing.
//!     For RPM components it states whether package removal will happen.
//!   * default — execute.
//!
//! `--purge` widens the raw scope from "uninstall" to "uninstall + drop
//! ANOLISA-owned config/cache/state fragments". External modifications
//! are always refused regardless of `--purge`. `--purge` keeps its
//! existing (plan-only) path and is independent of the RPM routing.
//!
//! `--force` is parsed today as a wire stub (the spec calls it out)
//! but the executor does not yet branch on it. We surface a warning so
//! users see the boundary instead of getting silent semantics.
//!
//! Error routing:
//!
//! | `LifecycleError`            | CLI code           | exit |
//! |-----------------------------|--------------------|------|
//! | `ComponentNotInstalled`     | `INVALID_ARGUMENT` | 2    |
//! | `UnsupportedOperation`      | `EXECUTION_FAILED` | 1    |
//! | `LockHeld`                  | `EXECUTION_FAILED` | 1    |
//! | `Lock`                      | `EXECUTION_FAILED` | 1    |
//! | `State`                     | `EXECUTION_FAILED` | 1    |
//! | `Log`                       | `EXECUTION_FAILED` | 1    |
//! | `Filesystem`                | `EXECUTION_FAILED` | 1    |
//! | `ExecuteGated`              | `NOT_IMPLEMENTED`  | 64   |
//!
//! The `ExecuteGated` mapping: when a CLI surface is wire-shipped but
//! the executor refuses to perform a destructive operation, the right
//! bucket is `NOT_IMPLEMENTED` (exit 64). The gate itself lives in
//! `anolisa-core::lifecycle::check_destructive_execute_gate`; see the
//! docstring there for the lift conditions.

use chrono::{SecondsFormat, Utc};
use clap::Parser;

use anolisa_core::central_log::{CentralLog, LogKind, LogRecord, LogStatus, Severity};
use anolisa_core::lock::InstallLock;
use anolisa_core::state::{OperationRecord, Ownership};
use anolisa_core::{
    ComponentManifest, HookPhase, LifecycleError, LifecycleOperation, LifecycleOutcome,
    LifecyclePlan, ObjectKind, ResolvedLifecycleHooks, execute_plan, resolve_manifest_hooks,
};
use anolisa_platform::pkg_query::{PackageQuery, PackageQueryError};
use anolisa_platform::pkg_transaction::{PackageTransaction, PackageTransactionError};
use anolisa_platform::privilege;
use anolisa_platform::rpm_query::RpmPackageQuery;
use anolisa_platform::rpm_transaction::RpmTransaction;

use crate::color::Palette;
use crate::commands::common;
use crate::context::CliContext;
use crate::response::{CliError, render_json};

const COMMAND: &str = "uninstall";

#[derive(Parser)]
pub struct UninstallArgs {
    /// Component to uninstall
    #[arg(value_name = "COMPONENT")]
    pub component: String,
    /// Also remove ANOLISA-owned config / cache / state fragments
    #[arg(long)]
    pub purge: bool,
    /// For an `rpm-observed` system RPM, delegate package removal to
    /// `dnf remove`. Without it, uninstall drops only ANOLISA state and
    /// leaves the preinstalled RPM in place. No effect on raw components.
    #[arg(long)]
    pub remove_system_package: bool,
    /// Reserved for forcing through warnings (spec only, no behavior change yet)
    #[arg(long)]
    pub force: bool,
}

/// Dispatch `uninstall <component>`: build the real rpm/dnf-backed query and
/// transaction, then route by recorded ownership.
///
/// # Errors
///
/// Returns [`CliError`] when the component is absent, has enabled adapter
/// receipts, or teardown fails. See the module docs for the ownership matrix.
pub fn handle(args: UninstallArgs, ctx: &CliContext) -> Result<(), CliError> {
    let query = RpmPackageQuery::system();
    let txn = RpmTransaction::system();
    handle_with_deps(args, ctx, &query, &txn, privilege::is_root())
}

/// Core of [`handle`] with the package query, transaction, and root status
/// injected so the RPM path is testable without a live rpmdb/dnf or real
/// privileges. The raw and purge paths ignore the injected dependencies.
// pub(crate): driven by the cross-command MVP lifecycle test (#963).
pub(crate) fn handle_with_deps(
    args: UninstallArgs,
    ctx: &CliContext,
    query: &dyn PackageQuery,
    txn: &dyn PackageTransaction,
    is_root: bool,
) -> Result<(), CliError> {
    let operation = if args.purge {
        LifecycleOperation::Purge
    } else {
        LifecycleOperation::Uninstall
    };
    let target = args.component.as_str();
    let command = format!("{} {}", operation.as_str(), target);

    // Load installed state to plan against. Missing state is the same
    // as "target not installed" — surface that as INVALID_ARGUMENT so
    // the user sees the right exit code.
    let installed = common::load_installed_state(ctx, COMMAND)?;

    // A name that only matches a legacy `kind = "capability"` row written
    // by an older release is not uninstallable — say so instead of a bare
    // "not installed".
    if installed
        .find_object(ObjectKind::Component, target)
        .is_none()
        && installed
            .find_object(ObjectKind::Capability, target)
            .is_some()
    {
        return Err(CliError::InvalidArgument {
            command,
            reason: format!(
                "'{target}' is a legacy capability state entry from an older release; \
                 the capability concept is removed. The entry is pruned automatically \
                 on the next install/uninstall; use `anolisa list` to see components"
            ),
        });
    }
    // Adapter receipts must be released before the component is removed.
    // Uninstall does not auto-cascade into framework state (a framework CLI
    // might be unavailable, and silently orphaning a registered plugin is
    // worse than refusing). Block the real run and point the user at
    // `adapter disable`; a dry-run still renders its preview.
    if !ctx.dry_run {
        let claims = installed.adapter_claims_for_component(target);
        if !claims.is_empty() {
            let mut frameworks: Vec<&str> = claims.iter().map(|c| c.framework.as_str()).collect();
            frameworks.sort_unstable();
            frameworks.dedup();
            return Err(CliError::InvalidArgument {
                command,
                reason: format!(
                    "'{target}' has enabled adapters ({}); run `anolisa adapter disable {target}` \
                     for each framework before uninstalling",
                    frameworks.join(", ")
                ),
            });
        }
    }

    // `--force` is a wire stub on every path; surface it on real runs so users do
    // not assume it changes behavior. Hoisted above the ownership routing so the
    // RPM path (which returns before the raw executor) warns too. Dry-run stays
    // quiet, matching the previous release.
    if args.force && !ctx.dry_run {
        eprintln!("warning: --force is a spec stub today and has no behavioral effect yet");
    }

    // Ownership routing: an RPM-backed component (managed or observed) takes the
    // dnf-delegating path, not the raw file-removal executor. Only the plain
    // `Uninstall` operation reroutes; `--purge` keeps its existing plan-only
    // path regardless of ownership.
    if matches!(operation, LifecycleOperation::Uninstall)
        && let Some(obj) = installed.find_object(ObjectKind::Component, target)
    {
        let ownership = obj.effective_ownership();
        if ownership.is_rpm() {
            let package = obj
                .rpm_metadata
                .as_ref()
                .map(|m| m.package_name.clone())
                .filter(|p| !p.is_empty())
                .ok_or_else(|| CliError::Runtime {
                    command: command.clone(),
                    reason: format!(
                        "component '{target}' is recorded as an RPM component but has no package metadata; run `anolisa repair {target}` to refresh it before uninstalling"
                    ),
                })?;
            return uninstall_rpm_component(
                target,
                &package,
                ownership,
                args.remove_system_package,
                ctx,
                query,
                txn,
                is_root,
                &command,
            );
        }
        // Raw component: `--remove-system-package` only governs observed system
        // RPMs. Flag it instead of silently ignoring, then fall through to the
        // unchanged raw teardown path.
        if args.remove_system_package && !ctx.json {
            eprintln!(
                "warning: --remove-system-package has no effect for raw component '{target}' (there is no system RPM to remove)"
            );
        }
    }

    let plan = match operation {
        LifecycleOperation::Uninstall => LifecyclePlan::for_component_uninstall(target, &installed),
        LifecycleOperation::Purge => LifecyclePlan::for_component_purge(target, &installed),
    };

    if ctx.dry_run {
        if ctx.json {
            return render_json(COMMAND, &plan);
        }
        if !ctx.quiet {
            render_plan_human(&plan, ctx.no_color);
        }
        return Ok(());
    }

    let layout = common::resolve_layout(ctx);
    let install_mode = ctx.install_mode.as_str();
    let actor = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "cli".to_string());

    // `purge` is still gated pending manifest-driven config / cache /
    // state discovery — print the same plan-only warning the previous
    // release emitted so wrappers continue to see the boundary on
    // stderr. `uninstall` is no longer gated; it now goes through the
    // transaction-backed executor below.
    if matches!(operation, LifecycleOperation::Purge) && !ctx.json {
        let palette = Palette::new(ctx.no_color);
        eprintln!(
            "{} purge execute is currently plan-only; only --dry-run is supported in this release",
            palette.warn("warning:"),
        );
    }

    // Contract-driven uninstall hooks: read the installed component manifest
    // snapshot (persisted verbatim at install) and resolve its declared
    // pre/post-uninstall scripts. Best-effort — a missing or unreadable
    // snapshot (older installs, RPM-delegated paths) or an unresolvable
    // script path means no hooks for that phase, never a failed uninstall.
    let hooks = match common::installed_component_manifest_path(&layout, target, COMMAND)
        .ok()
        .and_then(|path| ComponentManifest::from_file(&path).ok())
    {
        Some(manifest) => ResolvedLifecycleHooks {
            pre_uninstall: resolve_manifest_hooks(
                &manifest.install.hooks,
                &layout,
                target,
                HookPhase::PreUninstall,
            )
            .unwrap_or_default(),
            post_uninstall: resolve_manifest_hooks(
                &manifest.install.hooks,
                &layout,
                target,
                HookPhase::PostUninstall,
            )
            .unwrap_or_default(),
        },
        None => ResolvedLifecycleHooks::default(),
    };

    let outcome = execute_plan(&plan, &layout, &actor, install_mode, &hooks)
        .map_err(|err| lifecycle_err_to_cli(&command, err))?;

    if ctx.json {
        let payload = UninstallPayload::from(&outcome);
        return render_json(COMMAND, &payload);
    }

    if !ctx.quiet {
        render_outcome_human(&outcome, ctx.no_color);
    }
    Ok(())
}

fn lifecycle_err_to_cli(command: &str, err: LifecycleError) -> CliError {
    match &err {
        LifecycleError::ComponentNotInstalled { component } => CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "component '{component}' is not installed — nothing to uninstall (run `anolisa status` to see what is installed)",
            ),
        },
        LifecycleError::UnsupportedOperation { op } => CliError::Runtime {
            command: command.to_string(),
            reason: format!("operation '{op}' is not supported by this executor"),
        },
        LifecycleError::LockHeld { path } => CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "install lock at {} is held by another process — run again after the other invocation finishes",
                path.display(),
            ),
        },
        LifecycleError::Lock { source } => CliError::Runtime {
            command: command.to_string(),
            reason: format!("install lock io: {source}"),
        },
        LifecycleError::State { source } => CliError::Runtime {
            command: command.to_string(),
            reason: format!("installed state write failed: {source}"),
        },
        LifecycleError::Log { source } => CliError::Runtime {
            command: command.to_string(),
            reason: format!("central log write failed: {source}"),
        },
        LifecycleError::Filesystem { path, source } => CliError::Runtime {
            command: command.to_string(),
            reason: format!("filesystem io failed for {}: {source}", path.display()),
        },
        // Transaction primitives (begin / persist / restore / finish)
        // are wrapped distinctly so operators can tell a journal-write
        // failure apart from a state-file or central-log failure when
        // diagnosing a partial uninstall.
        LifecycleError::Transaction { source } => CliError::Runtime {
            command: command.to_string(),
            reason: format!("transaction failed: {source}"),
        },
        // `purge` is still plan-only until manifest-driven config /
        // cache / state discovery ships. `uninstall` is no longer
        // gated; the executor runs through the transaction-backed
        // path. Surface the gate as NOT_IMPLEMENTED so wrappers see
        // the same boundary semantics as `disable --feature` /
        // `disable --purge`. The hint pipes through the lift-condition
        // text from `check_destructive_execute_gate`.
        LifecycleError::ExecuteGated { reason } => CliError::NotImplemented {
            command: command.to_string(),
            hint: Some(reason.clone()),
        },
        // pre_uninstall hook returned non-zero. The transaction has
        // already been rolled back, no files were deleted, and the
        // central log carries a `failed` operation record. Hint at
        // where to grep so operators don't have to chase down the
        // hook output by hand.
        LifecycleError::HookFailed {
            phase,
            component,
            summary,
            exit_code,
        } => CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "lifecycle hook {phase} for component '{component}' failed (exit {}): {summary} — inspect the central log (`anolisa logs --kind component --component {component}`) and the hook script before retrying",
                exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "?".to_string()),
            ),
        },
    }
}

/// What happens (or, on dry-run, would happen) to the underlying RPM package.
///
/// Distinguishes the three outcomes the `package_removal` field must report
/// accurately, instead of collapsing "kept on purpose" and "already gone" into
/// one label:
#[derive(Clone, Copy, PartialEq, Eq)]
enum PackageDisposition {
    /// `dnf remove` runs (real) or would run (dry-run intent).
    Removed,
    /// The package stays installed; only the ANOLISA state record is dropped —
    /// an `rpm-observed` default with no `--remove-system-package`.
    Kept,
    /// Removal was requested but the package is not in rpmdb — already gone via a
    /// manual `rpm -e` (the §10.2 Missing drift), so there is nothing to remove.
    AlreadyAbsent,
}

impl PackageDisposition {
    /// Wire label for the `package_removal` field.
    fn label(self) -> &'static str {
        match self {
            Self::Removed => "dnf remove",
            Self::Kept => "state only",
            Self::AlreadyAbsent => "already absent",
        }
    }
}

/// Decide the disposition from the removal intent and rpmdb presence.
///
/// `present` is `Some(true)`/`Some(false)` when an rpmdb probe confirmed the
/// package is/ isn't installed, and `None` when the probe could not confirm
/// (e.g. a query error) — in which case we preview the *intent* rather than
/// claim the package is absent.
fn disposition_for(remove_package: bool, present: Option<bool>) -> PackageDisposition {
    match (remove_package, present) {
        (false, _) => PackageDisposition::Kept,
        (true, Some(false)) => PackageDisposition::AlreadyAbsent,
        (true, _) => PackageDisposition::Removed,
    }
}

/// Uninstall an RPM-backed component (`rpm-managed` or `rpm-observed`).
///
/// `rpm-managed` owns its removal, so it delegates `dnf remove` by default;
/// `rpm-observed` is a preinstalled system RPM, so removal happens only when the
/// operator passes `--remove-system-package`. Either way the ANOLISA state
/// record is dropped (mirroring [`forget`](super::forget)). The dnf transaction
/// and state mutation run under the install lock so the adapter-claim guard
/// fires before the irreversible removal.
#[allow(clippy::too_many_arguments)]
fn uninstall_rpm_component(
    component: &str,
    package: &str,
    ownership: Ownership,
    remove_system_package: bool,
    ctx: &CliContext,
    query: &dyn PackageQuery,
    txn: &dyn PackageTransaction,
    is_root: bool,
    command: &str,
) -> Result<(), CliError> {
    // Dry-run: never locks, never needs root, never mutates rpmdb. Decide from the
    // pre-lock read and probe rpmdb so the preview reports accurately whether
    // removal would run, be declined (state only), or be skipped (already absent).
    if ctx.dry_run {
        let remove_package = ownership.owns_removal() || remove_system_package;
        let probe = query.query_installed(package);
        let installed_version = match &probe {
            Ok(Some(info)) => Some(info.version.to_string()),
            _ => None,
        };
        // Only a confirmed `Ok(None)` means "absent"; a query error is unconfirmed,
        // so the preview shows the removal *intent* rather than claiming absence.
        let present = match &probe {
            Ok(Some(_)) => Some(true),
            Ok(None) => Some(false),
            Err(_) => None,
        };
        let payload = UninstallRpmPayload {
            component: component.to_string(),
            package: package.to_string(),
            ownership: ownership.label(),
            install_mode: ctx.install_mode.as_str().to_string(),
            remove_system_package,
            package_removal: disposition_for(remove_package, present).label(),
            installed_version,
            state_dropped: false,
            dry_run: true,
            operation_id: None,
        };
        render_uninstall_rpm(ctx, &payload);
        return Ok(());
    }

    // Real run, entirely under the install lock. install/update delegate dnf
    // *outside* the lock, but neither gates a destructive removal on adapter
    // state. Here the adapter guard must fire before the irreversible
    // `dnf remove`, so a concurrent `adapter enable` cannot slip past a pre-lock
    // check and strand a removed package's plugin — hold the lock across the
    // whole critical section.
    let layout = common::resolve_layout(ctx);
    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;
    let mut state = common::load_installed_state(ctx, command)?;

    // Re-validate under the lock: the component must still exist, still be the
    // same RPM-owned package. A concurrent uninstall/forget/backend change must
    // not be clobbered.
    let obj = state
        .find_object(ObjectKind::Component, component)
        .ok_or_else(|| CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "component '{component}' disappeared from state during uninstall; nothing removed"
            ),
        })?;
    let locked_ownership = obj.effective_ownership();
    if !locked_ownership.is_rpm() {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "component '{component}' is no longer an RPM component in state; refusing to run an RPM uninstall"
            ),
        });
    }
    let package_matches = obj
        .rpm_metadata
        .as_ref()
        .is_some_and(|m| m.package_name == package);
    if !package_matches {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "component '{component}' RPM package identity changed during uninstall (expected '{package}'); run `anolisa status {component}`"
            ),
        });
    }

    // Recompute the removal decision from the *locked* ownership, never the
    // pre-lock value. If a concurrent op flipped this component from rpm-managed
    // to rpm-observed under the same package name, the pre-lock `owns_removal()`
    // would still say "remove" and a default uninstall would wrongly `dnf remove`
    // a preinstalled system RPM — exactly the safety guarantee rpm-observed
    // exists to provide. The locked read is the source of truth.
    let remove_package = locked_ownership.owns_removal() || remove_system_package;
    let ownership_label = locked_ownership.label();

    // Authoritative adapter-claim guard under the lock (the check in `handle` is
    // only a fast-fail / dry-run preview). Refuse before any dnf remove so a
    // registered plugin is never orphaned.
    let claims = state.adapter_claims_for_component(component);
    if !claims.is_empty() {
        let mut frameworks: Vec<&str> = claims.iter().map(|c| c.framework.as_str()).collect();
        frameworks.sort_unstable();
        frameworks.dedup();
        return Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "'{component}' has enabled adapters ({}); run `anolisa adapter disable {component}` for each framework before uninstalling",
                frameworks.join(", ")
            ),
        });
    }

    let mut warnings: Vec<String> = Vec::new();
    let disposition = if !remove_package {
        PackageDisposition::Kept
    } else {
        // Confirm the package is in rpmdb before removing. Already gone (a manual
        // `rpm -e`, the §10.2 Missing drift) is not an error: drop the stale state
        // record only and report it as already-absent.
        match query.query_installed(package) {
            Ok(Some(_)) => {
                if !is_root {
                    // Echo the flag in the hint only when it drove the removal:
                    // rpm-managed removes by default and needs no override.
                    let flag_suffix = if remove_system_package {
                        " --remove-system-package"
                    } else {
                        ""
                    };
                    return Err(CliError::Runtime {
                        command: command.to_string(),
                        reason: format!(
                            "removing system RPM '{package}' requires root privileges; re-run with sudo: `sudo anolisa --install-mode system uninstall {component}{flag_suffix}`"
                        ),
                    });
                }
                txn.remove(package).map_err(|err| match err {
                    // `dnf` missing even though the rpm-query above succeeded (rpm
                    // present, dnf absent): give the same ownership-aware guidance
                    // as the query-missing branch rather than a generic failure.
                    PackageTransactionError::CommandMissing { command: bin } => {
                        tooling_missing_err(command, &bin, package, component, locked_ownership)
                    }
                    other => txn_remove_err(other, command),
                })?;
                PackageDisposition::Removed
            }
            Ok(None) => {
                warnings.push(format!(
                    "RPM package '{package}' is not present in rpmdb (already removed by a manual `rpm -e`); dropping ANOLISA state only"
                ));
                PackageDisposition::AlreadyAbsent
            }
            Err(PackageQueryError::CommandMissing { command: bin }) => {
                return Err(tooling_missing_err(
                    command,
                    &bin,
                    package,
                    component,
                    locked_ownership,
                ));
            }
            Err(PackageQueryError::UnexpectedOutput { detail, .. }) => {
                // Covers malformed output as well as the multi-version invariant
                // violation; surface `detail` instead of guessing the cause.
                return Err(CliError::Runtime {
                    command: command.to_string(),
                    reason: format!(
                        "cannot remove '{package}': unexpected rpmdb query result ({detail}); refusing to remove against an ambiguous package — run `anolisa status {component}`"
                    ),
                });
            }
            Err(err) => {
                return Err(CliError::Runtime {
                    command: command.to_string(),
                    reason: format!("failed to query rpmdb for '{package}': {err}"),
                });
            }
        }
    };

    // Drop the ANOLISA state record (mirrors `forget::persist_forget`). The
    // provenance reported to the user comes from `locked_ownership` above — the
    // value observed under this same lock.
    state
        .remove_object(ObjectKind::Component, component)
        .ok_or_else(|| CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "component '{component}' disappeared from state during uninstall; nothing removed"
            ),
        })?;
    remove_component_manifest_snapshot(&layout, component, command)?;

    let now = now_iso8601();
    let lock_ts = Utc::now();
    let operation_id = format!(
        "op-uninstall-{}-{}",
        lock_ts.format("%Y%m%d%H%M%S"),
        lock_ts.timestamp_subsec_nanos()
    );
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

    // Audit log is best-effort: the state already persisted, so a log failure
    // downgrades to a warning instead of unwinding.
    let log = CentralLog::open(layout.central_log.clone());
    let message = match disposition {
        PackageDisposition::Removed => format!(
            "uninstalled component {component}: removed RPM package {package} via dnf and dropped ANOLISA state"
        ),
        PackageDisposition::Kept => format!(
            "uninstalled component {component}: dropped ANOLISA state; RPM package {package} left installed"
        ),
        PackageDisposition::AlreadyAbsent => format!(
            "uninstalled component {component}: dropped ANOLISA state; RPM package {package} was already absent from rpmdb"
        ),
    };
    let record = LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.clone()),
        command: command.to_string(),
        source: "anolisa-cli".to_string(),
        component: Some(component.to_string()),
        severity: Severity::Info,
        message,
        actor: "cli".to_string(),
        install_mode: Some(ctx.install_mode.as_str().to_string()),
        started_at: now.clone(),
        finished_at: Some(now),
        status: Some(LogStatus::Ok),
        objects: vec![component.to_string()],
        backup_ids: Vec::new(),
        warnings: warnings.clone(),
        details: serde_json::Value::Null,
    };
    if let Err(err) = log.append(&record) {
        eprintln!("warning: failed to write central log: {err}");
    }

    if !ctx.json && !ctx.quiet {
        let color = Palette::new(ctx.no_color);
        for w in &warnings {
            eprintln!("{} {w}", color.warn("warning:"));
        }
    }

    let payload = UninstallRpmPayload {
        component: component.to_string(),
        package: package.to_string(),
        ownership: ownership_label,
        install_mode: ctx.install_mode.as_str().to_string(),
        remove_system_package,
        package_removal: disposition.label(),
        installed_version: None,
        state_dropped: true,
        dry_run: false,
        operation_id: Some(operation_id),
    };
    render_uninstall_rpm(ctx, &payload);
    Ok(())
}

fn remove_component_manifest_snapshot(
    layout: &anolisa_platform::fs_layout::FsLayout,
    component: &str,
    command: &str,
) -> Result<(), CliError> {
    let dir = common::installed_component_manifest_dir(layout, component, command)?;
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "failed to remove component manifest snapshot at {}: {err}",
                dir.display()
            ),
        }),
    }
}

/// Build the actionable "rpm/dnf tooling missing" error for the RPM uninstall
/// path, shared by the rpmdb-query and the `dnf remove` missing-binary branches
/// so both give identical guidance.
///
/// The escape hatch depends on the removal *driver*, not the flag: an
/// owns-removal component (rpm-managed) is removed with or without the flag, so
/// its actionable fallback is `forget` (drop state, no package op); an
/// rpm-observed removal is driven solely by `--remove-system-package`, so there
/// the fallback is to drop the flag.
fn tooling_missing_err(
    command: &str,
    bin: &str,
    package: &str,
    component: &str,
    locked_ownership: Ownership,
) -> CliError {
    let alt = if locked_ownership.owns_removal() {
        format!(
            "run `anolisa forget {component}` to drop ANOLISA state without a package operation"
        )
    } else {
        "re-run without --remove-system-package to drop ANOLISA state only".to_string()
    };
    CliError::Runtime {
        command: command.to_string(),
        reason: format!(
            "cannot remove '{package}': {bin} not found on PATH — install rpm/dnf, or {alt}"
        ),
    }
}

/// Map a [`PackageTransactionError`] from `dnf remove` onto a CLI runtime error.
///
/// `CommandMissing` is handled at the call site via [`tooling_missing_err`] so
/// it carries ownership-aware guidance; this mapper covers the other variants.
fn txn_remove_err(err: PackageTransactionError, command: &str) -> CliError {
    match err {
        PackageTransactionError::CommandMissing { command: bin } => CliError::Runtime {
            command: command.to_string(),
            reason: format!("{bin} not found on PATH; cannot remove the RPM package"),
        },
        PackageTransactionError::PermissionDenied { command: bin } => CliError::Runtime {
            command: command.to_string(),
            reason: format!("permission denied running {bin}; re-run the uninstall with sudo"),
        },
        PackageTransactionError::TransactionFailed { code, stderr, .. } => CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "dnf remove failed (exit {}): {}",
                code.map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".to_string()),
                stderr.trim(),
            ),
        },
    }
}

/// RFC3339 UTC timestamp, seconds precision (matches the install/update paths).
fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Wire shape for an RPM-component uninstall result (`--json`) and its dry-run
/// preview.
#[derive(serde::Serialize)]
struct UninstallRpmPayload {
    component: String,
    package: String,
    /// Provenance label of the component (`rpm-managed` / `rpm-observed`).
    ownership: &'static str,
    install_mode: String,
    /// Whether the operator passed `--remove-system-package`.
    remove_system_package: bool,
    /// [`PackageDisposition::label`]: `"dnf remove"`, `"state only"`, or
    /// `"already absent"` — what happens (or would happen) to the package.
    package_removal: &'static str,
    /// Current rpmdb EVR; populated best-effort on the dry-run preview only.
    #[serde(skip_serializing_if = "Option::is_none")]
    installed_version: Option<String>,
    /// Whether the ANOLISA state record was dropped (false on dry-run).
    state_dropped: bool,
    dry_run: bool,
    /// `None` on dry-run (nothing recorded).
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
}

/// Human/JSON renderer for an RPM-component uninstall result.
fn render_uninstall_rpm(ctx: &CliContext, payload: &UninstallRpmPayload) {
    if ctx.json {
        // Errors here are unreachable for a plain Serialize struct; ignore the
        // Result so an (already-persisted) uninstall is not reported as failed.
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
            color.command("uninstall"),
            payload.component,
            color.muted(format!("({})", payload.ownership)),
            color.muted("(dry-run — nothing removed)"),
        );
        println!("{} {}", color.label("package:"), payload.package);
        if let Some(v) = &payload.installed_version {
            println!("{} {}", color.label("installed:"), v);
        }
        println!(
            "{} {}",
            color.label("package_removal:"),
            payload.package_removal,
        );
        match payload.package_removal {
            "state only" => println!(
                "  {}",
                color.muted(
                    "the system RPM stays installed; pass --remove-system-package to delegate removal to dnf"
                ),
            ),
            "already absent" => println!(
                "  {}",
                color.muted("the RPM package is not in rpmdb; uninstall will drop ANOLISA state only"),
            ),
            _ => {}
        }
        return;
    }
    println!(
        "{} {} {}",
        color.ok("✓ uninstalled"),
        payload.component,
        color.muted(format!("({})", payload.ownership)),
    );
    match payload.package_removal {
        "dnf remove" => println!(
            "    {} removed RPM package {} via dnf",
            color.label("note:"),
            payload.package,
        ),
        "already absent" => println!(
            "    {} ANOLISA state dropped; RPM package {} was already absent from rpmdb",
            color.label("note:"),
            payload.package,
        ),
        _ => println!(
            "    {} ANOLISA state dropped; RPM package {} left installed",
            color.label("note:"),
            payload.package,
        ),
    }
    if let Some(id) = &payload.operation_id {
        println!("{} {}", color.label("operation_id:"), color.id(id));
    }
}

#[derive(serde::Serialize)]
struct UninstallPayload {
    operation_id: String,
    operation: String,
    component: String,
    removed_files: Vec<String>,
    skipped_files: Vec<String>,
    state_object_removed: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    state_path: String,
    central_log_path: String,
}

impl From<&LifecycleOutcome> for UninstallPayload {
    fn from(o: &LifecycleOutcome) -> Self {
        Self {
            operation_id: o.operation_id.clone(),
            operation: o.operation.as_str().to_string(),
            component: o.component.clone(),
            removed_files: o
                .removed_files
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            skipped_files: o
                .skipped_files
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            state_object_removed: o.state_object_removed,
            warnings: o.warnings.clone(),
            state_path: o.state_path.display().to_string(),
            central_log_path: o.central_log_path.display().to_string(),
        }
    }
}

fn render_plan_human(plan: &LifecyclePlan, no_color: bool) {
    let color = Palette::new(no_color);
    println!(
        "{} {} {}",
        color.command(plan.operation.as_str()),
        plan.component,
        color.muted(format!(
            "(dry_run: true, risk: {:?}, requires_privilege: {})",
            plan.risk, plan.requires_privilege,
        )),
    );
    for c in &plan.components {
        println!("{} {}", color.header("component:"), c.name);
        if !c.files.is_empty() {
            println!("  {}", color.label("files:"));
            for f in &c.files {
                println!(
                    "    - {:?}  owner={:?}  action={:?}{}",
                    f.path,
                    f.owner,
                    f.action,
                    f.reason
                        .as_deref()
                        .map(|r| format!("  ({r})"))
                        .unwrap_or_default(),
                );
            }
        }
        if !c.configs.is_empty() {
            println!("  {}", color.label("configs:"));
            for f in &c.configs {
                println!("    - {:?}  action={:?}", f.path, f.action);
            }
        }
        if !c.services.is_empty() {
            println!("  {}", color.label("services:"));
            for s in &c.services {
                println!("    - {}  action={:?}", s.name, s.action);
            }
        }
    }
    if !plan.phases.is_empty() {
        println!("{}", color.header("phases:"));
        for p in &plan.phases {
            println!(
                "  - {:<14} {:<14} target={:<30} mode={:?}",
                p.name, p.action, p.target, p.mode,
            );
        }
    }
    if !plan.warnings.is_empty() {
        println!("{}", color.warn("warnings:"));
        for w in &plan.warnings {
            println!("  - {w}");
        }
    }
}

fn render_outcome_human(outcome: &LifecycleOutcome, no_color: bool) {
    let color = Palette::new(no_color);
    println!(
        "{} {} {}",
        color.command(outcome.operation.as_str()),
        outcome.component,
        color.ok("succeeded")
    );
    println!(
        "{} {}",
        color.label("operation_id:"),
        color.id(&outcome.operation_id)
    );
    println!(
        "{} {}",
        color.label("removed_files:"),
        outcome.removed_files.len()
    );
    for f in &outcome.removed_files {
        println!("  - {}", color.path(f.display()));
    }
    if !outcome.skipped_files.is_empty() {
        println!(
            "{} {}",
            color.label("skipped_files:"),
            outcome.skipped_files.len()
        );
        for f in &outcome.skipped_files {
            println!("  - {}", color.path(f.display()));
        }
    }
    println!(
        "{} {}",
        color.label("state:"),
        color.path(outcome.state_path.display())
    );
    println!(
        "{}   {}",
        color.label("log:"),
        color.path(outcome.central_log_path.display())
    );
    if !outcome.warnings.is_empty() {
        println!("{}", color.warn("warnings:"));
        for w in &outcome.warnings {
            println!("  - {w}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::context::InstallMode;
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;
    use tempfile::tempdir;

    use anolisa_core::adapter::claim::{AdapterClaim, ClaimStatus, DriverPayload, OpenClawClaim};
    use anolisa_core::state::{
        InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectStatus, RpmMetadata,
    };
    use anolisa_platform::pkg_query::{PackageInfo, PackageVersion};

    fn ctx_with_prefix(
        json: bool,
        dry_run: bool,
        install_mode: InstallMode,
        prefix: Option<PathBuf>,
    ) -> CliContext {
        CliContext {
            install_mode,
            prefix,
            json,
            dry_run,
            verbose: false,
            quiet: true,
            no_color: true,
        }
    }

    fn args(component: &str, purge: bool) -> UninstallArgs {
        UninstallArgs {
            component: component.to_string(),
            purge,
            remove_system_package: false,
            force: false,
        }
    }

    #[test]
    fn uninstall_help_names_positional_component() {
        let mut cmd = <UninstallArgs as clap::CommandFactory>::command();
        let help = cmd.render_help().to_string();

        assert!(
            help.contains("<COMPONENT>"),
            "uninstall help must expose a component-first positional name: {help}"
        );
        assert!(
            !help.contains("<CAPABILITY>"),
            "uninstall help must not expose the legacy capability positional name: {help}"
        );
    }

    /// Asking to uninstall a component that is not installed must
    /// surface `INVALID_ARGUMENT` (exit 2), not `EXECUTION_FAILED`,
    /// so wrapping scripts can rely on the routing.
    #[test]
    fn uninstall_unknown_component_routes_to_invalid_argument_exit_2() {
        let tmp = tempdir().expect("tmpdir");
        let err = handle(
            args("agentsight", false),
            &ctx_with_prefix(
                false,
                false,
                InstallMode::System,
                Some(tmp.path().to_path_buf()),
            ),
        )
        .expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert_eq!(err.exit_code(), 2);
        assert!(
            err.reason().contains("not installed"),
            "reason must mention 'not installed': {}",
            err.reason(),
        );
    }

    /// A name that only matches a legacy `kind = "capability"` row must
    /// get the migration hint, not a bare "not installed".
    #[test]
    fn uninstall_legacy_capability_name_gets_migration_hint() {
        use anolisa_core::{InstalledObject, InstalledState, ObjectKind, ObjectStatus};
        use anolisa_platform::fs_layout::FsLayout;

        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");

        let mut state = InstalledState::default();
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Capability,
            name: "agent-observability".to_string(),
            version: "0.1.0".to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: None,
            distribution_source: None,
            raw_package: None,
            install_backend: None,
            ownership: None,
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
        });
        state
            .save(&layout.state_dir.join("installed.toml"))
            .expect("seed state save");

        let err = handle(
            args("agent-observability", false),
            &ctx_with_prefix(
                false,
                false,
                InstallMode::System,
                Some(tmp.path().to_path_buf()),
            ),
        )
        .expect_err("legacy capability name must be rejected");

        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert_eq!(err.exit_code(), 2);
        assert!(
            err.reason().contains("legacy capability"),
            "reason must explain the legacy entry: {}",
            err.reason(),
        );
    }

    /// Dry-run path must not touch the filesystem even when the
    /// component is not installed: the planner builds an empty plan
    /// and we return Ok(()).
    #[test]
    fn uninstall_dry_run_on_unknown_component_returns_empty_plan() {
        let tmp = tempdir().expect("tmpdir");
        let result = handle(
            args("agentsight", false),
            &ctx_with_prefix(
                false,
                true,
                InstallMode::System,
                Some(tmp.path().to_path_buf()),
            ),
        );
        result.expect("dry-run must produce a plan even for absent components");
    }

    /// `lifecycle_err_to_cli` routing pin: every LifecycleError variant
    /// must surface as the documented CLI code. We test the buckets
    /// that route to EXECUTION_FAILED here so a future refactor cannot
    /// silently downgrade one to INVALID_ARGUMENT.
    #[test]
    fn lifecycle_err_lock_held_maps_to_execution_failed_exit_1() {
        let err = lifecycle_err_to_cli(
            "uninstall agentsight",
            LifecycleError::LockHeld {
                path: PathBuf::from("/var/lib/anolisa/lock"),
            },
        );
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert_eq!(err.exit_code(), 1);
        assert!(err.reason().contains("/var/lib/anolisa/lock"));
    }

    #[test]
    fn lifecycle_err_component_not_installed_maps_to_invalid_argument_exit_2() {
        let err = lifecycle_err_to_cli(
            "uninstall agentsight",
            LifecycleError::ComponentNotInstalled {
                component: "agentsight".to_string(),
            },
        );
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert_eq!(err.exit_code(), 2);
    }

    /// Plan-only gate: `LifecycleError::ExecuteGated` must surface as
    /// `NOT_IMPLEMENTED` (exit 64). The gate's reason text MUST be
    /// plumbed through to the CLI hint so users can see why the
    /// executor refused and that `--dry-run` is the supported
    /// alternative.
    #[test]
    fn lifecycle_err_execute_gated_maps_to_not_implemented_exit_64() {
        let err = lifecycle_err_to_cli(
            "uninstall agentsight",
            LifecycleError::ExecuteGated {
                reason: "uninstall execute is gated pending transaction-backed file removal \
                         (P1-D integration); run with --dry-run to preview the plan"
                    .to_string(),
            },
        );
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
        assert_eq!(err.exit_code(), 64);
        let hint = err.hint().unwrap_or_default();
        assert!(
            hint.contains("gated") && hint.contains("--dry-run"),
            "hint must explain the gate and point at --dry-run: {hint:?}",
        );
    }

    /// End-to-end success: when the component IS installed, the
    /// executor must remove the ANOLISA-owned file, drop the component
    /// object from `installed.toml`, write started + succeeded
    /// central-log entries, and return `Ok(())`.
    #[test]
    fn uninstall_execute_on_installed_component_removes_owned_files_and_succeeds() {
        use anolisa_core::{
            FileOwner, InstalledObject, InstalledState, ObjectKind, ObjectStatus, OwnedFile,
        };
        use anolisa_platform::fs_layout::FsLayout;

        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));

        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");
        std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bin");
        let owned = layout.bin_dir.join("agentsight");
        std::fs::write(&owned, b"binary").expect("write owned");

        let mut state = InstalledState::default();
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Component,
            name: "agentsight".to_string(),
            version: "0.2.0".to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: None,
            distribution_source: Some("file:///fake".to_string()),
            raw_package: None,
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
            files: vec![OwnedFile {
                path: owned.clone(),
                owner: FileOwner::Anolisa,
                sha256: Some("0".repeat(64)),
            }],
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        });
        let state_path = layout.state_dir.join("installed.toml");
        state.save(&state_path).expect("seed state save");

        handle(
            args("agentsight", false),
            &ctx_with_prefix(
                false,
                false,
                InstallMode::System,
                Some(tmp.path().to_path_buf()),
            ),
        )
        .expect("component uninstall execute must succeed");

        assert!(
            !owned.exists(),
            "ANOLISA-owned file must be removed by component uninstall",
        );

        let after = InstalledState::load(&state_path).expect("reload state");
        assert!(
            after
                .find_object(ObjectKind::Component, "agentsight")
                .is_none(),
            "component object must be dropped from installed.toml",
        );
        assert!(
            layout.central_log.exists(),
            "component uninstall must append a central-log record",
        );
    }

    /// End-to-end wiring: uninstall reads the installed component-manifest
    /// snapshot, resolves its `[[component.hooks]]` pre-uninstall script
    /// (placeholder-expanded, contract `strict`), and runs it before the
    /// files are removed. Pins that the CLI feeds contract hooks into
    /// `execute_plan` — the no-snapshot path is covered by
    /// `uninstall_execute_on_installed_component_removes_owned_files_and_succeeds`.
    #[test]
    #[cfg(unix)]
    fn uninstall_runs_contract_declared_pre_uninstall_hook() {
        use anolisa_core::{
            FileOwner, InstalledObject, InstalledState, ObjectKind, ObjectStatus, OwnedFile,
        };
        use anolisa_platform::fs_layout::FsLayout;
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");
        std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bin");
        let owned = layout.bin_dir.join("ws-ckpt");
        std::fs::write(&owned, b"binary").expect("write owned");

        // Hook script shipped under the datadir, declared by the contract.
        let hook_dir = layout.datadir.join("hooks").join("ws-ckpt");
        std::fs::create_dir_all(&hook_dir).expect("mkdir hook dir");
        let hook_script = hook_dir.join("pre-uninstall.sh");
        let sentinel = tmp.path().join("pre-uninstall.ran");
        std::fs::write(
            &hook_script,
            format!("#!/bin/sh\ntouch {}\n", sentinel.display()),
        )
        .expect("write hook");
        let mut perm = std::fs::metadata(&hook_script).expect("stat").permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&hook_script, perm).expect("chmod");

        // Installed component-manifest snapshot carrying the contract hook.
        let manifest_path =
            common::installed_component_manifest_path(&layout, "ws-ckpt", "uninstall")
                .expect("manifest path");
        std::fs::create_dir_all(manifest_path.parent().unwrap()).expect("mkdir manifest dir");
        std::fs::write(
            &manifest_path,
            r#"
            [component]
            name = "ws-ckpt"
            version = "0.1.0"

            # Hooks parse only on the minimal-schema path, which is gated on
            # the presence of [component.layout].
            [component.layout]
            modes = ["system"]

            [[component.hooks]]
            phase = "pre_uninstall"
            script = "{datadir}/hooks/ws-ckpt/pre-uninstall.sh"
            strict = false
            "#,
        )
        .expect("write installed manifest");

        let mut state = InstalledState::default();
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Component,
            name: "ws-ckpt".to_string(),
            version: "0.1.0".to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: None,
            distribution_source: Some("file:///fake".to_string()),
            raw_package: None,
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
            files: vec![OwnedFile {
                path: owned.clone(),
                owner: FileOwner::Anolisa,
                sha256: Some("0".repeat(64)),
            }],
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        });
        state
            .save(&layout.state_dir.join("installed.toml"))
            .expect("seed state save");

        handle(
            args("ws-ckpt", false),
            &ctx_with_prefix(
                false,
                false,
                InstallMode::System,
                Some(tmp.path().to_path_buf()),
            ),
        )
        .expect("uninstall with contract hook must succeed");

        assert!(
            sentinel.exists(),
            "contract-declared pre_uninstall hook must have run",
        );
        assert!(!owned.exists(), "owned file must be removed after the hook");
    }

    /// Purge stays gated until manifest-driven config/cache/state
    /// discovery lands. Pins that the gate text mentions purge and
    /// steers users at `--dry-run` / the uninstall subset, and that no
    /// filesystem state is touched while the gate fires.
    #[test]
    fn purge_execute_is_still_gated_with_clear_hint() {
        use anolisa_core::{
            FileOwner, InstalledObject, InstalledState, ObjectKind, ObjectStatus, OwnedFile,
        };
        use anolisa_platform::fs_layout::FsLayout;

        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");
        std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bin");
        let owned = layout.bin_dir.join("agentsight");
        std::fs::write(&owned, b"binary").expect("write owned");

        let mut state = InstalledState::default();
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Component,
            name: "agentsight".to_string(),
            version: "0.2.0".to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: None,
            distribution_source: Some("file:///fake".to_string()),
            raw_package: None,
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
            files: vec![OwnedFile {
                path: owned.clone(),
                owner: FileOwner::Anolisa,
                sha256: Some("0".repeat(64)),
            }],
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        });
        let state_path = layout.state_dir.join("installed.toml");
        state.save(&state_path).expect("seed state save");
        let prior_bytes = std::fs::read(&state_path).expect("read prior");

        let err = handle(
            args("agentsight", true),
            &ctx_with_prefix(
                false,
                false,
                InstallMode::System,
                Some(tmp.path().to_path_buf()),
            ),
        )
        .expect_err("purge execute must remain gated");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
        assert_eq!(err.exit_code(), 64);
        let hint = err.hint().unwrap_or_default();
        assert!(
            hint.contains("purge execute is gated"),
            "hint must explain the gate: {hint:?}",
        );

        assert!(owned.exists(), "purge gate must not touch owned files");
        let after_bytes = std::fs::read(&state_path).expect("read after");
        assert_eq!(
            after_bytes, prior_bytes,
            "purge gate must not mutate installed.toml",
        );
    }

    // ── RPM ownership-aware uninstall (#962) ────────────────────────────

    /// In-memory rpm world implementing **both** [`PackageQuery`] and
    /// [`PackageTransaction`] so one fake drives the uninstall flow. A successful
    /// `remove` clears the package from rpmdb; `install`/`update` panic — the
    /// uninstall path must never reach them.
    struct FakeRpm {
        package: String,
        installed: RefCell<Option<PackageInfo>>,
        remove_succeeds: bool,
        /// When set, `query_installed` reports the rpm/dnf tooling is missing,
        /// exercising the [`PackageQueryError::CommandMissing`] branch.
        tooling_missing: bool,
        /// When set, `query_installed` succeeds but `remove` reports the dnf
        /// binary missing — the rpm-present / dnf-absent case that must still
        /// reach the ownership-aware tooling-missing guidance at the call site.
        remove_tooling_missing: bool,
        remove_calls: Cell<usize>,
    }

    impl FakeRpm {
        fn present(package: &str) -> Self {
            Self {
                package: package.to_string(),
                installed: RefCell::new(Some(pkg_info(package, "2.2.0", Some("1.al8"), "x86_64"))),
                remove_succeeds: true,
                tooling_missing: false,
                remove_tooling_missing: false,
                remove_calls: Cell::new(0),
            }
        }
        fn absent(package: &str) -> Self {
            Self {
                package: package.to_string(),
                installed: RefCell::new(None),
                remove_succeeds: true,
                tooling_missing: false,
                remove_tooling_missing: false,
                remove_calls: Cell::new(0),
            }
        }
        fn failing(mut self) -> Self {
            self.remove_succeeds = false;
            self
        }
        fn tooling_missing(mut self) -> Self {
            self.tooling_missing = true;
            self
        }
        fn remove_tooling_missing(mut self) -> Self {
            self.remove_tooling_missing = true;
            self
        }
    }

    impl PackageQuery for FakeRpm {
        fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
            if package != self.package {
                return Ok(None);
            }
            if self.tooling_missing {
                return Err(PackageQueryError::CommandMissing {
                    command: "rpm".to_string(),
                });
            }
            Ok(self.installed.borrow().clone())
        }
        fn query_available(&self, _package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
            Ok(Vec::new())
        }
    }

    impl PackageTransaction for FakeRpm {
        fn install(&self, _package: &str) -> Result<(), PackageTransactionError> {
            panic!("uninstall path must not delegate a dnf install");
        }
        fn update(&self, _package: &str) -> Result<(), PackageTransactionError> {
            panic!("uninstall path must not delegate a dnf update");
        }
        fn remove(&self, package: &str) -> Result<(), PackageTransactionError> {
            self.remove_calls.set(self.remove_calls.get() + 1);
            assert_eq!(package, self.package, "remove targeted the wrong package");
            if self.remove_tooling_missing {
                return Err(PackageTransactionError::CommandMissing {
                    command: "dnf".to_string(),
                });
            }
            if !self.remove_succeeds {
                return Err(PackageTransactionError::TransactionFailed {
                    command: "dnf".to_string(),
                    operation: "remove".to_string(),
                    code: Some(1),
                    stderr: "dnf remove failed".to_string(),
                });
            }
            *self.installed.borrow_mut() = None;
            Ok(())
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

    fn rpm_object(component: &str, package: &str, ownership: Ownership) -> InstalledObject {
        InstalledObject {
            kind: ObjectKind::Component,
            name: component.to_string(),
            version: "2.2.0-1.al8".to_string(),
            status: if matches!(ownership, Ownership::RpmObserved) {
                ObjectStatus::Adopted
            } else {
                ObjectStatus::Installed
            },
            manifest_digest: None,
            distribution_source: None,
            raw_package: None,
            install_backend: Some("rpm".to_string()),
            ownership: Some(ownership),
            rpm_metadata: Some(RpmMetadata {
                package_name: package.to_string(),
                evr: Some("2.2.0-1.al8".to_string()),
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
        }
    }

    fn sample_claim(component: &str, framework: &str) -> AdapterClaim {
        AdapterClaim {
            claim_schema: 1,
            component: component.to_string(),
            framework: framework.to_string(),
            plugin_id: None,
            enabled_at: "2026-06-01T10:00:00Z".to_string(),
            resource_root: PathBuf::from("/tmp/anolisa-uninstall-test"),
            bundle_digest: None,
            driver_schema: 1,
            status: ClaimStatus::Enabled,
            resources: Vec::new(),
            driver_payload: DriverPayload::OpenClaw(OpenClawClaim {
                state_dir_resource: "state".to_string(),
                plugin_resource: "plugin".to_string(),
                skill_resources: Vec::new(),
                config_resources: Vec::new(),
            }),
        }
    }

    fn seed(ctx: &CliContext, objs: Vec<InstalledObject>, claims: Vec<AdapterClaim>) {
        let layout = common::resolve_layout(ctx);
        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");
        let mut state = InstalledState {
            install_mode: StateInstallMode::System,
            prefix: layout.prefix.clone(),
            ..Default::default()
        };
        for obj in objs {
            state.upsert_object(obj);
        }
        for claim in claims {
            state.upsert_adapter_claim(claim);
        }
        state
            .save(&layout.state_dir.join("installed.toml"))
            .expect("seed state");
    }

    fn load_state(ctx: &CliContext) -> InstalledState {
        let layout = common::resolve_layout(ctx);
        InstalledState::load(&layout.state_dir.join("installed.toml")).expect("load state")
    }

    fn seed_manifest_snapshot(ctx: &CliContext, component: &str) -> PathBuf {
        let layout = common::resolve_layout(ctx);
        let snapshot = common::installed_component_manifest_path(&layout, component, COMMAND)
            .expect("snapshot path");
        let dir = snapshot.parent().expect("snapshot dir").to_path_buf();
        std::fs::create_dir_all(&dir).expect("mkdir snapshot dir");
        std::fs::write(&snapshot, "component snapshot").expect("write snapshot");
        let provenance =
            anolisa_platform::fs_layout::FsLayout::provenance_path_for_snapshot(&snapshot);
        std::fs::write(provenance, "schema_version = 1\n").expect("write provenance");
        dir
    }

    fn args_rm(component: &str) -> UninstallArgs {
        UninstallArgs {
            component: component.to_string(),
            purge: false,
            remove_system_package: true,
            force: false,
        }
    }

    fn run(
        args: UninstallArgs,
        ctx: &CliContext,
        rpm: &FakeRpm,
        is_root: bool,
    ) -> Result<(), CliError> {
        handle_with_deps(args, ctx, rpm, rpm, is_root)
    }

    /// The new `--remove-system-package` flag parses to the positional + flag.
    #[test]
    fn uninstall_parses_remove_system_package_flag() {
        use clap::Parser as _;
        let a = UninstallArgs::try_parse_from([
            "uninstall",
            "copilot-shell",
            "--remove-system-package",
        ])
        .expect("parse");
        assert_eq!(a.component, "copilot-shell");
        assert!(a.remove_system_package);
    }

    /// Acceptance ①: a default uninstall of an `rpm-observed` component drops only
    /// ANOLISA state — it must NOT run `dnf remove`.
    #[test]
    fn uninstall_rpm_observed_default_drops_state_without_dnf_remove() {
        let tmp = tempdir().expect("tmpdir");
        let c = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        seed(
            &c,
            vec![rpm_object(
                "copilot-shell",
                "copilot-shell",
                Ownership::RpmObserved,
            )],
            Vec::new(),
        );
        let snapshot_dir = seed_manifest_snapshot(&c, "copilot-shell");
        let rpm = FakeRpm::present("copilot-shell");
        run(args("copilot-shell", false), &c, &rpm, true).expect("uninstall ok");

        assert_eq!(
            rpm.remove_calls.get(),
            0,
            "rpm-observed default must not run dnf remove",
        );
        let after = load_state(&c);
        assert!(
            after
                .find_object(ObjectKind::Component, "copilot-shell")
                .is_none(),
            "ANOLISA state record must be dropped",
        );
        assert!(
            after
                .operations
                .iter()
                .any(|o| o.command == "uninstall copilot-shell"),
            "an operation record must be appended",
        );
        assert!(
            !snapshot_dir.exists(),
            "component manifest snapshot dir must be removed",
        );
    }

    /// Acceptance ②: `--remove-system-package` on an `rpm-observed` component (as
    /// root) delegates `dnf remove` and then drops state.
    #[test]
    fn uninstall_rpm_observed_remove_system_package_runs_dnf_remove() {
        let tmp = tempdir().expect("tmpdir");
        let c = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        seed(
            &c,
            vec![rpm_object(
                "copilot-shell",
                "copilot-shell",
                Ownership::RpmObserved,
            )],
            Vec::new(),
        );
        let rpm = FakeRpm::present("copilot-shell");
        run(args_rm("copilot-shell"), &c, &rpm, true).expect("uninstall ok");

        assert_eq!(
            rpm.remove_calls.get(),
            1,
            "dnf remove must run with the flag"
        );
        assert!(
            rpm.installed.borrow().is_none(),
            "package must be gone from rpmdb after dnf remove",
        );
        assert!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .is_none(),
            "state record must be dropped",
        );
    }

    /// `--remove-system-package` on a non-root real run is refused with an
    /// actionable message; dnf never runs and the state record stays put.
    #[test]
    fn uninstall_rpm_observed_remove_system_package_non_root_refused() {
        let tmp = tempdir().expect("tmpdir");
        let c = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        seed(
            &c,
            vec![rpm_object(
                "copilot-shell",
                "copilot-shell",
                Ownership::RpmObserved,
            )],
            Vec::new(),
        );
        let rpm = FakeRpm::present("copilot-shell");
        let err = run(args_rm("copilot-shell"), &c, &rpm, false).expect_err("must refuse");

        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("root") && err.reason().contains("sudo"),
            "must point at sudo: {}",
            err.reason(),
        );
        assert_eq!(rpm.remove_calls.get(), 0, "dnf must not run without root");
        assert!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .is_some(),
            "state must be intact when the root gate refuses",
        );
    }

    /// `rpm-managed` owns its removal, so a default uninstall delegates
    /// `dnf remove` even without `--remove-system-package`.
    #[test]
    fn uninstall_rpm_managed_default_runs_dnf_remove() {
        let tmp = tempdir().expect("tmpdir");
        let c = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        seed(
            &c,
            vec![rpm_object(
                "copilot-shell",
                "copilot-shell",
                Ownership::RpmManaged,
            )],
            Vec::new(),
        );
        let rpm = FakeRpm::present("copilot-shell");
        run(args("copilot-shell", false), &c, &rpm, true).expect("uninstall ok");

        assert_eq!(
            rpm.remove_calls.get(),
            1,
            "rpm-managed owns removal: dnf remove runs by default",
        );
        assert!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .is_none(),
            "state record must be dropped",
        );
    }

    /// Acceptance ③ (decision): `owns_removal() || --remove-system-package` drives
    /// whether the package is removed, across the matrix.
    #[test]
    fn rpm_removal_decision_matrix() {
        // `flag` is a bound variable so the test exercises the real branch rather
        // than a constant-folded literal.
        let decide = |ownership: Ownership, flag: bool| ownership.owns_removal() || flag;
        assert!(
            !decide(Ownership::RpmObserved, false),
            "rpm-observed default keeps the system RPM",
        );
        assert!(
            decide(Ownership::RpmObserved, true),
            "rpm-observed + flag removes the system RPM",
        );
        assert!(
            decide(Ownership::RpmManaged, false),
            "rpm-managed removes by default",
        );
        // NB: only RPM ownerships reach this formula. RawManaged also reports
        // owns_removal() == true, but raw components are routed to the file-removal
        // executor instead, never here — so it is intentionally not asserted.
    }

    /// Disposition labels are the stable wire strings the `package_removal` field
    /// and renderers branch on.
    #[test]
    fn package_disposition_labels() {
        assert_eq!(PackageDisposition::Removed.label(), "dnf remove");
        assert_eq!(PackageDisposition::Kept.label(), "state only");
        assert_eq!(PackageDisposition::AlreadyAbsent.label(), "already absent");
    }

    /// `disposition_for` maps (removal intent, rpmdb presence) onto the outcome:
    /// kept when not removing, already-absent only when absence is *confirmed*,
    /// and removed otherwise (including the unconfirmed/query-error case).
    #[test]
    fn disposition_for_maps_intent_and_presence() {
        assert_eq!(disposition_for(false, Some(true)).label(), "state only");
        assert_eq!(disposition_for(false, None).label(), "state only");
        assert_eq!(disposition_for(true, Some(true)).label(), "dnf remove");
        assert_eq!(disposition_for(true, Some(false)).label(), "already absent");
        assert_eq!(
            disposition_for(true, None).label(),
            "dnf remove",
            "unconfirmed presence previews the removal intent, not absence",
        );
    }

    /// Acceptance ③ (safety): dry-run, even with the removal flag, touches
    /// neither rpmdb nor ANOLISA state.
    #[test]
    fn uninstall_rpm_observed_dry_run_touches_nothing() {
        let tmp = tempdir().expect("tmpdir");
        let c = ctx_with_prefix(
            false,
            true,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        seed(
            &c,
            vec![rpm_object(
                "copilot-shell",
                "copilot-shell",
                Ownership::RpmObserved,
            )],
            Vec::new(),
        );
        let snapshot_dir = seed_manifest_snapshot(&c, "copilot-shell");
        let rpm = FakeRpm::present("copilot-shell");
        run(args_rm("copilot-shell"), &c, &rpm, true).expect("dry-run ok");

        assert_eq!(rpm.remove_calls.get(), 0, "dry-run must not run dnf");
        assert!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .is_some(),
            "dry-run must not drop the state record",
        );
        assert!(
            snapshot_dir.exists(),
            "dry-run must not remove the manifest snapshot dir",
        );
    }

    /// `--remove-system-package` but the package is already gone from rpmdb
    /// (manual `rpm -e`, the §10.2 Missing drift): no dnf remove, state-only drop.
    #[test]
    fn uninstall_rpm_observed_remove_system_package_already_absent_drops_state_only() {
        let tmp = tempdir().expect("tmpdir");
        let c = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        seed(
            &c,
            vec![rpm_object(
                "copilot-shell",
                "copilot-shell",
                Ownership::RpmObserved,
            )],
            Vec::new(),
        );
        let rpm = FakeRpm::absent("copilot-shell");
        run(args_rm("copilot-shell"), &c, &rpm, true).expect("uninstall ok");

        assert_eq!(
            rpm.remove_calls.get(),
            0,
            "already-absent package must not trigger dnf remove",
        );
        assert!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .is_none(),
            "state record must still be dropped",
        );
    }

    /// A `dnf remove` failure aborts the uninstall and leaves the state record in
    /// place so the operator can retry or `forget`.
    #[test]
    fn uninstall_rpm_observed_dnf_failure_keeps_state() {
        let tmp = tempdir().expect("tmpdir");
        let c = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        seed(
            &c,
            vec![rpm_object(
                "copilot-shell",
                "copilot-shell",
                Ownership::RpmObserved,
            )],
            Vec::new(),
        );
        let rpm = FakeRpm::present("copilot-shell").failing();
        let err =
            run(args_rm("copilot-shell"), &c, &rpm, true).expect_err("dnf failure must surface");

        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("dnf remove failed"),
            "must report the dnf failure: {}",
            err.reason(),
        );
        assert!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .is_some(),
            "state must be intact when dnf remove fails",
        );
    }

    /// rpm-managed + `--remove-system-package`, but rpm/dnf tooling is missing:
    /// the CommandMissing hint must steer at `forget` — for an owns-removal
    /// component, dropping the flag would NOT avoid the package operation, so the
    /// "re-run without --remove-system-package" advice would be non-actionable.
    #[test]
    fn uninstall_rpm_managed_tooling_missing_points_at_forget() {
        let tmp = tempdir().expect("tmpdir");
        let c = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        seed(
            &c,
            vec![rpm_object(
                "copilot-shell",
                "copilot-shell",
                Ownership::RpmManaged,
            )],
            Vec::new(),
        );
        let rpm = FakeRpm::present("copilot-shell").tooling_missing();
        let err = run(args_rm("copilot-shell"), &c, &rpm, true)
            .expect_err("missing rpm tooling must surface");

        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("forget"),
            "rpm-managed path must steer at forget: {}",
            err.reason(),
        );
        assert!(
            !err.reason().contains("--remove-system-package"),
            "must not give the non-actionable drop-the-flag hint to an owns-removal component: {}",
            err.reason(),
        );
        assert!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .is_some(),
            "state must be intact when the rpmdb probe errors",
        );
    }

    /// rpm-observed + `--remove-system-package`, tooling missing: here dropping the
    /// flag *does* fall back to state-only, so the hint points at the flag (the
    /// counterpart to the rpm-managed branch above).
    #[test]
    fn uninstall_rpm_observed_tooling_missing_points_at_dropping_flag() {
        let tmp = tempdir().expect("tmpdir");
        let c = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        seed(
            &c,
            vec![rpm_object(
                "copilot-shell",
                "copilot-shell",
                Ownership::RpmObserved,
            )],
            Vec::new(),
        );
        let rpm = FakeRpm::present("copilot-shell").tooling_missing();
        let err = run(args_rm("copilot-shell"), &c, &rpm, true)
            .expect_err("missing rpm tooling must surface");

        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("--remove-system-package"),
            "rpm-observed path must point at dropping the flag: {}",
            err.reason(),
        );
    }

    /// rpm present but `dnf` absent: the rpmdb query succeeds, so the
    /// missing-tooling guidance must come from the `txn.remove` `CommandMissing`
    /// branch at the call site — and it must match the query-missing branch
    /// (owns-removal → `forget`), not fall back to `txn_remove_err`'s generic
    /// "cannot remove the RPM package" message.
    #[test]
    fn uninstall_rpm_managed_dnf_missing_points_at_forget() {
        let tmp = tempdir().expect("tmpdir");
        let c = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        seed(
            &c,
            vec![rpm_object(
                "copilot-shell",
                "copilot-shell",
                Ownership::RpmManaged,
            )],
            Vec::new(),
        );
        let rpm = FakeRpm::present("copilot-shell").remove_tooling_missing();
        let err = run(args("copilot-shell", false), &c, &rpm, true)
            .expect_err("missing dnf must surface");

        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert_eq!(
            rpm.remove_calls.get(),
            1,
            "the rpmdb query succeeded, so dnf remove was attempted before failing",
        );
        assert!(
            err.reason().contains("forget"),
            "owns-removal must steer at forget even when dnf (not rpm) is missing: {}",
            err.reason(),
        );
        assert!(
            !err.reason().contains("cannot remove the RPM package"),
            "must not fall back to the generic txn_remove_err message: {}",
            err.reason(),
        );
        assert!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .is_some(),
            "state must be intact when the removal could not run",
        );
    }

    /// A raw component with `--remove-system-package` ignores the flag (warns) and
    /// runs the unchanged raw teardown: owned files removed, state dropped.
    #[test]
    fn uninstall_raw_with_remove_system_package_flag_warns_but_succeeds() {
        use anolisa_core::{FileOwner, OwnedFile};
        use anolisa_platform::fs_layout::FsLayout;

        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");
        std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bin");
        let owned = layout.bin_dir.join("agentsight");
        std::fs::write(&owned, b"binary").expect("write owned");

        let mut state = InstalledState::default();
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Component,
            name: "agentsight".to_string(),
            version: "0.2.0".to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: None,
            distribution_source: Some("file:///fake".to_string()),
            raw_package: None,
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
            files: vec![OwnedFile {
                path: owned.clone(),
                owner: FileOwner::Anolisa,
                sha256: Some("0".repeat(64)),
            }],
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        });
        state
            .save(&layout.state_dir.join("installed.toml"))
            .expect("seed state save");

        let c = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        handle(args_rm("agentsight"), &c).expect("raw uninstall must succeed");

        assert!(
            !owned.exists(),
            "raw teardown must still remove the owned file"
        );
        assert!(
            load_state(&c)
                .find_object(ObjectKind::Component, "agentsight")
                .is_none(),
            "raw component object must be dropped",
        );
    }

    /// An `rpm-observed` component with an enabled adapter receipt is refused
    /// (fast-fail in `handle`) before any removal — dnf must not run.
    #[test]
    fn uninstall_rpm_observed_refuses_with_enabled_adapter() {
        let tmp = tempdir().expect("tmpdir");
        let c = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        seed(
            &c,
            vec![rpm_object(
                "copilot-shell",
                "copilot-shell",
                Ownership::RpmObserved,
            )],
            vec![sample_claim("copilot-shell", "openclaw")],
        );
        let rpm = FakeRpm::present("copilot-shell");
        let err = run(args_rm("copilot-shell"), &c, &rpm, true).expect_err("adapter must block");

        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("adapter disable"),
            "must point at adapter disable: {}",
            err.reason(),
        );
        assert_eq!(
            rpm.remove_calls.get(),
            0,
            "no dnf remove while adapters enabled"
        );
        assert!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .is_some(),
            "component must remain when refused",
        );
    }

    /// `uninstall_rpm_component` re-checks the adapter guard under the lock,
    /// bypassing the pre-lock fast-fail (as a concurrent `adapter enable` would).
    /// It must refuse **before** the irreversible `dnf remove`.
    #[test]
    fn uninstall_rpm_component_rechecks_adapter_under_lock() {
        let tmp = tempdir().expect("tmpdir");
        let c = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        seed(
            &c,
            vec![rpm_object(
                "copilot-shell",
                "copilot-shell",
                Ownership::RpmObserved,
            )],
            vec![sample_claim("copilot-shell", "openclaw")],
        );
        let rpm = FakeRpm::present("copilot-shell");
        let err = uninstall_rpm_component(
            "copilot-shell",
            "copilot-shell",
            Ownership::RpmObserved,
            true,
            &c,
            &rpm,
            &rpm,
            true,
            "uninstall copilot-shell",
        )
        .expect_err("locked claim check must refuse");

        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("adapter disable"));
        assert_eq!(
            rpm.remove_calls.get(),
            0,
            "guard must fire before the irreversible dnf remove",
        );
        assert!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .is_some(),
            "object must remain when the locked claim check refuses",
        );
    }

    /// Regression for the locked-recompute Blocker: the caller passes a stale
    /// pre-lock `rpm-managed` ownership (owns_removal == true), but the on-disk
    /// state is `rpm-observed`. The locked recompute must win — a default
    /// uninstall (no flag) must NOT run `dnf remove` on the system RPM.
    #[test]
    fn uninstall_rpm_recomputes_removal_decision_under_lock() {
        let tmp = tempdir().expect("tmpdir");
        let c = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        seed(
            &c,
            vec![rpm_object(
                "copilot-shell",
                "copilot-shell",
                Ownership::RpmObserved,
            )],
            Vec::new(),
        );
        let rpm = FakeRpm::present("copilot-shell");
        uninstall_rpm_component(
            "copilot-shell",
            "copilot-shell",
            Ownership::RpmManaged, // stale pre-lock read; the on-disk state is observed
            false,                 // no --remove-system-package
            &c,
            &rpm,
            &rpm,
            true,
            "uninstall copilot-shell",
        )
        .expect("uninstall ok");

        assert_eq!(
            rpm.remove_calls.get(),
            0,
            "locked rpm-observed must not dnf remove despite a stale rpm-managed pre-lock read",
        );
        assert!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .is_none(),
            "state record must still be dropped",
        );
    }

    /// An RPM component whose state lost its package metadata is steered at
    /// `repair` rather than running a removal against an empty package name.
    #[test]
    fn uninstall_rpm_missing_package_metadata_points_at_repair() {
        let tmp = tempdir().expect("tmpdir");
        let c = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        let mut obj = rpm_object("copilot-shell", "copilot-shell", Ownership::RpmObserved);
        obj.rpm_metadata = None;
        seed(&c, vec![obj], Vec::new());
        let rpm = FakeRpm::present("copilot-shell");
        let err = run(args("copilot-shell", false), &c, &rpm, true).expect_err("must refuse");

        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("repair"),
            "must point at repair: {}",
            err.reason(),
        );
    }
}
