//! `anolisa uninstall <COMPONENT>` (with optional `--purge` /
//! `--remove-system-package`).
//!
//! Teardown runs the thin-shell pipeline: assemble facts, ask the planner
//! (decision rows X1–X6), and hand the step sequence to the matching
//! executor. The plan shape follows the record's authority:
//!
//!   * **Owned** (raw) — X1: pre-uninstall hooks, stop services, remove the
//!     recorded files, post-uninstall hooks, drop the record. Executed by
//!     the owned executor through the raw teardown port.
//!   * **Delegated managed** — X2/X3: `dnf remove` (skipped with a note when
//!     the package is already gone externally), then drop the record.
//!   * **Delegated adopted/observed** — X4: drop only the record by
//!     default; `--remove-system-package` grants per-invocation removal
//!     authority over the native package.
//!   * **Quarantined** — the record drops without any package operation;
//!     with `--remove-system-package` the unverified identity refuses.
//!
//! Adapter receipts block teardown before any table arm — uninstall never
//! auto-cascades into framework state.
//!
//! Two surfaces apply everywhere: `--dry-run` renders the plan, touching
//! nothing; default executes. `--purge` keeps its existing (plan-only)
//! legacy path pending manifest-driven config/cache/state discovery.
//! `--force` is parsed as a wire stub and surfaced with a warning.

use chrono::{SecondsFormat, Utc};
use clap::Parser;

use anolisa_core::central_log::{CentralLog, LogKind, LogRecord, LogStatus, Severity};
use anolisa_core::domain::{InstallationScope, NativePm, ProviderBinding};
use anolisa_core::executor::{DelegatedExecutionTarget, execute_delegated_steps};
use anolisa_core::facts::{JournalEvidence, ObserveRequest, assemble_facts};
use anolisa_core::lock::InstallLock;
use anolisa_core::owned_executor::{OwnedExecutionError, execute_owned_steps};
use anolisa_core::planner::{
    HookKind, Intent, InvocationForm, Plan, PlanError, PlanNote, RecordFacts, Step,
    UninstallRequest, plan,
};
use anolisa_core::providers::DelegatedProvider;
use anolisa_core::record_sink::{DelegatedIdentity, RecordContext, StoreRecordSink};
use anolisa_core::state::OperationRecord;
use anolisa_core::state_store::StateStore;
use anolisa_core::{
    ComponentManifest, HookPhase, LifecycleOperation, LifecyclePlan, ObjectKind,
    ResolvedLifecycleHooks, resolve_manifest_hooks,
};
use anolisa_platform::pkg_query::PackageQuery;
use anolisa_platform::pkg_transaction::PackageTransaction;
use anolisa_platform::privilege;
use anolisa_platform::rpm_query::RpmPackageQuery;
use anolisa_platform::rpm_transaction::RpmTransaction;

use crate::color::Palette;
use crate::commands::common;
use crate::commands::tier1::install::RawTeardownOps;
use crate::commands::tier1::recovery::LockedJournalGate;
use crate::commands::tier1::rpm_install;
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
    /// For an adopted or observed system RPM, delegate package removal to
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
/// injected so the delegated path is testable without a live rpmdb/dnf or
/// real privileges. The purge path ignores the injected dependencies.
// pub(crate): driven by cross-command lifecycle tests.
pub(crate) fn handle_with_deps(
    args: UninstallArgs,
    ctx: &CliContext,
    query: &dyn PackageQuery,
    txn: &dyn PackageTransaction,
    is_root: bool,
) -> Result<(), CliError> {
    if args.purge {
        return handle_purge(&args, ctx);
    }
    uninstall_component(&args, ctx, query, txn, is_root)
}

/// The plain-uninstall pipeline: observe, plan (rows X1–X6), execute through
/// the matching step-family executor.
fn uninstall_component(
    args: &UninstallArgs,
    ctx: &CliContext,
    query: &dyn PackageQuery,
    txn: &dyn PackageTransaction,
    is_root: bool,
) -> Result<(), CliError> {
    let input = args.component.as_str();
    let command = format!("{COMMAND} {input}");
    let scope_command = scope_guard_command(args, input);
    let layout = common::resolve_layout(ctx);
    let state_path = layout.state_dir.join("installed.toml");
    let journal_dir = rpm_install::journal_dir(&layout);
    let uid = privilege::effective_uid();
    let scope = match ctx.install_mode {
        crate::context::InstallMode::System => InstallationScope::System,
        crate::context::InstallMode::User => InstallationScope::User { uid },
    };
    let now = now_iso8601();

    let (resolved, view) = common::resolve_mutation_target(input, ctx, &scope_command)?;
    let store = view.writable.state;
    let target = resolved.as_str();

    if store.find(ObjectKind::Component, target).is_none() {
        // A name that only matched a legacy `kind = "capability"` row was
        // dropped by the state migration — say so instead of a bare "not
        // installed".
        if store.dropped_capabilities.iter().any(|name| name == target) {
            return Err(CliError::InvalidArgument {
                command,
                reason: format!(
                    "'{target}' is a legacy capability state entry from an older release; \
                     the capability concept is removed. The entry is pruned automatically \
                     on the next install/uninstall; use `anolisa list` to see components"
                ),
            });
        }
    }

    // `--force` is a wire stub; surface it on real runs so users do not
    // assume it changes behavior. Dry-run stays quiet.
    if args.force && !ctx.dry_run {
        eprintln!("warning: --force is a spec stub today and has no behavioral effect yet");
    }

    // `--remove-system-package` only governs delegated records. Flag it on
    // an owned record instead of silently ignoring it.
    if args.remove_system_package
        && !ctx.json
        && matches!(
            store
                .find(ObjectKind::Component, target)
                .map(|r| &r.binding),
            Some(ProviderBinding::Owned { .. })
        )
    {
        eprintln!(
            "warning: --remove-system-package has no effect for raw component '{target}' (there is no system RPM to remove)"
        );
    }

    // The probe target comes from the record; uninstall never resolves a
    // new package.
    let native_package = match store.find(ObjectKind::Component, target) {
        Some(installation) => match &installation.binding {
            ProviderBinding::Delegated { package, .. } => match package.resolved_name() {
                Some(name) => Some(name.to_string()),
                None => {
                    return Err(CliError::Runtime {
                        command,
                        reason: format!(
                            "the record for '{target}' has no resolved package name; run `anolisa repair {target}` first"
                        ),
                    });
                }
            },
            ProviderBinding::Owned { .. } => None,
        },
        None => None,
    };

    // Whether the record's relation grants removal authority by itself —
    // drives the missing-tooling guidance and the locked re-validation.
    let record_is_managed = matches!(
        store
            .find(ObjectKind::Component, target)
            .map(|r| &r.binding),
        Some(ProviderBinding::Delegated {
            relation: anolisa_core::domain::ManagementRelation::Managed { .. },
            ..
        })
    );

    let provider = DelegatedProvider::new(query, txn);
    let observe_request = ObserveRequest {
        kind: ObjectKind::Component,
        name: target,
        scope,
        native_package: native_package.as_deref(),
        observed_at: &now,
        verify_owned_files: false,
    };
    let facts = match assemble_facts(
        &observe_request,
        &store,
        Some(&provider),
        &layout,
        &journal_dir,
    ) {
        Ok(facts) => facts,
        // rpm missing on PATH. When this uninstall would run a package
        // operation, steer at the actionable fallback; a record-only drop
        // consults no native authority, so re-observe without the probe and
        // keep going.
        Err(anolisa_core::facts::FactsError::Probe(
            anolisa_core::providers::ProviderError::Query(
                anolisa_platform::pkg_query::PackageQueryError::CommandMissing { command: bin },
            ),
        )) => {
            if record_is_managed || args.remove_system_package {
                return Err(tooling_missing_err(
                    &command,
                    &bin,
                    native_package.as_deref().unwrap_or(target),
                    target,
                    record_is_managed,
                ));
            }
            assemble_facts(&observe_request, &store, None, &layout, &journal_dir).map_err(
                |err| CliError::Runtime {
                    command: command.clone(),
                    reason: err.to_string(),
                },
            )?
        }
        Err(err) => {
            return Err(CliError::Runtime {
                command: command.clone(),
                reason: err.to_string(),
            });
        }
    };

    let intent = Intent::Uninstall(UninstallRequest {
        remove_system_package: args.remove_system_package,
        invocation: InvocationForm::SingleNamed,
    });
    let (steps, notes) = match plan(&intent, &facts) {
        Ok(Plan::Execute { steps, notes }) => (steps, notes),
        Ok(Plan::NoOp { .. }) => {
            // The uninstall table has no NoOp rows today; render an honest
            // "nothing to do" if the planner ever grows one.
            return render_result(
                ctx,
                target,
                None,
                &UninstallDisposition::StateOnly,
                true,
                &[],
                None,
            );
        }
        Err(err) => return Err(plan_error_to_cli(err, target, &command, &store)),
    };

    let plan_labels: Vec<String> = steps.iter().map(step_label).collect();
    let disposition = disposition_for(&steps, &notes);

    if ctx.dry_run {
        return render_result(
            ctx,
            target,
            native_package.as_deref(),
            &disposition,
            true,
            &plan_labels,
            None,
        );
    }

    // Route by step family: delegated plans (including record-only drops)
    // run through the delegated executor, owned plans through the teardown
    // port.
    let is_delegated_plan = steps.iter().all(|step| {
        matches!(
            step,
            Step::NativeTransaction { .. }
                | Step::Observe { .. }
                | Step::WriteRecord(_)
                | Step::DropRecord
        )
    });
    if !is_delegated_plan {
        return uninstall_owned(
            target,
            ctx,
            &layout,
            &state_path,
            &journal_dir,
            scope,
            &now,
            &intent,
            &command,
        );
    }

    // A native removal runs dnf and requires root; a record-only drop does
    // not (mirroring `forget`).
    let runs_native_txn = steps
        .iter()
        .any(|step| matches!(step, Step::NativeTransaction { .. }));
    if runs_native_txn && !is_root {
        let flag_suffix = if args.remove_system_package {
            " --remove-system-package"
        } else {
            ""
        };
        return Err(CliError::Runtime {
            command,
            reason: format!(
                "removing system RPM '{}' requires root privileges; re-run with sudo: `sudo anolisa uninstall {target}{flag_suffix}`",
                native_package.as_deref().unwrap_or(target)
            ),
        });
    }

    // Real run under the install lock, with state re-read and the adapter
    // guard re-checked inside it — a concurrent `adapter enable` must not
    // slip past the pre-lock plan and strand a removed package's plugin.
    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.clone(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;
    let mut store = StateStore::load_for_layout(&state_path, uid, &layout).map_err(|err| {
        CliError::Runtime {
            command: command.clone(),
            reason: format!("failed to load installed state: {err}"),
        }
    })?;
    ensure_no_adapter_claims(&store, target, &command)?;
    let (native_package, record_is_managed) = match store
        .find(ObjectKind::Component, target)
        .map(|record| &record.binding)
    {
        Some(ProviderBinding::Delegated {
            package, relation, ..
        }) => {
            let package = package.resolved_name().ok_or_else(|| CliError::Runtime {
                command: command.clone(),
                reason: format!(
                    "the locked record for '{target}' has no resolved package name; run `anolisa repair {target}` first"
                ),
            })?;
            (
                Some(package.to_string()),
                matches!(
                    relation,
                    anolisa_core::domain::ManagementRelation::Managed { .. }
                ),
            )
        }
        Some(ProviderBinding::Owned { .. }) => {
            return Err(CliError::Runtime {
                command,
                reason: format!(
                    "component '{target}' changed provider authority while this uninstall was waiting for the lock; nothing was changed — re-run `anolisa uninstall {target}`"
                ),
            });
        }
        None if store.record_facts(ObjectKind::Component, target) != RecordFacts::Absent => {
            (None, false)
        }
        None => {
            return Err(CliError::Runtime {
                command,
                reason: format!(
                    "component '{target}' disappeared from state during uninstall; nothing removed"
                ),
            });
        }
    };

    // Re-observe and re-plan after taking the lock. Package identity,
    // relation, pending recovery and disposition now come from one locked
    // fact set; no pre-lock package or record-only plan can reach execution.
    let locked_request = ObserveRequest {
        kind: ObjectKind::Component,
        name: target,
        scope,
        native_package: native_package.as_deref(),
        observed_at: &now,
        verify_owned_files: false,
    };
    let facts = match assemble_facts(
        &locked_request,
        &store,
        Some(&provider),
        &layout,
        &journal_dir,
    ) {
        Ok(facts) => facts,
        Err(anolisa_core::facts::FactsError::Probe(
            anolisa_core::providers::ProviderError::Query(
                anolisa_platform::pkg_query::PackageQueryError::CommandMissing { command: bin },
            ),
        )) => {
            if record_is_managed || args.remove_system_package {
                return Err(tooling_missing_err(
                    &command,
                    &bin,
                    native_package.as_deref().unwrap_or(target),
                    target,
                    record_is_managed,
                ));
            }
            assemble_facts(&locked_request, &store, None, &layout, &journal_dir).map_err(|err| {
                CliError::Runtime {
                    command: command.clone(),
                    reason: err.to_string(),
                }
            })?
        }
        Err(err) => {
            return Err(CliError::Runtime {
                command: command.clone(),
                reason: err.to_string(),
            });
        }
    };
    let (steps, notes) = match plan(&intent, &facts) {
        Ok(Plan::Execute { steps, notes }) => (steps, notes),
        Ok(Plan::NoOp { .. }) => {
            return Err(CliError::Runtime {
                command,
                reason: format!(
                    "component '{target}' changed while this uninstall was waiting for the lock; nothing was changed — re-run `anolisa uninstall {target}`"
                ),
            });
        }
        Err(err) => return Err(plan_error_to_cli(err, target, &command, &store)),
    };
    if !steps.iter().all(|step| {
        matches!(
            step,
            Step::NativeTransaction { .. }
                | Step::Observe { .. }
                | Step::WriteRecord(_)
                | Step::DropRecord
        )
    }) {
        return Err(CliError::Runtime {
            command,
            reason: format!(
                "component '{target}' changed provider authority while this uninstall was waiting for the lock; nothing was changed — re-run `anolisa uninstall {target}`"
            ),
        });
    }
    let plan_labels: Vec<String> = steps.iter().map(step_label).collect();
    let disposition = disposition_for(&steps, &notes);
    let runs_native_txn = steps
        .iter()
        .any(|step| matches!(step, Step::NativeTransaction { .. }));
    if runs_native_txn && !is_root {
        let flag_suffix = if args.remove_system_package {
            " --remove-system-package"
        } else {
            ""
        };
        return Err(CliError::Runtime {
            command,
            reason: format!(
                "removing system RPM '{}' requires root privileges; re-run with sudo: `sudo anolisa uninstall {target}{flag_suffix}`",
                native_package.as_deref().unwrap_or(target)
            ),
        });
    }

    let evidence = JournalEvidence::new(&journal_dir, &store.operations);
    let mut journal_gate = LockedJournalGate::load(&_lock, evidence, &command)?;
    let mut journal = journal_gate.begin(COMMAND, target, state_path.clone(), &command)?;
    let operation_id = journal.operation_id.clone();

    let context = RecordContext {
        kind: ObjectKind::Component,
        name: target.to_string(),
        scope,
        now: now.clone(),
        operation_id: Some(operation_id.clone()),
        delegated: native_package.as_deref().map(|package| DelegatedIdentity {
            pm: NativePm::Rpm,
            package: package.to_string(),
        }),
        owned_artifact: None,
    };
    {
        let mut sink = StoreRecordSink::new(&mut store, &state_path, context);
        execute_delegated_steps(
            &steps,
            DelegatedExecutionTarget::new(NativePm::Rpm, native_package.as_deref()),
            &provider,
            &mut sink,
            &mut journal,
            &now,
        )
    }
    .map_err(|err| match err {
        // dnf missing even though the rpmdb query above succeeded: give the
        // same ownership-aware guidance as the query-missing branch rather
        // than a generic failure.
        anolisa_core::executor::ExecutionError::TransactionFailed {
            source:
                anolisa_core::providers::ProviderError::Transaction(
                    anolisa_platform::pkg_transaction::PackageTransactionError::CommandMissing {
                        command: bin,
                    },
                ),
            ..
        } => tooling_missing_err(
            &command,
            &bin,
            native_package.as_deref().unwrap_or(target),
            target,
            record_is_managed,
        ),
        other => CliError::Runtime {
            command: command.clone(),
            reason: format!(
                "uninstall of '{target}' failed: {other}; the native transaction is never undone automatically — run `anolisa repair {target}` to reconcile"
            ),
        },
    })?;

    // The manifest snapshot travels with the record. Best-effort: the
    // record drop is already committed.
    if let Err(err) = remove_component_manifest_snapshot(&layout, target, &command) {
        eprintln!("warning: {err}");
    }

    // Operation history is best-effort bookkeeping on top of the committed
    // record drop.
    store.operations.push(OperationRecord {
        id: operation_id.clone(),
        command: command.clone(),
        status: "ok".to_string(),
        started_at: now.clone(),
        finished_at: Some(now_iso8601()),
        parent_operation_id: None,
    });
    if let Err(err) = store.save(&state_path) {
        eprintln!("warning: failed to record operation history: {err}");
    }

    if matches!(disposition, UninstallDisposition::AlreadyAbsent) && !ctx.json && !ctx.quiet {
        let color = Palette::new(ctx.no_color);
        eprintln!(
            "{} RPM package '{}' is not present in rpmdb (already removed by a manual `rpm -e`); dropping ANOLISA state only",
            color.warn("warning:"),
            native_package.as_deref().unwrap_or(target),
        );
    }

    append_uninstall_log(
        &layout,
        ctx,
        target,
        &command,
        &operation_id,
        &now,
        &disposition,
        native_package.as_deref(),
    );

    render_result(
        ctx,
        target,
        native_package.as_deref(),
        &disposition,
        false,
        &plan_labels,
        Some(&operation_id),
    )
}

/// Execute an owned teardown plan (X1) through the raw teardown port.
#[expect(clippy::too_many_arguments)]
fn uninstall_owned(
    target: &str,
    ctx: &CliContext,
    layout: &anolisa_platform::fs_layout::FsLayout,
    state_path: &std::path::Path,
    journal_dir: &std::path::Path,
    scope: InstallationScope,
    now: &str,
    intent: &Intent,
    command: &str,
) -> Result<(), CliError> {
    // No root pre-check for owned teardown: `--prefix` may point at a
    // user-writable tree, and a genuine permission problem fails the exact
    // step with an honest journal status instead of a blanket refusal.

    // Lock, then re-read state under the lock: the file and service sets to
    // tear down must reflect what is recorded now, and the adapter guard
    // must hold at the moment of removal.
    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;
    let mut store = StateStore::load_for_layout(state_path, privilege::effective_uid(), layout)
        .map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!("failed to load installed state: {err}"),
        })?;
    ensure_no_adapter_claims(&store, target, command)?;
    let prior = match store
        .find(ObjectKind::Component, target)
        .map(|r| &r.binding)
    {
        Some(ProviderBinding::Owned { artifact }) => artifact.clone(),
        _ => {
            return Err(CliError::Runtime {
                command: command.to_string(),
                reason: format!(
                    "component '{target}' is no longer an owned installation; nothing was changed — re-run `anolisa uninstall {target}`"
                ),
            });
        }
    };

    let facts = assemble_facts(
        &ObserveRequest {
            kind: ObjectKind::Component,
            name: target,
            scope,
            native_package: None,
            observed_at: now,
            verify_owned_files: false,
        },
        &store,
        None,
        layout,
        journal_dir,
    )
    .map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: err.to_string(),
    })?;
    let (steps, notes) = match plan(intent, &facts) {
        Ok(Plan::Execute { steps, notes }) => (steps, notes),
        Ok(Plan::NoOp { .. }) => {
            return Err(CliError::Runtime {
                command: command.to_string(),
                reason: format!(
                    "component '{target}' changed while this uninstall was waiting for the lock; nothing was changed — re-run `anolisa uninstall {target}`"
                ),
            });
        }
        Err(err) => return Err(plan_error_to_cli(err, target, command, &store)),
    };
    if steps
        .iter()
        .any(|step| matches!(step, Step::NativeTransaction { .. } | Step::Observe { .. }))
    {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "component '{target}' changed provider authority while this uninstall was waiting for the lock; nothing was changed — re-run `anolisa uninstall {target}`"
            ),
        });
    }
    let plan_labels: Vec<String> = steps.iter().map(step_label).collect();
    let disposition = disposition_for(&steps, &notes);

    // Contract-driven uninstall hooks from the installed manifest snapshot.
    // Best-effort: a missing or unreadable snapshot means no hooks, never a
    // failed uninstall.
    let hooks = match common::installed_component_manifest_path(layout, target, COMMAND)
        .ok()
        .and_then(|path| ComponentManifest::from_file(&path).ok())
    {
        Some(manifest) => ResolvedLifecycleHooks {
            pre_uninstall: resolve_manifest_hooks(
                &manifest.install.hooks,
                layout,
                target,
                HookPhase::PreUninstall,
            )
            .unwrap_or_default(),
            post_uninstall: resolve_manifest_hooks(
                &manifest.install.hooks,
                layout,
                target,
                HookPhase::PostUninstall,
            )
            .unwrap_or_default(),
        },
        None => ResolvedLifecycleHooks::default(),
    };

    let evidence = JournalEvidence::new(journal_dir, &store.operations);
    let mut journal_gate = LockedJournalGate::load(&_lock, evidence, command)?;
    let mut journal = journal_gate.begin(COMMAND, target, state_path.to_path_buf(), command)?;
    let operation_id = journal.operation_id.clone();

    let outcome = {
        let mut ops = RawTeardownOps::new(
            ctx,
            layout,
            target.to_string(),
            operation_id.clone(),
            prior,
            hooks,
            &mut store,
            state_path,
        );
        execute_owned_steps(&steps, &mut ops, &mut journal)
    }
    .map_err(|err| owned_teardown_error_to_cli(err, target, scope, command))?;

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

    if !ctx.json && !ctx.quiet {
        let color = Palette::new(ctx.no_color);
        for warning in &outcome.warnings {
            eprintln!("{} {warning}", color.warn("warning:"));
        }
    }

    append_uninstall_log(
        layout,
        ctx,
        target,
        command,
        &operation_id,
        now,
        &disposition,
        None,
    );

    render_result(
        ctx,
        target,
        None,
        &disposition,
        false,
        &plan_labels,
        Some(&operation_id),
    )
}

/// The `--purge` pipeline: plan-only pending manifest-driven
/// config/cache/state discovery.
fn handle_purge(args: &UninstallArgs, ctx: &CliContext) -> Result<(), CliError> {
    let operation = LifecycleOperation::Purge;
    let input = args.component.as_str();
    let command = format!("{} {}", operation.as_str(), input);
    let scope_command = scope_guard_command(args, input);

    // Resolve against the full visible identity set, then plan only against
    // the writable root returned by the same snapshot.
    let (resolved, view) = common::resolve_mutation_target(input, ctx, &scope_command)?;
    let installed = view.writable.state;
    let target = resolved.as_str();

    if installed.find(ObjectKind::Component, target).is_none()
        && installed
            .dropped_capabilities
            .iter()
            .any(|name| name == target)
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
    if !ctx.dry_run {
        let claims: Vec<_> = installed
            .adapter_claims
            .iter()
            .filter(|claim| claim.component == target)
            .collect();
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

    if args.force && !ctx.dry_run {
        eprintln!("warning: --force is a spec stub today and has no behavioral effect yet");
    }

    let plan = LifecyclePlan::for_component_purge(target, &installed);

    if ctx.dry_run {
        if ctx.json {
            let payload = PlanDryRunPayload {
                dry_run: true,
                plan: &plan,
            };
            return render_json(COMMAND, &payload);
        }
        if !ctx.quiet {
            render_plan_human(&plan, ctx.no_color);
        }
        return Ok(());
    }

    // `purge` execute stays gated pending manifest-driven config /
    // cache / state discovery — print the same plan-only warning the
    // previous release emitted so wrappers continue to see the boundary
    // on stderr, then refuse with the gate's lift-condition hint.
    if !ctx.json {
        let palette = Palette::new(ctx.no_color);
        eprintln!(
            "{} purge execute is currently plan-only; only --dry-run is supported in this release",
            palette.warn("warning:"),
        );
    }

    Err(CliError::NotImplemented {
        command,
        hint: Some(
            "purge execute is gated pending manifest-driven config/cache/state              discovery; run with --dry-run to preview the plan, or use              `anolisa uninstall <component>` for the file-removal subset"
                .to_string(),
        ),
    })
}

fn scope_guard_command(args: &UninstallArgs, input: &str) -> String {
    let mut command = COMMAND.to_string();
    if args.purge {
        command.push_str(" --purge");
    }
    if args.remove_system_package {
        command.push_str(" --remove-system-package");
    }
    command.push(' ');
    command.push_str(input);
    command
}

/// What happens (or, on dry-run, would happen) to the backing package or
/// files. Distinguishes the outcomes the `package_removal` field must report
/// accurately, instead of collapsing "kept on purpose" and "already gone"
/// into one label.
#[derive(Clone, Copy, PartialEq, Eq)]
enum UninstallDisposition {
    /// `dnf remove` runs (X2), or would on dry-run.
    NativeRemove,
    /// Only the ANOLISA record is dropped: a tracked package without removal
    /// authority (X4), or a quarantined record (X5').
    StateOnly,
    /// Removal was planned but the package is not in rpmdb — already gone
    /// via a manual `rpm -e` (X3), so only the record is dropped.
    AlreadyAbsent,
    /// ANOLISA-owned files are removed from disk (X1).
    OwnedRemoval,
}

impl UninstallDisposition {
    /// Wire label for the `package_removal` field.
    fn label(self) -> &'static str {
        match self {
            Self::NativeRemove => "dnf remove",
            Self::StateOnly => "state only",
            Self::AlreadyAbsent => "already absent",
            Self::OwnedRemoval => "owned files removed",
        }
    }
}

/// Read the disposition off the plan: the planner already decided it.
fn disposition_for(steps: &[Step], notes: &[PlanNote]) -> UninstallDisposition {
    if notes.contains(&PlanNote::PackageAlreadyAbsent) {
        return UninstallDisposition::AlreadyAbsent;
    }
    if steps
        .iter()
        .any(|step| matches!(step, Step::NativeTransaction { .. }))
    {
        return UninstallDisposition::NativeRemove;
    }
    if steps
        .iter()
        .any(|step| matches!(step, Step::RemoveOwnedFiles))
    {
        return UninstallDisposition::OwnedRemoval;
    }
    UninstallDisposition::StateOnly
}

/// Build the actionable "rpm/dnf tooling missing" error, shared by the
/// rpmdb-probe and the `dnf remove` missing-binary branches so both give
/// identical guidance.
///
/// The escape hatch depends on the removal *driver*, not the flag: a managed
/// component is removed with or without the flag, so its actionable fallback
/// is `forget` (drop state, no package op); an adopted/observed removal is
/// driven solely by `--remove-system-package`, so there the fallback is to
/// drop the flag.
fn tooling_missing_err(
    command: &str,
    bin: &str,
    package: &str,
    target: &str,
    managed: bool,
) -> CliError {
    let alt = if managed {
        format!("run `anolisa forget {target}` to drop ANOLISA state without a package operation")
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

/// Refuse teardown while adapter receipts still claim the component.
/// Uninstall does not auto-cascade into framework state (a framework CLI
/// might be unavailable, and silently orphaning a registered plugin is worse
/// than refusing).
fn ensure_no_adapter_claims(
    store: &StateStore,
    target: &str,
    command: &str,
) -> Result<(), CliError> {
    let mut frameworks: Vec<&str> = store
        .adapter_claims
        .iter()
        .filter(|claim| claim.component == target)
        .map(|claim| claim.framework.as_str())
        .collect();
    if frameworks.is_empty() {
        return Ok(());
    }
    frameworks.sort_unstable();
    frameworks.dedup();
    Err(CliError::InvalidArgument {
        command: command.to_string(),
        reason: format!(
            "'{target}' has enabled adapters ({}); run `anolisa adapter disable {target}` \
             for each framework before uninstalling",
            frameworks.join(", ")
        ),
    })
}

/// Map a planning refusal to an actionable CLI error. The planner names the
/// way out; this mapping only renders it.
fn plan_error_to_cli(err: PlanError, target: &str, command: &str, store: &StateStore) -> CliError {
    match err {
        PlanError::NotInstalled => CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "component '{target}' is not installed — nothing to uninstall (run `anolisa status` to see what is installed)",
            ),
        },
        // The planner only says "claims exist"; the store names them.
        PlanError::AdapterClaimsActive => ensure_no_adapter_claims(store, target, command)
            .err()
            .unwrap_or_else(|| CliError::InvalidArgument {
                command: command.to_string(),
                reason: format!(
                    "'{target}' has enabled adapters; run `anolisa adapter disable {target}` first"
                ),
            }),
        PlanError::NeedsAttention => CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "the record for '{target}' was quarantined by the state migration and its package identity is unverified; run `anolisa repair {target}` first, or re-run without --remove-system-package to drop the record only"
            ),
        },
        PlanError::PendingOperation => CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "a previous operation on '{target}' is pending recovery; run `anolisa repair {target}` before retrying"
            ),
        },
        PlanError::RemoveSystemPackageRequiresSingleTarget => CliError::InvalidArgument {
            command: command.to_string(),
            reason:
                "--remove-system-package is only honored when uninstalling a single named component"
                    .to_string(),
        },
        PlanError::PackageUnresolved => CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "the record for '{target}' has no resolved package name; run `anolisa repair {target}` first"
            ),
        },
        other => CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!("cannot uninstall '{target}': {other:?}"),
        },
    }
}

/// Map an owned-executor failure onto an honest CLI error. X1 registers no
/// compensations, so the report is about what already happened, not what was
/// undone.
fn owned_teardown_error_to_cli(
    err: OwnedExecutionError,
    target: &str,
    scope: InstallationScope,
    command: &str,
) -> CliError {
    let repair = common::scoped_component_command(scope, "repair", target);
    let reason = match err {
        OwnedExecutionError::StepFailed { step, source, .. } => {
            format!(
                "uninstall of '{target}' failed at '{}': {source}; run `{repair}` to reconcile the record with what remains on disk",
                step_label(&step)
            )
        }
        OwnedExecutionError::RecoveryUncertain { detail, .. } => {
            format!("uninstall of '{target}' failed: {detail}; run `{repair}`")
        }
        other => format!("uninstall of '{target}' failed: {other}"),
    };
    CliError::Runtime {
        command: command.to_string(),
        reason,
    }
}

/// Human-facing label for a plan step (preview rendering).
fn step_label(step: &Step) -> String {
    match step {
        Step::NativeTransaction {
            action, packages, ..
        } => format!("dnf {} {}", action.verb(), packages.join(" ")),
        Step::Observe { packages } => format!("observe {}", packages.join(" ")),
        Step::WriteRecord(write) => format!("record: {}", write.label()),
        Step::DropRecord => "record: drop".to_string(),
        Step::RunHook(kind) => format!(
            "run {} hooks",
            match kind {
                HookKind::PreInstall => "pre-install",
                HookKind::PostInstall => "post-install",
                HookKind::PreUninstall => "pre-uninstall",
                HookKind::PostUninstall => "post-uninstall",
            }
        ),
        Step::StopServices => "stop services".to_string(),
        Step::RemoveOwnedFiles => "remove owned files".to_string(),
        other => format!("{other:?}"),
    }
}

/// Best-effort central-log record for a committed uninstall.
#[expect(clippy::too_many_arguments)]
fn append_uninstall_log(
    layout: &anolisa_platform::fs_layout::FsLayout,
    ctx: &CliContext,
    component: &str,
    command: &str,
    operation_id: &str,
    started_at: &str,
    disposition: &UninstallDisposition,
    package: Option<&str>,
) {
    let package = package.unwrap_or(component);
    let message = match disposition {
        UninstallDisposition::NativeRemove => format!(
            "uninstalled component {component}: removed RPM package {package} via dnf and dropped ANOLISA state"
        ),
        UninstallDisposition::StateOnly => format!(
            "uninstalled component {component}: dropped ANOLISA state; RPM package {package} left installed"
        ),
        UninstallDisposition::AlreadyAbsent => format!(
            "uninstalled component {component}: dropped ANOLISA state; RPM package {package} was already absent from rpmdb"
        ),
        UninstallDisposition::OwnedRemoval => format!(
            "uninstalled component {component}: removed ANOLISA-owned files and dropped state"
        ),
    };
    let log = CentralLog::open(layout.central_log.clone());
    let record = LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.to_string()),
        command: command.to_string(),
        source: "anolisa-cli".to_string(),
        component: Some(component.to_string()),
        severity: Severity::Info,
        message,
        actor: "cli".to_string(),
        install_mode: Some(ctx.install_mode.as_str().to_string()),
        started_at: started_at.to_string(),
        finished_at: Some(now_iso8601()),
        status: Some(LogStatus::Ok),
        objects: vec![component.to_string()],
        backup_ids: Vec::new(),
        warnings: Vec::new(),
        details: serde_json::Value::Null,
    };
    if let Err(err) = log.append(&record) {
        eprintln!("warning: failed to write central log: {err}");
    }
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

/// RFC3339 UTC timestamp, seconds precision (matches the install/update paths).
fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Wire shape for an uninstall result (`--json`) and its dry-run preview.
#[derive(serde::Serialize)]
struct UninstallResultPayload {
    component: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    package: Option<String>,
    /// [`UninstallDisposition::label`]: what happens (or would happen) to
    /// the backing package or files.
    package_removal: &'static str,
    /// Whether the ANOLISA state record was dropped (false on dry-run).
    state_dropped: bool,
    dry_run: bool,
    plan: Vec<String>,
    /// `None` on dry-run (nothing recorded).
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
}

/// Render the uninstall result (or its dry-run preview).
fn render_result(
    ctx: &CliContext,
    component: &str,
    package: Option<&str>,
    disposition: &UninstallDisposition,
    dry_run: bool,
    plan_labels: &[String],
    operation_id: Option<&str>,
) -> Result<(), CliError> {
    if ctx.json {
        return render_json(
            COMMAND,
            UninstallResultPayload {
                component: component.to_string(),
                package: package.map(str::to_string),
                package_removal: disposition.label(),
                state_dropped: !dry_run,
                dry_run,
                plan: plan_labels.to_vec(),
                operation_id: operation_id.map(str::to_string),
            },
        );
    }
    if ctx.quiet {
        return Ok(());
    }
    let color = Palette::new(ctx.no_color);
    if dry_run {
        println!(
            "{} {component} {}",
            color.command("uninstall"),
            color.muted("(dry-run — nothing removed)"),
        );
        for label in plan_labels {
            println!("  - {label}");
        }
        match disposition {
            UninstallDisposition::StateOnly if package.is_some() => println!(
                "  {}",
                color.muted(
                    "the system RPM stays installed; pass --remove-system-package to delegate removal to dnf"
                ),
            ),
            UninstallDisposition::AlreadyAbsent => println!(
                "  {}",
                color.muted(
                    "the RPM package is not in rpmdb; uninstall will drop ANOLISA state only"
                ),
            ),
            _ => {}
        }
        return Ok(());
    }
    println!("{} {component}", color.ok("✓ uninstalled"));
    match disposition {
        UninstallDisposition::NativeRemove => println!(
            "    {} removed RPM package {} via dnf",
            color.label("note:"),
            package.unwrap_or(component),
        ),
        UninstallDisposition::AlreadyAbsent => println!(
            "    {} ANOLISA state dropped; RPM package {} was already absent from rpmdb",
            color.label("note:"),
            package.unwrap_or(component),
        ),
        UninstallDisposition::StateOnly => match package {
            Some(package) => println!(
                "    {} ANOLISA state dropped; RPM package {package} left installed",
                color.label("note:"),
            ),
            None => println!("    {} ANOLISA state record dropped", color.label("note:")),
        },
        UninstallDisposition::OwnedRemoval => println!(
            "    {} removed ANOLISA-owned files and dropped state",
            color.label("note:"),
        ),
    }
    if let Some(id) = operation_id {
        println!("{} {}", color.label("operation_id:"), color.id(id));
    }
    Ok(())
}

/// JSON wire wrapper for a `--dry-run` [`LifecyclePlan`] (the generic
/// uninstall/purge plan view).
///
/// [`LifecyclePlan`] is a render-context-free planning model and deliberately
/// carries no `dry_run` field — the plan is identical whether or not the caller
/// asked to preview it. The CLI only serializes it on the `--dry-run` path, so
/// the flag is stamped here at the same `data` level as the component view's
/// [`UninstallResultPayload::dry_run`]. That gives clients a single `data.dry_run`
/// field to detect a dry-run across both views without a per-view schema branch.
///
/// `#[serde(flatten)]` keeps every plan field at the top of `data` (rather than
/// nesting it under a `plan` key), so the plan's existing wire shape is
/// preserved and only augmented with `dry_run`.
#[derive(serde::Serialize)]
struct PlanDryRunPayload<'a> {
    dry_run: bool,
    #[serde(flatten)]
    plan: &'a LifecyclePlan,
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::context::InstallMode;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::tempdir;

    use anolisa_core::adapter::claim::{AdapterClaim, ClaimStatus, DriverPayload, OpenClawClaim};
    use anolisa_core::state::{
        InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectStatus, Ownership,
        RpmMetadata,
    };
    use anolisa_platform::fs_layout::{FsLayout, InstallMode as LayoutInstallMode};
    use anolisa_platform::pkg_query::{PackageInfo, PackageQueryError, PackageVersion};
    use anolisa_platform::pkg_transaction::PackageTransactionError;

    fn ctx_with_prefix(
        json: bool,
        dry_run: bool,
        install_mode: InstallMode,
        prefix: Option<PathBuf>,
    ) -> CliContext {
        let root = prefix
            .as_deref()
            .expect("stateful uninstall tests require an isolated prefix");
        crate::test_support::context_for_root(
            root,
            install_mode,
            prefix.clone(),
            crate::test_support::TestContextOptions {
                json,
                dry_run,
                ..Default::default()
            },
        )
    }

    fn legacy_state_for_layout(layout: &FsLayout) -> InstalledState {
        InstalledState {
            install_mode: match layout.mode {
                LayoutInstallMode::System => StateInstallMode::System,
                LayoutInstallMode::User => StateInstallMode::User,
            },
            prefix: layout.prefix.clone(),
            ..InstalledState::default()
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
        use anolisa_core::{InstalledObject, ObjectKind, ObjectStatus};
        use anolisa_platform::fs_layout::FsLayout;

        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");

        let mut state = legacy_state_for_layout(&layout);
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
            provisioned_packages: Vec::new(),
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

    /// Dry-run renders the plan the real run would execute — and for an
    /// absent component there is no plan, only the planner's refusal. The
    /// preview must agree with reality instead of showing an empty success.
    #[test]
    fn uninstall_dry_run_on_unknown_component_reports_not_installed() {
        let tmp = tempdir().expect("tmpdir");
        let err = handle(
            args("agentsight", false),
            &ctx_with_prefix(
                false,
                true,
                InstallMode::System,
                Some(tmp.path().to_path_buf()),
            ),
        )
        .expect_err("dry-run must report the same refusal as a real run");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("not installed"), "{}", err.reason());
    }

    /// End-to-end success: when the component IS installed, the
    /// executor must remove the ANOLISA-owned file, drop the component
    /// object from `installed.toml`, write started + succeeded
    /// central-log entries, and return `Ok(())`.
    #[test]
    fn uninstall_execute_on_installed_component_removes_owned_files_and_succeeds() {
        use anolisa_core::{
            FileOwner, InstalledObject, ObjectKind, ObjectStatus, OwnedFile, OwnedFileKind,
        };
        use anolisa_platform::fs_layout::FsLayout;

        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));

        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");
        std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bin");
        let owned = layout.bin_dir.join("agentsight");
        std::fs::write(&owned, b"binary").expect("write owned");

        let mut state = legacy_state_for_layout(&layout);
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
                kind: OwnedFileKind::File,
                referent: None,
            }],
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
            provisioned_packages: Vec::new(),
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

        let after = StateStore::load(&state_path, 0).expect("reload state");
        assert!(
            after.find(ObjectKind::Component, "agentsight").is_none(),
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
            FileOwner, InstalledObject, ObjectKind, ObjectStatus, OwnedFile, OwnedFileKind,
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

        let mut state = legacy_state_for_layout(&layout);
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
                kind: OwnedFileKind::File,
                referent: None,
            }],
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
            provisioned_packages: Vec::new(),
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
            FileOwner, InstalledObject, ObjectKind, ObjectStatus, OwnedFile, OwnedFileKind,
        };
        use anolisa_platform::fs_layout::FsLayout;

        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");
        std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bin");
        let owned = layout.bin_dir.join("agentsight");
        std::fs::write(&owned, b"binary").expect("write owned");

        let mut state = legacy_state_for_layout(&layout);
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
                kind: OwnedFileKind::File,
                referent: None,
            }],
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
            provisioned_packages: Vec::new(),
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
        fn install(&self, _packages: &[&str]) -> Result<(), PackageTransactionError> {
            panic!("uninstall path must not delegate a dnf install");
        }
        fn update(&self, _packages: &[&str]) -> Result<(), PackageTransactionError> {
            panic!("uninstall path must not delegate a dnf update");
        }
        fn reinstall(&self, _packages: &[&str]) -> Result<(), PackageTransactionError> {
            panic!("uninstall path must not delegate a dnf reinstall");
        }
        fn remove(&self, packages: &[&str]) -> Result<(), PackageTransactionError> {
            let &[package] = packages else {
                panic!("expected exactly one package, got {packages:?}");
            };
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

    /// Host fake that mutates installed state during the pre-lock probe, then
    /// exposes both the old and new package identities. This deterministically
    /// exercises the lock window without threads or timing assumptions.
    struct RacingRpm {
        installed: RefCell<HashMap<String, Option<PackageInfo>>>,
        before_locked_read: RefCell<Option<Box<dyn FnOnce()>>>,
        remove_calls: RefCell<Vec<String>>,
    }

    impl RacingRpm {
        fn new(packages: &[&str], before_locked_read: impl FnOnce() + 'static) -> Self {
            let installed = packages
                .iter()
                .map(|package| {
                    (
                        (*package).to_string(),
                        Some(pkg_info(package, "2.2.0", Some("1.al8"), "x86_64")),
                    )
                })
                .collect();
            Self {
                installed: RefCell::new(installed),
                before_locked_read: RefCell::new(Some(Box::new(before_locked_read))),
                remove_calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl PackageQuery for RacingRpm {
        fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
            if let Some(mutate) = self.before_locked_read.borrow_mut().take() {
                mutate();
            }
            Ok(self.installed.borrow().get(package).cloned().flatten())
        }

        fn query_available(&self, _package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
            Ok(Vec::new())
        }
    }

    impl PackageTransaction for RacingRpm {
        fn install(&self, _packages: &[&str]) -> Result<(), PackageTransactionError> {
            panic!("uninstall path must not install")
        }

        fn update(&self, _packages: &[&str]) -> Result<(), PackageTransactionError> {
            panic!("uninstall path must not update")
        }

        fn reinstall(&self, _packages: &[&str]) -> Result<(), PackageTransactionError> {
            panic!("uninstall path must not reinstall")
        }

        fn remove(&self, packages: &[&str]) -> Result<(), PackageTransactionError> {
            let &[package] = packages else {
                panic!("expected one package, got {packages:?}");
            };
            self.remove_calls.borrow_mut().push(package.to_string());
            self.installed
                .borrow_mut()
                .insert(package.to_string(), None);
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
            provisioned_packages: Vec::new(),
        }
    }

    fn sample_claim(component: &str, framework: &str) -> AdapterClaim {
        AdapterClaim {
            claim_schema: 1,
            component: component.to_string(),
            framework: framework.to_string(),
            plugin_id: None,
            adapter_type: None,
            enabled_at: "2026-06-01T10:00:00Z".to_string(),
            resource_root: PathBuf::from("/tmp/anolisa-uninstall-test"),
            bundle_digest: None,
            driver_schema: 1,
            status: ClaimStatus::Enabled,
            notices: Vec::new(),
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

    fn load_state(ctx: &CliContext) -> StateStore {
        let layout = common::resolve_layout(ctx);
        StateStore::load(&layout.state_dir.join("installed.toml"), 0).expect("load state")
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

    fn args_rm(component: &str) -> UninstallArgs {
        UninstallArgs {
            component: component.to_string(),
            purge: false,
            remove_system_package: true,
            force: false,
        }
    }

    #[test]
    fn scope_guard_command_preserves_uninstall_flags() {
        assert_eq!(
            scope_guard_command(&args("system-tool", false), "system-tool"),
            "uninstall system-tool"
        );
        assert_eq!(
            scope_guard_command(&args("system-tool", true), "system-tool"),
            "uninstall --purge system-tool"
        );
        assert_eq!(
            scope_guard_command(&args_rm("system-tool"), "system-tool"),
            "uninstall --remove-system-package system-tool"
        );

        let mut both = args_rm("system-tool");
        both.purge = true;
        assert_eq!(
            scope_guard_command(&both, "system-tool"),
            "uninstall --purge --remove-system-package system-tool"
        );
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
            after.find(ObjectKind::Component, "copilot-shell").is_none(),
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

    #[test]
    fn uninstall_scope_mismatch_fails_before_dnf_remove() {
        let tmp = tempdir().expect("tmpdir");
        let ctx = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        let layout = common::resolve_layout(&ctx);
        let mut state = InstalledState::default();
        state.upsert_object(rpm_object(
            "copilot-shell",
            "copilot-shell",
            Ownership::RpmManaged,
        ));
        state
            .save(&layout.state_dir.join("installed.toml"))
            .expect("save mismatched state");
        let rpm = FakeRpm::present("copilot-shell");

        let err = run(args("copilot-shell", false), &ctx, &rpm, true)
            .expect_err("scope mismatch must fail closed");

        assert!(err.reason().contains("does not match the active layout"));
        assert_eq!(rpm.remove_calls.get(), 0, "dnf must not run");
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
                .find(ObjectKind::Component, "copilot-shell")
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
                .find(ObjectKind::Component, "copilot-shell")
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
                .find(ObjectKind::Component, "copilot-shell")
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
        assert_eq!(UninstallDisposition::NativeRemove.label(), "dnf remove");
        assert_eq!(UninstallDisposition::StateOnly.label(), "state only");
        assert_eq!(
            UninstallDisposition::AlreadyAbsent.label(),
            "already absent"
        );
        assert_eq!(
            UninstallDisposition::OwnedRemoval.label(),
            "owned files removed"
        );
    }

    /// `disposition_for` reads the outcome off the plan the planner produced:
    /// the already-absent note wins, then a native transaction, then owned
    /// file removal, and a bare record drop is state-only.
    #[test]
    fn disposition_for_reads_the_plan() {
        assert_eq!(
            disposition_for(&[Step::DropRecord], &[PlanNote::PackageAlreadyAbsent]).label(),
            "already absent"
        );
        assert_eq!(
            disposition_for(
                &[
                    Step::NativeTransaction {
                        pm: NativePm::Rpm,
                        action: anolisa_core::planner::NativeAction::Remove,
                        packages: vec!["cosh".to_string()],
                    },
                    Step::DropRecord,
                ],
                &[],
            )
            .label(),
            "dnf remove"
        );
        assert_eq!(
            disposition_for(&[Step::RemoveOwnedFiles, Step::DropRecord], &[]).label(),
            "owned files removed"
        );
        assert_eq!(
            disposition_for(&[Step::DropRecord], &[]).label(),
            "state only"
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
                .find(ObjectKind::Component, "copilot-shell")
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
                .find(ObjectKind::Component, "copilot-shell")
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
                .find(ObjectKind::Component, "copilot-shell")
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
                .find(ObjectKind::Component, "copilot-shell")
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
                .find(ObjectKind::Component, "copilot-shell")
                .is_some(),
            "state must be intact when the removal could not run",
        );
    }

    /// A raw component with `--remove-system-package` ignores the flag (warns) and
    /// runs the unchanged raw teardown: owned files removed, state dropped.
    #[test]
    fn uninstall_raw_with_remove_system_package_flag_warns_but_succeeds() {
        use anolisa_core::{FileOwner, OwnedFile, OwnedFileKind};
        use anolisa_platform::fs_layout::FsLayout;

        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");
        std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bin");
        let owned = layout.bin_dir.join("agentsight");
        std::fs::write(&owned, b"binary").expect("write owned");

        let mut state = legacy_state_for_layout(&layout);
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
                kind: OwnedFileKind::File,
                referent: None,
            }],
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
            provisioned_packages: Vec::new(),
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
                .find(ObjectKind::Component, "agentsight")
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
                .find(ObjectKind::Component, "copilot-shell")
                .is_some(),
            "component must remain when refused",
        );
    }

    /// The locked adapter guard names the offending frameworks and refuses;
    /// a component with no claims passes. This is the check the pipeline
    /// re-runs under the install lock so a concurrent `adapter enable`
    /// cannot slip past the pre-lock plan.
    #[test]
    fn adapter_claim_guard_names_frameworks_and_refuses() {
        let mut store = StateStore::empty();
        store
            .adapter_claims
            .push(sample_claim("copilot-shell", "openclaw"));

        let err = ensure_no_adapter_claims(&store, "copilot-shell", "uninstall copilot-shell")
            .expect_err("claims must refuse");

        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("adapter disable"));
        assert!(err.reason().contains("openclaw"));
        assert!(ensure_no_adapter_claims(&store, "other", "uninstall other").is_ok());
    }

    #[test]
    fn locked_replan_uses_the_current_package_identity() {
        let tmp = tempdir().expect("tmpdir");
        let c = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        seed(
            &c,
            vec![rpm_object("copilot-shell", "pkg-a", Ownership::RpmManaged)],
            Vec::new(),
        );
        let layout = common::resolve_layout(&c);
        let state_path = layout.state_dir.join("installed.toml");
        let mutate_path = state_path.clone();
        let rpm = RacingRpm::new(&["pkg-a", "pkg-b"], move || {
            let mut store = StateStore::load(&mutate_path, 0).expect("load for race");
            let record = store
                .find_mut(ObjectKind::Component, "copilot-shell")
                .expect("record");
            let ProviderBinding::Delegated { package, .. } = &mut record.binding else {
                panic!("delegated seed");
            };
            *package = anolisa_core::domain::PackageIdentity::Resolved {
                name: "pkg-b".to_string(),
            };
            store.save(&mutate_path).expect("persist race");
        });

        handle_with_deps(args("copilot-shell", false), &c, &rpm, &rpm, true)
            .expect("locked plan should uninstall the current binding");

        assert!(
            rpm.remove_calls.borrow().as_slice() == ["pkg-b"],
            "only the locked package may be removed: {:?}",
            rpm.remove_calls.borrow()
        );
        assert!(
            StateStore::load(&state_path, 0)
                .expect("reload")
                .find(ObjectKind::Component, "copilot-shell")
                .is_none(),
            "the current record should be dropped after pkg-b removal",
        );
    }

    #[test]
    fn stale_record_only_plan_cannot_drop_a_new_owned_record() {
        let tmp = tempdir().expect("tmpdir");
        let c = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        seed(
            &c,
            vec![rpm_object("copilot-shell", "pkg-a", Ownership::RpmObserved)],
            Vec::new(),
        );
        let layout = common::resolve_layout(&c);
        let state_path = layout.state_dir.join("installed.toml");
        let mutate_path = state_path.clone();
        let rpm = RacingRpm::new(&["pkg-a"], move || {
            let mut store = StateStore::load(&mutate_path, 0).expect("load for race");
            let record = store
                .find_mut(ObjectKind::Component, "copilot-shell")
                .expect("record");
            record.binding = ProviderBinding::Owned {
                artifact: anolisa_core::domain::OwnedArtifact {
                    version: "3.0.0".to_string(),
                    distribution_source: None,
                    raw_package: Some("copilot-shell".to_string()),
                    manifest_digest: None,
                    files: Vec::new(),
                    services: Vec::new(),
                    external_modified_files: Vec::new(),
                    provisioned_packages: Vec::new(),
                },
            };
            store.save(&mutate_path).expect("persist race");
        });

        let err = handle_with_deps(args("copilot-shell", false), &c, &rpm, &rpm, true)
            .expect_err("provider-family drift must abort");

        assert!(err.reason().contains("changed provider authority"), "{err}");
        assert!(
            rpm.remove_calls.borrow().is_empty(),
            "no stale package transaction may run",
        );
        assert!(
            matches!(
                StateStore::load(&state_path, 0)
                    .expect("reload")
                    .find(ObjectKind::Component, "copilot-shell")
                    .map(|record| &record.binding),
                Some(ProviderBinding::Owned { .. })
            ),
            "the new owned record must remain",
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

    /// Uninstall by package alias (e.g., "copilot-shell") must resolve to the
    /// canonical component name ("cosh") before addressing state, matching the
    /// resolution that `install` performs when recording the component.
    #[test]
    fn uninstall_rpm_observed_via_package_alias_succeeds() {
        let tmp = tempdir().expect("tmpdir");
        let c = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );

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

        // State records the canonical name "cosh", not the package alias
        // "copilot-shell" — this is what install writes after resolution.
        seed(
            &c,
            vec![rpm_object("cosh", "copilot-shell", Ownership::RpmObserved)],
            Vec::new(),
        );
        let _snapshot_dir = seed_manifest_snapshot(&c, "cosh");
        let rpm = FakeRpm::present("copilot-shell");

        // User types the alias, not the canonical name.
        run(args("copilot-shell", false), &c, &rpm, true).expect("uninstall via alias");

        assert_eq!(
            rpm.remove_calls.get(),
            0,
            "rpm-observed default must not run dnf remove",
        );
        let after = load_state(&c);
        assert!(
            after.find(ObjectKind::Component, "cosh").is_none(),
            "ANOLISA state record for 'cosh' must be dropped",
        );
    }

    // ── #1471: generic plan-view dry-run JSON contract ──────────────────

    /// #1471: the generic plan view's dry-run JSON must carry
    /// `data.dry_run == true` and keep every plan field flattened at the
    /// `data` top level (never nested under a `plan` key), so a client
    /// detects a dry-run with one field across both the plan and RPM
    /// views. For an absent component the flattened `phases` must be empty.
    #[test]
    fn plan_dry_run_payload_flattens_plan_and_stamps_dry_run() {
        let empty = anolisa_core::state_store::StateStore::empty();
        let plan = LifecyclePlan::for_component_uninstall("agentsight", &empty);
        let payload = PlanDryRunPayload {
            dry_run: true,
            plan: &plan,
        };
        let value = serde_json::to_value(&payload).expect("serialize payload");
        let obj = value
            .as_object()
            .expect("payload serializes to a JSON object");

        assert_eq!(
            obj.get("dry_run"),
            Some(&serde_json::Value::Bool(true)),
            "data.dry_run must be true: {value}",
        );
        assert!(
            obj.get("plan").is_none(),
            "flatten must not introduce a nested 'plan' key: {value}",
        );
        assert_eq!(
            obj.get("operation").and_then(|v| v.as_str()),
            Some("uninstall"),
            "flattened plan field 'operation' must sit at the data top level: {value}",
        );
        assert_eq!(
            obj.get("component").and_then(|v| v.as_str()),
            Some("agentsight"),
            "flattened plan field 'component' must sit at the data top level: {value}",
        );
        assert_eq!(
            obj.get("phases"),
            Some(&serde_json::Value::Array(Vec::new())),
            "absent-component phases must be empty: {value}",
        );
    }

    /// #1471: a dry-run over an *installed* raw-managed component keeps the
    /// legitimate phase `mode == "execute"` — that value describes the real
    /// execute-time behavior and must not be relabeled for the dry-run
    /// context — while the payload still reports `dry_run == true`.
    #[test]
    fn plan_dry_run_payload_installed_keeps_execute_mode() {
        use anolisa_core::{FileOwner, OwnedFile, OwnedFileKind};

        let owned = PathBuf::from("/usr/local/bin/agentsight");
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
            last_operation_id: None,
            managed: true,
            adopted: false,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: vec![OwnedFile {
                path: owned.clone(),
                owner: FileOwner::Anolisa,
                sha256: Some("0".repeat(64)),
                kind: OwnedFileKind::File,
                referent: None,
            }],
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
            provisioned_packages: Vec::new(),
        });
        let migration = anolisa_core::state_migration::migrate_state(
            &state.objects,
            anolisa_core::domain::InstallationScope::System,
        );
        assert!(
            migration.quarantined.is_empty(),
            "fixtures must migrate cleanly"
        );
        let mut store = anolisa_core::state_store::StateStore::empty();
        store.installations = migration.active;

        let plan = LifecyclePlan::for_component_uninstall("agentsight", &store);
        let payload = PlanDryRunPayload {
            dry_run: true,
            plan: &plan,
        };
        let value = serde_json::to_value(&payload).expect("serialize payload");
        let obj = value
            .as_object()
            .expect("payload serializes to a JSON object");

        assert_eq!(
            obj.get("dry_run"),
            Some(&serde_json::Value::Bool(true)),
            "installed dry-run must still stamp dry_run: {value}",
        );
        let phases = obj
            .get("phases")
            .and_then(|v| v.as_array())
            .expect("phases array present");
        let remove_file = phases
            .iter()
            .find(|p| p.get("name").and_then(|v| v.as_str()) == Some("remove_file"))
            .expect("owned-file removal phase present for installed component");
        assert_eq!(
            remove_file.get("mode").and_then(|v| v.as_str()),
            Some("execute"),
            "the owned-file removal phase keeps mode=execute: {value}",
        );
        let remove_state = phases
            .iter()
            .find(|p| p.get("name").and_then(|v| v.as_str()) == Some("remove_state"))
            .expect("remove_state phase present for installed component");
        assert_eq!(
            remove_state.get("mode").and_then(|v| v.as_str()),
            Some("execute"),
            "the remove_state phase keeps mode=execute: {value}",
        );
    }

    /// #1471: `--purge --dry-run` shares the same wrapper, so its JSON also
    /// carries `data.dry_run == true` with the plan flattened at the top.
    #[test]
    fn plan_dry_run_payload_covers_purge() {
        let empty = anolisa_core::state_store::StateStore::empty();
        let plan = LifecyclePlan::for_component_purge("agentsight", &empty);
        let payload = PlanDryRunPayload {
            dry_run: true,
            plan: &plan,
        };
        let value = serde_json::to_value(&payload).expect("serialize payload");
        let obj = value
            .as_object()
            .expect("payload serializes to a JSON object");

        assert_eq!(
            obj.get("dry_run"),
            Some(&serde_json::Value::Bool(true)),
            "purge dry-run must stamp dry_run: {value}",
        );
        assert_eq!(
            obj.get("operation").and_then(|v| v.as_str()),
            Some("purge"),
            "purge operation must sit flattened at the data top level: {value}",
        );
        assert!(
            obj.get("plan").is_none(),
            "purge payload must also stay flat: {value}",
        );
    }
}
