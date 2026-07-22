//! `anolisa update` — unified update surface.
//!
//! Three forms:
//! - `update <component>` - update one ANOLISA-managed component.
//! - `update self` - update the `anolisa` CLI binary only.
//! - `update all` - update every ANOLISA-managed runtime, osbase, and
//!   adapter object.
//!
//! The component is a positional argument; `self` / `all` are subcommands
//! (kept mutually exclusive with the positional via
//! `args_conflicts_with_subcommands`). A component literally named `self` or
//! `all` would be shadowed by the subcommand — those are reserved.
//!
//! Explicit invariant: `update all` does **not** include CLI self-update. The
//! binary swap never shares a transaction with component updates.
//!
//! `update <component>` runs the thin-shell pipeline: assemble facts, ask
//! the planner (decision rows U1–U8), and hand the step sequence to the
//! matching executor. The plan shape follows the record's authority:
//!
//!   * **Owned** (raw) — U3: the CLI resolves the latest published version
//!     and classifies it against the recorded one; a newer version replaces
//!     the owned files through the owned executor (compensating back to the
//!     previous files on failure), the same version is a clean no-op (U2),
//!     and an older or non-orderable version refuses (U4).
//!   * **Delegated managed/adopted** — U5: `dnf update` through the
//!     delegated executor, then re-observe and refresh the cached
//!     observation. dnf picks the target version. After the record refresh
//!     persisted, the package-owned contract snapshot is refreshed too
//!     ([`complete_delegated_update`]); a refresh failure demotes the
//!     operation to `partial` instead of overstating it as `ok`.
//!   * **Delegated observed** — U6: refuses; management consent (`adopt`)
//!     is required before native transactions.
//!
//! Backend/authority is never switched by an update. `update all` (the
//! [`all`] module) plans every recorded component through the same decision
//! rows and merges the delegated refreshes (U5) into one native transaction.

use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use clap::{Parser, Subcommand};
use serde::Serialize;

use anolisa_core::central_log::{CentralLog, LogKind, LogRecord, LogStatus, Severity};
use anolisa_core::domain::{
    InstallationScope, ManagementRelation, NativePm, OwnedArtifact, ProviderBinding,
};
use anolisa_core::executor::{DelegatedExecutionTarget, execute_delegated_steps};
use anolisa_core::facts::{JournalEvidence, ObserveRequest, assemble_facts};
use anolisa_core::lock::InstallLock;
use anolisa_core::owned_executor::{OwnedExecutionError, execute_owned_steps};
use anolisa_core::planner::{
    Facts, Intent, NativeProbe, OwnedUpdateResolution, Plan, PlanError, Step, UpdateRequest,
    VersionRelation, plan,
};
use anolisa_core::providers::DelegatedProvider;
use anolisa_core::record_sink::{DelegatedIdentity, RecordContext, StoreRecordSink};
use anolisa_core::self_update::{self, ProgressFn, SelfUpdateOutcome};
use anolisa_core::state::{ObjectKind, OperationRecord};
use anolisa_core::state_store::StateStore;
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::pkg_query::{PackageQuery, PackageQueryError};
use anolisa_platform::pkg_transaction::{PackageTransaction, PackageTransactionError};
use anolisa_platform::privilege;
use anolisa_platform::rpm_query::RpmPackageQuery;
use anolisa_platform::rpm_repo::DnfRepoSource;
use anolisa_platform::rpm_transaction::RpmTransaction;

use super::install::{
    RawReplayOps, RawResolution, refresh_datadir_contract_snapshot, resolve_raw,
    resolve_raw_inputs_for_component,
};
use super::recovery::LockedJournalGate;
use super::rpm_install;
use crate::color::Palette;
use crate::commands::common;
use crate::commands::common::RepoPersistPolicy;
use crate::context::CliContext;
use crate::repo_config::{HostVars, RepoConfig};
use crate::response::{self, CliError};

// `pub(crate)` so `anolisa upgrade` (issue #1411) can reuse the read-only
// planner (`check::compute_update_check_report`) instead of re-deriving it.
pub(crate) mod all;
pub(crate) mod check;

/// Command label for JSON envelopes and error routing.
const COMMAND: &str = "update";

const CLI_CHANGELOG_URL: &str = "https://agentic-os.sh/#anolisa-cli-changelog";

const ANOLISA_RPM_REPO_ID: &str = "anolisa-configured";

/// Arguments for the unified update command surface.
///
/// `anolisa update <component>` updates a single component directly; the
/// `self` and `all` subcommands cover the CLI binary and batch
/// update. `args_conflicts_with_subcommands` keeps the positional and the
/// subcommands mutually exclusive so `update foo self` is a parse error.
#[derive(Debug, Parser)]
#[command(args_conflicts_with_subcommands = true)]
pub struct UpdateArgs {
    /// Component to update (omit when using a `self` / `all` subcommand)
    #[arg(value_name = "COMPONENT")]
    pub component: Option<String>,
    /// Update the CLI binary (`self`) or every component (`all`) instead of a
    /// single component.
    #[command(subcommand)]
    pub command: Option<UpdateCommands>,
    /// Report which RPM-backed components can be upgraded, read-only.
    ///
    /// `--check` only runs read-only rpm/dnf queries (it does run `dnf
    /// repoquery` for candidates, but no mutating `dnf` transaction), never
    /// writes ANOLISA state, and never persists repo/adapter configuration. It
    /// is mutually exclusive with a component
    /// argument and with the `self` / `all` subcommands (the latter is enforced
    /// by `args_conflicts_with_subcommands`). `--motd`, `--refresh`, and
    /// `--target` are only meaningful together with `--check`.
    #[arg(long)]
    pub check: bool,
    /// With `--check`: emit a short, low-noise MOTD summary (silent when
    /// nothing can be upgraded).
    #[arg(long, requires = "check")]
    pub motd: bool,
    /// With `--check`: bypass the cached report and re-query.
    #[arg(long, requires = "check")]
    pub refresh: bool,
    /// With `--check`: evaluate against a named target profile so its missing
    /// default components are reported as installable.
    #[arg(long, requires = "check", value_name = "TARGET")]
    pub target: Option<String>,
}

/// Update operations that intentionally keep CLI self-update and batch update
/// separate from a single-component update.
#[derive(Debug, Subcommand)]
pub enum UpdateCommands {
    /// Update the anolisa CLI binary only
    #[command(name = "self")]
    SelfBin,
    /// Update every recorded ANOLISA component.
    ///
    /// Plans each component through the same decision rows as a single
    /// update and merges the delegated refreshes into one dnf transaction.
    /// Does NOT include the CLI binary itself — use `anolisa update self`
    /// for that.
    All,
}

/// Dispatches the selected `anolisa update` form.
///
/// # Errors
///
/// Returns [`CliError`] when the selected update operation fails or no target
/// is given.
pub fn handle(args: UpdateArgs, ctx: &CliContext) -> Result<(), CliError> {
    // `--check` is the read-only upgrade-detection entry (issue #1410). It is
    // dispatched before the mutating forms so a stray component/subcommand is
    // rejected instead of silently updating something.
    if args.check {
        if args.component.is_some() {
            return Err(CliError::InvalidArgument {
                command: check::CHECK_COMMAND.to_string(),
                reason: "`--check` reports upgrades for every managed component and takes no component argument; run `anolisa update --check`".to_string(),
            });
        }
        // `args_conflicts_with_subcommands` already makes `update --check self`
        // a parse error; this guard keeps the invariant explicit for any direct
        // caller that bypasses clap.
        if args.command.is_some() {
            return Err(CliError::InvalidArgument {
                command: check::CHECK_COMMAND.to_string(),
                reason: "`--check` cannot be combined with the `self` or `all` subcommands"
                    .to_string(),
            });
        }
        return check::handle_update_check(&args, ctx);
    }

    // `args_conflicts_with_subcommands` guarantees `command` and `component`
    // are never both set, so a present subcommand always wins.
    match (args.command, args.component) {
        (Some(UpdateCommands::SelfBin), _) => handle_self_update(ctx),
        (Some(UpdateCommands::All), _) => all::handle_update_all(ctx),
        (None, Some(component)) => handle_component_update(&component, ctx),
        (None, None) => Err(CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: "specify a component to update (e.g. `anolisa update <component>`), or use `anolisa update self` / `anolisa update all`".to_string(),
        }),
    }
}

// `pub(crate)`: shared with `check` (read-only) and `upgrade` (issue #1411) so
// all three resolve the configured ANOLISA RPM repository the same way.
pub(crate) fn rpm_repo_source_for_update(
    repo_config: &RepoConfig,
    env: &anolisa_env::EnvFacts,
    command: &str,
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
        .map_err(|err| CliError::InvalidArgument {
            command: command.to_string(),
            reason: err.to_string(),
        })?;
    Ok(Some(DnfRepoSource::new(
        ANOLISA_RPM_REPO_ID,
        base_url,
        backend.gpgcheck,
    )))
}

// ── component update: the thin-shell pipeline ──

/// Wire shape for an `update <component>` result (`--json`) and its dry-run
/// preview.
#[derive(Serialize)]
struct UpdateResultPayload {
    component: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    package: Option<String>,
    /// Version recorded/observed before the update, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    from_version: Option<String>,
    /// Version after the update (or the resolved target on dry-run).
    #[serde(skip_serializing_if = "Option::is_none")]
    to_version: Option<String>,
    /// Whether the version actually changed (false on "already latest").
    updated: bool,
    dry_run: bool,
    plan: Vec<String>,
    /// `None` on dry-run (nothing recorded).
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
}

/// Wire `update <component>` to the live host: a delegated record updates
/// from the configured ANOLISA RPM repository (which must be declared in
/// repo.toml), an owned record resolves its latest published version through
/// the raw backend inside the pipeline.
fn handle_component_update(component: &str, ctx: &CliContext) -> Result<(), CliError> {
    let (query, txn) = update_backends(component, ctx)?;
    update_component_with_deps(component, ctx, &query, &txn, privilege::is_root()).map(|_| ())
}

/// Real host backends for one component update: rpm query/transaction
/// pointed at the configured ANOLISA repo when the record is delegated, so
/// candidate probes and the update transaction never fall back to arbitrary
/// host repos.
pub(crate) fn update_backends(
    component: &str,
    ctx: &CliContext,
) -> Result<(RpmPackageQuery, RpmTransaction), CliError> {
    let command = format!("update {component}");
    let layout = common::resolve_layout(ctx);
    let (resolved, view) = common::resolve_mutation_target(component, ctx, &command)?;
    let store = &view.writable.state;
    let is_delegated = matches!(
        store
            .find(ObjectKind::Component, &resolved)
            .map(|r| &r.binding),
        Some(ProviderBinding::Delegated { .. })
    );
    if is_delegated {
        let repo_config =
            common::load_repo_config(ctx, &layout, &command, RepoPersistPolicy::Require)?;
        let env = anolisa_env::EnvService::detect();
        let repo = rpm_repo_source_for_update(&repo_config, &env, &command)?.ok_or_else(|| {
            CliError::InvalidArgument {
                command: command.clone(),
                reason: "repo.toml has no [backends.rpm] table; cannot update an RPM-backed component from the configured ANOLISA repository".to_string(),
            }
        })?;
        return Ok((
            RpmPackageQuery::system_with_repo(repo.clone()),
            RpmTransaction::system_with_repo(repo),
        ));
    }
    Ok((RpmPackageQuery::system(), RpmTransaction::system()))
}

/// What a component update left behind, for batch summaries: a transaction
/// (or file replacement) ran, or the record already covered the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpdateOutcome {
    Updated,
    AlreadyCurrent,
}

/// What the planning prefix decided for one component update, before any
/// side effect ran. The single-component path executes it directly; batch
/// orchestration classifies on the route to group delegated refreshes (U5)
/// into one merged native transaction.
pub(crate) struct PlannedComponentUpdate {
    pub(crate) command: String,
    pub(crate) target: String,
    pub(crate) native_package: Option<String>,
    pub(crate) scope: InstallationScope,
    pub(crate) now: String,
    /// Raw artifact resolution prepared for an owned record (U2–U4).
    pub(crate) owned_execution: Option<(RawResolution, OwnedArtifact)>,
    /// `(recorded, latest published)` versions for an owned record.
    pub(crate) owned_versions: Option<(String, String)>,
    /// rpmdb EVR the planning observation saw, for the wire `from` field and
    /// the merged-failure fact check.
    pub(crate) native_from: Option<String>,
    pub(crate) route: PlannedUpdateRoute,
}

/// Which executor family the update plan routed to, or the idempotent NoOp.
pub(crate) enum PlannedUpdateRoute {
    /// U2: the recorded version is already the latest published one.
    AlreadyCurrent,
    /// Delegated step family (U5: one native update, observe, refresh).
    Delegated { steps: Vec<Step> },
    /// Owned step family (U3: replace files through the raw backend).
    Owned { steps: Vec<Step> },
}

/// Core of [`handle_component_update`] with the package backends injected so
/// tests drive every branch without a live rpmdb/dnf or real privileges.
///
/// The pipeline observes, plans (decision rows U1–U8), and routes the step
/// sequence to the matching executor: a delegated plan re-runs `dnf update`
/// through the delegated executor, an owned plan replaces the recorded files
/// with the resolved latest published version through the owned executor.
pub(crate) fn update_component_with_deps(
    input: &str,
    ctx: &CliContext,
    query: &dyn PackageQuery,
    txn: &dyn PackageTransaction,
    is_root: bool,
) -> Result<UpdateOutcome, CliError> {
    let planned = plan_component_update(input, ctx, query, txn)?;
    execute_planned_update(planned, ctx, query, txn, is_root)
}

/// Planning prefix of a component update: resolve the record, assemble host
/// facts (and the owned artifact resolution when the record is owned), and
/// ask the planner for the step sequence. Read-only against the host —
/// every side effect belongs to [`execute_planned_update`].
pub(crate) fn plan_component_update(
    input: &str,
    ctx: &CliContext,
    query: &dyn PackageQuery,
    txn: &dyn PackageTransaction,
) -> Result<PlannedComponentUpdate, CliError> {
    let command = format!("update {input}");
    let layout = common::resolve_layout(ctx);
    let journal_dir = rpm_install::journal_dir(&layout);
    let uid = privilege::effective_uid();
    let scope = match ctx.install_mode {
        crate::context::InstallMode::System => InstallationScope::System,
        crate::context::InstallMode::User => InstallationScope::User { uid },
    };
    let now = now_iso8601();

    let (resolved, view) = common::resolve_mutation_target(input, ctx, &command)?;
    let store = view.writable.state;
    let target = resolved.as_str();

    // The probe target comes from the record; update never switches the
    // package a component is bound to.
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

    // Whether the record's relation would drive a native transaction —
    // decides between hard "install rpm/dnf" guidance and replanning without
    // the probe (an observed record refuses with "adopt first" whether or
    // not the tooling exists).
    let record_needs_native = matches!(
        store
            .find(ObjectKind::Component, target)
            .map(|r| &r.binding),
        Some(ProviderBinding::Delegated {
            relation: ManagementRelation::Managed { .. } | ManagementRelation::Adopted { .. },
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
        // rpm missing on PATH. An update that would run a package operation
        // cannot proceed; anything else replans without the probe so the
        // planner can name the real way out.
        Err(anolisa_core::facts::FactsError::Probe(
            anolisa_core::providers::ProviderError::Query(PackageQueryError::CommandMissing {
                command: bin,
            }),
        )) => {
            if record_needs_native {
                return Err(tooling_missing_err(&command, &bin, target));
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

    // An owned record needs its update target resolved before planning: the
    // CLI resolves the latest published version and classifies it against
    // the recorded one; the planner turns that relation into U2–U4.
    let mut owned_execution: Option<(RawResolution, OwnedArtifact)> = None;
    let owned_resolution = match store
        .find(ObjectKind::Component, target)
        .map(|r| &r.binding)
    {
        Some(ProviderBinding::Owned { artifact }) => {
            let repo_config =
                common::load_repo_config(ctx, &layout, &command, RepoPersistPolicy::Require)?;
            let env = anolisa_env::EnvService::detect();
            let inputs = resolve_raw_inputs_for_component(
                target.to_string(),
                "raw",
                artifact.raw_package.as_deref(),
                &env,
                &repo_config,
                &command,
            )?;
            let resolution =
                resolve_raw(ctx, &layout, &env, inputs).map_err(|e| e.with_command(&command))?;
            let to_version = resolution.entry.version.clone();
            let relation = version_relation(&artifact.version, &to_version);
            owned_execution = Some((resolution, artifact.clone()));
            Some(OwnedUpdateResolution {
                to_version,
                relation,
            })
        }
        _ => None,
    };
    let owned_versions = owned_execution
        .as_ref()
        .map(|(resolution, prior)| (prior.version.clone(), resolution.entry.version.clone()));

    let intent = Intent::Update(UpdateRequest { owned_resolution });
    let route = match plan(&intent, &facts) {
        Ok(Plan::Execute { steps, .. }) => {
            // Route by step family: owned plans replace files through the raw
            // backend, delegated plans re-run the native transaction.
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
                PlannedUpdateRoute::Delegated { steps }
            } else {
                PlannedUpdateRoute::Owned { steps }
            }
        }
        Ok(Plan::NoOp { .. }) => PlannedUpdateRoute::AlreadyCurrent,
        Err(err) => {
            return Err(plan_error_to_cli(
                err,
                target,
                &command,
                owned_versions.as_ref(),
            ));
        }
    };

    Ok(PlannedComponentUpdate {
        command,
        target: target.to_string(),
        native_package,
        scope,
        now,
        owned_execution,
        owned_versions,
        native_from: native_observed_version(&facts),
        route,
    })
}

/// Execution half of [`update_component_with_deps`]: render the idempotent
/// NoOp, replace an owned artifact's files, or run the delegated native
/// transaction. Dry-run renders the plan and stops before any side effect.
fn execute_planned_update(
    planned: PlannedComponentUpdate,
    ctx: &CliContext,
    query: &dyn PackageQuery,
    txn: &dyn PackageTransaction,
    is_root: bool,
) -> Result<UpdateOutcome, CliError> {
    let PlannedComponentUpdate {
        command,
        target,
        native_package,
        scope,
        now,
        owned_execution,
        owned_versions,
        native_from,
        route,
    } = planned;
    let target = target.as_str();
    let layout = common::resolve_layout(ctx);
    let state_path = layout.state_dir.join("installed.toml");
    let journal_dir = rpm_install::journal_dir(&layout);
    let uid = privilege::effective_uid();

    let steps = match route {
        PlannedUpdateRoute::AlreadyCurrent => {
            // U2: the recorded version is already the latest published one.
            let (from, to) = match owned_versions {
                Some((from, to)) => (Some(from), Some(to)),
                None => (None, None),
            };
            let package = owned_execution
                .map(|(resolution, _)| resolution.package)
                .or(native_package);
            render_result(
                ctx,
                target,
                package.as_deref(),
                from.as_deref(),
                to.as_deref(),
                false,
                ctx.dry_run,
                &[],
                None,
            )?;
            return Ok(UpdateOutcome::AlreadyCurrent);
        }
        PlannedUpdateRoute::Owned { steps } => {
            let plan_labels: Vec<String> = steps.iter().map(step_label).collect();
            if ctx.dry_run {
                let from_version = owned_versions.as_ref().map(|(from, _)| from.clone());
                let to_version = owned_versions.as_ref().map(|(_, to)| to.clone());
                let package = owned_execution
                    .as_ref()
                    .map(|(resolution, _)| resolution.package.clone())
                    .or_else(|| native_package.clone());
                render_result(
                    ctx,
                    target,
                    package.as_deref(),
                    from_version.as_deref(),
                    to_version.as_deref(),
                    false,
                    true,
                    &plan_labels,
                    None,
                )?;
                return Ok(UpdateOutcome::Updated);
            }
            let (resolution, prior) = owned_execution.ok_or_else(|| CliError::Runtime {
                command: command.clone(),
                reason: format!(
                    "internal: planner produced an owned plan but no resolution was prepared for '{target}'"
                ),
            })?;
            update_owned(
                target,
                ctx,
                &layout,
                &state_path,
                &journal_dir,
                scope,
                &now,
                &steps,
                &plan_labels,
                resolution,
                prior,
                &command,
            )?;
            return Ok(UpdateOutcome::Updated);
        }
        PlannedUpdateRoute::Delegated { steps } => steps,
    };

    let plan_labels: Vec<String> = steps.iter().map(step_label).collect();

    if ctx.dry_run {
        render_result(
            ctx,
            target,
            native_package.as_deref(),
            native_from.as_deref(),
            None,
            false,
            true,
            &plan_labels,
            None,
        )?;
        return Ok(UpdateOutcome::Updated);
    }

    // dnf transactions need root; check up front so the user gets an
    // actionable message instead of dnf's raw mid-transaction refusal.
    if !is_root {
        return Err(CliError::Runtime {
            command,
            reason: format!(
                "updating system RPM '{}' requires root privileges; re-run with sudo: `sudo anolisa update {target}`",
                native_package.as_deref().unwrap_or(target)
            ),
        });
    }

    let provider = DelegatedProvider::new(query, txn);
    let from_version = native_from;

    // Real run under the install lock, with state re-read and the update
    // authority re-validated inside it: dnf runs against the pre-lock
    // package identity, and grafting its result onto a record a concurrent
    // operation re-pointed or downgraded would corrupt it.
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
    if !native_update_authorized(&store, target, native_package.as_deref()) {
        return Err(CliError::Runtime {
            command,
            reason: format!(
                "component '{target}' changed while this update was planning; nothing was changed — re-run `anolisa update {target}`"
            ),
        });
    }

    let package = native_package.clone().unwrap_or_else(|| target.to_string());
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
        delegated: Some(DelegatedIdentity {
            pm: NativePm::Rpm,
            package: package.clone(),
        }),
        owned_artifact: None,
    };
    let outcome = {
        let mut sink = StoreRecordSink::new(&mut store, &state_path, context);
        execute_delegated_steps(
            &steps,
            DelegatedExecutionTarget::new(NativePm::Rpm, Some(&package)),
            &provider,
            &mut sink,
            &mut journal,
            &now,
        )
    }
    .map_err(|err| match err {
        // dnf missing even though the rpmdb query succeeded: same guidance
        // as the query-missing branch rather than a generic failure.
        anolisa_core::executor::ExecutionError::TransactionFailed {
            source:
                anolisa_core::providers::ProviderError::Transaction(
                    PackageTransactionError::CommandMissing { command: bin },
                ),
            ..
        } => tooling_missing_err(&command, &bin, target),
        other => CliError::Runtime {
            command: command.clone(),
            reason: format!(
                "update of '{target}' failed: {other}; the native transaction is never undone automatically — run `anolisa repair {target}` to reconcile"
            ),
        },
    })?;

    // The record refresh was committed by the executor's sink; refresh the
    // package-owned contract snapshot before the durable status is written,
    // so `status`/`doctor`/adapter resolution read the new contract without
    // waiting for a repair, and a refresh failure demotes the operation to
    // `partial` instead of overstating it as `ok`.
    let completion = complete_delegated_update(&layout, ctx, target, &package, &command);

    // Operation history is best-effort bookkeeping on top of the committed
    // record refresh.
    store.operations.push(OperationRecord {
        id: operation_id.clone(),
        command: command.clone(),
        status: completion.operation_status.to_string(),
        started_at: now.clone(),
        finished_at: Some(now_iso8601()),
        parent_operation_id: None,
    });
    if let Err(err) = store.save(&state_path) {
        eprintln!("warning: failed to record operation history: {err}");
    }

    let to_version = outcome
        .observation
        .as_ref()
        .map(|o| o.evr.clone().unwrap_or_else(|| o.version.clone()));
    let updated = match (&from_version, &to_version) {
        (Some(from), Some(to)) => from != to,
        _ => true,
    };

    append_update_log(
        &layout,
        ctx,
        target,
        &command,
        &operation_id,
        &now,
        &package,
        to_version.as_deref(),
        completion.failure.as_deref(),
    );

    if let Some(reason) = completion.failure {
        return Err(CliError::Runtime {
            command: command.clone(),
            reason: format!(
                "the update of '{target}' committed, but {reason}; run `anolisa repair {target}` to reconcile"
            ),
        });
    }

    render_result(
        ctx,
        target,
        Some(&package),
        from_version.as_deref(),
        to_version.as_deref(),
        updated,
        false,
        &plan_labels,
        Some(&operation_id),
    )?;
    Ok(if updated {
        UpdateOutcome::Updated
    } else {
        UpdateOutcome::AlreadyCurrent
    })
}

/// Execute an owned update plan (U3) through the raw backend: replace the
/// recorded files with the resolved latest published version, compensating
/// back to the previous files on failure.
///
/// The store is re-read under the install lock so the backup/remove set can
/// never come from a stale snapshot; a version drift under the lock aborts
/// before anything is touched.
#[expect(clippy::too_many_arguments)]
fn update_owned(
    target: &str,
    ctx: &CliContext,
    layout: &FsLayout,
    state_path: &Path,
    journal_dir: &Path,
    scope: InstallationScope,
    now: &str,
    steps: &[Step],
    plan_labels: &[String],
    resolution: RawResolution,
    prior: OwnedArtifact,
    command: &str,
) -> Result<(), CliError> {
    // No root pre-check: `--prefix` may point at a user-writable tree, and a
    // genuine permission problem fails the exact step and unwinds honestly
    // instead of a blanket refusal.
    let resolve_warnings = resolution.warnings.clone();
    let package = resolution.package.clone();
    let from_version = prior.version.clone();
    let to_version = resolution.entry.version.clone();

    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;
    let mut store = StateStore::load_for_layout(state_path, privilege::effective_uid(), layout)
        .map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!("failed to load installed state: {err}"),
        })?;
    let prior = match store
        .find(ObjectKind::Component, target)
        .map(|r| &r.binding)
    {
        Some(ProviderBinding::Owned { artifact }) if artifact.version == prior.version => {
            artifact.clone()
        }
        Some(ProviderBinding::Owned { artifact }) => {
            return Err(CliError::Runtime {
                command: command.to_string(),
                reason: format!(
                    "component '{target}' changed from {} to {} while this update was resolving; nothing was changed — re-run `anolisa update {target}`",
                    prior.version, artifact.version
                ),
            });
        }
        _ => {
            return Err(CliError::Runtime {
                command: command.to_string(),
                reason: format!(
                    "component '{target}' is no longer an owned installation; nothing was changed — re-run `anolisa update {target}`"
                ),
            });
        }
    };

    let evidence = JournalEvidence::new(journal_dir, &store.operations);
    let mut journal_gate = LockedJournalGate::load(&_lock, evidence, command)?;
    let mut journal = journal_gate.begin(COMMAND, target, state_path.to_path_buf(), command)?;
    let operation_id = journal.operation_id.clone();

    let outcome = {
        let mut ops = RawReplayOps::new(
            ctx,
            layout,
            target.to_string(),
            scope,
            now.to_string(),
            operation_id.clone(),
            resolution,
            prior,
            &mut store,
            state_path,
        )
        .with_runtime_preflight();
        let result = execute_owned_steps(steps, &mut ops, &mut journal);
        if result.is_ok() {
            // Per-operation backups are rollback scratch; a failed plan keeps
            // them on disk for forensics.
            ops.discard_backups();
        }
        result
    }
    .map_err(|err| owned_error_to_cli(err, target, scope, command))?;

    // Operation history is best-effort bookkeeping on top of the committed
    // record, exactly like the delegated path.
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

    append_update_log(
        layout,
        ctx,
        target,
        command,
        &operation_id,
        now,
        &package,
        Some(&to_version),
        // Owned updates replace the manifest as part of the owned artifact;
        // there is no datadir contract refresh to fail.
        None,
    );

    render_result(
        ctx,
        target,
        Some(&package),
        Some(&from_version),
        Some(&to_version),
        true,
        false,
        plan_labels,
        Some(&operation_id),
    )
}

/// Classify a resolved `candidate` version against the `installed` one,
/// semver-aware (tolerating a leading `v`).
///
/// When either side is not valid semver, equal normalized strings are
/// [`VersionRelation::Same`] and anything else is
/// [`VersionRelation::Indeterminate`] — a non-semver version is never
/// silently treated as an upgrade, so the planner's downgrade guard (U4)
/// stays effective for it (a non-semver installed version that is actually
/// newer must not be replaced by an older published one).
fn version_relation(installed: &str, candidate: &str) -> VersionRelation {
    fn norm(s: &str) -> &str {
        let t = s.trim();
        t.strip_prefix('v').unwrap_or(t)
    }
    match (
        semver::Version::parse(norm(installed)),
        semver::Version::parse(norm(candidate)),
    ) {
        (Ok(installed), Ok(candidate)) => match candidate.cmp(&installed) {
            std::cmp::Ordering::Less => VersionRelation::Older,
            std::cmp::Ordering::Equal => VersionRelation::Same,
            std::cmp::Ordering::Greater => VersionRelation::Newer,
        },
        _ if norm(installed) == norm(candidate) => VersionRelation::Same,
        _ => VersionRelation::Indeterminate,
    }
}

/// EVR (or plain version) the pre-update native probe observed, for the
/// human/JSON "from" field.
fn native_observed_version(facts: &Facts) -> Option<String> {
    match &facts.native {
        NativeProbe::Present { observation, .. } => Some(
            observation
                .evr
                .clone()
                .unwrap_or_else(|| observation.version.clone()),
        ),
        _ => None,
    }
}

/// Whether the record, as re-read under the install lock, still authorizes
/// the planned native update: it must still be delegated, still point at the
/// same resolved package (`dnf update` ran against the pre-lock identity,
/// and recording its result against a different package would graft one
/// package's version onto another's record), and still carry management
/// consent (an observed record never plans a native update).
pub(crate) fn native_update_authorized(
    store: &StateStore,
    target: &str,
    package: Option<&str>,
) -> bool {
    match store
        .find(ObjectKind::Component, target)
        .map(|r| &r.binding)
    {
        Some(ProviderBinding::Delegated {
            relation: ManagementRelation::Managed { .. } | ManagementRelation::Adopted { .. },
            package: recorded,
            ..
        }) => recorded.resolved_name() == package,
        _ => false,
    }
}

/// Map a planning refusal to an actionable CLI error. The planner names the
/// way out; this mapping only renders it. `owned_versions` carries the
/// `(installed, latest)` pair resolved for an owned record so the downgrade
/// refusals can name both sides.
fn plan_error_to_cli(
    err: PlanError,
    target: &str,
    command: &str,
    owned_versions: Option<&(String, String)>,
) -> CliError {
    let command = command.to_string();
    match err {
        PlanError::NotInstalled => CliError::InvalidArgument {
            command,
            reason: format!(
                "component '{target}' is not installed — nothing to update (run `anolisa status` to see what is installed, or `anolisa install {target}` to install it)"
            ),
        },
        PlanError::NotAdopted => CliError::InvalidArgument {
            command,
            reason: format!(
                "component '{target}' is only observed, not managed; run `anolisa adopt {target}` first, then update"
            ),
        },
        PlanError::ExternallyRemoved => CliError::InvalidArgument {
            command,
            reason: format!(
                "the package backing '{target}' was removed outside ANOLISA; run `anolisa repair {target}` to reconcile or `anolisa forget {target}` to drop the record"
            ),
        },
        PlanError::RefuseDowngrade => {
            let detail = match owned_versions {
                Some((installed, latest)) => format!(
                    "the latest version published for '{target}' is {latest}, older than the installed {installed}"
                ),
                None => format!(
                    "the latest version published for '{target}' is older than the installed one"
                ),
            };
            CliError::InvalidArgument {
                command,
                reason: format!("{detail}; refusing to downgrade (update only moves forward)"),
            }
        }
        PlanError::IndeterminateVersion => {
            let detail = match owned_versions {
                Some((installed, latest)) => format!(
                    "cannot tell whether the published {latest} is newer than the installed {installed} for '{target}' (non-semver version)"
                ),
                None => format!(
                    "cannot tell whether the published version is newer than the installed one for '{target}' (non-semver version)"
                ),
            };
            CliError::InvalidArgument {
                command,
                reason: format!(
                    "{detail}; refusing to replace it to avoid an accidental downgrade"
                ),
            }
        }
        PlanError::NeedsAttention => CliError::InvalidArgument {
            command,
            reason: format!(
                "the record for '{target}' was quarantined by the state migration; run `anolisa repair {target}` to resolve it"
            ),
        },
        PlanError::PendingOperation => CliError::Runtime {
            command,
            reason: format!(
                "a previous operation on '{target}' is pending recovery; run `anolisa repair {target}` before retrying"
            ),
        },
        PlanError::PackageUnresolved => CliError::Runtime {
            command,
            reason: format!(
                "the record for '{target}' has no resolved package name; run `anolisa repair {target}` first"
            ),
        },
        other => CliError::InvalidArgument {
            command,
            reason: format!("cannot update '{target}': {other:?}"),
        },
    }
}

/// Map an owned-executor failure to a CLI error that reports honestly what
/// happened to the host: cleanly restored, partially restored, or untouched.
fn owned_error_to_cli(
    err: OwnedExecutionError,
    target: &str,
    scope: InstallationScope,
    command: &str,
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
                format!("update of '{target}' failed at '{at}': {source}; the host was not changed")
            } else if rollback_warnings.is_empty() {
                format!(
                    "update of '{target}' failed at '{at}': {source}; the previous files were restored"
                )
            } else {
                format!(
                    "update of '{target}' failed at '{at}': {source}; restoring the previous files reported problems ({}) — run `{repair}`",
                    rollback_warnings.join("; ")
                )
            }
        }
        OwnedExecutionError::RecoveryUncertain { detail, .. } => {
            format!("update of '{target}' failed: {detail}; run `{repair}`")
        }
        other => format!("update of '{target}' failed: {other}"),
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
        Step::DownloadVerify => "download and verify artifact".to_string(),
        Step::BackupFiles => "back up current files".to_string(),
        Step::PlaceFiles => "place files".to_string(),
        Step::SetCapabilities => "apply file capabilities".to_string(),
        Step::RestartServices => "restart services".to_string(),
        Step::RemoveOwnedFiles => "remove owned files".to_string(),
        other => format!("{other:?}"),
    }
}

/// Actionable "rpm/dnf tooling missing" error: a delegated component cannot
/// update without the native package manager.
fn tooling_missing_err(command: &str, bin: &str, target: &str) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: format!(
            "cannot update '{target}': {bin} not found on PATH — install rpm/dnf and retry"
        ),
    }
}

/// Shared completion for a committed delegated update: contract snapshot
/// refresh plus the durable operation status derived from it.
///
/// Both the single-component path and the merged `update all` members run
/// this after their record refresh persisted, so `status`, `doctor`, and
/// adapter resolution read the post-update contract without waiting for a
/// later `repair` or `upgrade` reconciliation.
pub(crate) struct DelegatedUpdateCompletion {
    /// `ok`, or `partial` when the refresh genuinely failed — a durable
    /// operation must never overstate what happened (upgrade/repair
    /// semantics).
    pub(crate) operation_status: &'static str,
    /// Why the refresh did not complete, for the result and central log.
    pub(crate) failure: Option<String>,
}

/// Refresh the package-owned contract snapshot after a delegated update
/// committed and derive the final operation status.
///
/// Must run only after the component record was persisted, so a failed
/// state commit never exposes a new contract. A package that publishes no
/// contract stays a success without new warnings ([`ContractRefreshOutcome`]
/// reports `NotApplicable`); an unreadable contract or a failed
/// snapshot/provenance write demotes the operation to `partial`.
///
/// [`ContractRefreshOutcome`]: super::install::ContractRefreshOutcome
pub(crate) fn complete_delegated_update(
    layout: &FsLayout,
    ctx: &CliContext,
    target: &str,
    package: &str,
    command: &str,
) -> DelegatedUpdateCompletion {
    let refresh = refresh_datadir_contract_snapshot(layout, target, command);
    if !ctx.quiet {
        for warning in &refresh.warnings {
            eprintln!("warning: {warning}");
        }
    }
    match refresh.error_detail() {
        Some(detail) => DelegatedUpdateCompletion {
            operation_status: "partial",
            failure: Some(format!(
                "component manifest refresh for package '{package}' did not complete: {detail}"
            )),
        },
        None => DelegatedUpdateCompletion {
            operation_status: "ok",
            failure: None,
        },
    }
}

/// Best-effort central-log record for a committed update.
///
/// A `completion_failure` (delegated contract refresh not completed) logs
/// `Partial` so the durable history never overstates what happened.
#[expect(clippy::too_many_arguments)]
pub(crate) fn append_update_log(
    layout: &FsLayout,
    ctx: &CliContext,
    component: &str,
    command: &str,
    operation_id: &str,
    started_at: &str,
    package: &str,
    to_version: Option<&str>,
    completion_failure: Option<&str>,
) {
    let log = CentralLog::open(layout.central_log.clone());
    let base = match to_version {
        Some(version) => format!("updated component {component} ({package}) to {version}"),
        None => format!("updated component {component} ({package})"),
    };
    let record = LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.to_string()),
        command: command.to_string(),
        source: "anolisa-cli".to_string(),
        component: Some(component.to_string()),
        severity: Severity::Info,
        message: match completion_failure {
            Some(_) => format!("{base}, but the component manifest refresh did not complete"),
            None => base,
        },
        actor: "cli".to_string(),
        install_mode: Some(ctx.install_mode.as_str().to_string()),
        started_at: started_at.to_string(),
        finished_at: Some(now_iso8601()),
        status: Some(match completion_failure {
            Some(_) => LogStatus::Partial,
            None => LogStatus::Ok,
        }),
        objects: vec![component.to_string()],
        backup_ids: Vec::new(),
        warnings: completion_failure
            .map(|failure| vec![failure.to_string()])
            .unwrap_or_default(),
        details: serde_json::Value::Null,
    };
    if let Err(err) = log.append(&record) {
        eprintln!("warning: failed to write central log: {err}");
    }
}

/// Render the update result (or its dry-run preview).
#[expect(clippy::too_many_arguments)]
fn render_result(
    ctx: &CliContext,
    component: &str,
    package: Option<&str>,
    from_version: Option<&str>,
    to_version: Option<&str>,
    updated: bool,
    dry_run: bool,
    plan_labels: &[String],
    operation_id: Option<&str>,
) -> Result<(), CliError> {
    if ctx.json {
        return response::render_json(
            COMMAND,
            UpdateResultPayload {
                component: component.to_string(),
                package: package.map(str::to_string),
                from_version: from_version.map(str::to_string),
                to_version: to_version.map(str::to_string),
                updated,
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
    // A no-op "already latest" reads the same whether previewed or run.
    if !updated && plan_labels.is_empty() {
        println!(
            "{} {component} is already up to date{}",
            color.ok("✓"),
            from_version.map(|v| format!(" ({v})")).unwrap_or_default(),
        );
        return Ok(());
    }
    if dry_run {
        println!(
            "{} {component} {}",
            color.command("update"),
            color.muted("(dry-run — nothing updated)"),
        );
        match (from_version, to_version) {
            (Some(from), Some(to)) => {
                println!("{} {from} → {to}", color.label("would update:"));
            }
            (Some(from), None) => println!("{} {from}", color.label("current:")),
            _ => {}
        }
        for label in plan_labels {
            println!("  - {label}");
        }
        return Ok(());
    }
    if updated {
        println!(
            "{} {component} {} → {}",
            color.ok("✓ updated"),
            from_version.unwrap_or("-"),
            to_version.unwrap_or("-"),
        );
    } else {
        println!(
            "{} {component} is already up to date{}",
            color.ok("✓"),
            from_version.map(|v| format!(" ({v})")).unwrap_or_default(),
        );
    }
    if let Some(id) = operation_id {
        println!("{} {}", color.label("operation_id:"), color.id(id));
    }
    Ok(())
}

/// RFC3339 UTC timestamp, seconds precision (matches the install path).
pub(crate) fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Execute CLI self-update: fetch release manifest, compare versions,
/// download and atomically replace the running binary.
///
/// Also called from `anolisa self update` as a convenience alias.
///
/// # Errors
///
/// Returns [`CliError::Runtime`] when the manifest fetch, version check,
/// download, or binary replacement fails.
pub(in crate::commands) fn handle_self_update(ctx: &CliContext) -> Result<(), CliError> {
    let url = self_update::update_url();
    let current_version = env!("CARGO_PKG_VERSION");
    let ops = SystemSelfUpdateOps;
    let query = RpmPackageQuery::system();
    let txn = RpmTransaction::system();

    let progress_cb: Option<ProgressFn> = if !ctx.json && !ctx.quiet {
        Some(Box::new(move |downloaded: u64, total: Option<u64>| {
            render_progress(downloaded, total);
        }))
    } else {
        None
    };

    let result = run_self_update_with_deps(
        &url,
        current_version,
        ctx,
        &ops,
        &query,
        &txn,
        privilege::is_root(),
        progress_cb.as_ref(),
    );

    // Clear the progress line before any output (success or error).
    if progress_cb.is_some() {
        eprint!("\r\x1b[2K");
    }

    let run = result?;

    if ctx.json {
        return render_json_outcome(&run, ctx.dry_run);
    }

    if ctx.quiet {
        return Ok(());
    }

    let color = Palette::new(ctx.no_color);
    match &run.manifest_outcome {
        SelfUpdateOutcome::AlreadyLatest { version } => {
            println!(
                "{} anolisa {} is already the latest version",
                color.ok("✓"),
                version
            );
        }
        SelfUpdateOutcome::UpdateAvailable { from, to } if ctx.dry_run => {
            println!("{} update available: {} → {}", color.warn("⬆"), from, to);
            println!("  run without --dry-run to apply");
        }
        SelfUpdateOutcome::UpdateAvailable { from, to } => match &run.apply_mode {
            SelfUpdateApplyMode::Binary => {
                println!("{} anolisa updated: {} → {}", color.ok("✓"), from, to);
                println!("  view the changelog at {}", color.path(CLI_CHANGELOG_URL));
                eprintln!(
                    "  {} signature verification not yet implemented; \
                     update trust relies on HTTPS only",
                    color.warn("⚠")
                );
            }
            SelfUpdateApplyMode::RpmPackage {
                package,
                before_version,
                after_version,
            } => {
                println!(
                    "{} delegated anolisa self-update to dnf package {}",
                    color.ok("✓"),
                    color.path(package)
                );
                println!("  release manifest advertises {to} (running binary was {from})");
                render_rpm_version_observation(before_version.as_deref(), after_version.as_deref());
            }
            SelfUpdateApplyMode::None => {}
        },
    }

    Ok(())
}

#[derive(Debug)]
struct SelfUpdateRun {
    manifest_outcome: SelfUpdateOutcome,
    apply_mode: SelfUpdateApplyMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SelfUpdateApplyMode {
    None,
    Binary,
    RpmPackage {
        package: String,
        before_version: Option<String>,
        after_version: Option<String>,
    },
}

/// Host operations used by `update self`, split out so tests can prove the RPM
/// branch delegates to the package manager instead of overwriting the binary.
trait SelfUpdateOps {
    fn check_update(
        &self,
        endpoint_url: &str,
        current_version: &str,
    ) -> Result<Option<self_update::ReleaseManifest>, self_update::SelfUpdateError>;

    fn resolve_current_exe(&self) -> Result<PathBuf, self_update::SelfUpdateError>;

    fn perform_binary_update(
        &self,
        artifact: &self_update::ReleaseArtifact,
        current_exe: &Path,
        on_progress: Option<&ProgressFn>,
    ) -> Result<(), self_update::SelfUpdateError>;
}

struct SystemSelfUpdateOps;

impl SelfUpdateOps for SystemSelfUpdateOps {
    fn check_update(
        &self,
        endpoint_url: &str,
        current_version: &str,
    ) -> Result<Option<self_update::ReleaseManifest>, self_update::SelfUpdateError> {
        self_update::check_update(endpoint_url, current_version)
    }

    fn resolve_current_exe(&self) -> Result<PathBuf, self_update::SelfUpdateError> {
        self_update::resolve_current_exe()
    }

    fn perform_binary_update(
        &self,
        artifact: &self_update::ReleaseArtifact,
        current_exe: &Path,
        on_progress: Option<&ProgressFn>,
    ) -> Result<(), self_update::SelfUpdateError> {
        self_update::perform_update(artifact, current_exe, on_progress)
    }
}

/// Check for a CLI self-update and apply it through the correct owner.
///
/// Raw/binary installs keep the existing verified archive replacement. If the
/// running executable is owned by an installed RPM, the RPM database owns that
/// path, so the update is delegated to `dnf update <package>` instead.
#[allow(clippy::too_many_arguments)]
fn run_self_update_with_deps(
    endpoint_url: &str,
    current_version: &str,
    ctx: &CliContext,
    ops: &dyn SelfUpdateOps,
    query: &dyn PackageQuery,
    txn: &dyn PackageTransaction,
    is_root: bool,
    on_progress: Option<&ProgressFn>,
) -> Result<SelfUpdateRun, CliError> {
    let manifest = match ops
        .check_update(endpoint_url, current_version)
        .map_err(self_update_cli_err)?
    {
        None => {
            return Ok(SelfUpdateRun {
                manifest_outcome: SelfUpdateOutcome::AlreadyLatest {
                    version: current_version.to_string(),
                },
                apply_mode: SelfUpdateApplyMode::None,
            });
        }
        Some(manifest) => manifest,
    };

    let os = self_update::current_os();
    let arch = self_update::current_arch();
    let artifact = manifest
        .artifact_for(os, arch)
        .ok_or_else(|| self_update::SelfUpdateError::NoArtifact {
            os: os.to_string(),
            arch: arch.to_string(),
        })
        .map_err(self_update_cli_err)?;

    if ctx.dry_run {
        return Ok(SelfUpdateRun {
            manifest_outcome: SelfUpdateOutcome::UpdateAvailable {
                from: current_version.to_string(),
                to: manifest.version,
            },
            apply_mode: SelfUpdateApplyMode::None,
        });
    }

    let current_exe = ops.resolve_current_exe().map_err(self_update_cli_err)?;
    let apply_mode = if let Some(package) = rpm_owner_for_current_exe(query, &current_exe)? {
        if !is_root {
            return Err(CliError::Runtime {
                command: "update self".to_string(),
                reason: format!(
                    "updating RPM-owned anolisa package '{package}' requires root privileges; re-run with sudo: `sudo anolisa update self`"
                ),
            });
        }
        let before_version = installed_package_version_best_effort(query, &package);
        txn.update(&[package.as_str()])
            .map_err(|err| txn_err(err, "update self"))?;
        let after_version = installed_package_version_best_effort(query, &package);
        SelfUpdateApplyMode::RpmPackage {
            package,
            before_version,
            after_version,
        }
    } else {
        ops.perform_binary_update(artifact, &current_exe, on_progress)
            .map_err(self_update_cli_err)?;
        SelfUpdateApplyMode::Binary
    };

    Ok(SelfUpdateRun {
        manifest_outcome: SelfUpdateOutcome::UpdateAvailable {
            from: current_version.to_string(),
            to: manifest.version,
        },
        apply_mode,
    })
}

/// Map a [`PackageTransactionError`] onto a CLI runtime error with an
/// actionable hint (self-update's dnf delegation).
fn txn_err(err: PackageTransactionError, command: &str) -> CliError {
    match err {
        PackageTransactionError::CommandMissing { .. } => CliError::Runtime {
            command: command.to_string(),
            reason: "rpm/dnf not found: cannot update an RPM-owned package without the package manager. Install rpm/dnf and retry".to_string(),
        },
        PackageTransactionError::PermissionDenied { command: bin } => {
            common::package_permission_error(command, &bin, "update")
        }
        PackageTransactionError::TransactionFailed { code, stderr, .. } => {
            common::package_transaction_failed_error(command, "update", code, &stderr)
        }
    }
}

fn render_rpm_version_observation(before_version: Option<&str>, after_version: Option<&str>) {
    match (before_version, after_version) {
        (Some(before), Some(after)) if before != after => {
            println!("  installed RPM version changed: {before} → {after}");
        }
        (Some(version), Some(_)) => {
            println!("  installed RPM version remains {version}");
        }
        (Some(before), None) => {
            println!(
                "  installed RPM version before dnf was {before}; after dnf was not confirmed"
            );
        }
        (None, Some(after)) => {
            println!("  installed RPM version after dnf: {after}");
        }
        (None, None) => {
            println!("  installed RPM version was not confirmed after dnf");
        }
    }
}

fn installed_package_version_best_effort(
    query: &dyn PackageQuery,
    package: &str,
) -> Option<String> {
    query
        .query_installed(package)
        .ok()
        .flatten()
        .map(|info| info.version.to_string())
}

fn self_update_cli_err(err: self_update::SelfUpdateError) -> CliError {
    CliError::Runtime {
        command: "update self".to_string(),
        reason: err.to_string(),
    }
}

fn rpm_owner_for_current_exe(
    query: &dyn PackageQuery,
    current_exe: &Path,
) -> Result<Option<String>, CliError> {
    let capability = current_exe.to_str().ok_or_else(|| CliError::Runtime {
        command: "update self".to_string(),
        reason: format!(
            "current executable path is not valid UTF-8: {}",
            current_exe.display()
        ),
    })?;

    match query.what_provides_installed(capability) {
        Ok(packages) => match packages.as_slice() {
            [] => Ok(None),
            [package] => Ok(Some(package.clone())),
            _ => Err(CliError::Runtime {
                command: "update self".to_string(),
                reason: format!(
                    "current executable '{}' is provided by multiple RPM packages ({}); refusing to choose one for self-update",
                    current_exe.display(),
                    packages.join(", ")
                ),
            }),
        },
        Err(PackageQueryError::CommandMissing { .. }) => Ok(None),
        Err(err) => Err(CliError::Runtime {
            command: "update self".to_string(),
            reason: format!(
                "cannot determine whether current executable '{}' is RPM-owned: {err}",
                current_exe.display()
            ),
        }),
    }
}

fn render_progress(downloaded: u64, total: Option<u64>) {
    match total {
        Some(t) if t > 0 => {
            let pct = (downloaded as f64 / t as f64 * 100.0).min(100.0);
            eprint!(
                "\r  downloading ... {:.1} / {:.1} MiB ({:.0}%)",
                downloaded as f64 / 1_048_576.0,
                t as f64 / 1_048_576.0,
                pct,
            );
        }
        _ => {
            eprint!(
                "\r  downloading ... {:.1} MiB",
                downloaded as f64 / 1_048_576.0,
            );
        }
    }
}

#[derive(Serialize)]
struct SelfUpdateData {
    current_version: String,
    latest_version: String,
    update_available: bool,
    updated: bool,
    apply_mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    package: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rpm_version_before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rpm_version_after: Option<String>,
}

fn build_json_data(run: &SelfUpdateRun, dry_run: bool) -> SelfUpdateData {
    let (current_version, latest_version, update_available) = match &run.manifest_outcome {
        SelfUpdateOutcome::AlreadyLatest { version } => (version.clone(), version.clone(), false),
        SelfUpdateOutcome::UpdateAvailable { from, to } => (from.clone(), to.clone(), true),
    };
    let (apply_mode, package, rpm_version_before, rpm_version_after) = match &run.apply_mode {
        SelfUpdateApplyMode::None => ("none", None, None, None),
        SelfUpdateApplyMode::Binary => ("binary", None, None, None),
        SelfUpdateApplyMode::RpmPackage {
            package,
            before_version,
            after_version,
        } => (
            "rpm_package",
            Some(package.clone()),
            before_version.clone(),
            after_version.clone(),
        ),
    };
    let updated = match &run.apply_mode {
        SelfUpdateApplyMode::Binary => update_available && !dry_run,
        SelfUpdateApplyMode::RpmPackage {
            before_version,
            after_version,
            ..
        } => {
            update_available
                && !dry_run
                && before_version
                    .as_ref()
                    .zip(after_version.as_ref())
                    .is_some_and(|(before, after)| before != after)
        }
        SelfUpdateApplyMode::None => false,
    };

    SelfUpdateData {
        current_version,
        latest_version,
        update_available,
        updated,
        apply_mode,
        package,
        rpm_version_before,
        rpm_version_after,
    }
}

fn render_json_outcome(run: &SelfUpdateRun, dry_run: bool) -> Result<(), CliError> {
    response::render_json("update self", build_json_data(run, dry_run))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};

    use anolisa_platform::pkg_query::PackageVersion;

    #[test]
    fn json_dry_run_reports_available_but_not_updated() {
        let outcome = SelfUpdateOutcome::UpdateAvailable {
            from: "0.1.0".into(),
            to: "0.2.0".into(),
        };
        let run = self_run(outcome, SelfUpdateApplyMode::None);
        let data = build_json_data(&run, true);
        assert!(data.update_available);
        assert!(!data.updated);
        assert_eq!(data.apply_mode, "none");
    }

    #[test]
    fn json_binary_update_reports_both_true() {
        let outcome = SelfUpdateOutcome::UpdateAvailable {
            from: "0.1.0".into(),
            to: "0.2.0".into(),
        };
        let run = self_run(outcome, SelfUpdateApplyMode::Binary);
        let data = build_json_data(&run, false);
        assert!(data.update_available);
        assert!(data.updated);
        assert_eq!(data.apply_mode, "binary");
        assert_eq!(data.package, None);
    }

    #[test]
    fn json_rpm_delegation_reports_package_and_version_change() {
        let outcome = SelfUpdateOutcome::UpdateAvailable {
            from: "0.1.0".into(),
            to: "0.2.0".into(),
        };
        let run = self_run(
            outcome,
            SelfUpdateApplyMode::RpmPackage {
                package: "anolisa".to_string(),
                before_version: Some("0.1.0".to_string()),
                after_version: Some("0.2.0".to_string()),
            },
        );

        let data = build_json_data(&run, false);

        assert!(data.update_available);
        assert!(data.updated);
        assert_eq!(data.apply_mode, "rpm_package");
        assert_eq!(data.package.as_deref(), Some("anolisa"));
        assert_eq!(data.rpm_version_before.as_deref(), Some("0.1.0"));
        assert_eq!(data.rpm_version_after.as_deref(), Some("0.2.0"));
    }

    #[test]
    fn json_rpm_delegation_noops_are_not_reported_as_updated() {
        let outcome = SelfUpdateOutcome::UpdateAvailable {
            from: "0.1.0".into(),
            to: "0.2.0".into(),
        };
        let run = self_run(
            outcome,
            SelfUpdateApplyMode::RpmPackage {
                package: "anolisa".to_string(),
                before_version: Some("0.1.0".to_string()),
                after_version: Some("0.1.0".to_string()),
            },
        );

        let data = build_json_data(&run, false);

        assert!(data.update_available);
        assert!(!data.updated);
        assert_eq!(data.apply_mode, "rpm_package");
        assert_eq!(data.package.as_deref(), Some("anolisa"));
    }

    #[test]
    fn json_already_latest_reports_both_false() {
        let outcome = SelfUpdateOutcome::AlreadyLatest {
            version: "0.1.0".into(),
        };
        let run = self_run(outcome, SelfUpdateApplyMode::None);
        let data = build_json_data(&run, false);
        assert!(!data.update_available);
        assert!(!data.updated);
        assert_eq!(data.apply_mode, "none");
    }

    fn self_manifest(version: &str) -> self_update::ReleaseManifest {
        self_update::ReleaseManifest {
            schema_version: 1,
            version: version.to_string(),
            artifacts: vec![self_update::ReleaseArtifact {
                os: self_update::current_os().to_string(),
                arch: self_update::current_arch().to_string(),
                url: "https://example.invalid/anolisa.tar.gz".to_string(),
                sha256: "0".repeat(64),
                size: Some(1),
            }],
        }
    }

    fn self_run(outcome: SelfUpdateOutcome, apply_mode: SelfUpdateApplyMode) -> SelfUpdateRun {
        SelfUpdateRun {
            manifest_outcome: outcome,
            apply_mode,
        }
    }

    fn package_info(name: &str, version: &str) -> PackageInfo {
        PackageInfo {
            name: name.to_string(),
            version: PackageVersion {
                epoch: None,
                version: version.to_string(),
                release: None,
            },
            arch: "x86_64".to_string(),
            origin: None,
        }
    }

    fn self_ctx(prefix: PathBuf, dry_run: bool) -> CliContext {
        CliContext {
            install_mode: crate::context::InstallMode::System,
            prefix: Some(prefix),
            json: false,
            dry_run,
            verbose: false,
            quiet: true,
            no_color: true,
        }
    }

    struct FakeSelfUpdateOps {
        manifest: Option<self_update::ReleaseManifest>,
        current_exe: PathBuf,
        binary_updates: Cell<usize>,
    }

    impl FakeSelfUpdateOps {
        fn new(current_exe: &str) -> Self {
            Self {
                manifest: Some(self_manifest("0.2.0")),
                current_exe: PathBuf::from(current_exe),
                binary_updates: Cell::new(0),
            }
        }
    }

    impl SelfUpdateOps for FakeSelfUpdateOps {
        fn check_update(
            &self,
            _endpoint_url: &str,
            _current_version: &str,
        ) -> Result<Option<self_update::ReleaseManifest>, self_update::SelfUpdateError> {
            Ok(self.manifest.clone())
        }

        fn resolve_current_exe(&self) -> Result<PathBuf, self_update::SelfUpdateError> {
            Ok(self.current_exe.clone())
        }

        fn perform_binary_update(
            &self,
            _artifact: &self_update::ReleaseArtifact,
            current_exe: &Path,
            _on_progress: Option<&ProgressFn>,
        ) -> Result<(), self_update::SelfUpdateError> {
            assert_eq!(current_exe, self.current_exe.as_path());
            self.binary_updates.set(self.binary_updates.get() + 1);
            Ok(())
        }
    }

    struct FakeSelfQuery {
        expected_capability: String,
        providers: FakeSelfProviders,
        queries: Cell<usize>,
        expected_installed_package: Option<String>,
        installed_versions: RefCell<VecDeque<Option<String>>>,
        installed_queries: Cell<usize>,
    }

    enum FakeSelfProviders {
        Packages(Vec<String>),
        CommandMissing,
    }

    impl FakeSelfQuery {
        fn new(expected_capability: &str, providers: Vec<&str>) -> Self {
            Self {
                expected_capability: expected_capability.to_string(),
                providers: FakeSelfProviders::Packages(
                    providers.into_iter().map(str::to_string).collect(),
                ),
                queries: Cell::new(0),
                expected_installed_package: None,
                installed_versions: RefCell::new(VecDeque::new()),
                installed_queries: Cell::new(0),
            }
        }

        fn missing_rpm(expected_capability: &str) -> Self {
            Self {
                expected_capability: expected_capability.to_string(),
                providers: FakeSelfProviders::CommandMissing,
                queries: Cell::new(0),
                expected_installed_package: None,
                installed_versions: RefCell::new(VecDeque::new()),
                installed_queries: Cell::new(0),
            }
        }

        fn with_installed_versions(
            mut self,
            expected_package: &str,
            versions: Vec<Option<&str>>,
        ) -> Self {
            self.expected_installed_package = Some(expected_package.to_string());
            self.installed_versions = RefCell::new(
                versions
                    .into_iter()
                    .map(|version| version.map(str::to_string))
                    .collect(),
            );
            self
        }
    }

    impl PackageQuery for FakeSelfQuery {
        fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
            self.installed_queries.set(self.installed_queries.get() + 1);
            if let Some(expected_package) = &self.expected_installed_package {
                assert_eq!(package, expected_package);
            }
            Ok(self
                .installed_versions
                .borrow_mut()
                .pop_front()
                .flatten()
                .map(|version| package_info(package, &version)))
        }

        fn query_available(&self, _package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
            Ok(Vec::new())
        }

        fn what_provides_installed(
            &self,
            capability: &str,
        ) -> Result<Vec<String>, PackageQueryError> {
            self.queries.set(self.queries.get() + 1);
            assert_eq!(capability, self.expected_capability);
            match &self.providers {
                FakeSelfProviders::Packages(providers) => Ok(providers.clone()),
                FakeSelfProviders::CommandMissing => Err(PackageQueryError::CommandMissing {
                    command: "rpm".to_string(),
                }),
            }
        }
    }

    struct FakeSelfTxn {
        expected_package: String,
        update_calls: Cell<usize>,
    }

    impl FakeSelfTxn {
        fn new(expected_package: &str) -> Self {
            Self {
                expected_package: expected_package.to_string(),
                update_calls: Cell::new(0),
            }
        }
    }

    impl PackageTransaction for FakeSelfTxn {
        fn install(&self, _packages: &[&str]) -> Result<(), PackageTransactionError> {
            panic!("self-update must not run dnf install");
        }

        fn update(&self, packages: &[&str]) -> Result<(), PackageTransactionError> {
            let &[package] = packages else {
                panic!("expected exactly one package, got {packages:?}");
            };
            self.update_calls.set(self.update_calls.get() + 1);
            assert_eq!(package, self.expected_package);
            Ok(())
        }

        fn reinstall(&self, _packages: &[&str]) -> Result<(), PackageTransactionError> {
            panic!("self-update must not run dnf reinstall");
        }

        fn remove(&self, _packages: &[&str]) -> Result<(), PackageTransactionError> {
            panic!("self-update must not run dnf remove");
        }
    }

    #[test]
    fn rpm_owned_self_update_delegates_to_dnf_without_binary_swap() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = self_ctx(tmp.path().to_path_buf(), false);
        let ops = FakeSelfUpdateOps::new("/usr/bin/anolisa");
        let query = FakeSelfQuery::new("/usr/bin/anolisa", vec!["anolisa"])
            .with_installed_versions("anolisa", vec![Some("0.1.0"), Some("0.2.0")]);
        let txn = FakeSelfTxn::new("anolisa");

        let run = run_self_update_with_deps(
            "https://example.invalid/release-manifest.toml",
            "0.1.0",
            &c,
            &ops,
            &query,
            &txn,
            true,
            None,
        )
        .expect("rpm-owned self update should succeed through dnf");

        assert!(matches!(
            run.manifest_outcome,
            SelfUpdateOutcome::UpdateAvailable { from, to }
                if from == "0.1.0" && to == "0.2.0"
        ));
        assert_eq!(
            run.apply_mode,
            SelfUpdateApplyMode::RpmPackage {
                package: "anolisa".to_string(),
                before_version: Some("0.1.0".to_string()),
                after_version: Some("0.2.0".to_string())
            }
        );
        assert_eq!(query.queries.get(), 1, "rpm ownership must be probed");
        assert_eq!(
            query.installed_queries.get(),
            2,
            "rpm package version must be checked before and after dnf"
        );
        assert_eq!(txn.update_calls.get(), 1, "dnf update must run once");
        assert_eq!(
            ops.binary_updates.get(),
            0,
            "RPM-owned executable must not be overwritten directly"
        );
    }

    #[test]
    fn rpm_owned_self_update_dnf_noop_is_not_reported_as_updated() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = self_ctx(tmp.path().to_path_buf(), false);
        let ops = FakeSelfUpdateOps::new("/usr/bin/anolisa");
        let query = FakeSelfQuery::new("/usr/bin/anolisa", vec!["anolisa"])
            .with_installed_versions("anolisa", vec![Some("0.1.0"), Some("0.1.0")]);
        let txn = FakeSelfTxn::new("anolisa");

        let run = run_self_update_with_deps(
            "https://example.invalid/release-manifest.toml",
            "0.1.0",
            &c,
            &ops,
            &query,
            &txn,
            true,
            None,
        )
        .expect("dnf no-op is still a successful delegation");

        let data = build_json_data(&run, false);

        assert!(data.update_available);
        assert!(!data.updated);
        assert_eq!(data.apply_mode, "rpm_package");
        assert_eq!(data.package.as_deref(), Some("anolisa"));
    }

    #[test]
    fn non_rpm_self_update_keeps_binary_replacement_path() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = self_ctx(tmp.path().to_path_buf(), false);
        let ops = FakeSelfUpdateOps::new("/opt/anolisa/bin/anolisa");
        let query = FakeSelfQuery::new("/opt/anolisa/bin/anolisa", Vec::new());
        let txn = FakeSelfTxn::new("anolisa");

        let run = run_self_update_with_deps(
            "https://example.invalid/release-manifest.toml",
            "0.1.0",
            &c,
            &ops,
            &query,
            &txn,
            false,
            None,
        )
        .expect("non-rpm self update should use binary replacement");

        assert_eq!(run.apply_mode, SelfUpdateApplyMode::Binary);
        assert_eq!(query.queries.get(), 1);
        assert_eq!(txn.update_calls.get(), 0, "dnf must not run");
        assert_eq!(ops.binary_updates.get(), 1, "binary replacement must run");
    }

    #[test]
    fn missing_rpm_tooling_keeps_binary_replacement_path() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = self_ctx(tmp.path().to_path_buf(), false);
        let ops = FakeSelfUpdateOps::new("/opt/anolisa/bin/anolisa");
        let query = FakeSelfQuery::missing_rpm("/opt/anolisa/bin/anolisa");
        let txn = FakeSelfTxn::new("anolisa");

        let run = run_self_update_with_deps(
            "https://example.invalid/release-manifest.toml",
            "0.1.0",
            &c,
            &ops,
            &query,
            &txn,
            false,
            None,
        )
        .expect("missing rpm must not block raw self-update");

        assert_eq!(run.apply_mode, SelfUpdateApplyMode::Binary);
        assert_eq!(query.queries.get(), 1);
        assert_eq!(txn.update_calls.get(), 0, "dnf must not run");
        assert_eq!(ops.binary_updates.get(), 1, "binary replacement must run");
    }

    #[test]
    fn rpm_owned_self_update_requires_root_before_dnf() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = self_ctx(tmp.path().to_path_buf(), false);
        let ops = FakeSelfUpdateOps::new("/usr/bin/anolisa");
        let query = FakeSelfQuery::new("/usr/bin/anolisa", vec!["anolisa"]);
        let txn = FakeSelfTxn::new("anolisa");

        let err = run_self_update_with_deps(
            "https://example.invalid/release-manifest.toml",
            "0.1.0",
            &c,
            &ops,
            &query,
            &txn,
            false,
            None,
        )
        .expect_err("rpm-owned self update needs root");

        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("root") && err.reason().contains("sudo"),
            "reason must point at sudo: {}",
            err.reason()
        );
        assert_eq!(txn.update_calls.get(), 0, "dnf must not run");
        assert_eq!(
            ops.binary_updates.get(),
            0,
            "binary replacement must not run"
        );
    }

    // ── component update: delegated (RPM-backed) records ──

    use anolisa_core::domain::LifecycleStatus;
    use anolisa_core::state::{
        FileOwner, InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectStatus,
        OwnedFile, OwnedFileKind, Ownership, RpmMetadata, ServiceRef,
    };
    use anolisa_platform::pkg_query::PackageInfo;

    use crate::context::InstallMode;

    /// In-memory rpm world implementing **both** [`PackageQuery`] and
    /// [`PackageTransaction`], so one fake drives the whole update flow.
    ///
    /// `installed` mutates: a successful [`update`](PackageTransaction::update)
    /// applies `upgrade_to`, modelling rpmdb advancing after dnf runs — so the
    /// pre-update query and the post-update refresh return different EVRs.
    struct FakeRpm {
        package: String,
        installed: RefCell<Option<PackageInfo>>,
        /// PackageInfo the rpmdb holds after a successful update; `None` keeps
        /// the same version (a no-op "already latest").
        upgrade_to: Option<PackageInfo>,
        /// `false` makes the dnf transaction fail.
        update_succeeds: bool,
        /// `true` makes `query_installed` report a same-name multi-version drift.
        multi_version: bool,
        /// `true` makes the *post-update* `query_installed` report the package
        /// gone, modelling a failed rpmdb re-read after a successful dnf update.
        post_update_missing: bool,
        update_calls: Cell<usize>,
    }

    impl FakeRpm {
        fn new(package: &str, installed: Option<PackageInfo>) -> Self {
            Self {
                package: package.to_string(),
                installed: RefCell::new(installed),
                upgrade_to: None,
                update_succeeds: true,
                multi_version: false,
                post_update_missing: false,
                update_calls: Cell::new(0),
            }
        }
        fn upgrading_to(mut self, info: PackageInfo) -> Self {
            self.upgrade_to = Some(info);
            self
        }
        fn failing_update(mut self) -> Self {
            self.update_succeeds = false;
            self
        }
        fn multi_version(mut self) -> Self {
            self.multi_version = true;
            self
        }
        fn post_update_missing(mut self) -> Self {
            self.post_update_missing = true;
            self
        }
    }

    impl PackageQuery for FakeRpm {
        fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
            if package != self.package {
                return Ok(None);
            }
            // Simulate a failed post-update re-read: the package "vanishes" only
            // after dnf update has run, so the pre-update query still succeeds.
            if self.post_update_missing && self.update_calls.get() > 0 {
                return Ok(None);
            }
            if self.multi_version {
                return Err(PackageQueryError::UnexpectedOutput {
                    command: "rpm".to_string(),
                    detail: "2 installed versions".to_string(),
                });
            }
            Ok(self.installed.borrow().clone())
        }

        fn query_available(&self, package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
            let _ = package;
            Ok(Vec::new())
        }
    }

    impl PackageTransaction for FakeRpm {
        fn install(&self, _packages: &[&str]) -> Result<(), PackageTransactionError> {
            // The update flow never installs; a call here is a routing bug.
            panic!("update path must not delegate a dnf install");
        }

        fn update(&self, packages: &[&str]) -> Result<(), PackageTransactionError> {
            let &[package] = packages else {
                panic!("expected exactly one package, got {packages:?}");
            };
            self.update_calls.set(self.update_calls.get() + 1);
            assert_eq!(package, self.package, "update targeted the wrong package");
            if !self.update_succeeds {
                return Err(PackageTransactionError::TransactionFailed {
                    command: "dnf".to_string(),
                    operation: "update".to_string(),
                    code: Some(1),
                    stderr: "repo unreachable".to_string(),
                });
            }
            if let Some(next) = &self.upgrade_to {
                *self.installed.borrow_mut() = Some(next.clone());
            }
            Ok(())
        }

        fn reinstall(&self, _packages: &[&str]) -> Result<(), PackageTransactionError> {
            // The update flow never reinstalls; a call here is a routing bug.
            panic!("update path must not delegate a dnf reinstall");
        }

        fn remove(&self, _packages: &[&str]) -> Result<(), PackageTransactionError> {
            // The update flow never removes; a call here is a routing bug.
            panic!("update path must not delegate a dnf remove");
        }
    }

    pub(crate) fn pkg_info(
        name: &str,
        version: &str,
        release: Option<&str>,
        arch: &str,
    ) -> PackageInfo {
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

    pub(crate) fn ctx(prefix: PathBuf, install_mode: InstallMode, dry_run: bool) -> CliContext {
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

    /// Build a legacy (v4) RPM-backed component object; the store migrates it
    /// on load, so these tests double as migration coverage.
    pub(crate) fn rpm_object(
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

    /// A v4 rpm-observed object that was never adopted (`adopted = false`),
    /// migrating to the Observed relation.
    fn rpm_observed_unadopted(component: &str, package: &str, evr: &str) -> InstalledObject {
        let mut obj = rpm_object(
            component,
            package,
            evr,
            Ownership::RpmObserved,
            ObjectStatus::Installed,
        );
        obj.adopted = false;
        obj
    }

    /// A legacy (v4) raw-managed component object (no rpm metadata).
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

    /// Seed `installed.toml` for `ctx`'s scope with one v4 object.
    pub(crate) fn seed(ctx: &CliContext, obj: InstalledObject) {
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

    pub(crate) fn write_repo_toml(ctx: &CliContext, body: &str) {
        let layout = common::resolve_layout(ctx);
        std::fs::create_dir_all(&layout.etc_dir).expect("mkdir etc");
        std::fs::write(layout.etc_dir.join("repo.toml"), body).expect("write repo.toml");
    }

    /// Load the (migrated) v5 store.
    pub(crate) fn load_store(ctx: &CliContext) -> StateStore {
        let layout = common::resolve_layout(ctx);
        StateStore::load(&layout.state_dir.join("installed.toml"), 0).expect("load state")
    }

    /// Find `name`'s component record in the (migrated) v5 store.
    fn find_component(ctx: &CliContext, name: &str) -> anolisa_core::domain::Installation {
        load_store(ctx)
            .find(ObjectKind::Component, name)
            .cloned()
            .expect("component record present")
    }

    /// The delegated binding's cached observation EVR, for version assertions.
    fn observed_evr(record: &anolisa_core::domain::Installation) -> Option<String> {
        match &record.binding {
            ProviderBinding::Delegated { last_observed, .. } => last_observed
                .as_ref()
                .map(|o| o.evr.clone().unwrap_or_else(|| o.version.clone())),
            _ => None,
        }
    }

    /// The owned binding's artifact, for raw-path assertions.
    fn owned_artifact(record: &anolisa_core::domain::Installation) -> OwnedArtifact {
        match &record.binding {
            ProviderBinding::Owned { artifact } => artifact.clone(),
            other => panic!("expected an owned record, got {other:?}"),
        }
    }

    /// Adopted RPM component, root, real run: dnf update runs, the cached
    /// observation is refreshed from rpmdb, and the delegated relation is
    /// preserved (update never changes authority).
    #[test]
    fn rpm_observed_update_refreshes_evr_and_keeps_ownership() {
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
        let rpm = FakeRpm::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.2.0", Some("1.al8"), "x86_64")),
        )
        .upgrading_to(pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"));

        update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true).expect("update ok");
        assert_eq!(rpm.update_calls.get(), 1, "dnf update must run once");

        let record = find_component(&c, "copilot-shell");
        assert_eq!(
            observed_evr(&record).as_deref(),
            Some("2.3.0-1.al8"),
            "observation refreshed from rpmdb"
        );
        assert!(
            matches!(
                &record.binding,
                ProviderBinding::Delegated {
                    relation: ManagementRelation::Adopted { .. },
                    ..
                }
            ),
            "the management relation must be preserved: {:?}",
            record.binding
        );
        assert_eq!(record.status, LifecycleStatus::Installed);
        assert_ne!(record.last_operation_id.as_deref(), Some("op-prior"));
    }

    /// rpm-managed component updates the same way (different relation).
    #[test]
    fn rpm_managed_update_refreshes_evr() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "copilot-shell",
                "1.0.0-1.al8",
                Ownership::RpmManaged,
                ObjectStatus::Installed,
            ),
        );
        let rpm = FakeRpm::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "1.0.0", Some("1.al8"), "x86_64")),
        )
        .upgrading_to(pkg_info("copilot-shell", "1.1.0", Some("1.al8"), "x86_64"));

        update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true).expect("update ok");

        let record = find_component(&c, "copilot-shell");
        assert_eq!(observed_evr(&record).as_deref(), Some("1.1.0-1.al8"));
        assert!(matches!(
            &record.binding,
            ProviderBinding::Delegated {
                relation: ManagementRelation::Managed { .. },
                ..
            }
        ));
        assert_eq!(record.status, LifecycleStatus::Installed);
    }

    /// Publish a package-owned contract in the FHS package datadir plus the
    /// stale pre-update snapshot an earlier install left behind, as an RPM
    /// upgrade of the package would leave them.
    pub(crate) fn seed_package_contract_and_stale_snapshot(
        c: &CliContext,
        component: &str,
    ) -> (PathBuf, PathBuf) {
        let layout = common::resolve_layout(c);
        let package_datadir = layout.package_datadir().expect("package datadir");
        let source = FsLayout::component_contract_path(&package_datadir, component);
        std::fs::create_dir_all(source.parent().expect("source parent")).expect("mkdir source");
        std::fs::write(&source, "framework = \"new\"\n").expect("write package contract");
        let snapshot = FsLayout::component_manifest_snapshot_path(&layout.state_dir, component);
        std::fs::create_dir_all(snapshot.parent().expect("snapshot parent"))
            .expect("mkdir snapshot");
        std::fs::write(&snapshot, "framework = \"old\"\n").expect("write stale snapshot");
        (source, snapshot)
    }

    /// The durable status of the single recorded `update <component>`
    /// operation.
    fn update_operation_status(c: &CliContext, target: &str) -> String {
        let command = format!("update {target}");
        let store = load_store(c);
        let ops: Vec<_> = store
            .operations
            .iter()
            .filter(|op| op.command == command)
            .collect();
        assert_eq!(ops.len(), 1, "exactly one update operation recorded");
        ops[0].status.clone()
    }

    /// A committed delegated update refreshes the contract snapshot and its
    /// provenance directly — `status`/`doctor` read the new contract without
    /// waiting for a later repair.
    #[test]
    fn rpm_update_refreshes_contract_snapshot_and_provenance() {
        use anolisa_core::adapter::contract::{ContractSourceKind, read_snapshot_provenance};

        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "copilot-shell",
                "1.0.0-1.al8",
                Ownership::RpmManaged,
                ObjectStatus::Installed,
            ),
        );
        let (source, snapshot) = seed_package_contract_and_stale_snapshot(&c, "copilot-shell");
        let rpm = FakeRpm::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "1.0.0", Some("1.al8"), "x86_64")),
        )
        .upgrading_to(pkg_info("copilot-shell", "1.1.0", Some("1.al8"), "x86_64"));

        update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true).expect("update ok");

        assert_eq!(
            std::fs::read_to_string(&snapshot).expect("read snapshot"),
            "framework = \"new\"\n",
            "the snapshot must carry the post-update contract"
        );
        let provenance = read_snapshot_provenance(&snapshot).expect("snapshot provenance");
        assert_eq!(provenance.source_kind, ContractSourceKind::Datadir);
        assert_eq!(provenance.source_path, source);
        assert_eq!(update_operation_status(&c, "copilot-shell"), "ok");
    }

    /// A package that publishes no contract stays a clean success
    /// (`NotApplicable`): no snapshot write, no demotion.
    #[test]
    fn rpm_update_without_package_contract_stays_ok() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "copilot-shell",
                "1.0.0-1.al8",
                Ownership::RpmManaged,
                ObjectStatus::Installed,
            ),
        );
        // A stale snapshot without a package-owned contract: nothing
        // authoritative exists to refresh from, so it must stay untouched.
        let layout = common::resolve_layout(&c);
        let snapshot =
            FsLayout::component_manifest_snapshot_path(&layout.state_dir, "copilot-shell");
        std::fs::create_dir_all(snapshot.parent().expect("snapshot parent"))
            .expect("mkdir snapshot");
        std::fs::write(&snapshot, "framework = \"old\"\n").expect("write stale snapshot");
        let rpm = FakeRpm::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "1.0.0", Some("1.al8"), "x86_64")),
        )
        .upgrading_to(pkg_info("copilot-shell", "1.1.0", Some("1.al8"), "x86_64"));

        update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true).expect("update ok");

        assert_eq!(
            std::fs::read_to_string(&snapshot).expect("read snapshot"),
            "framework = \"old\"\n",
            "no package contract means no snapshot write"
        );
        assert_eq!(update_operation_status(&c, "copilot-shell"), "ok");
    }

    /// A contract refresh that cannot complete demotes the committed update
    /// to `partial` and reports it — the durable operation never overstates
    /// what happened, and the record still carries the refreshed EVR.
    #[test]
    fn rpm_update_contract_refresh_failure_is_partial() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "copilot-shell",
                "1.0.0-1.al8",
                Ownership::RpmManaged,
                ObjectStatus::Installed,
            ),
        );
        let layout = common::resolve_layout(&c);
        let package_datadir = layout.package_datadir().expect("package datadir");
        let source = FsLayout::component_contract_path(&package_datadir, "copilot-shell");
        std::fs::create_dir_all(source.parent().expect("source parent")).expect("mkdir source");
        std::fs::write(&source, "framework = \"new\"\n").expect("write package contract");
        // A non-empty directory where the snapshot file belongs blocks the
        // refresh's atomic replacement.
        let snapshot =
            FsLayout::component_manifest_snapshot_path(&layout.state_dir, "copilot-shell");
        std::fs::create_dir_all(&snapshot).expect("create blocking snapshot directory");
        std::fs::write(snapshot.join("keep"), b"x").expect("write blocking marker");
        let rpm = FakeRpm::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "1.0.0", Some("1.al8"), "x86_64")),
        )
        .upgrading_to(pkg_info("copilot-shell", "1.1.0", Some("1.al8"), "x86_64"));

        let err = update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true)
            .expect_err("a failed contract refresh must not report a clean success");

        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("committed") && err.reason().contains("repair"),
            "reason must state the update committed and point at repair: {}",
            err.reason()
        );
        assert_eq!(
            observed_evr(&find_component(&c, "copilot-shell")).as_deref(),
            Some("1.1.0-1.al8"),
            "the committed record refresh must be kept"
        );
        assert_eq!(update_operation_status(&c, "copilot-shell"), "partial");
    }

    /// U6 (breaking): an observed record carries no management consent, so
    /// update refuses and points at `adopt` — dnf never runs on a system RPM
    /// that was merely observed.
    #[test]
    fn observed_component_requires_adoption_first() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_observed_unadopted("copilot-shell", "copilot-shell", "2.2.0-1.al8"),
        );
        let rpm = FakeRpm::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.2.0", Some("1.al8"), "x86_64")),
        );

        let err = update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true)
            .expect_err("an observed record must refuse to update");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("adopt"),
            "reason must point at adopt: {}",
            err.reason()
        );
        assert_eq!(rpm.update_calls.get(), 0, "dnf must not run");
    }

    /// Non-root real run is refused with an actionable message; dnf never runs
    /// and state is untouched.
    #[test]
    fn non_root_update_is_refused_without_running_dnf() {
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
        let rpm = FakeRpm::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.2.0", Some("1.al8"), "x86_64")),
        );

        let err = update_component_with_deps("copilot-shell", &c, &rpm, &rpm, false)
            .expect_err("must refuse without root");
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("root") && err.reason().contains("sudo"),
            "reason must point at sudo: {}",
            err.reason()
        );
        assert_eq!(rpm.update_calls.get(), 0, "dnf must not run without root");
        assert_eq!(
            find_component(&c, "copilot-shell")
                .last_operation_id
                .as_deref(),
            Some("op-prior"),
            "state must be unchanged"
        );
    }

    /// Dry-run previews the plan without running dnf, needing root, or writing
    /// state or the contract snapshot — even for a non-root caller.
    #[test]
    fn dry_run_previews_without_dnf_or_state_write() {
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
        let (_, snapshot) = seed_package_contract_and_stale_snapshot(&c, "copilot-shell");
        let rpm = FakeRpm::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.2.0", Some("1.al8"), "x86_64")),
        );

        update_component_with_deps("copilot-shell", &c, &rpm, &rpm, false).expect("dry-run ok");
        assert_eq!(rpm.update_calls.get(), 0, "dry-run must not run dnf");
        assert_eq!(
            find_component(&c, "copilot-shell")
                .last_operation_id
                .as_deref(),
            Some("op-prior"),
            "dry-run must not write state"
        );
        assert_eq!(
            std::fs::read_to_string(&snapshot).expect("read snapshot"),
            "framework = \"old\"\n",
            "dry-run must not touch the snapshot"
        );
    }

    /// A component absent from state routes to INVALID_ARGUMENT (exit 2), not a
    /// runtime failure, and never runs dnf.
    #[test]
    fn unknown_component_routes_to_invalid_argument() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let rpm = FakeRpm::new("copilot-shell", None);
        let err = update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true)
            .expect_err("absent component must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert_eq!(err.exit_code(), 2);
        assert!(err.reason().contains("not installed"));
        assert_eq!(rpm.update_calls.get(), 0);
    }

    /// Regression: a bare `anolisa update` (no component, no subcommand) fails
    /// validation as INVALID_ARGUMENT and does not provision repo config.
    #[test]
    fn bare_update_errors_before_repo_config_provisioning() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let repo_toml = common::resolve_layout(&c).etc_dir.join("repo.toml");

        let err = handle(
            UpdateArgs {
                component: None,
                command: None,
                check: false,
                motd: false,
                refresh: false,
                target: None,
            },
            &c,
        )
        .expect_err("bare `update` must fail validation");

        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert_eq!(err.exit_code(), 2);
        assert!(
            !repo_toml.exists(),
            "no repo config must be written for an invalid invocation: {} exists",
            repo_toml.display()
        );
    }

    #[test]
    fn rpm_update_repo_source_uses_repo_toml_backend() {
        let repo = RepoConfig::from_toml_str(
            r#"schema_version = 1
default_backend = "rpm"

[backends.rpm]
base_url = "https://repo.example/$os/$arch/os"
gpgcheck = false
"#,
        )
        .expect("repo config");
        let env = anolisa_env::EnvService::detect_for("linux");

        let source = rpm_repo_source_for_update(&repo, &env, "update copilot-shell")
            .expect("repo source")
            .expect("rpm backend");

        assert_eq!(source.id(), ANOLISA_RPM_REPO_ID);
        assert_eq!(
            source.base_url(),
            format!("https://repo.example/linux/{}/os", env.arch)
        );
        assert_eq!(source.gpgcheck(), Some(false));
    }

    #[test]
    fn rpm_update_requires_configured_rpm_backend() {
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
        write_repo_toml(
            &c,
            r#"schema_version = 1
default_backend = "raw"

[backends.raw]
base_url = "https://repo.example/raw/v1"
"#,
        );

        let err = handle_component_update("copilot-shell", &c)
            .expect_err("rpm update must require rpm backend");

        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("[backends.rpm]"),
            "reason must explain the missing rpm backend: {}",
            err.reason()
        );
    }

    /// State records the RPM but rpmdb no longer has it (rpm -e drift, U7):
    /// refuse with repair/forget pointers rather than running dnf blindly.
    #[test]
    fn missing_from_rpmdb_refuses_with_forget_hint() {
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
        // rpmdb reports nothing installed for the package.
        let rpm = FakeRpm::new("copilot-shell", None);
        let err = update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true)
            .expect_err("drift must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("forget"),
            "reason must point at forget: {}",
            err.reason()
        );
        assert_eq!(rpm.update_calls.get(), 0);
    }

    /// `dnf update` failure surfaces as EXECUTION_FAILED with a repair pointer
    /// and does not refresh state or the contract snapshot.
    #[test]
    fn dnf_failure_surfaces_and_leaves_state_untouched() {
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
        let (_, snapshot) = seed_package_contract_and_stale_snapshot(&c, "copilot-shell");
        let rpm = FakeRpm::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.2.0", Some("1.al8"), "x86_64")),
        )
        .failing_update();

        let err = update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true)
            .expect_err("dnf failure must propagate");
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("dnf update failed") && err.reason().contains("repair"),
            "reason must carry the dnf failure and a repair pointer: {}",
            err.reason()
        );
        assert_eq!(
            find_component(&c, "copilot-shell")
                .last_operation_id
                .as_deref(),
            Some("op-prior"),
            "failed update must not refresh the record"
        );
        assert_eq!(
            std::fs::read_to_string(&snapshot).expect("read snapshot"),
            "framework = \"old\"\n",
            "a failed native transaction must not touch the snapshot"
        );
    }

    /// A same-name multi-version rpmdb (e.g. installonly packages) still
    /// counts as present, so dnf update runs — but the post-update observation
    /// cannot be recorded as one version, so the refresh fails honestly
    /// instead of caching an ambiguous EVR.
    #[test]
    fn multi_version_rpmdb_updates_but_refuses_to_record_ambiguity() {
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
        let rpm = FakeRpm::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.2.0", Some("1.al8"), "x86_64")),
        )
        .multi_version();
        let err = update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true)
            .expect_err("an ambiguous post-update observation must error");
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("multiple installed versions"),
            "got: {}",
            err.reason()
        );
        assert_eq!(
            rpm.update_calls.get(),
            1,
            "dnf update ran against the present package"
        );
        assert_eq!(
            find_component(&c, "copilot-shell")
                .last_operation_id
                .as_deref(),
            Some("op-prior"),
            "the ambiguous observation must not be recorded"
        );
    }

    /// No-op update (already latest): dnf runs (it owns the resolution), the
    /// EVR is unchanged, and the refreshed observation is still recorded.
    #[test]
    fn already_latest_reports_no_change() {
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
        // upgrade_to is None => update() is a no-op; EVR stays the same.
        let rpm = FakeRpm::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64")),
        );
        update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true).expect("update ok");
        assert_eq!(rpm.update_calls.get(), 1);
        let record = find_component(&c, "copilot-shell");
        assert_eq!(observed_evr(&record).as_deref(), Some("2.3.0-1.al8"));
        // Operation still recorded (last_operation_id advanced from the seed).
        assert_ne!(record.last_operation_id.as_deref(), Some("op-prior"));
    }

    /// dnf update applied, but the post-update rpmdb re-read cannot confirm the
    /// new EVR: surface a repair-pointing failure rather than recording the
    /// stale EVR, and leave the recorded state as-is.
    #[test]
    fn refresh_failure_after_successful_update_errors_and_leaves_state() {
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
        let rpm = FakeRpm::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.2.0", Some("1.al8"), "x86_64")),
        )
        .post_update_missing();

        let err = update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true)
            .expect_err("a failed post-update refresh must surface");
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("repair"),
            "reason must point at repair: {}",
            err.reason()
        );
        assert_eq!(rpm.update_calls.get(), 1, "dnf update did run");
        assert_eq!(
            find_component(&c, "copilot-shell")
                .last_operation_id
                .as_deref(),
            Some("op-prior"),
            "no stale EVR may be recorded as success"
        );
    }

    /// Post-lock guard (pure): the locked re-read only authorizes recording
    /// when the record is still delegated, consent still holds, and the
    /// package identity is unchanged — recording against a re-pointed record
    /// would graft one package's version onto another's metadata.
    #[test]
    fn native_update_authority_is_recomputed_from_the_locked_read() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "anolisa-pkg-b",
                "1.0.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        let store = load_store(&c);

        assert!(native_update_authorized(
            &store,
            "copilot-shell",
            Some("anolisa-pkg-b")
        ));
        assert!(
            !native_update_authorized(&store, "copilot-shell", Some("anolisa-pkg-a")),
            "a re-pointed package identity must not be recorded against"
        );
        assert!(
            !native_update_authorized(&store, "missing", Some("anolisa-pkg-b")),
            "a vanished record never authorizes recording"
        );
    }

    /// The locked re-read also refuses when consent was downgraded to
    /// observed while dnf ran.
    #[test]
    fn native_update_authority_requires_management_consent() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_observed_unadopted("copilot-shell", "copilot-shell", "1.0.0-1.al8"),
        );
        let store = load_store(&c);
        assert!(
            !native_update_authorized(&store, "copilot-shell", Some("copilot-shell")),
            "an observed record must not authorize a native update"
        );
    }

    // ── component update: owned (raw) records ──

    fn tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use tar::{Builder, Header};
        let enc = GzEncoder::new(Vec::new(), Compression::default());
        let mut tar = Builder::new(enc);
        for (path, data) in entries {
            let mut header = Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, *path, *data)
                .expect("append tar entry");
        }
        tar.into_inner()
            .expect("finish tar")
            .finish()
            .expect("finish gzip")
    }

    fn raw_manifest(component: &str, version: &str) -> String {
        format!(
            r#"[component]
name = "{component}"
version = "{version}"

[component.layout]
modes = ["system", "user"]

[[component.layout.files]]
source = "bin/{component}"
target = "{{bindir}}/{component}"
mode = "0755"
type = "executable"
"#
        )
    }

    /// tar.gz carrying the embedded manifest plus the binary it declares.
    fn raw_artifact(component: &str, version: &str, body: &[u8]) -> Vec<u8> {
        let manifest = raw_manifest(component, version);
        tar_gz(&[
            (".anolisa/component.toml", manifest.as_bytes()),
            (format!("bin/{component}").as_str(), body),
        ])
    }

    /// tar.gz whose manifest declares the binary but omits it, so the install
    /// runner fails after the old files have been backed up and removed.
    fn raw_artifact_missing_binary(component: &str, version: &str) -> Vec<u8> {
        let manifest = raw_manifest(component, version);
        tar_gz(&[(".anolisa/component.toml", manifest.as_bytes())])
    }

    /// Publish one version of `component` to a local file:// raw repo under
    /// `root` and point `layout`'s repo.toml at it. Returns the repo base URL.
    fn publish_raw_repo(
        root: &Path,
        layout: &FsLayout,
        component: &str,
        version: &str,
        artifact: &[u8],
    ) -> String {
        use sha2::{Digest, Sha256};
        let v1 = root.join("v1");
        std::fs::create_dir_all(&v1).expect("create repo dirs");
        let artifact_name = format!("{component}.tar.gz");
        std::fs::write(v1.join(&artifact_name), artifact).expect("write artifact");
        let sha = format!("{:x}", Sha256::digest(artifact));
        let env = anolisa_env::EnvService::detect();
        let index = format!(
            r#"schema_version = 1
channel = "stable"
publisher = "test"

[[entries]]
component = "{component}"
version = "{version}"
channel = "stable"
artifact_type = "tar_gz"
backend = "raw"
url = "{artifact_name}"
os = "{os}"
arch = "{arch}"
install_modes = ["system", "user"]
sha256 = "{sha}"
"#,
            os = env.os,
            arch = env.arch,
        );
        std::fs::write(v1.join("index.toml"), index).expect("write index");
        let base_url = format!("file://{}", v1.display());

        std::fs::create_dir_all(&layout.etc_dir).expect("etc dir");
        std::fs::write(
            layout.etc_dir.join("repo.toml"),
            format!(
                "schema_version = 1\ndefault_backend = \"raw\"\n\n[backends.raw]\nbase_url = \"{base_url}\"\n"
            ),
        )
        .expect("write repo.toml");
        base_url
    }

    /// Seed an installed raw component at `version` with one owned binary
    /// holding `body` plus its manifest snapshot.
    fn seed_installed_raw(ctx: &CliContext, component: &str, version: &str, body: &[u8]) {
        use sha2::{Digest, Sha256};
        let layout = common::resolve_layout(ctx);
        std::fs::create_dir_all(&layout.bin_dir).expect("bin dir");
        let bin = layout.bin_dir.join(component);
        std::fs::write(&bin, body).expect("write bin");
        let bin_sha = format!("{:x}", Sha256::digest(body));

        let manifest_path = common::installed_component_manifest_path(&layout, component, "update")
            .expect("manifest path");
        if let Some(parent) = manifest_path.parent() {
            std::fs::create_dir_all(parent).expect("manifest dir");
        }
        std::fs::write(&manifest_path, raw_manifest(component, version)).expect("write manifest");

        let files = vec![
            OwnedFile {
                path: bin,
                owner: FileOwner::Anolisa,
                sha256: Some(bin_sha),
                kind: OwnedFileKind::File,
                referent: None,
            },
            OwnedFile {
                path: manifest_path,
                owner: FileOwner::Anolisa,
                sha256: None,
                kind: OwnedFileKind::File,
                referent: None,
            },
        ];
        let mut obj = raw_object(component, version);
        obj.files = files;
        seed(ctx, obj);
    }

    /// Raw update resolves the latest published version, replaces the owned
    /// files, preserves the owned authority, and records the operation. The
    /// manifest is replaced as part of the owned artifact — the delegated
    /// datadir-contract refresh must not run.
    #[test]
    fn raw_update_upgrades_to_latest_and_preserves_ownership() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().join("sys"), InstallMode::System, false);
        seed_installed_raw(&c, "foo", "0.1.0", b"old v1 binary\n");
        // A datadir contract that only the delegated refresh would consult;
        // an owned update must never copy it over the owned manifest.
        let layout = common::resolve_layout(&c);
        let package_datadir = layout.package_datadir().expect("package datadir");
        let contract = FsLayout::component_contract_path(&package_datadir, "foo");
        std::fs::create_dir_all(contract.parent().expect("contract parent"))
            .expect("mkdir contract");
        std::fs::write(&contract, "framework = \"datadir\"\n").expect("write datadir contract");
        let new_body: &[u8] = b"#!/bin/sh\necho foo v2\n";
        publish_raw_repo(
            &tmp.path().join("repo"),
            &common::resolve_layout(&c),
            "foo",
            "0.2.0",
            &raw_artifact("foo", "0.2.0", new_body),
        );
        let rpm = FakeRpm::new("unused", None);

        update_component_with_deps("foo", &c, &rpm, &rpm, false).expect("raw update must succeed");

        assert_eq!(
            std::fs::read(layout.bin_dir.join("foo")).expect("read bin"),
            new_body,
            "binary must be replaced with the v2 payload"
        );
        let manifest = FsLayout::component_manifest_snapshot_path(&layout.state_dir, "foo");
        assert_eq!(
            std::fs::read_to_string(&manifest).expect("read manifest"),
            raw_manifest("foo", "0.2.0"),
            "the manifest must be the owned artifact's, not the datadir contract"
        );
        assert!(
            !FsLayout::provenance_path_for_snapshot(&manifest).exists(),
            "an owned update must not publish contract provenance"
        );
        let record = find_component(&c, "foo");
        let artifact = owned_artifact(&record);
        assert_eq!(artifact.version, "0.2.0");
        assert_eq!(record.status, LifecycleStatus::Installed);
        assert!(
            record
                .last_operation_id
                .as_deref()
                .is_some_and(|id| id.starts_with("op-update-")),
            "operation id must carry the update verb, got {:?}",
            record.last_operation_id
        );
        let store = load_store(&c);
        assert!(
            store.operations.iter().any(|o| o.command == "update foo"),
            "update operation must be recorded"
        );
        assert!(
            layout
                .backup_dir
                .read_dir()
                .map(|mut d| d.next().is_none())
                .unwrap_or(true),
            "backups must be pruned after a successful update"
        );
    }

    /// When the recorded version already matches the latest published version,
    /// update is a clean no-op (U2): no file or state change, no operation
    /// recorded.
    #[test]
    fn raw_update_already_latest_is_noop() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().join("sys"), InstallMode::System, false);
        let body: &[u8] = b"current binary\n";
        seed_installed_raw(&c, "foo", "0.2.0", body);
        publish_raw_repo(
            &tmp.path().join("repo"),
            &common::resolve_layout(&c),
            "foo",
            "0.2.0",
            &raw_artifact("foo", "0.2.0", b"unused\n"),
        );
        let rpm = FakeRpm::new("unused", None);

        update_component_with_deps("foo", &c, &rpm, &rpm, false).expect("no-op must succeed");

        let layout = common::resolve_layout(&c);
        assert_eq!(
            std::fs::read(layout.bin_dir.join("foo")).expect("read bin"),
            body,
            "no-op must not touch the binary"
        );
        assert!(
            load_store(&c).operations.is_empty(),
            "no-op records no operation"
        );
    }

    /// Dry-run reports without touching the filesystem or recorded state.
    #[test]
    fn raw_update_dry_run_does_not_touch_files_or_state() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().join("sys"), InstallMode::System, true);
        let body: &[u8] = b"old binary\n";
        seed_installed_raw(&c, "foo", "0.1.0", body);
        publish_raw_repo(
            &tmp.path().join("repo"),
            &common::resolve_layout(&c),
            "foo",
            "0.2.0",
            &raw_artifact("foo", "0.2.0", b"new\n"),
        );
        let rpm = FakeRpm::new("unused", None);

        update_component_with_deps("foo", &c, &rpm, &rpm, false).expect("dry-run must succeed");

        let layout = common::resolve_layout(&c);
        assert_eq!(
            std::fs::read(layout.bin_dir.join("foo")).expect("read bin"),
            body,
            "dry-run must not touch the binary"
        );
        assert_eq!(
            owned_artifact(&find_component(&c, "foo")).version,
            "0.1.0",
            "dry-run must not change the recorded version"
        );
    }

    /// A failure while installing the new version compensates back: the old
    /// files are restored from backup and the recorded version is unchanged.
    #[test]
    fn raw_update_rolls_back_on_install_failure() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().join("sys"), InstallMode::System, false);
        let body: &[u8] = b"original v1 binary\n";
        seed_installed_raw(&c, "foo", "0.1.0", body);
        publish_raw_repo(
            &tmp.path().join("repo"),
            &common::resolve_layout(&c),
            "foo",
            "0.2.0",
            &raw_artifact_missing_binary("foo", "0.2.0"),
        );
        let rpm = FakeRpm::new("unused", None);

        let err = update_component_with_deps("foo", &c, &rpm, &rpm, false)
            .expect_err("install of the new version must fail");
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("the previous files were restored"),
            "the failure must report the compensation honestly: {}",
            err.reason()
        );

        let layout = common::resolve_layout(&c);
        assert_eq!(
            std::fs::read(layout.bin_dir.join("foo")).expect("read bin"),
            body,
            "old binary must be restored from backup"
        );
        assert_eq!(
            owned_artifact(&find_component(&c, "foo")).version,
            "0.1.0",
            "failed update must not change the recorded version"
        );
    }

    /// resolve_raw always selects the highest published version; if the index
    /// only offers an older release, update must refuse rather than downgrade
    /// (U4).
    #[test]
    fn raw_update_refuses_downgrade() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().join("sys"), InstallMode::System, false);
        let body: &[u8] = b"installed 0.2.0\n";
        seed_installed_raw(&c, "foo", "0.2.0", body);
        // The repo only publishes the older 0.1.0.
        publish_raw_repo(
            &tmp.path().join("repo"),
            &common::resolve_layout(&c),
            "foo",
            "0.1.0",
            &raw_artifact("foo", "0.1.0", b"older\n"),
        );
        let rpm = FakeRpm::new("unused", None);

        let err = update_component_with_deps("foo", &c, &rpm, &rpm, false)
            .expect_err("a downgrade must be refused");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("refusing to downgrade"),
            "got: {}",
            err.reason()
        );

        let layout = common::resolve_layout(&c);
        assert_eq!(
            std::fs::read(layout.bin_dir.join("foo")).expect("read bin"),
            body,
            "refused downgrade must not touch the binary"
        );
        assert_eq!(
            owned_artifact(&find_component(&c, "foo")).version,
            "0.2.0",
            "refused downgrade must not change the recorded version"
        );
    }

    /// A newer artifact that declares a dependency the host lacks must be
    /// refused with nothing effectively changed: the owned update runs the
    /// same runtime preflight as a fresh install during download-verify.
    #[test]
    fn raw_update_refuses_when_new_artifact_adds_unmet_dependency() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().join("sys"), InstallMode::System, false);
        let old_body: &[u8] = b"installed 0.1.0\n";
        seed_installed_raw(&c, "foo", "0.1.0", old_body);

        // v2's contract adds a system-package whose probe can never succeed, so
        // the preflight fails on every host.
        let deps = r#"
[[component.dependencies]]
name = "absent-tool"
kind = "system-package"
probe = "anolisa-nonexistent-probe-xyz --version"
packages = { rpm = "absent-tool", deb = "absent-tool" }
"#;
        let manifest = format!("{}{}", raw_manifest("foo", "0.2.0"), deps);
        let new_body: &[u8] = b"#!/bin/sh\necho foo v2\n";
        let artifact = tar_gz(&[
            (".anolisa/component.toml", manifest.as_bytes()),
            ("bin/foo", new_body),
        ]);
        publish_raw_repo(
            &tmp.path().join("repo"),
            &common::resolve_layout(&c),
            "foo",
            "0.2.0",
            &artifact,
        );
        let rpm = FakeRpm::new("unused", None);

        let err = update_component_with_deps("foo", &c, &rpm, &rpm, false)
            .expect_err("update must refuse when the new artifact adds an unmet dependency");
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("missing runtime dependencies"),
            "error must come from the runtime preflight, got: {}",
            err.reason()
        );

        let layout = common::resolve_layout(&c);
        assert_eq!(
            std::fs::read(layout.bin_dir.join("foo")).expect("read bin"),
            old_body,
            "refused update must not change the old binary"
        );
        assert_eq!(
            owned_artifact(&find_component(&c, "foo")).version,
            "0.1.0",
            "refused update must not change the recorded version"
        );
    }

    /// A successful update resets transient state: status returns to Installed
    /// and stale service rows from the old version are cleared (the new
    /// manifest declares no services here).
    #[test]
    fn raw_update_resets_status_and_clears_stale_state() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().join("sys"), InstallMode::System, false);
        seed_installed_raw(&c, "foo", "0.1.0", b"old\n");
        // Poison transient state as if a prior op had failed and left rows.
        {
            let layout = common::resolve_layout(&c);
            let path = layout.state_dir.join("installed.toml");
            let mut state = InstalledState::load(&path).expect("load state");
            let obj = state
                .find_object_mut(ObjectKind::Component, "foo")
                .expect("seeded object");
            obj.status = ObjectStatus::Failed;
            obj.services = vec![ServiceRef {
                name: "stale.service".to_string(),
                manager: "systemd".to_string(),
                restartable: true,
                enabled: false,
                scope: anolisa_core::ServiceScope::System,
            }];
            state.save(&path).expect("save poisoned state");
        }
        publish_raw_repo(
            &tmp.path().join("repo"),
            &common::resolve_layout(&c),
            "foo",
            "0.2.0",
            &raw_artifact("foo", "0.2.0", b"new\n"),
        );
        let rpm = FakeRpm::new("unused", None);

        update_component_with_deps("foo", &c, &rpm, &rpm, false).expect("update must succeed");

        let record = find_component(&c, "foo");
        let artifact = owned_artifact(&record);
        assert_eq!(artifact.version, "0.2.0");
        assert_eq!(
            record.status,
            LifecycleStatus::Installed,
            "status must reset to Installed after a clean update"
        );
        assert!(
            artifact.services.is_empty(),
            "stale services must be cleared when the new manifest declares none"
        );
    }

    /// version_relation classifies semver pairs and, crucially, refuses to
    /// guess a direction for non-semver versions so the downgrade guard holds.
    #[test]
    fn version_relation_classifies_semver_and_non_semver() {
        // Plain semver precedence.
        assert_eq!(version_relation("0.1.0", "0.2.0"), VersionRelation::Newer);
        assert_eq!(version_relation("0.2.0", "0.1.0"), VersionRelation::Older);
        assert_eq!(version_relation("1.0.0", "1.0.0"), VersionRelation::Same);
        // A leading `v` is normalized away before comparison.
        assert_eq!(version_relation("v1.2.3", "1.2.3"), VersionRelation::Same);
        // Non-semver: equal normalized strings are Same, anything else is
        // Indeterminate — never silently treated as an upgrade.
        assert_eq!(
            version_relation("2026.06", "2026.06"),
            VersionRelation::Same
        );
        assert_eq!(
            version_relation("2026.06", "0.5.0"),
            VersionRelation::Indeterminate
        );
        assert_eq!(
            version_relation("0.5.0", "nightly"),
            VersionRelation::Indeterminate
        );
    }

    /// A non-semver installed version cannot be ordered against the published
    /// one, so update refuses rather than risk replacing a newer custom build
    /// with an older published release (U4).
    #[test]
    fn raw_update_refuses_non_semver_version() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().join("sys"), InstallMode::System, false);
        let body: &[u8] = b"calver build\n";
        seed_installed_raw(&c, "foo", "2026.06", body);
        publish_raw_repo(
            &tmp.path().join("repo"),
            &common::resolve_layout(&c),
            "foo",
            "0.5.0",
            &raw_artifact("foo", "0.5.0", b"older semver\n"),
        );
        let rpm = FakeRpm::new("unused", None);

        let err = update_component_with_deps("foo", &c, &rpm, &rpm, false)
            .expect_err("a non-orderable version must be refused");
        assert_eq!(err.code(), "INVALID_ARGUMENT");

        let layout = common::resolve_layout(&c);
        assert_eq!(
            std::fs::read(layout.bin_dir.join("foo")).expect("read bin"),
            body,
            "refused update must not touch the binary"
        );
        assert_eq!(
            owned_artifact(&find_component(&c, "foo")).version,
            "2026.06",
            "refused update must not change the recorded version"
        );
    }

    /// The recorded version is re-validated under the install lock: a prior
    /// snapshot that no longer matches the locked read (a concurrent update
    /// landed in between) aborts before anything is touched.
    #[test]
    fn raw_update_aborts_on_concurrent_version_drift() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().join("sys"), InstallMode::System, false);
        let body: &[u8] = b"already at 0.2.0\n";
        seed_installed_raw(&c, "foo", "0.2.0", body);
        publish_raw_repo(
            &tmp.path().join("repo"),
            &common::resolve_layout(&c),
            "foo",
            "0.2.0",
            &raw_artifact("foo", "0.2.0", b"new payload\n"),
        );

        let layout = common::resolve_layout(&c);
        let state_path = layout.state_dir.join("installed.toml");
        let journal_dir = rpm_install::journal_dir(&layout);
        let env = anolisa_env::EnvService::detect();
        let repo_config =
            common::load_repo_config(&c, &layout, "update foo", RepoPersistPolicy::Require)
                .expect("repo config");
        let inputs = resolve_raw_inputs_for_component(
            "foo".to_string(),
            "raw",
            None,
            &env,
            &repo_config,
            "update foo",
        )
        .expect("resolve inputs");
        let resolution = resolve_raw(&c, &layout, &env, inputs).expect("resolve latest");

        // The pre-lock snapshot carries a stale version, as if a concurrent
        // update advanced the record between resolve and lock.
        let mut stale = owned_artifact(&find_component(&c, "foo"));
        stale.version = "0.1.0".to_string();

        let err = update_owned(
            "foo",
            &c,
            &layout,
            &state_path,
            &journal_dir,
            InstallationScope::System,
            "2026-07-16T00:00:00Z",
            &[],
            &[],
            resolution,
            stale,
            "update foo",
        )
        .expect_err("a drifted snapshot must abort under the lock");
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("while this update was resolving"),
            "got: {}",
            err.reason()
        );

        assert_eq!(
            std::fs::read(layout.bin_dir.join("foo")).expect("read bin"),
            body,
            "aborted update must not touch the binary"
        );
        assert_eq!(
            owned_artifact(&find_component(&c, "foo")).version,
            "0.2.0",
            "aborted update must not change the recorded version"
        );
    }

    /// A component installed with `--package` (recorded on the artifact as
    /// `raw_package`) updates against that package, not one re-derived from
    /// the component name. Published only under the non-default key `altpkg`,
    /// so a re-derived `foo` would resolve nothing.
    #[test]
    fn raw_update_reuses_recorded_package() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().join("sys"), InstallMode::System, false);
        seed_installed_raw(&c, "foo", "0.1.0", b"old foo\n");
        // Record the alternate package the component was installed from.
        {
            let layout = common::resolve_layout(&c);
            let path = layout.state_dir.join("installed.toml");
            let mut state = InstalledState::load(&path).expect("load state");
            state
                .find_object_mut(ObjectKind::Component, "foo")
                .expect("seeded object")
                .raw_package = Some("altpkg".to_string());
            state.save(&path).expect("save state");
        }
        let new_body: &[u8] = b"new foo fetched via altpkg\n";
        publish_raw_repo(
            &tmp.path().join("repo"),
            &common::resolve_layout(&c),
            "altpkg",
            "0.2.0",
            &raw_artifact("foo", "0.2.0", new_body),
        );
        let rpm = FakeRpm::new("unused", None);

        update_component_with_deps("foo", &c, &rpm, &rpm, false)
            .expect("update must resolve via the recorded package");

        let layout = common::resolve_layout(&c);
        assert_eq!(
            std::fs::read(layout.bin_dir.join("foo")).expect("read bin"),
            new_body,
            "binary must be replaced with the version fetched via the recorded package"
        );
        let artifact = owned_artifact(&find_component(&c, "foo"));
        assert_eq!(artifact.version, "0.2.0");
        assert_eq!(
            artifact.raw_package.as_deref(),
            Some("altpkg"),
            "the recorded package must survive the update"
        );
    }

    /// With no recorded package, update derives it from the component name; if
    /// the index only publishes a non-default package the resolve fails. This
    /// is the failing half that proves the recorded package is what made
    /// [`raw_update_reuses_recorded_package`] succeed.
    #[test]
    fn raw_update_without_recorded_package_cannot_find_alt_package() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().join("sys"), InstallMode::System, false);
        seed_installed_raw(&c, "foo", "0.1.0", b"old foo\n");
        publish_raw_repo(
            &tmp.path().join("repo"),
            &common::resolve_layout(&c),
            "altpkg",
            "0.2.0",
            &raw_artifact("foo", "0.2.0", b"unreachable\n"),
        );
        let rpm = FakeRpm::new("unused", None);

        update_component_with_deps("foo", &c, &rpm, &rpm, false)
            .expect_err("deriving 'foo' must not resolve the 'altpkg'-only index");

        let layout = common::resolve_layout(&c);
        assert_eq!(
            std::fs::read(layout.bin_dir.join("foo")).expect("read bin"),
            b"old foo\n",
            "a failed resolve must not touch the binary"
        );
    }

    /// An owned component routes to the raw backend and never runs dnf —
    /// even when an RPM of the same name is installed.
    ///
    /// Uses System mode: `resolve_layout` honours `prefix` only for System
    /// mode, so a User-mode test would read and mutate the real user home.
    #[test]
    fn raw_component_update_never_runs_dnf() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().join("sys"), InstallMode::System, false);
        seed_installed_raw(&c, "copilot-shell", "0.1.0", b"old\n");
        publish_raw_repo(
            &tmp.path().join("repo"),
            &common::resolve_layout(&c),
            "copilot-shell",
            "0.2.0",
            &raw_artifact("copilot-shell", "0.2.0", b"new\n"),
        );
        let rpm = FakeRpm::new(
            "copilot-shell",
            Some(pkg_info("copilot-shell", "2.2.0", Some("1.al8"), "x86_64")),
        );

        update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true)
            .expect("raw update must succeed");

        assert_eq!(
            rpm.update_calls.get(),
            0,
            "raw update must never run dnf on the system RPM"
        );
        assert_eq!(
            owned_artifact(&find_component(&c, "copilot-shell")).version,
            "0.2.0",
            "the raw component must be updated to the published version"
        );
    }

    // ── CLI surface: `update <component>` is the direct form ────────────

    use clap::Parser;

    /// `update <component>` parses to the positional, with no subcommand.
    #[test]
    fn update_component_parses_as_positional() {
        let a = UpdateArgs::try_parse_from(["update", "copilot-shell"]).expect("parse");
        assert_eq!(a.component.as_deref(), Some("copilot-shell"));
        assert!(a.command.is_none());
    }

    /// `update self` parses to the self subcommand, not a component named
    /// "self" (subcommands take precedence over the positional).
    #[test]
    fn update_self_parses_as_subcommand() {
        let a = UpdateArgs::try_parse_from(["update", "self"]).expect("parse");
        assert!(matches!(a.command, Some(UpdateCommands::SelfBin)));
        assert!(a.component.is_none());
    }

    /// `update all` parses to the all subcommand.
    #[test]
    fn update_all_parses_as_subcommand() {
        let a = UpdateArgs::try_parse_from(["update", "all"]).expect("parse");
        assert!(matches!(a.command, Some(UpdateCommands::All)));
        assert!(a.component.is_none());
    }

    /// A positional and a subcommand are mutually exclusive.
    #[test]
    fn update_component_with_subcommand_is_a_parse_error() {
        UpdateArgs::try_parse_from(["update", "copilot-shell", "self"])
            .expect_err("positional + subcommand must conflict");
    }

    /// `update` with no target is an INVALID_ARGUMENT, not a panic or a silent
    /// no-op — the dispatcher needs a target.
    #[test]
    fn update_with_no_target_is_invalid_argument() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, true);
        let args = UpdateArgs {
            component: None,
            command: None,
            check: false,
            motd: false,
            refresh: false,
            target: None,
        };
        let err = handle(args, &c).expect_err("must require a target");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("specify a component"));
    }
}
