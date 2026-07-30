//! `anolisa adopt <component>` — take an already-installed system RPM under
//! ANOLISA tracking as a delegated-adopted record.
//!
//! The handler is a thin shell over the planner pipeline (decision table
//! A1–A7): resolve the component to its RPM package, assemble host facts,
//! and execute the planner's step sequence. Adoption fetches nothing and
//! runs no `dnf`/`rpm` transaction — a fresh adopt observes rpmdb and writes
//! the record (A1); re-adopting an observed component upgrades the
//! management consent in place without refreshing the cached observation
//! (A6); an already-adopted component is a NoOp (A7). Ambiguous, absent, or
//! differently-owned components are refused with guidance toward the right
//! command.

use clap::Parser;
use serde::Serialize;

use anolisa_core::domain::{InstallationScope, ManagementRelation, NativePm, ProviderBinding};
use anolisa_core::executor::{DelegatedExecutionTarget, execute_delegated_steps};
use anolisa_core::facts::{
    FactsError, JournalEvidence, ObserveRequest, assemble_facts, pending_journal_for,
};
use anolisa_core::lock::InstallLock;
use anolisa_core::planner::{Intent, Plan, PlanError, Step, plan};
use anolisa_core::providers::{DelegatedProvider, ProviderError};
use anolisa_core::record_sink::{DelegatedIdentity, RecordContext, StoreRecordSink};
use anolisa_core::state::{ObjectKind, OperationRecord};
use anolisa_core::state_store::StateStore;
use anolisa_platform::pkg_query::{PackageQuery, PackageQueryError};
use anolisa_platform::pkg_transaction::{PackageTransaction, PackageTransactionError};
use anolisa_platform::privilege;
use anolisa_platform::rpm_query::RpmPackageQuery;

use crate::commands::common;
use crate::commands::common::RepoPersistPolicy;
use crate::commands::tier1::install::{
    installed_version_label, now_iso8601, rpm_package_candidates_with_index,
    snapshot_datadir_contract,
};
use crate::commands::tier1::recovery::LockedJournalGate;
use crate::commands::tier1::rpm_install;
use crate::context::{CliContext, InstallMode};
use crate::resolution::{ResolutionUse, load_optional_component_index};
use crate::response::{CliError, render_json};

/// Command label for JSON envelopes and error routing.
const COMMAND: &str = "adopt";

/// Arguments for `anolisa adopt <component>`.
#[derive(Debug, Parser)]
pub struct AdoptArgs {
    /// Component to record as an existing system RPM
    #[arg(value_name = "COMPONENT")]
    pub component: String,
    /// Pin the RPM package name when the component maps to several candidates
    #[arg(long, value_name = "NAME")]
    pub package: Option<String>,
}

/// Dispatch `adopt <component>` against the live host.
pub fn handle(args: AdoptArgs, ctx: &CliContext) -> Result<(), CliError> {
    let query = RpmPackageQuery::system();
    adopt_with_query(&args.component, args.package.as_deref(), ctx, &query)
}

/// Adopt runs no native transaction; the provider contract needs one anyway,
/// so this stub refuses honestly if a plan ever routes a transaction here.
struct NoNativeTransaction;

impl NoNativeTransaction {
    fn refused(operation: &str, packages: &[&str]) -> PackageTransactionError {
        PackageTransactionError::TransactionFailed {
            command: COMMAND.to_string(),
            operation: operation.to_string(),
            code: None,
            stderr: format!(
                "adopt never runs a native transaction (attempted {operation} {})",
                packages.join(" ")
            ),
        }
    }
}

impl PackageTransaction for NoNativeTransaction {
    fn install(&self, packages: &[&str]) -> Result<(), PackageTransactionError> {
        Err(Self::refused("install", packages))
    }
    fn update(&self, packages: &[&str]) -> Result<(), PackageTransactionError> {
        Err(Self::refused("update", packages))
    }
    fn reinstall(&self, packages: &[&str]) -> Result<(), PackageTransactionError> {
        Err(Self::refused("reinstall", packages))
    }
    fn remove(&self, packages: &[&str]) -> Result<(), PackageTransactionError> {
        Err(Self::refused("remove", packages))
    }
}

/// What the pre-lock plan decided this adopt writes, re-checked against the
/// store as re-read under the install lock.
enum AdoptShape {
    /// A1: no record existed; the adopt writes a fresh delegated-adopted one.
    Fresh,
    /// A6: an observed record existed for this package; the adopt upgrades
    /// its management relation in place.
    UpgradeObserved {
        /// RPM package the observed record must still point at.
        package: String,
    },
}

/// Core of [`handle`] with the package query injected so tests drive every
/// branch without a live rpmdb.
pub(crate) fn adopt_with_query(
    target: &str,
    cli_override: Option<&str>,
    ctx: &CliContext,
    query: &dyn PackageQuery,
) -> Result<(), CliError> {
    let command = format!("{COMMAND} {target}");
    if ctx.install_mode == InstallMode::User {
        return Err(CliError::InvalidArgument {
            command,
            reason: "adopt is available only in system mode".to_string(),
        });
    }
    let layout = common::resolve_layout(ctx);
    let state_path = layout.state_dir.join("installed.toml");
    let journal_dir = rpm_install::journal_dir(&layout);
    let scope = InstallationScope::System;
    let now = now_iso8601();
    let env = anolisa_env::EnvService::detect();

    let (mut component, view) = common::resolve_mutation_target(target, ctx, &command)?;
    let store = view.writable.state;

    // Quarantined records and pending journals decide the outcome before any
    // rpmdb resolution has to run — the refusal must not depend on the
    // candidate chain resolving.
    if quarantined(&store, &component) {
        return Err(plan_error_to_cli(
            PlanError::NeedsAttention,
            &component,
            &component,
            &command,
        ));
    }
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
            &component,
            &command,
        ));
    }

    // Resolve the RPM package to probe and the shape the write would take.
    // Active-record arms never run the candidate chain: the planner rules on
    // the recorded identity (A4–A7), and repointing a tracked component at a
    // different package is an identity migration adopt refuses up front.
    let active_binding = store
        .find(ObjectKind::Component, &component)
        .map(|installation| installation.binding.clone());
    let prior_version = store
        .find(ObjectKind::Component, &component)
        .map(installed_version_label);
    let (native_package, shape): (Option<String>, AdoptShape) = match &active_binding {
        Some(ProviderBinding::Owned { .. }) => {
            // A4 refuses before any probe; nothing to resolve.
            (None, AdoptShape::Fresh)
        }
        Some(ProviderBinding::Delegated {
            package: recorded,
            relation,
            ..
        }) => {
            let recorded_name = recorded.resolved_name().map(str::to_string);
            if let (Some(requested), Some(prev)) = (cli_override, recorded_name.as_deref())
                && !prev.is_empty()
                && prev != requested
                && !matches!(relation, ManagementRelation::Managed { .. })
            {
                return Err(CliError::InvalidArgument {
                    command,
                    reason: format!(
                        "component '{component}' is already adopted from RPM package '{prev}', not '{requested}'; adopt will not silently repoint it to a different package — run `anolisa forget {component}` first, then adopt the new package"
                    ),
                });
            }
            let package = cli_override
                .map(str::to_string)
                .or(recorded_name)
                .unwrap_or_else(|| component.clone());
            let shape = if matches!(relation, ManagementRelation::Observed) {
                AdoptShape::UpgradeObserved {
                    package: package.clone(),
                }
            } else {
                AdoptShape::Fresh
            };
            (Some(package), shape)
        }
        None => {
            if !matches!(scope, InstallationScope::System) {
                // The planner refuses user scope before consulting the probe
                // (its first guard), so nothing touches the rpmdb here.
                (None, AdoptShape::Fresh)
            } else {
                let (package, resolved_component) = resolve_fresh_adopt(
                    cli_override,
                    ctx,
                    &layout,
                    &env,
                    &component,
                    query,
                    &command,
                )?;
                component = resolved_component;
                (Some(package), AdoptShape::Fresh)
            }
        }
    };

    let txn = NoNativeTransaction;
    let provider = DelegatedProvider::new(query, &txn);
    let observe_request = ObserveRequest {
        kind: ObjectKind::Component,
        name: &component,
        scope,
        native_package: native_package.as_deref(),
        observed_at: &now,
        verify_owned_files: false,
    };
    let facts = assemble_facts(
        &observe_request,
        &store,
        Some(&provider),
        &layout,
        &journal_dir,
    )
    .map_err(|err| adopt_facts_error(err, &command, &component))?;

    let package_label = native_package.clone().unwrap_or_else(|| component.clone());
    let steps = match plan(&Intent::Adopt, &facts) {
        Ok(Plan::Execute { steps, .. }) => steps,
        Ok(Plan::NoOp { .. }) => {
            // A7: adopt is idempotent over an already-adopted record.
            render_result(
                ctx,
                &AdoptResultPayload {
                    component,
                    package: native_package,
                    version: prior_version,
                    action: "already-adopted",
                    operation_id: None,
                    dry_run: ctx.dry_run,
                    plan: Vec::new(),
                },
            )?;
            return Ok(());
        }
        Err(err) => return Err(plan_error_to_cli(err, &component, &package_label, &command)),
    };
    let plan_labels: Vec<String> = steps.iter().map(step_label).collect();

    if ctx.dry_run {
        return render_result(
            ctx,
            &AdoptResultPayload {
                component,
                package: native_package,
                version: prior_version,
                action: "planned",
                operation_id: None,
                dry_run: true,
                plan: plan_labels,
            },
        );
    }

    execute_adopt_plan(
        &component,
        &package_label,
        &shape,
        ctx,
        &layout,
        &state_path,
        &journal_dir,
        scope,
        &now,
        &provider,
        &command,
    )
}

/// Candidate-chain resolution for a component with no record: CLI
/// `--package` override, repo-side component index, repo.toml `package_map`,
/// then rpmdb Provides metadata. repo.toml is supplementary here, so an
/// unreadable config degrades to "no rpm backend config" rather than
/// failing the adopt.
fn resolve_fresh_adopt(
    cli_override: Option<&str>,
    ctx: &CliContext,
    layout: &anolisa_platform::fs_layout::FsLayout,
    env: &anolisa_env::EnvFacts,
    component: &str,
    query: &dyn PackageQuery,
    command: &str,
) -> Result<(String, String), CliError> {
    let repo_config =
        common::load_repo_config(ctx, layout, COMMAND, RepoPersistPolicy::BestEffort).ok();
    let rpm_backend = repo_config.as_ref().and_then(|c| c.backends.get("rpm"));
    let component_index = repo_config
        .as_ref()
        .and_then(|cfg| load_optional_component_index(layout, env, cfg));

    let candidates = rpm_package_candidates_with_index(
        cli_override,
        rpm_backend,
        component_index.as_ref(),
        query,
        component,
        ResolutionUse::Adopt,
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
                "component '{component}' is not an ANOLISA RPM component; configure the repo-side component index or publish Provides: anolisa-component({component})"
            ),
        }),
        [single] => Ok((single.package.clone(), single.component.clone())),
        many => Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "multiple RPM candidates match '{component}': {}; cannot adopt unambiguously — pin one with `--package <name>`",
                many.iter()
                    .map(|target| target.package.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }),
    }
}

/// Execute an adopt plan (A1 or A6): at most one observation plus a record
/// write, under the install lock.
///
/// The store and native facts are re-read under the lock, then the planner is
/// run again so execution never consumes a stale lock-external decision.
#[expect(clippy::too_many_arguments)]
fn execute_adopt_plan(
    component: &str,
    package: &str,
    shape: &AdoptShape,
    ctx: &CliContext,
    layout: &anolisa_platform::fs_layout::FsLayout,
    state_path: &std::path::Path,
    journal_dir: &std::path::Path,
    scope: InstallationScope,
    now: &str,
    provider: &DelegatedProvider,
    command: &str,
) -> Result<(), CliError> {
    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;
    let mut store = StateStore::load_for_layout(state_path, privilege::effective_uid(), layout)
        .map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!("failed to load installed state: {err}"),
        })?;
    adopt_authorized(&store, component, shape, command)?;

    let observe_request = ObserveRequest {
        kind: ObjectKind::Component,
        name: component,
        scope,
        native_package: Some(package),
        observed_at: now,
        verify_owned_files: false,
    };
    let locked_facts = assemble_facts(
        &observe_request,
        &store,
        Some(provider),
        layout,
        journal_dir,
    )
    .map_err(|err| adopt_facts_error(err, command, component))?;
    let locked_steps = match plan(&Intent::Adopt, &locked_facts) {
        Ok(Plan::Execute { steps, .. }) => steps,
        Ok(Plan::NoOp { .. }) => {
            return Err(CliError::Runtime {
                command: command.to_string(),
                reason: format!(
                    "the facts for '{component}' changed while this adopt was resolving; nothing was changed — re-run `anolisa adopt {component}`"
                ),
            });
        }
        Err(err) => return Err(plan_error_to_cli(err, component, package, command)),
    };
    let plan_labels = locked_steps.iter().map(step_label).collect::<Vec<_>>();
    let prior_version = store
        .find(ObjectKind::Component, component)
        .map(installed_version_label);

    let evidence = JournalEvidence::new(journal_dir, &store.operations);
    let mut journal_gate = LockedJournalGate::load(&_lock, evidence, command)?;
    let mut journal = journal_gate.begin(COMMAND, component, state_path.to_path_buf(), command)?;
    let operation_id = journal.operation_id.clone();

    let context = RecordContext {
        kind: ObjectKind::Component,
        name: component.to_string(),
        scope,
        now: now.to_string(),
        operation_id: Some(operation_id.clone()),
        delegated: Some(DelegatedIdentity {
            pm: NativePm::Rpm,
            package: package.to_string(),
        }),
        owned_artifact: None,
    };
    let outcome = {
        let mut sink = StoreRecordSink::new(&mut store, state_path, context);
        execute_delegated_steps(
            &locked_steps,
            DelegatedExecutionTarget::new(NativePm::Rpm, Some(package)),
            provider,
            &mut sink,
            &mut journal,
            now,
        )
    }
    .map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("adopt of '{component}' failed: {err}"),
    })?;

    // Operation history is best-effort bookkeeping on top of the committed
    // record: the adopt already succeeded, so a history-write failure
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

    // Best-effort: snapshot the datadir component contract so adapter
    // commands can discover declared adapters. Missing or unwritable
    // contracts produce warnings, never failures.
    for warning in snapshot_datadir_contract(layout, component, command, ctx.packaged_data_probe())
    {
        eprintln!("warning: {warning}");
    }

    // A1 carries a fresh observation; A6 writes no observation and keeps the
    // record's cached one.
    let version = outcome
        .observation
        .as_ref()
        .map(|o| o.version.clone())
        .or(prior_version);
    render_result(
        ctx,
        &AdoptResultPayload {
            component: component.to_string(),
            package: Some(package.to_string()),
            version,
            action: "adopted",
            operation_id: Some(operation_id),
            dry_run: false,
            plan: plan_labels,
        },
    )
}

fn adopt_facts_error(err: FactsError, command: &str, component: &str) -> CliError {
    match err {
        FactsError::Probe(ProviderError::Query(PackageQueryError::CommandMissing {
            command: bin,
        })) => rpm_tooling_missing_error(command, &bin, component),
        err => CliError::Runtime {
            command: command.to_string(),
            reason: err.to_string(),
        },
    }
}

/// Re-validation under the install lock: the plan was made from a pre-lock
/// read, and a concurrent operation may have won the lock first. Any drift
/// from the planned shape is refused instead of overwriting what the winner
/// recorded — an adopt must never clobber a raw install or downgrade a
/// managed component to adopted.
fn adopt_authorized(
    store: &StateStore,
    component: &str,
    shape: &AdoptShape,
    command: &str,
) -> Result<(), CliError> {
    let drift = |detail: String| CliError::Runtime {
        command: command.to_string(),
        reason: format!(
            "{detail} while this adopt was resolving; nothing was changed — re-run `anolisa adopt {component}`"
        ),
    };
    match shape {
        AdoptShape::Fresh => {
            if store.find(ObjectKind::Component, component).is_some()
                || quarantined(store, component)
            {
                return Err(drift(format!("a record for '{component}' appeared")));
            }
        }
        AdoptShape::UpgradeObserved { package } => {
            let Some(installation) = store.find(ObjectKind::Component, component) else {
                return Err(drift(format!("the record for '{component}' disappeared")));
            };
            match &installation.binding {
                ProviderBinding::Delegated {
                    relation: ManagementRelation::Observed,
                    package: recorded,
                    ..
                } if recorded.resolved_name().is_none_or(|name| name == package) => {}
                _ => return Err(drift(format!("the record for '{component}' changed"))),
            }
        }
    }
    Ok(())
}

/// Whether the store holds a quarantined record for this component.
fn quarantined(store: &StateStore, component: &str) -> bool {
    store
        .quarantined
        .iter()
        .any(|q| q.record.kind == ObjectKind::Component && q.record.name == component)
}

/// Map a planning refusal to an actionable CLI error. The planner names the
/// way out; this mapping only renders it.
fn plan_error_to_cli(err: PlanError, component: &str, package: &str, command: &str) -> CliError {
    let command = command.to_string();
    match err {
        PlanError::AdoptRequiresSystemScope => CliError::InvalidArgument {
            command,
            reason: format!(
                "adopt records a system RPM and requires system scope; re-run as `sudo anolisa adopt {component}`"
            ),
        },
        PlanError::NothingToAdopt => CliError::InvalidArgument {
            command,
            reason: format!(
                "no installed RPM '{package}' found for component '{component}'; adopt only records an already-installed system RPM — run `sudo anolisa install {component}` to install it"
            ),
        },
        PlanError::AmbiguousPackage => CliError::InvalidArgument {
            command,
            reason: format!(
                "RPM package '{package}' has multiple installed versions; refusing to adopt a single version automatically — resolve the duplicate first"
            ),
        },
        PlanError::ProvenanceConflict => CliError::InvalidArgument {
            command,
            reason: format!(
                "component '{component}' is already tracked as a raw install; run `anolisa uninstall {component}` first to re-adopt it as a system package"
            ),
        },
        PlanError::AlreadyManaged => CliError::InvalidArgument {
            command,
            reason: format!(
                "component '{component}' is already tracked as rpm-managed; run `anolisa repair {component}` to refresh its state from rpmdb"
            ),
        },
        PlanError::TrackedButAbsent => CliError::InvalidArgument {
            command,
            reason: format!(
                "'{component}' is tracked but its package is no longer installed; run `anolisa forget {component}` to drop the record, then install again"
            ),
        },
        PlanError::NeedsAttention => CliError::InvalidArgument {
            command,
            reason: format!(
                "the record for '{component}' was quarantined by the state migration; run `anolisa repair {component}` to resolve it"
            ),
        },
        PlanError::PendingOperation => CliError::Runtime {
            command,
            reason: format!(
                "a previous operation on '{component}' is pending recovery; run `anolisa repair {component}` before retrying"
            ),
        },
        other => CliError::InvalidArgument {
            command,
            reason: format!("cannot adopt '{component}': {other:?}"),
        },
    }
}

/// Warn-and-exit error raised when the rpmdb probe cannot run because
/// `rpm`/`dnf` is absent: adopt records what rpmdb reports, so without the
/// tooling there is nothing it could observe.
fn rpm_tooling_missing_error(command: &str, bin: &str, component: &str) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: format!(
            "cannot adopt '{component}': {bin} not found on PATH — adopt reads rpmdb to record the installed package; install rpm/dnf and retry"
        ),
    }
}

fn pkg_query_err(err: PackageQueryError, command: &str) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: format!("rpm query failed: {err}"),
    }
}

/// Human-facing label for a plan step (preview rendering). Adopt plans only
/// carry observe/record steps; anything else falls back to its debug form.
fn step_label(step: &Step) -> String {
    match step {
        Step::Observe { packages } => format!("observe {}", packages.join(" ")),
        Step::WriteRecord(write) => format!("record: {}", write.label()),
        other => format!("{other:?}"),
    }
}

/// JSON payload for a completed (or previewed, or idempotent) adopt.
#[derive(Debug, Serialize)]
struct AdoptResultPayload {
    component: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    package: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    /// `adopted` | `planned` (dry-run) | `already-adopted`.
    action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    dry_run: bool,
    plan: Vec<String>,
}

fn render_result(ctx: &CliContext, payload: &AdoptResultPayload) -> Result<(), CliError> {
    if ctx.json {
        return render_json(COMMAND, payload);
    }
    if ctx.quiet {
        return Ok(());
    }
    if payload.dry_run {
        println!("adopt {} (dry-run):", payload.component);
        for label in &payload.plan {
            println!("  - {label}");
        }
        return Ok(());
    }
    match (payload.action, &payload.version) {
        ("already-adopted", Some(version)) => {
            println!("{} {version} is already adopted", payload.component);
        }
        ("already-adopted", None) => println!("{} is already adopted", payload.component),
        (_, Some(version)) => println!("adopted {} {version}", payload.component),
        (_, None) => println!("adopted {}", payload.component),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::InstallMode;

    use std::cell::Cell;
    use std::path::PathBuf;

    use anolisa_core::domain::PackageIdentity;
    use anolisa_core::state::{
        InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectStatus, Ownership,
        RpmMetadata,
    };
    use anolisa_platform::pkg_query::{PackageInfo, PackageQueryError, PackageVersion};

    /// In-memory [`PackageQuery`] for the adopt tests. Adopt runs no
    /// transaction, so a query alone drives every branch (candidate chain +
    /// probe + origin lookup).
    #[derive(Default)]
    struct FakeQuery {
        /// package name → installed info reported by `query_installed`.
        installed: Vec<(String, PackageInfo)>,
        /// package names that report several installed versions.
        multi_version: Vec<String>,
        /// capability → provider package names for `what_provides_installed`.
        provides: Vec<(String, Vec<String>)>,
        /// capability → provider package names for `what_provides_available`.
        available_provides: Vec<(String, Vec<String>)>,
        /// package → declared provides capabilities.
        package_provides: Vec<(String, Vec<String>)>,
        /// package → source repo for `installed_origin`.
        origins: Vec<(String, String)>,
        calls: Cell<usize>,
    }

    impl PackageQuery for FakeQuery {
        fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
            self.calls.set(self.calls.get() + 1);
            if self.multi_version.iter().any(|p| p == package) {
                return Err(PackageQueryError::UnexpectedOutput {
                    command: "rpm".to_string(),
                    detail: "2 installed versions".to_string(),
                });
            }
            Ok(self
                .installed
                .iter()
                .find(|(p, _)| p == package)
                .map(|(_, info)| info.clone()))
        }
        fn query_available(&self, _package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
            self.calls.set(self.calls.get() + 1);
            Ok(Vec::new())
        }
        fn installed_origin(&self, package: &str) -> Result<Option<String>, PackageQueryError> {
            self.calls.set(self.calls.get() + 1);
            Ok(self
                .origins
                .iter()
                .find(|(p, _)| p == package)
                .map(|(_, o)| o.clone()))
        }
        fn what_provides_installed(
            &self,
            capability: &str,
        ) -> Result<Vec<String>, PackageQueryError> {
            self.calls.set(self.calls.get() + 1);
            Ok(self
                .provides
                .iter()
                .find(|(c, _)| c == capability)
                .map(|(_, v)| v.clone())
                .unwrap_or_default())
        }
        fn what_provides_available(
            &self,
            capability: &str,
        ) -> Result<Vec<String>, PackageQueryError> {
            self.calls.set(self.calls.get() + 1);
            Ok(self
                .available_provides
                .iter()
                .find(|(c, _)| c == capability)
                .map(|(_, v)| v.clone())
                .unwrap_or_default())
        }
        fn provided_capabilities_installed(
            &self,
            package: &str,
        ) -> Result<Vec<String>, PackageQueryError> {
            self.calls.set(self.calls.get() + 1);
            Ok(self
                .package_provides
                .iter()
                .find(|(p, _)| p == package)
                .map(|(_, v)| v.clone())
                .unwrap_or_default())
        }
    }

    struct DisappearingQuery {
        installed: PackageInfo,
        calls: Cell<usize>,
    }

    impl PackageQuery for DisappearingQuery {
        fn query_installed(
            &self,
            _package: &str,
        ) -> Result<Option<PackageInfo>, PackageQueryError> {
            let call = self.calls.get();
            self.calls.set(call + 1);
            Ok((call == 0).then(|| self.installed.clone()))
        }

        fn query_available(&self, _package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
            Ok(Vec::new())
        }

        fn installed_origin(&self, _package: &str) -> Result<Option<String>, PackageQueryError> {
            Ok(None)
        }

        fn what_provides_installed(
            &self,
            _capability: &str,
        ) -> Result<Vec<String>, PackageQueryError> {
            Ok(Vec::new())
        }

        fn what_provides_available(
            &self,
            _capability: &str,
        ) -> Result<Vec<String>, PackageQueryError> {
            Ok(Vec::new())
        }

        fn provided_capabilities_installed(
            &self,
            _package: &str,
        ) -> Result<Vec<String>, PackageQueryError> {
            Ok(Vec::new())
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

    fn component_provider(component: &str, package: &str) -> (String, Vec<String>) {
        (
            format!("anolisa-component({component})"),
            vec![package.to_string()],
        )
    }

    fn package_component_provide(package: &str, component: &str) -> (String, Vec<String>) {
        (
            package.to_string(),
            vec![format!("anolisa-component({component})")],
        )
    }

    fn ctx(prefix: PathBuf, install_mode: InstallMode, dry_run: bool) -> CliContext {
        crate::test_support::context_for_root(
            &prefix,
            install_mode,
            Some(prefix.clone()),
            crate::test_support::TestContextOptions {
                dry_run,
                ..Default::default()
            },
        )
    }

    /// A tracked component object with the given provenance, as legacy v4
    /// state; loading it exercises the migration into the v5 store. `adopted`
    /// splits `RpmObserved` into the Adopted vs Observed relations.
    fn component_object(name: &str, ownership: Ownership, adopted: bool) -> InstalledObject {
        let is_rpm = ownership.is_rpm();
        InstalledObject {
            kind: ObjectKind::Component,
            name: name.to_string(),
            version: "1.0.0-1.al8".to_string(),
            status: if adopted {
                ObjectStatus::Adopted
            } else {
                ObjectStatus::Installed
            },
            manifest_digest: None,
            distribution_source: None,
            raw_package: None,
            install_backend: Some(if is_rpm { "rpm" } else { "raw" }.to_string()),
            ownership: Some(ownership),
            rpm_metadata: is_rpm.then(|| RpmMetadata {
                package_name: name.to_string(),
                evr: Some("1.0.0-1.al8".to_string()),
                arch: Some("x86_64".to_string()),
                source_repo: Some("@System".to_string()),
            }),
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: Some("op-prior".to_string()),
            managed: matches!(ownership, Ownership::RawManaged | Ownership::RpmManaged),
            adopted,
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

    /// Write a seed state (creating the state dir) so the lock-held write
    /// path has somewhere to land.
    fn seed(ctx: &CliContext, objs: Vec<InstalledObject>) {
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
        state
            .save(&layout.state_dir.join("installed.toml"))
            .expect("seed state");
    }

    fn load_store(ctx: &CliContext) -> StateStore {
        let layout = common::resolve_layout(ctx);
        StateStore::load(&layout.state_dir.join("installed.toml"), 0).expect("load store")
    }

    /// The delegated binding pieces of a recorded component, for assertions.
    fn delegated_parts(
        store: &StateStore,
        name: &str,
    ) -> (String, ManagementRelation, Option<String>) {
        let installation = store
            .find(ObjectKind::Component, name)
            .expect("component recorded");
        match &installation.binding {
            ProviderBinding::Delegated {
                package,
                relation,
                last_observed,
                ..
            } => (
                package.resolved_name().unwrap_or_default().to_string(),
                relation.clone(),
                last_observed.as_ref().and_then(|o| o.evr.clone()),
            ),
            other => panic!("expected a delegated binding, got {other:?}"),
        }
    }

    /// A unique installed RPM with no prior state is recorded as
    /// delegated-adopted with a fresh observation (A1).
    #[test]
    fn adopt_records_unique_rpm_as_adopted() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(&c, Vec::new());
        let q = FakeQuery {
            installed: vec![(
                "copilot-shell".to_string(),
                pkg_info("copilot-shell", "2.2.0", Some("1.al8"), "x86_64"),
            )],
            package_provides: vec![package_component_provide("copilot-shell", "copilot-shell")],
            origins: vec![("copilot-shell".to_string(), "@System".to_string())],
            ..Default::default()
        };
        adopt_with_query("copilot-shell", None, &c, &q).expect("adopt ok");

        let store = load_store(&c);
        let (package, relation, evr) = delegated_parts(&store, "copilot-shell");
        assert_eq!(package, "copilot-shell");
        assert!(
            matches!(relation, ManagementRelation::Adopted { .. }),
            "adopt records the adopted relation, got {relation:?}",
        );
        assert_eq!(evr.as_deref(), Some("2.2.0-1.al8"), "fresh observation");
        assert!(
            store
                .operations
                .iter()
                .any(|o| o.command == "adopt copilot-shell"),
            "an operation record must be appended",
        );
    }

    /// Re-adopting an observed component upgrades the management relation in
    /// place (A6) without refreshing the cached observation — install/adopt
    /// never refresh EVR implicitly; that is repair's job.
    #[test]
    fn adopt_upgrades_observed_record_to_adopted() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            vec![component_object(
                "copilot-shell",
                Ownership::RpmObserved,
                false,
            )],
        );
        let q = FakeQuery {
            installed: vec![(
                "copilot-shell".to_string(),
                pkg_info("copilot-shell", "2.0.0", Some("1.al8"), "x86_64"),
            )],
            ..Default::default()
        };
        adopt_with_query("copilot-shell", None, &c, &q).expect("upgrade ok");

        let store = load_store(&c);
        let (package, relation, evr) = delegated_parts(&store, "copilot-shell");
        assert_eq!(package, "copilot-shell");
        assert!(
            matches!(relation, ManagementRelation::Adopted { .. }),
            "observed must upgrade to adopted, got {relation:?}",
        );
        assert_eq!(
            evr.as_deref(),
            Some("1.0.0-1.al8"),
            "the cached observation is preserved, not refreshed",
        );
    }

    #[test]
    fn adopt_rejects_observed_package_removed_before_locked_execution() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            vec![component_object(
                "copilot-shell",
                Ownership::RpmObserved,
                false,
            )],
        );
        let query = DisappearingQuery {
            installed: pkg_info("copilot-shell", "2.0.0", Some("1.al8"), "x86_64"),
            calls: Cell::new(0),
        };

        let result = adopt_with_query("copilot-shell", None, &c, &query);

        result.expect_err("the locked observation must reject the vanished package");
        let store = load_store(&c);
        let (_, relation, _) = delegated_parts(&store, "copilot-shell");
        assert_eq!(relation, ManagementRelation::Observed);
        assert_eq!(query.calls.get(), 2);
    }

    /// Re-adopting an already-adopted component is a NoOp (A7): nothing is
    /// rewritten, no operation is recorded.
    #[test]
    fn adopt_already_adopted_is_a_noop() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            vec![component_object(
                "copilot-shell",
                Ownership::RpmObserved,
                true,
            )],
        );
        let q = FakeQuery {
            installed: vec![(
                "copilot-shell".to_string(),
                pkg_info("copilot-shell", "9.9.9", Some("1.al8"), "x86_64"),
            )],
            ..Default::default()
        };
        adopt_with_query("copilot-shell", None, &c, &q).expect("noop ok");

        let store = load_store(&c);
        let (_, relation, evr) = delegated_parts(&store, "copilot-shell");
        assert!(matches!(relation, ManagementRelation::Adopted { .. }));
        assert_eq!(
            evr.as_deref(),
            Some("1.0.0-1.al8"),
            "a NoOp must not refresh the observation",
        );
        assert!(
            store.operations.is_empty(),
            "a NoOp must not append an operation record",
        );
    }

    /// Re-adopting a tracked component with `--package` pointing at a
    /// *different* RPM is a package-identity migration, not a refresh: it is
    /// refused up front and steers the user through forget→adopt.
    #[test]
    fn adopt_refuses_repointing_observed_to_different_package() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            vec![component_object(
                "copilot-shell",
                Ownership::RpmObserved,
                true,
            )],
        );
        let q = FakeQuery {
            installed: vec![(
                "anolisa-other".to_string(),
                pkg_info("anolisa-other", "9.9.9", Some("1.al8"), "x86_64"),
            )],
            provides: vec![component_provider("copilot-shell", "anolisa-other")],
            ..Default::default()
        };
        let err = adopt_with_query("copilot-shell", Some("anolisa-other"), &c, &q)
            .expect_err("repointing to a different package must be refused");

        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("copilot-shell")
                && err.reason().contains("anolisa-other")
                && err.reason().contains("forget"),
            "refusal must name both packages and point at forget: {}",
            err.reason(),
        );
        // The state must be untouched — no repoint, no EVR bump.
        let store = load_store(&c);
        let (package, _, evr) = delegated_parts(&store, "copilot-shell");
        assert_eq!(
            package, "copilot-shell",
            "package identity must be preserved when the repoint is refused",
        );
        assert_eq!(evr.as_deref(), Some("1.0.0-1.al8"), "EVR unchanged");
    }

    /// The repoint refusal must also fire on `--dry-run`: the preview cannot
    /// promise a plan the real run would reject.
    #[test]
    fn adopt_dry_run_refuses_repointing_observed_to_different_package() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, true);
        seed(
            &c,
            vec![component_object(
                "copilot-shell",
                Ownership::RpmObserved,
                true,
            )],
        );
        let q = FakeQuery {
            installed: vec![(
                "anolisa-other".to_string(),
                pkg_info("anolisa-other", "9.9.9", Some("1.al8"), "x86_64"),
            )],
            provides: vec![component_provider("copilot-shell", "anolisa-other")],
            ..Default::default()
        };
        let err = adopt_with_query("copilot-shell", Some("anolisa-other"), &c, &q)
            .expect_err("dry-run must refuse the repoint, matching the real run");

        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("copilot-shell")
                && err.reason().contains("anolisa-other")
                && err.reason().contains("forget"),
            "dry-run refusal must match the real run: {}",
            err.reason(),
        );
        let store = load_store(&c);
        let (package, _, _) = delegated_parts(&store, "copilot-shell");
        assert_eq!(package, "copilot-shell");
    }

    /// A raw-managed component is not silently converted; adopt points at
    /// uninstall (A4).
    #[test]
    fn adopt_refuses_raw_managed() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            vec![component_object(
                "copilot-shell",
                Ownership::RawManaged,
                false,
            )],
        );
        let err = adopt_with_query("copilot-shell", None, &c, &FakeQuery::default())
            .expect_err("raw must be refused");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("uninstall"),
            "raw refusal points at uninstall: {}",
            err.reason()
        );
    }

    /// An rpm-managed component is refused; adopt points at repair (A5).
    #[test]
    fn adopt_refuses_rpm_managed() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            vec![component_object(
                "copilot-shell",
                Ownership::RpmManaged,
                false,
            )],
        );
        let err = adopt_with_query("copilot-shell", None, &c, &FakeQuery::default())
            .expect_err("rpm-managed must be refused");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("repair"),
            "rpm-managed refusal points at repair: {}",
            err.reason()
        );
    }

    /// A tracked component whose package left rpmdb points at forget.
    #[test]
    fn adopt_of_tracked_but_absent_points_at_forget() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            vec![component_object(
                "copilot-shell",
                Ownership::RpmObserved,
                false,
            )],
        );
        // Query reports nothing installed: the observed package is gone.
        let err = adopt_with_query("copilot-shell", None, &c, &FakeQuery::default())
            .expect_err("tracked-but-absent must be refused");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("forget"),
            "absence of a tracked package points at forget: {}",
            err.reason()
        );
    }

    /// Adoption is system-scope; user mode is refused by the planner.
    #[test]
    fn adopt_refuses_in_user_mode() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::User, false);
        let query = FakeQuery::default();
        let err = adopt_with_query("copilot-shell", None, &c, &query)
            .expect_err("user mode must be refused");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("system"),
            "user-mode refusal mentions system scope: {}",
            err.reason()
        );
        assert_eq!(query.calls.get(), 0, "user refusal must not query rpmdb");
        assert_eq!(
            std::fs::read_dir(tmp.path()).expect("sandbox root").count(),
            0,
            "user refusal must not create filesystem state"
        );
    }

    /// No installed RPM under the name: adopt does not install, points at
    /// install (A2).
    #[test]
    fn adopt_refuses_absent_package() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let q = FakeQuery {
            available_provides: vec![component_provider("copilot-shell", "copilot-shell")],
            ..Default::default()
        };
        let err = adopt_with_query("copilot-shell", None, &c, &q)
            .expect_err("absent package must be refused");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("install copilot-shell"),
            "absent refusal points at the install command: {}",
            err.reason()
        );
    }

    /// Multiple provider packages cannot be adopted unambiguously.
    #[test]
    fn adopt_refuses_ambiguous_candidates() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let q = FakeQuery {
            provides: vec![(
                "anolisa-component(copilot-shell)".to_string(),
                vec!["pkg-a".to_string(), "pkg-b".to_string()],
            )],
            ..Default::default()
        };
        let err =
            adopt_with_query("copilot-shell", None, &c, &q).expect_err("ambiguous must be refused");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("--package"),
            "ambiguous refusal points at --package: {}",
            err.reason()
        );
    }

    /// A same-name multi-version rpmdb is refused rather than adopted
    /// blindly (A3).
    #[test]
    fn adopt_refuses_multi_version() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let q = FakeQuery {
            installed: vec![(
                "copilot-shell".to_string(),
                pkg_info("copilot-shell", "2.2.0", Some("1.al8"), "x86_64"),
            )],
            available_provides: vec![component_provider("copilot-shell", "copilot-shell")],
            multi_version: vec!["copilot-shell".to_string()],
            ..Default::default()
        };
        let err = adopt_with_query("copilot-shell", None, &c, &q)
            .expect_err("multi-version must be refused");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("multiple installed versions"));
    }

    /// `--dry-run` previews without writing any state.
    #[test]
    fn adopt_dry_run_writes_nothing() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, true);
        let q = FakeQuery {
            installed: vec![(
                "copilot-shell".to_string(),
                pkg_info("copilot-shell", "2.2.0", Some("1.al8"), "x86_64"),
            )],
            package_provides: vec![package_component_provide("copilot-shell", "copilot-shell")],
            ..Default::default()
        };
        adopt_with_query("copilot-shell", None, &c, &q).expect("dry-run ok");
        let layout = common::resolve_layout(&c);
        assert!(
            !layout.state_dir.join("installed.toml").exists(),
            "dry-run must not write state",
        );
    }

    /// A pending operation journal blocks adopt before any rpmdb resolution.
    #[test]
    fn adopt_refuses_pending_rpm_install_claim() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let layout = common::resolve_layout(&c);
        rpm_install::begin_fresh_install(&layout, "cosh", "copilot-shell", "install cosh")
            .expect("begin pending install");
        let q = FakeQuery::default();

        let err = adopt_with_query("cosh", Some("copilot-shell"), &c, &q)
            .expect_err("adopt must not bypass a pending managed install");
        assert!(err.reason().contains("repair cosh"));
    }

    /// `--package` pins the RPM name, bypassing the candidate chain.
    #[test]
    fn adopt_with_package_override_adopts_named() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(&c, Vec::new());
        let q = FakeQuery {
            installed: vec![(
                "custom-pkg".to_string(),
                pkg_info("custom-pkg", "3.0.0", Some("1"), "x86_64"),
            )],
            provides: vec![component_provider("copilot-shell", "custom-pkg")],
            ..Default::default()
        };
        adopt_with_query("copilot-shell", Some("custom-pkg"), &c, &q).expect("adopt ok");
        let store = load_store(&c);
        let (package, _, _) = delegated_parts(&store, "copilot-shell");
        assert_eq!(package, "custom-pkg", "the pinned package is recorded");
    }

    // ── in-lock re-validation (concurrent-writer refusals) ──

    fn empty_store() -> StateStore {
        StateStore::load(std::path::Path::new("/nonexistent/installed.toml"), 0)
            .expect("empty store")
    }

    fn store_with(binding: ProviderBinding) -> StateStore {
        let mut store = empty_store();
        store.upsert(anolisa_core::domain::Installation {
            kind: ObjectKind::Component,
            name: "copilot-shell".to_string(),
            scope: InstallationScope::System,
            binding,
            status: anolisa_core::domain::LifecycleStatus::Installed,
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: None,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            health: Vec::new(),
        });
        store
    }

    fn owned_binding() -> ProviderBinding {
        ProviderBinding::Owned {
            artifact: anolisa_core::domain::OwnedArtifact {
                version: "1.0.0".to_string(),
                distribution_source: None,
                raw_package: None,
                manifest_digest: None,
                files: Vec::new(),
                services: Vec::new(),
                external_modified_files: Vec::new(),
                provisioned_packages: Vec::new(),
            },
        }
    }

    fn delegated_binding(relation: ManagementRelation) -> ProviderBinding {
        ProviderBinding::Delegated {
            pm: NativePm::Rpm,
            package: PackageIdentity::Resolved {
                name: "copilot-shell".to_string(),
            },
            relation,
            last_observed: None,
        }
    }

    /// A fresh adopt planned against an empty store must refuse when a
    /// concurrent raw install recorded the component first.
    #[test]
    fn adopt_authorized_refuses_concurrent_raw_install() {
        let store = store_with(owned_binding());
        let err = adopt_authorized(&store, "copilot-shell", &AdoptShape::Fresh, "adopt x")
            .expect_err("a record that appeared under the lock must refuse the adopt");
        assert!(
            err.reason().contains("appeared") && err.reason().contains("nothing was changed"),
            "got: {}",
            err.reason()
        );
    }

    /// An observed→adopted upgrade must refuse when a concurrent managed
    /// install replaced the observed record — adopt must never silently
    /// downgrade managed provenance.
    #[test]
    fn adopt_authorized_refuses_concurrent_managed_install() {
        let store = store_with(delegated_binding(ManagementRelation::Managed {
            since: "2026-06-01T10:00:00Z".to_string(),
        }));
        let shape = AdoptShape::UpgradeObserved {
            package: "copilot-shell".to_string(),
        };
        let err = adopt_authorized(&store, "copilot-shell", &shape, "adopt x")
            .expect_err("a record that changed under the lock must refuse the adopt");
        assert!(err.reason().contains("changed"), "got: {}", err.reason());
    }

    /// The happy paths pass re-validation: an empty store for a fresh adopt,
    /// a matching observed record for an upgrade.
    #[test]
    fn adopt_authorized_allows_planned_shapes() {
        adopt_authorized(
            &empty_store(),
            "copilot-shell",
            &AdoptShape::Fresh,
            "adopt x",
        )
        .expect("fresh adopt over an empty store");
        let store = store_with(delegated_binding(ManagementRelation::Observed));
        let shape = AdoptShape::UpgradeObserved {
            package: "copilot-shell".to_string(),
        };
        adopt_authorized(&store, "copilot-shell", &shape, "adopt x")
            .expect("upgrade over the matching observed record");
    }

    /// `AdoptArgs` parses the positional component and the optional
    /// `--package`.
    #[test]
    fn adopt_parses_positional_and_package_flag() {
        use clap::Parser;
        let args = AdoptArgs::try_parse_from(["adopt", "copilot-shell", "--package", "pkg-x"])
            .expect("parse");
        assert_eq!(args.component, "copilot-shell");
        assert_eq!(args.package.as_deref(), Some("pkg-x"));

        let bare = AdoptArgs::try_parse_from(["adopt", "copilot-shell"]).expect("parse");
        assert_eq!(bare.package, None);
    }
}
