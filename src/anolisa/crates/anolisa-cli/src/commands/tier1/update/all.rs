//! `update all` batch support: every recorded component runs through the
//! U-series planner, and the delegated refreshes (U5) merge into **one**
//! native transaction so dnf's solver resolves the whole set at once.
//!
//! A merged transaction that fails degrades by fact: a member whose
//! installed EVR provably did not move re-plans individually so the
//! offending package fails with its own diagnostic; a member whose EVR
//! moved anyway keeps a `Partial` journal and routes to repair
//! (forward-only, never retried). Components that do not plan a delegated
//! refresh — owned records, NoOps, planning errors — keep the per-item
//! pipeline unchanged.

use std::collections::HashMap;

use serde::Serialize;

use anolisa_core::domain::NativePm;
use anolisa_core::executor::{
    DelegatedExecutionTarget, PHASE_NATIVE_TXN, delegated_recovery_context,
    execute_delegated_steps_resumed,
};
use anolisa_core::facts::JournalEvidence;
use anolisa_core::lock::InstallLock;
use anolisa_core::planner::{NativeAction, NativeProbe, RecordWrite, Step};
use anolisa_core::providers::DelegatedProvider;
use anolisa_core::record_sink::{DelegatedIdentity, RecordContext, StoreRecordSink};
use anolisa_core::state::{ObjectKind, OperationRecord};
use anolisa_core::state_store::StateStore;
use anolisa_core::transaction::{
    Transaction, TransactionOutcomeStatus, TransactionStep, mint_operation_id,
};
use anolisa_platform::pkg_query::PackageQuery;
use anolisa_platform::pkg_transaction::PackageTransaction;
use anolisa_platform::privilege;

use crate::color::Palette;
use crate::commands::common;
use crate::commands::tier1::recovery::LockedJournalGate;
use crate::commands::tier1::rpm_install;
use crate::context::CliContext;
use crate::response::{CliError, render_json, render_json_with_status};

use super::{
    COMMAND, PlannedComponentUpdate, PlannedUpdateRoute, UpdateOutcome, append_update_log,
    complete_delegated_update, native_update_authorized, now_iso8601, plan_component_update,
    step_label, update_backends, update_component_with_deps,
};

const BATCH_COMMAND: &str = "update all";

/// Wire shape for a batch entry. `status` is one of:
/// `updated` | `planned` (dry-run) | `already-current` | `failed`.
#[derive(Serialize)]
struct UpdateAllItem {
    component: String,
    status: &'static str,
    reason: Option<String>,
    /// Planned step labels, present only on dry-run for members of the
    /// merged group — the per-item pipeline renders its own plan.
    #[serde(skip_serializing_if = "Option::is_none")]
    plan: Option<Vec<String>>,
}

#[derive(Serialize)]
struct UpdateAllPayload {
    total: usize,
    updated: usize,
    planned: usize,
    /// Idempotent NoOps: the record already covers the latest version.
    already_current: usize,
    failed: usize,
    dry_run: bool,
    /// Packages that share one merged native transaction, present only when
    /// the batch found two or more delegated refreshes to merge.
    #[serde(skip_serializing_if = "Option::is_none")]
    merged_transaction: Option<Vec<String>>,
    items: Vec<UpdateAllItem>,
}

pub(crate) fn handle_update_all(ctx: &CliContext) -> Result<(), CliError> {
    let layout = common::resolve_layout(ctx);
    let state_path = layout.state_dir.join("installed.toml");
    let store = StateStore::load_for_layout(&state_path, privilege::effective_uid(), &layout)
        .map_err(|err| CliError::Runtime {
            command: BATCH_COMMAND.to_string(),
            reason: format!("failed to load installed state: {err}"),
        })?;
    let names: Vec<String> = store
        .installations
        .iter()
        .filter(|installation| installation.kind == ObjectKind::Component)
        .map(|installation| installation.name.clone())
        .collect();
    drop(store);

    if names.is_empty() {
        if !ctx.quiet && !ctx.json {
            let color = Palette::new(ctx.no_color);
            println!(
                "{}",
                color.muted("no components are recorded in ANOLISA state; nothing to update")
            );
        }
        if ctx.json {
            return render_json(
                BATCH_COMMAND,
                UpdateAllPayload {
                    total: 0,
                    updated: 0,
                    planned: 0,
                    already_current: 0,
                    failed: 0,
                    dry_run: ctx.dry_run,
                    merged_transaction: None,
                    items: Vec::new(),
                },
            );
        }
        return Ok(());
    }

    // Suppress per-component rendering: the batch owns the final output.
    let suppressed_ctx = CliContext {
        json: false,
        quiet: true,
        ..ctx.clone()
    };

    // Peek phase: plan every component read-only and classify. Delegated
    // refreshes (U5) merge into one native transaction; everything else
    // (owned plans, NoOps, planning errors) re-plans through the per-item
    // pipeline, which reproduces the same outcome — the extra probe is
    // cheap next to a dnf run.
    let mut merged: Vec<MergedUpdate> = Vec::new();
    let mut per_item: Vec<String> = Vec::new();
    for name in &names {
        let candidate = update_backends(name, &suppressed_ctx)
            .and_then(|(query, txn)| plan_component_update(name, &suppressed_ctx, &query, &txn))
            .ok()
            .and_then(|planned| {
                merged_update_package(&planned).map(|package| MergedUpdate {
                    name: name.clone(),
                    package,
                    planned,
                })
            });
        match candidate {
            Some(item) => merged.push(item),
            None => per_item.push(name.clone()),
        }
    }

    // A single delegated refresh gains nothing from merging; keep it on the
    // per-item pipeline.
    if merged.len() < 2 {
        per_item = names.clone();
        merged.clear();
    }
    let merged_transaction: Option<Vec<String>> =
        (!merged.is_empty()).then(|| merged.iter().map(|item| item.package.clone()).collect());

    let mut results: HashMap<String, UpdateAllItem> = HashMap::with_capacity(names.len());

    if !merged.is_empty() {
        if !ctx.quiet && !ctx.json {
            let color = Palette::new(ctx.no_color);
            let members = merged
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            println!("{} {members} (one rpm transaction)", color.label("==>"));
        }
        if ctx.dry_run {
            // Each member previews its own plan in the single-component
            // format; the group header above already announced that the
            // native transactions will merge into one dnf run.
            for item in &merged {
                let steps = match &item.planned.route {
                    PlannedUpdateRoute::Delegated { steps } => steps.as_slice(),
                    _ => &[],
                };
                let labels: Vec<String> = steps.iter().map(step_label).collect();
                if !ctx.quiet && !ctx.json {
                    let color = Palette::new(ctx.no_color);
                    println!(
                        "{} {} {}",
                        color.command("update"),
                        item.name,
                        color.muted("(dry-run — nothing updated)"),
                    );
                    if let Some(from) = &item.planned.native_from {
                        println!("{} {from}", color.label("current:"));
                    }
                    for label in &labels {
                        println!("  - {label}");
                    }
                }
                results.insert(
                    item.name.clone(),
                    UpdateAllItem {
                        component: item.name.clone(),
                        status: "planned",
                        reason: None,
                        plan: Some(labels),
                    },
                );
            }
        } else {
            for item in execute_merged_updates(merged, ctx) {
                results.insert(item.component.clone(), item);
            }
        }
    }

    for name in &per_item {
        if !ctx.quiet && !ctx.json {
            let color = Palette::new(ctx.no_color);
            println!("{} {name}", color.label("==>"));
        }
        let outcome = update_backends(name, &suppressed_ctx).and_then(|(query, txn)| {
            update_component_with_deps(name, &suppressed_ctx, &query, &txn, privilege::is_root())
        });
        let item = match outcome {
            Ok(outcome) => UpdateAllItem {
                component: name.clone(),
                status: item_status(outcome, ctx.dry_run),
                reason: None,
                plan: None,
            },
            Err(err) => UpdateAllItem {
                component: name.clone(),
                status: "failed",
                reason: Some(err.reason().to_string()),
                plan: None,
            },
        };
        results.insert(name.clone(), item);
    }

    // Rebuild the summary in the original record order.
    let items: Vec<UpdateAllItem> = names
        .iter()
        .filter_map(|name| results.remove(name))
        .collect();

    let updated = items.iter().filter(|i| i.status == "updated").count();
    let planned = items.iter().filter(|i| i.status == "planned").count();
    let already_current = items
        .iter()
        .filter(|i| i.status == "already-current")
        .count();
    let failed = items.iter().filter(|i| i.status == "failed").count();

    if ctx.json {
        // The batch summary is the single, complete JSON response; a partial
        // failure returns BatchPartial so the exit code is non-zero without
        // a second render.
        render_json_with_status(
            BATCH_COMMAND,
            failed == 0,
            UpdateAllPayload {
                total: names.len(),
                updated,
                planned,
                already_current,
                failed,
                dry_run: ctx.dry_run,
                merged_transaction,
                items,
            },
        )?;
        return if failed > 0 {
            Err(CliError::BatchPartial {
                command: BATCH_COMMAND.to_string(),
            })
        } else {
            Ok(())
        };
    }

    if !ctx.quiet {
        let color = Palette::new(ctx.no_color);
        println!();
        let ok_word = if ctx.dry_run { "planned" } else { "updated" };
        let ok_count = if ctx.dry_run { planned } else { updated };
        let current_segment = if already_current > 0 {
            format!("  already-current={already_current}")
        } else {
            String::new()
        };
        if failed == 0 {
            println!(
                "{} total={}  {ok_word}={ok_count}{current_segment}",
                color.label("summary:"),
                names.len(),
            );
        } else {
            let failed_names: Vec<&str> = items
                .iter()
                .filter(|i| i.status == "failed")
                .map(|i| i.component.as_str())
                .collect();
            println!(
                "{} total={}  {ok_word}={ok_count}{current_segment}  failed={failed} ({})",
                color.label("summary:"),
                names.len(),
                failed_names.join(", "),
            );
            for item in items.iter().filter(|i| i.status == "failed") {
                if let Some(reason) = &item.reason {
                    eprintln!("{} {}: {reason}", color.err("failed:"), item.component);
                }
            }
        }
    }

    if failed > 0 {
        Err(CliError::BatchPartial {
            command: BATCH_COMMAND.to_string(),
        })
    } else {
        Ok(())
    }
}

/// Batch status string for a successful per-item update, combining the
/// outcome with dry-run: nothing is written on dry-run, so a would-be update
/// reads "planned" while an idempotent NoOp reads the same either way.
fn item_status(outcome: UpdateOutcome, dry_run: bool) -> &'static str {
    match (outcome, dry_run) {
        (UpdateOutcome::Updated, false) => "updated",
        (UpdateOutcome::Updated, true) => "planned",
        (UpdateOutcome::AlreadyCurrent, _) => "already-current",
    }
}

/// One member of the merged delegated group: a U5 refresh whose native
/// transaction can share the batch dnf run.
struct MergedUpdate {
    /// Component name as the batch addressed it (summary key).
    name: String,
    /// Native package the plan updates.
    package: String,
    /// The read-only planning result; its route holds the U5 steps.
    planned: PlannedComponentUpdate,
}

/// The native package of a mergeable plan: exactly the delegated refresh
/// shape (U5) over a single package. Anything else keeps the per-item
/// pipeline.
fn merged_update_package(planned: &PlannedComponentUpdate) -> Option<String> {
    let PlannedUpdateRoute::Delegated { steps } = &planned.route else {
        return None;
    };
    match steps.as_slice() {
        [
            Step::NativeTransaction {
                action: NativeAction::Update,
                packages,
                ..
            },
            Step::Observe { packages: observed },
            Step::WriteRecord(RecordWrite::RefreshObservation),
        ] if packages.len() == 1 && observed == packages => Some(packages[0].clone()),
        _ => None,
    }
}

/// Execute the merged group against the live host: real backends pointed at
/// the configured ANOLISA repo, degrade re-plans through the per-item
/// pipeline.
fn execute_merged_updates(group: Vec<MergedUpdate>, ctx: &CliContext) -> Vec<UpdateAllItem> {
    let suppressed_ctx = CliContext {
        json: false,
        quiet: true,
        ..ctx.clone()
    };
    let (query, txn) = match update_backends(&group[0].name, &suppressed_ctx) {
        Ok(backends) => backends,
        Err(err) => {
            let reason = err.reason().to_string();
            return group
                .iter()
                .map(|item| failed_item(&item.name, reason.clone()))
                .collect();
        }
    };
    let mut degrade = |name: &str| {
        let (query, txn) = update_backends(name, &suppressed_ctx)?;
        update_component_with_deps(name, &suppressed_ctx, &query, &txn, privilege::is_root())
    };
    execute_merged_updates_with_deps(group, ctx, &query, &txn, privilege::is_root(), &mut degrade)
}

/// Core of the merged execution with the backends and the degrade pipeline
/// injected, so tests drive every branch without a live dnf.
///
/// One native transaction covers every member; each member keeps its own
/// journal (subject = component) so an interruption leaves per-component
/// pending journals that route the next intent to repair. After the
/// transaction commits, each member's remaining steps (observe, refresh)
/// run with its own record sink. A transaction failure degrades by fact:
/// members whose installed EVR did not move re-plan individually through
/// `degrade` (their slot is provably untouched), members whose EVR moved
/// anyway get a `Partial` journal and a repair hint (forward-only, never
/// retried).
fn execute_merged_updates_with_deps(
    group: Vec<MergedUpdate>,
    ctx: &CliContext,
    query: &dyn PackageQuery,
    txn: &dyn PackageTransaction,
    is_root: bool,
    degrade: &mut dyn FnMut(&str) -> Result<UpdateOutcome, CliError>,
) -> Vec<UpdateAllItem> {
    let all_failed = |group: &[MergedUpdate], reason: &str| -> Vec<UpdateAllItem> {
        group
            .iter()
            .map(|item| failed_item(&item.name, reason.to_string()))
            .collect()
    };

    if !is_root {
        return all_failed(
            &group,
            "updating system RPMs requires root privileges; re-run with sudo: `sudo anolisa update all`",
        );
    }

    let layout = common::resolve_layout(ctx);
    let state_path = layout.state_dir.join("installed.toml");
    let journal_dir = rpm_install::journal_dir(&layout);
    let now = now_iso8601();

    let _lock = match InstallLock::acquire(&layout.lock_file) {
        Ok(lock) => lock,
        Err(err) => return all_failed(&group, &format!("failed to acquire install lock: {err}")),
    };
    let mut store =
        match StateStore::load_for_layout(&state_path, privilege::effective_uid(), &layout) {
            Ok(store) => store,
            Err(err) => {
                return all_failed(&group, &format!("failed to load installed state: {err}"));
            }
        };
    let evidence = JournalEvidence::new(&journal_dir, &store.operations);
    let mut journal_gate = match LockedJournalGate::load(&_lock, evidence, BATCH_COMMAND) {
        Ok(gate) => gate,
        Err(err) => return all_failed(&group, &err.reason()),
    };

    // Re-validate each member's authority under the lock and open its
    // journal, mirroring the single-component race check.
    let mut items: Vec<UpdateAllItem> = Vec::with_capacity(group.len());
    let mut active: Vec<(MergedUpdate, Transaction)> = Vec::with_capacity(group.len());
    for item in group {
        let target = item.planned.target.as_str();
        if !native_update_authorized(&store, target, Some(&item.package)) {
            items.push(failed_item(
                &item.name,
                format!(
                    "component '{target}' changed while this update was planning; nothing was changed — re-run `anolisa update {target}`"
                ),
            ));
            continue;
        }
        match journal_gate.begin(COMMAND, target, state_path.clone(), BATCH_COMMAND) {
            Ok(journal) => active.push((item, journal)),
            Err(err) => items.push(failed_item(&item.name, err.reason().to_string())),
        }
    }
    if active.is_empty() {
        return items;
    }

    // Journal the shared transaction step in every member's journal before
    // dnf runs: an interruption mid-transaction leaves each component with a
    // pending journal naming the merged package set.
    let all_packages: Vec<String> = active
        .iter()
        .map(|(item, _)| item.package.clone())
        .collect();
    let txn_label = all_packages.join(",");
    for (item, journal) in &mut active {
        let plan_steps = match &item.planned.route {
            PlannedUpdateRoute::Delegated { steps } => steps.as_slice(),
            _ => unreachable!("merged members are classified as delegated"),
        };
        let journal_result = delegated_recovery_context(
            DelegatedExecutionTarget::new(NativePm::Rpm, Some(&item.package)),
            plan_steps,
        )
        .map_err(|err| err.to_string())
        .and_then(|context| {
            journal
                .record_delegated_steps(
                    context,
                    [TransactionStep::planned(
                        PHASE_NATIVE_TXN,
                        txn_label.clone(),
                        "update",
                        None,
                    )],
                )
                .map_err(|err| err.to_string())
        });
        if let Err(err) = journal_result {
            // A journal that cannot be written now would also not absorb the
            // transaction outcome; refusing the whole group before any side
            // effect is the honest move.
            let reason = format!(
                "failed to journal the merged transaction for '{}': {err}",
                item.name
            );
            for (member, mut journal) in active {
                let _ = journal.finish(TransactionOutcomeStatus::Failed);
                items.push(failed_item(&member.name, reason.clone()));
            }
            return items;
        }
    }

    let provider = DelegatedProvider::new(query, txn);
    match provider.transact(NativeAction::Update, &all_packages) {
        Ok(()) => {
            // The members' per-component operations link back to one parent
            // batch operation: the audit trail must show that these refreshes
            // came out of a single native transaction, not N independent
            // updates. The parent owns no journal — each member journals
            // for itself.
            let batch_operation_id = mint_operation_id("update-all");
            let members_start = items.len();
            for (item, mut journal) in active {
                let target = item.planned.target.clone();
                if let Err(err) = journal.mark_done(0) {
                    items.push(failed_item(
                        &item.name,
                        format!(
                            "update of '{target}' failed: the merged native transaction committed but its journal could not be updated: {err}; run `anolisa repair {target}` to reconcile"
                        ),
                    ));
                    continue;
                }
                let operation_id = journal.operation_id.clone();
                let context = RecordContext {
                    kind: ObjectKind::Component,
                    name: target.clone(),
                    scope: item.planned.scope,
                    now: now.clone(),
                    operation_id: Some(operation_id.clone()),
                    delegated: Some(DelegatedIdentity {
                        pm: NativePm::Rpm,
                        package: item.package.clone(),
                    }),
                    owned_artifact: None,
                };
                // The member's own plan tail: observe its package, refresh
                // its record. The shared transaction step is already done.
                let tail = match &item.planned.route {
                    PlannedUpdateRoute::Delegated { steps } => &steps[1..],
                    _ => unreachable!("merged members are classified as delegated"),
                };
                let result = {
                    let mut sink = StoreRecordSink::new(&mut store, &state_path, context);
                    execute_delegated_steps_resumed(
                        tail,
                        DelegatedExecutionTarget::new(NativePm::Rpm, Some(&item.package)),
                        &provider,
                        &mut sink,
                        &mut journal,
                        &now,
                        true,
                    )
                };
                match result {
                    Ok(outcome) => {
                        // Same completion semantics as the single-component
                        // path: the member's record refresh persisted, so
                        // refresh its contract snapshot. A refresh failure
                        // only demotes this member (and the batch summary
                        // below) — every other member keeps its own outcome.
                        let completion = complete_delegated_update(
                            &layout,
                            ctx,
                            &target,
                            &item.package,
                            BATCH_COMMAND,
                        );
                        let command = format!("{COMMAND} {target}");
                        store.operations.push(OperationRecord {
                            id: operation_id.clone(),
                            command: command.clone(),
                            status: completion.operation_status.to_string(),
                            started_at: now.clone(),
                            finished_at: Some(now_iso8601()),
                            parent_operation_id: Some(batch_operation_id.clone()),
                        });
                        let to_version = outcome
                            .observation
                            .as_ref()
                            .map(|o| o.evr.clone().unwrap_or_else(|| o.version.clone()));
                        append_update_log(
                            &layout,
                            ctx,
                            &target,
                            &command,
                            &operation_id,
                            &now,
                            &item.package,
                            to_version.as_deref(),
                            completion.failure.as_deref(),
                        );
                        if let Some(reason) = completion.failure {
                            items.push(failed_item(
                                &item.name,
                                format!(
                                    "the update of '{target}' committed, but {reason}; run `anolisa repair {target}` to reconcile"
                                ),
                            ));
                            continue;
                        }
                        let moved = match (&item.planned.native_from, &to_version) {
                            (Some(from), Some(to)) => from != to,
                            _ => true,
                        };
                        items.push(UpdateAllItem {
                            component: item.name.clone(),
                            status: if moved { "updated" } else { "already-current" },
                            reason: None,
                            plan: None,
                        });
                    }
                    Err(err) => items.push(failed_item(
                        &item.name,
                        format!(
                            "update of '{target}' failed: {err}; the native transaction is never undone automatically — run `anolisa repair {target}` to reconcile"
                        ),
                    )),
                }
            }
            // The parent record itself: `partial` when any member's tail
            // failed after the shared transaction committed, so the history
            // reads the same as the members' journals.
            let any_member_failed = items[members_start..]
                .iter()
                .any(|item| item.status == "failed");
            store.operations.push(OperationRecord {
                id: batch_operation_id,
                command: BATCH_COMMAND.to_string(),
                status: if any_member_failed { "partial" } else { "ok" }.to_string(),
                started_at: now.clone(),
                finished_at: Some(now_iso8601()),
                parent_operation_id: None,
            });
            // Operation history is best-effort bookkeeping on top of the
            // committed record refreshes, exactly like the single-component
            // path.
            if let Err(err) = store.save(&state_path) {
                eprintln!("warning: failed to record operation history: {err}");
            }
            items
        }
        Err(source) => {
            // Forward-only classification: only the exact pre-transaction
            // EVR proves a member untouched. Every other observation remains
            // Partial because retrying could apply a second native update.
            let reason = source.to_string();
            let mut clean: Vec<String> = Vec::new();
            for (item, mut journal) in active {
                let target = item.planned.target.as_str();
                let uncertain_reason = match provider.observe(&item.package, &now) {
                    Ok(NativeProbe::Present { observation, .. }) => {
                        let seen = observation.evr.unwrap_or(observation.version);
                        match &item.planned.native_from {
                            Some(from) if &seen == from => None,
                            Some(_) => Some("changed in the rpmdb".to_string()),
                            None => {
                                Some("has no planning-time EVR for a safe comparison".to_string())
                            }
                        }
                    }
                    Ok(NativeProbe::Absent) => Some("is absent from the rpmdb".to_string()),
                    Ok(NativeProbe::MultipleVersions { .. }) => {
                        Some("has multiple installed versions in the rpmdb".to_string())
                    }
                    Ok(NativeProbe::NotProbed) => {
                        Some("could not be re-observed in the rpmdb".to_string())
                    }
                    Err(err) => Some(format!("could not be re-observed in the rpmdb: {err}")),
                };
                let journal_outcome = if let Some(uncertain_reason) = uncertain_reason {
                    items.push(failed_item(
                        &item.name,
                        format!(
                            "merged native transaction failed and '{}' {uncertain_reason}: {reason}; run `anolisa repair {target}` to reconcile",
                            item.package,
                        ),
                    ));
                    TransactionOutcomeStatus::Partial
                } else {
                    clean.push(item.name.clone());
                    TransactionOutcomeStatus::Failed
                };
                if let Err(err) = journal
                    .mark_failed(0, &reason)
                    .and_then(|()| journal.finish(journal_outcome))
                {
                    eprintln!(
                        "warning: failed to journal the merged transaction outcome for '{}': {err}",
                        item.name
                    );
                }
            }
            drop(store);
            drop(_lock);

            if clean.is_empty() {
                return items;
            }
            eprintln!(
                "warning: merged rpm transaction failed ({reason}); retrying {} component(s) individually",
                clean.len()
            );
            for name in clean {
                match degrade(&name) {
                    Ok(outcome) => items.push(UpdateAllItem {
                        component: name,
                        status: item_status(outcome, false),
                        reason: None,
                        plan: None,
                    }),
                    Err(err) => items.push(failed_item(&name, err.reason().to_string())),
                }
            }
            items
        }
    }
}

fn failed_item(name: &str, reason: String) -> UpdateAllItem {
    UpdateAllItem {
        component: name.to_string(),
        status: "failed",
        reason: Some(reason),
        plan: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::RefCell;
    use std::collections::HashMap as StdHashMap;

    use anolisa_core::domain::InstallationScope;
    use anolisa_core::facts::{JournalEvidence, pending_journal_for};
    use anolisa_core::state::{
        InstallMode as StateInstallMode, InstalledObject, InstalledState, Ownership,
    };
    use anolisa_platform::pkg_query::{PackageInfo, PackageQueryError};
    use anolisa_platform::pkg_transaction::PackageTransactionError;

    use super::super::tests::{
        ctx, load_store, pkg_info, rpm_object, seed_package_contract_and_stale_snapshot,
    };
    use crate::context::InstallMode;

    const NOW: &str = "2026-07-17T00:00:00Z";

    /// Multi-package host fake: rpmdb reads come from a static map, and the
    /// transaction records each call's full package set so tests can pin
    /// that a merged batch shared one dnf run.
    struct FakeHost {
        installed: StdHashMap<String, PackageInfo>,
        multiple_versions: Vec<String>,
        calls: RefCell<Vec<(String, Vec<String>)>>,
        fail_update: bool,
    }

    impl FakeHost {
        fn new(installed: &[(&str, PackageInfo)], fail_update: bool) -> Self {
            Self {
                installed: installed
                    .iter()
                    .map(|(name, info)| ((*name).to_string(), info.clone()))
                    .collect(),
                multiple_versions: Vec::new(),
                calls: RefCell::new(Vec::new()),
                fail_update,
            }
        }

        fn with_multiple_versions(mut self, packages: &[&str]) -> Self {
            self.multiple_versions = packages
                .iter()
                .map(|package| (*package).to_string())
                .collect();
            self
        }
    }

    impl PackageQuery for FakeHost {
        fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
            if self.multiple_versions.iter().any(|name| name == package) {
                return Err(PackageQueryError::UnexpectedOutput {
                    command: "rpm".to_string(),
                    detail: "2 installed versions".to_string(),
                });
            }
            Ok(self.installed.get(package).cloned())
        }
        fn query_available(&self, _package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
            Ok(Vec::new())
        }
        fn installed_origin(&self, _package: &str) -> Result<Option<String>, PackageQueryError> {
            Ok(None)
        }
    }

    impl PackageTransaction for FakeHost {
        fn install(&self, _packages: &[&str]) -> Result<(), PackageTransactionError> {
            panic!("merged update must not run a dnf install");
        }
        fn update(&self, packages: &[&str]) -> Result<(), PackageTransactionError> {
            self.calls.borrow_mut().push((
                "update".to_string(),
                packages.iter().map(|p| (*p).to_string()).collect(),
            ));
            if self.fail_update {
                return Err(PackageTransactionError::TransactionFailed {
                    command: "dnf".to_string(),
                    operation: "update".to_string(),
                    code: Some(1),
                    stderr: "fake dnf update failure".to_string(),
                });
            }
            Ok(())
        }
        fn reinstall(&self, _packages: &[&str]) -> Result<(), PackageTransactionError> {
            panic!("merged update must not run a dnf reinstall");
        }
        fn remove(&self, _packages: &[&str]) -> Result<(), PackageTransactionError> {
            panic!("merged update must not run a dnf remove");
        }
    }

    /// Seed one state file holding several v4 objects (the store migrates
    /// them on load).
    fn seed_many(ctx: &CliContext, objects: Vec<InstalledObject>) {
        let layout = common::resolve_layout(ctx);
        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");
        let mut state = InstalledState {
            install_mode: StateInstallMode::System,
            prefix: layout.prefix.clone(),
            ..Default::default()
        };
        for obj in objects {
            state.upsert_object(obj);
        }
        state
            .save(&layout.state_dir.join("installed.toml"))
            .expect("seed state");
    }

    fn managed_pair(layout_ctx: &CliContext) {
        seed_many(
            layout_ctx,
            vec![
                rpm_object(
                    "cosh",
                    "copilot-shell",
                    "1.0.0-1.al4",
                    Ownership::RpmManaged,
                    anolisa_core::state::ObjectStatus::Installed,
                ),
                rpm_object(
                    "sec-core",
                    "agent-sec-core",
                    "1.0.0-1.al4",
                    Ownership::RpmManaged,
                    anolisa_core::state::ObjectStatus::Installed,
                ),
            ],
        );
    }

    fn u5_planned(component: &str, package: &str, from_evr: &str) -> PlannedComponentUpdate {
        let steps = vec![
            Step::NativeTransaction {
                pm: NativePm::Rpm,
                action: NativeAction::Update,
                packages: vec![package.to_string()],
            },
            Step::Observe {
                packages: vec![package.to_string()],
            },
            Step::WriteRecord(RecordWrite::RefreshObservation),
        ];
        PlannedComponentUpdate {
            command: format!("update {component}"),
            target: component.to_string(),
            native_package: Some(package.to_string()),
            scope: InstallationScope::System,
            now: NOW.to_string(),
            owned_execution: None,
            owned_versions: None,
            native_from: Some(from_evr.to_string()),
            route: PlannedUpdateRoute::Delegated { steps },
        }
    }

    fn u5_item(component: &str, package: &str, from_evr: &str) -> MergedUpdate {
        MergedUpdate {
            name: component.to_string(),
            package: package.to_string(),
            planned: u5_planned(component, package, from_evr),
        }
    }

    fn find<'a>(items: &'a [UpdateAllItem], name: &str) -> &'a UpdateAllItem {
        items
            .iter()
            .find(|item| item.component == name)
            .unwrap_or_else(|| panic!("no summary item for {name}"))
    }

    #[test]
    fn merged_update_package_accepts_only_the_u5_shape() {
        assert_eq!(
            merged_update_package(&u5_planned("cosh", "copilot-shell", "1.0.0-1.al4")).as_deref(),
            Some("copilot-shell")
        );

        let mut owned = u5_planned("cosh", "copilot-shell", "1.0.0-1.al4");
        owned.route = PlannedUpdateRoute::Owned {
            steps: vec![Step::PlaceFiles],
        };
        assert!(merged_update_package(&owned).is_none());

        let mut noop = u5_planned("cosh", "copilot-shell", "1.0.0-1.al4");
        noop.route = PlannedUpdateRoute::AlreadyCurrent;
        assert!(merged_update_package(&noop).is_none());

        // An install-shaped delegated plan must never merge into an update
        // transaction.
        let mut install_shaped = u5_planned("cosh", "copilot-shell", "1.0.0-1.al4");
        install_shaped.route = PlannedUpdateRoute::Delegated {
            steps: vec![
                Step::NativeTransaction {
                    pm: NativePm::Rpm,
                    action: NativeAction::Install,
                    packages: vec!["copilot-shell".to_string()],
                },
                Step::Observe {
                    packages: vec!["copilot-shell".to_string()],
                },
                Step::WriteRecord(RecordWrite::RefreshObservation),
            ],
        };
        assert!(merged_update_package(&install_shaped).is_none());
    }

    #[test]
    fn merged_updates_share_one_dnf_transaction_and_refresh_each_record() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        managed_pair(&c);
        // rpmdb already shows the upgraded EVRs the merged transaction
        // produces.
        let host = FakeHost::new(
            &[
                (
                    "copilot-shell",
                    pkg_info("copilot-shell", "1.1.0", Some("1.al4"), "x86_64"),
                ),
                (
                    "agent-sec-core",
                    pkg_info("agent-sec-core", "1.1.0", Some("1.al4"), "x86_64"),
                ),
            ],
            false,
        );
        let mut degrade = |name: &str| -> Result<UpdateOutcome, CliError> {
            panic!("a committed merged transaction must not degrade ({name})");
        };

        let items = execute_merged_updates_with_deps(
            vec![
                u5_item("cosh", "copilot-shell", "1.0.0-1.al4"),
                u5_item("sec-core", "agent-sec-core", "1.0.0-1.al4"),
            ],
            &c,
            &host,
            &host,
            true,
            &mut degrade,
        );

        assert_eq!(
            host.calls.borrow().as_slice(),
            &[(
                "update".to_string(),
                vec!["copilot-shell".to_string(), "agent-sec-core".to_string()]
            )],
            "both packages must share one dnf update"
        );
        assert_eq!(find(&items, "cosh").status, "updated");
        assert_eq!(find(&items, "sec-core").status, "updated");

        // Each record absorbed its own fresh observation.
        let store = load_store(&c);
        for (component, evr) in [("cosh", "1.1.0-1.al4"), ("sec-core", "1.1.0-1.al4")] {
            let record = store
                .find(ObjectKind::Component, component)
                .unwrap_or_else(|| panic!("record for {component}"));
            match &record.binding {
                anolisa_core::domain::ProviderBinding::Delegated { last_observed, .. } => {
                    assert_eq!(
                        last_observed.as_ref().expect("observation").evr.as_deref(),
                        Some(evr)
                    );
                }
                other => panic!("expected delegated binding, got {other:?}"),
            }
        }

        // The history shows one parent batch operation and each member's
        // operation linked to it — the audit trail names the shared
        // transaction instead of two unrelated updates.
        let parent = store
            .operations
            .iter()
            .find(|op| op.command == BATCH_COMMAND)
            .expect("parent batch operation");
        assert!(parent.id.starts_with("op-update-all-"), "{}", parent.id);
        assert_eq!(parent.status, "ok");
        assert!(parent.parent_operation_id.is_none());
        let members: Vec<_> = store
            .operations
            .iter()
            .filter(|op| op.command != BATCH_COMMAND)
            .collect();
        assert_eq!(members.len(), 2, "one operation per member");
        for op in members {
            assert_eq!(op.parent_operation_id.as_deref(), Some(parent.id.as_str()));
        }

        // Both journals settled; nothing pending blocks the next intent.
        let layout = common::resolve_layout(&c);
        let journal_dir = rpm_install::journal_dir(&layout);
        for component in ["cosh", "sec-core"] {
            assert!(
                pending_journal_for(JournalEvidence::new(&journal_dir, &[]), component)
                    .expect("scan journals")
                    .is_none()
            );
        }
    }

    /// Every successful merged member refreshes its own contract snapshot,
    /// exactly like the single-component path — repair no longer has to
    /// reconcile a post-update drift.
    #[test]
    fn merged_updates_refresh_each_contract_snapshot() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        managed_pair(&c);
        let (_, cosh_snapshot) = seed_package_contract_and_stale_snapshot(&c, "cosh");
        let (_, sec_snapshot) = seed_package_contract_and_stale_snapshot(&c, "sec-core");
        let host = FakeHost::new(
            &[
                (
                    "copilot-shell",
                    pkg_info("copilot-shell", "1.1.0", Some("1.al4"), "x86_64"),
                ),
                (
                    "agent-sec-core",
                    pkg_info("agent-sec-core", "1.1.0", Some("1.al4"), "x86_64"),
                ),
            ],
            false,
        );
        let mut degrade = |name: &str| -> Result<UpdateOutcome, CliError> {
            panic!("a committed merged transaction must not degrade ({name})");
        };

        let items = execute_merged_updates_with_deps(
            vec![
                u5_item("cosh", "copilot-shell", "1.0.0-1.al4"),
                u5_item("sec-core", "agent-sec-core", "1.0.0-1.al4"),
            ],
            &c,
            &host,
            &host,
            true,
            &mut degrade,
        );

        assert_eq!(find(&items, "cosh").status, "updated");
        assert_eq!(find(&items, "sec-core").status, "updated");
        for snapshot in [&cosh_snapshot, &sec_snapshot] {
            assert_eq!(
                std::fs::read_to_string(snapshot).expect("read snapshot"),
                "framework = \"new\"\n",
                "each member must refresh its own snapshot"
            );
        }
        let store = load_store(&c);
        let parent = store
            .operations
            .iter()
            .find(|op| op.command == BATCH_COMMAND)
            .expect("parent batch operation");
        assert_eq!(parent.status, "ok");
    }

    /// A member whose contract refresh cannot complete is demoted alone: its
    /// item fails with a repair pointer and its operation reads `partial`,
    /// while the other member keeps its refreshed snapshot and `ok` status;
    /// the parent batch operation aggregates to `partial`.
    #[test]
    fn merged_member_contract_refresh_failure_only_demotes_that_member() {
        use anolisa_platform::fs_layout::FsLayout;

        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        managed_pair(&c);
        // cosh publishes a contract, but a non-empty directory blocks its
        // snapshot slot; sec-core's refresh is healthy.
        let layout = common::resolve_layout(&c);
        let package_datadir = layout.package_datadir().expect("package datadir");
        let cosh_contract = FsLayout::component_contract_path(&package_datadir, "cosh");
        std::fs::create_dir_all(cosh_contract.parent().expect("contract parent"))
            .expect("mkdir contract");
        std::fs::write(&cosh_contract, "framework = \"new\"\n").expect("write cosh contract");
        let cosh_snapshot = FsLayout::component_manifest_snapshot_path(&layout.state_dir, "cosh");
        std::fs::create_dir_all(&cosh_snapshot).expect("create blocking snapshot directory");
        std::fs::write(cosh_snapshot.join("keep"), b"x").expect("write blocking marker");
        let (_, sec_snapshot) = seed_package_contract_and_stale_snapshot(&c, "sec-core");
        let host = FakeHost::new(
            &[
                (
                    "copilot-shell",
                    pkg_info("copilot-shell", "1.1.0", Some("1.al4"), "x86_64"),
                ),
                (
                    "agent-sec-core",
                    pkg_info("agent-sec-core", "1.1.0", Some("1.al4"), "x86_64"),
                ),
            ],
            false,
        );
        let mut degrade = |name: &str| -> Result<UpdateOutcome, CliError> {
            panic!("a committed merged transaction must not degrade ({name})");
        };

        let items = execute_merged_updates_with_deps(
            vec![
                u5_item("cosh", "copilot-shell", "1.0.0-1.al4"),
                u5_item("sec-core", "agent-sec-core", "1.0.0-1.al4"),
            ],
            &c,
            &host,
            &host,
            true,
            &mut degrade,
        );

        let cosh_item = find(&items, "cosh");
        assert_eq!(cosh_item.status, "failed");
        let reason = cosh_item.reason.as_deref().expect("failure reason");
        assert!(
            reason.contains("committed") && reason.contains("repair"),
            "reason must state the update committed and point at repair: {reason}"
        );
        assert_eq!(find(&items, "sec-core").status, "updated");
        assert_eq!(
            std::fs::read_to_string(&sec_snapshot).expect("read snapshot"),
            "framework = \"new\"\n",
            "the healthy member must keep its refreshed snapshot"
        );

        let store = load_store(&c);
        let parent = store
            .operations
            .iter()
            .find(|op| op.command == BATCH_COMMAND)
            .expect("parent batch operation");
        assert_eq!(parent.status, "partial", "one demoted member is partial");
        let member_status = |command: &str| {
            store
                .operations
                .iter()
                .find(|op| op.command == command)
                .unwrap_or_else(|| panic!("no operation for {command}"))
                .status
                .clone()
        };
        assert_eq!(member_status("update cosh"), "partial");
        assert_eq!(member_status("update sec-core"), "ok");
    }

    #[test]
    fn pending_journal_injected_after_batch_planning_blocks_dnf_and_state_write() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        managed_pair(&c);
        let layout = common::resolve_layout(&c);
        let state_path = layout.state_dir.join("installed.toml");
        let journal_dir = rpm_install::journal_dir(&layout);
        let pending = Transaction::begin_with_subject(
            COMMAND,
            Some("cosh"),
            state_path.clone(),
            &journal_dir,
        )
        .expect("inject pending journal after planning");
        drop(pending);
        let state_before = std::fs::read(&state_path).expect("read state");
        let journals_before = std::fs::read_dir(&journal_dir)
            .expect("journal dir")
            .count();
        let host = FakeHost::new(
            &[(
                "copilot-shell",
                pkg_info("copilot-shell", "1.0.0", Some("1.al4"), "x86_64"),
            )],
            false,
        );
        let mut degrade = |name: &str| -> Result<UpdateOutcome, CliError> {
            panic!("a recovery-blocked member must not degrade ({name})");
        };

        let items = execute_merged_updates_with_deps(
            vec![u5_item("cosh", "copilot-shell", "1.0.0-1.al4")],
            &c,
            &host,
            &host,
            true,
            &mut degrade,
        );

        assert!(host.calls.borrow().is_empty(), "dnf must not run");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].status, "failed");
        assert!(
            items[0]
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("anolisa repair cosh"))
        );
        assert_eq!(
            std::fs::read(&state_path).expect("read state"),
            state_before
        );
        assert_eq!(
            std::fs::read_dir(&journal_dir)
                .expect("journal dir")
                .count(),
            journals_before,
            "the locked executor must not create a second journal"
        );
    }

    #[test]
    fn merged_update_failure_degrades_unmoved_members() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        managed_pair(&c);
        // Both packages still read their pre-update EVR after the failure:
        // provably untouched slots, so both re-plan individually.
        let host = FakeHost::new(
            &[
                (
                    "copilot-shell",
                    pkg_info("copilot-shell", "1.0.0", Some("1.al4"), "x86_64"),
                ),
                (
                    "agent-sec-core",
                    pkg_info("agent-sec-core", "1.0.0", Some("1.al4"), "x86_64"),
                ),
            ],
            true,
        );
        let degraded = RefCell::new(Vec::new());
        let mut degrade = |name: &str| -> Result<UpdateOutcome, CliError> {
            degraded.borrow_mut().push(name.to_string());
            if name == "cosh" {
                Err(CliError::Runtime {
                    command: "update cosh".to_string(),
                    reason: "repo unreachable for copilot-shell".to_string(),
                })
            } else {
                Ok(UpdateOutcome::Updated)
            }
        };

        let items = execute_merged_updates_with_deps(
            vec![
                u5_item("cosh", "copilot-shell", "1.0.0-1.al4"),
                u5_item("sec-core", "agent-sec-core", "1.0.0-1.al4"),
            ],
            &c,
            &host,
            &host,
            true,
            &mut degrade,
        );

        assert_eq!(degraded.borrow().as_slice(), ["cosh", "sec-core"]);
        let cosh = find(&items, "cosh");
        assert_eq!(cosh.status, "failed");
        assert!(cosh.reason.as_deref().unwrap().contains("repo unreachable"));
        assert_eq!(find(&items, "sec-core").status, "updated");

        // Clean failure journals never stay pending.
        let layout = common::resolve_layout(&c);
        let journal_dir = rpm_install::journal_dir(&layout);
        for component in ["cosh", "sec-core"] {
            assert!(
                pending_journal_for(JournalEvidence::new(&journal_dir, &[]), component)
                    .expect("scan journals")
                    .is_none()
            );
        }
    }

    #[test]
    fn merged_update_failure_keeps_partial_journal_for_moved_member() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        managed_pair(&c);
        // copilot-shell's EVR moved despite the failed merged transaction —
        // real side effects; agent-sec-core is provably untouched.
        let host = FakeHost::new(
            &[
                (
                    "copilot-shell",
                    pkg_info("copilot-shell", "1.1.0", Some("1.al4"), "x86_64"),
                ),
                (
                    "agent-sec-core",
                    pkg_info("agent-sec-core", "1.0.0", Some("1.al4"), "x86_64"),
                ),
            ],
            true,
        );
        let degraded = RefCell::new(Vec::new());
        let mut degrade = |name: &str| -> Result<UpdateOutcome, CliError> {
            degraded.borrow_mut().push(name.to_string());
            Ok(UpdateOutcome::Updated)
        };

        let items = execute_merged_updates_with_deps(
            vec![
                u5_item("cosh", "copilot-shell", "1.0.0-1.al4"),
                u5_item("sec-core", "agent-sec-core", "1.0.0-1.al4"),
            ],
            &c,
            &host,
            &host,
            true,
            &mut degrade,
        );

        // Forward-only for the moved member: no retry, repair reconciles.
        assert_eq!(degraded.borrow().as_slice(), ["sec-core"]);
        let cosh = find(&items, "cosh");
        assert_eq!(cosh.status, "failed");
        let reason = cosh.reason.as_deref().unwrap();
        assert!(reason.contains("changed in the rpmdb"), "got: {reason}");
        assert!(reason.contains("anolisa repair cosh"), "got: {reason}");
        assert_eq!(find(&items, "sec-core").status, "updated");

        let layout = common::resolve_layout(&c);
        let journal_dir = rpm_install::journal_dir(&layout);
        let pending = pending_journal_for(JournalEvidence::new(&journal_dir, &[]), "cosh")
            .expect("scan journals")
            .expect("partial journal for the moved member must stay pending");
        let journal = Transaction::load_journal(&pending).expect("load partial journal");
        let recovery = journal
            .delegated_recovery
            .expect("per-subject recovery contract");
        assert_eq!(recovery.package.as_deref(), Some("copilot-shell"));
        assert_eq!(
            recovery.record_action,
            anolisa_core::transaction::DelegatedRecordAction::Refresh
        );
        assert_eq!(journal.steps[0].target, "copilot-shell,agent-sec-core");
        assert!(
            pending_journal_for(JournalEvidence::new(&journal_dir, &[]), "sec-core")
                .expect("scan journals")
                .is_none()
        );
    }

    #[test]
    fn merged_update_failure_keeps_partial_journal_for_multiple_versions() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed_many(
            &c,
            vec![rpm_object(
                "cosh",
                "copilot-shell",
                "1.0.0-1.al4",
                Ownership::RpmManaged,
                anolisa_core::state::ObjectStatus::Installed,
            )],
        );
        let host = FakeHost::new(&[], true).with_multiple_versions(&["copilot-shell"]);
        let degraded = RefCell::new(Vec::new());
        let mut degrade = |name: &str| -> Result<UpdateOutcome, CliError> {
            degraded.borrow_mut().push(name.to_string());
            Ok(UpdateOutcome::Updated)
        };

        let items = execute_merged_updates_with_deps(
            vec![u5_item("cosh", "copilot-shell", "1.0.0-1.al4")],
            &c,
            &host,
            &host,
            true,
            &mut degrade,
        );

        assert_eq!(host.calls.borrow().len(), 1, "dnf must run only once");
        assert!(
            degraded.borrow().is_empty(),
            "an indeterminate rpmdb state must not trigger a second update"
        );
        let cosh = find(&items, "cosh");
        assert_eq!(cosh.status, "failed");
        assert!(
            cosh.reason
                .as_deref()
                .is_some_and(|reason| reason.contains("anolisa repair cosh"))
        );

        let layout = common::resolve_layout(&c);
        let journal_dir = rpm_install::journal_dir(&layout);
        assert!(
            pending_journal_for(JournalEvidence::new(&journal_dir, &[]), "cosh")
                .expect("scan journals")
                .is_some(),
            "an indeterminate result must retain its Partial journal"
        );
    }

    #[test]
    fn merged_update_without_root_fails_every_member_before_dnf() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        managed_pair(&c);
        let host = FakeHost::new(&[], false);
        let mut degrade = |name: &str| -> Result<UpdateOutcome, CliError> {
            panic!("a refused merged group must not degrade ({name})");
        };

        let items = execute_merged_updates_with_deps(
            vec![
                u5_item("cosh", "copilot-shell", "1.0.0-1.al4"),
                u5_item("sec-core", "agent-sec-core", "1.0.0-1.al4"),
            ],
            &c,
            &host,
            &host,
            false,
            &mut degrade,
        );

        assert!(host.calls.borrow().is_empty(), "dnf must not run");
        for component in ["cosh", "sec-core"] {
            let item = find(&items, component);
            assert_eq!(item.status, "failed");
            assert!(item.reason.as_deref().unwrap().contains("requires root"));
        }
    }

    #[test]
    fn merged_update_revalidates_authority_under_the_lock() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        // Only cosh is recorded; sec-core's plan is stale (its record is
        // gone), so its slot must be refused under the lock while cosh
        // proceeds.
        seed_many(
            &c,
            vec![rpm_object(
                "cosh",
                "copilot-shell",
                "1.0.0-1.al4",
                Ownership::RpmManaged,
                anolisa_core::state::ObjectStatus::Installed,
            )],
        );
        let host = FakeHost::new(
            &[(
                "copilot-shell",
                pkg_info("copilot-shell", "1.1.0", Some("1.al4"), "x86_64"),
            )],
            false,
        );
        let mut degrade = |name: &str| -> Result<UpdateOutcome, CliError> {
            panic!("must not degrade ({name})");
        };

        let items = execute_merged_updates_with_deps(
            vec![
                u5_item("cosh", "copilot-shell", "1.0.0-1.al4"),
                u5_item("sec-core", "agent-sec-core", "1.0.0-1.al4"),
            ],
            &c,
            &host,
            &host,
            true,
            &mut degrade,
        );

        let sec = find(&items, "sec-core");
        assert_eq!(sec.status, "failed");
        assert!(
            sec.reason
                .as_deref()
                .unwrap()
                .contains("changed while this update was planning")
        );
        assert_eq!(find(&items, "cosh").status, "updated");
        // The refused member never entered the transaction.
        assert_eq!(
            host.calls.borrow().as_slice(),
            &[("update".to_string(), vec!["copilot-shell".to_string()])]
        );
    }

    /// The `plan` key is dry-run-only wire surface: absent entirely for
    /// executed items so existing consumers of the batch summary never see
    /// a new key outside preview mode.
    #[test]
    fn summary_item_serializes_plan_only_on_dry_run_members() {
        let executed = UpdateAllItem {
            component: "cosh".to_string(),
            status: "updated",
            reason: None,
            plan: None,
        };
        let json = serde_json::to_value(&executed).expect("serialize");
        assert!(json.get("plan").is_none(), "{json}");

        let planned = u5_planned("cosh", "copilot-shell", "1.0.0-1.al4");
        let steps = match &planned.route {
            PlannedUpdateRoute::Delegated { steps } => steps.as_slice(),
            _ => unreachable!("u5 plans are delegated"),
        };
        let previewed = UpdateAllItem {
            component: "cosh".to_string(),
            status: "planned",
            reason: None,
            plan: Some(steps.iter().map(step_label).collect()),
        };
        let json = serde_json::to_value(&previewed).expect("serialize");
        assert_eq!(json["plan"][0], "dnf update copilot-shell");
        assert_eq!(json["plan"][1], "observe copilot-shell");
    }
}
