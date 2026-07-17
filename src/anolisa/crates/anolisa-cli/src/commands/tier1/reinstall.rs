//! `anolisa reinstall <component>` — re-run the file transaction for an
//! installed component at its current version.
//!
//! Reinstall is its own verb (vocabulary-aligned with `dnf reinstall`):
//! `install` stays idempotent over healthy records and `repair` only fixes
//! detected drift, so neither ever re-places files on demand — this command
//! does. It is also the reference implementation of the thin-shell pipeline:
//! the handler only assembles facts, asks the planner, and hands the step
//! sequence to the matching executor. No lifecycle policy lives here.
//!
//! Both step families execute end to end: a delegated (RPM-backed) record
//! re-runs the native file transaction through the delegated executor, and an
//! owned (raw) record replays its artifact through the owned executor with
//! [`RawReplayOps`] as the side-effect port.

use std::path::Path;

use clap::Parser;

use anolisa_core::domain::{InstallationScope, NativePm, OwnedArtifact, ProviderBinding};
use anolisa_core::executor::execute_delegated_steps;
use anolisa_core::facts::{ObserveRequest, assemble_facts};
use anolisa_core::lock::InstallLock;
use anolisa_core::owned_executor::{OwnedExecutionError, execute_owned_steps};
use anolisa_core::planner::{HookKind, Intent, Plan, PlanError, Step, plan};
use anolisa_core::providers::DelegatedProvider;
use anolisa_core::record_sink::{DelegatedIdentity, RecordContext, StoreRecordSink};
use anolisa_core::state::{ObjectKind, OperationRecord};
use anolisa_core::state_store::StateStore;
use anolisa_core::transaction::Transaction;
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::pkg_query::PackageQuery;
use anolisa_platform::pkg_transaction::PackageTransaction;
use anolisa_platform::privilege;
use anolisa_platform::rpm_query::RpmPackageQuery;
use anolisa_platform::rpm_transaction::RpmTransaction;
use chrono::{SecondsFormat, Utc};
use serde::Serialize;

use crate::commands::common;
use crate::commands::common::RepoPersistPolicy;
use crate::commands::tier1::install::{
    RawReplayOps, ResolveInputs, resolve_raw, resolve_raw_inputs_for_component,
};
use crate::commands::tier1::rpm_install;
use crate::context::CliContext;
use crate::response::{CliError, render_json};

/// Command label for JSON envelopes and error routing.
const COMMAND: &str = "reinstall";

/// Arguments for `anolisa reinstall <component>`.
#[derive(Debug, Parser)]
pub struct ReinstallArgs {
    /// Component to reinstall at its currently installed version
    #[arg(value_name = "COMPONENT")]
    pub component: String,
}

/// Dispatch `reinstall <component>` against the live host.
///
/// # Errors
///
/// Returns [`CliError`] when the component is not installed, is tracked
/// without management consent (adopt first), was removed externally, or the
/// native transaction fails.
pub fn handle(args: ReinstallArgs, ctx: &CliContext) -> Result<(), CliError> {
    let query = RpmPackageQuery::system();
    let txn = RpmTransaction::system();
    reinstall_with_host(&args.component, ctx, &query, &txn, privilege::is_root())
}

/// JSON payload for a completed (or previewed) reinstall.
#[derive(Debug, Serialize)]
struct ReinstallPayload {
    component: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    package: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    dry_run: bool,
    plan: Vec<String>,
}

/// Core of [`handle`] with the package backends injected so tests drive
/// every branch without a live rpmdb or dnf.
pub(crate) fn reinstall_with_host(
    target: &str,
    ctx: &CliContext,
    query: &dyn PackageQuery,
    txn: &dyn PackageTransaction,
    is_root: bool,
) -> Result<(), CliError> {
    let command = format!("{COMMAND} {target}");
    let layout = common::resolve_layout(ctx);
    let state_path = layout.state_dir.join("installed.toml");
    let journal_dir = rpm_install::journal_dir(&layout);
    let uid = privilege::effective_uid();
    let scope = match ctx.install_mode {
        crate::context::InstallMode::System => InstallationScope::System,
        crate::context::InstallMode::User => InstallationScope::User { uid },
    };
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);

    let mut store = StateStore::load(&state_path, uid).map_err(|err| CliError::Runtime {
        command: command.clone(),
        reason: format!("failed to load installed state: {err}"),
    })?;

    // The probe target comes from the record: reinstall never resolves a new
    // package, it re-places what is already tracked.
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

    let provider = DelegatedProvider::new(query, txn);
    let facts = assemble_facts(
        &ObserveRequest {
            kind: ObjectKind::Component,
            name: target,
            scope,
            native_package: native_package.as_deref(),
            observed_at: &now,
            verify_owned_files: false,
        },
        &store,
        Some(&provider),
        &layout,
        &journal_dir,
    )
    .map_err(|err| CliError::Runtime {
        command: command.clone(),
        reason: err.to_string(),
    })?;

    let steps = match plan(&Intent::Reinstall, &facts) {
        Ok(Plan::Execute { steps, .. }) => steps,
        Ok(Plan::NoOp { .. }) => {
            // The reinstall table has no NoOp rows today; render an honest
            // "nothing to do" if the planner ever grows one.
            return render_result(ctx, target, None, None, None, true, &[]);
        }
        Err(err) => return Err(plan_error_to_cli(err, target, &command)),
    };

    let plan_labels: Vec<String> = steps.iter().map(step_label).collect();

    if ctx.dry_run {
        return render_result(
            ctx,
            target,
            native_package.as_deref(),
            None,
            None,
            true,
            &plan_labels,
        );
    }

    // Route by step family: delegated plans re-run the native transaction,
    // owned plans replay the recorded artifact through the raw backend.
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
        let prior = match store
            .find(ObjectKind::Component, target)
            .map(|r| &r.binding)
        {
            Some(ProviderBinding::Owned { artifact }) => artifact.clone(),
            _ => {
                return Err(CliError::Runtime {
                    command,
                    reason: format!(
                        "internal: planner produced an owned plan but the record for '{target}' is not owned"
                    ),
                });
            }
        };
        return reinstall_owned(
            target,
            ctx,
            &layout,
            &state_path,
            &journal_dir,
            scope,
            &now,
            &steps,
            &plan_labels,
            prior,
            &command,
        );
    }

    if !is_root {
        return Err(CliError::PermissionDenied {
            command,
            reason: "reinstalling an RPM-backed component runs dnf and requires root".to_string(),
            hint: Some(format!("sudo anolisa reinstall {target}")),
        });
    }

    let package = native_package.clone().unwrap_or_else(|| target.to_string());
    let mut journal =
        Transaction::begin_with_subject(COMMAND, Some(target), state_path.clone(), &journal_dir)
            .map_err(|err| CliError::Runtime {
                command: command.clone(),
                reason: format!("failed to begin operation journal: {err}"),
            })?;
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
        execute_delegated_steps(&steps, &provider, &mut sink, &mut journal, &now)
    }
    .map_err(|err| CliError::Runtime {
        command: command.clone(),
        reason: format!(
            "reinstall of '{target}' failed: {err}; the native transaction is never undone automatically — run `anolisa repair {target}` to reconcile"
        ),
    })?;

    // Operation history is best-effort bookkeeping on top of the committed
    // record: the reinstall already succeeded, so a history-write failure
    // degrades to a warning instead of unwinding anything.
    store.operations.push(OperationRecord {
        id: operation_id.clone(),
        command: command.clone(),
        status: "ok".to_string(),
        started_at: now.clone(),
        finished_at: Some(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)),
        parent_operation_id: None,
    });
    if let Err(err) = store.save(&state_path) {
        eprintln!("warning: failed to record operation history: {err}");
    }

    let version = outcome.observation.as_ref().map(|o| o.version.clone());
    render_result(
        ctx,
        target,
        Some(&package),
        version.as_deref(),
        Some(&operation_id),
        false,
        &plan_labels,
    )
}

/// Execute an owned replay plan against the raw backend.
///
/// Resolution is pinned to the recorded version — reinstall re-places what is
/// installed, it never upgrades — and requires the version to still be
/// published (the record holds no artifact digest, and an unverifiable
/// artifact is never installed). The store is re-read under the install lock
/// so the backup/remove set can never come from a stale snapshot.
#[expect(clippy::too_many_arguments)]
fn reinstall_owned(
    target: &str,
    ctx: &CliContext,
    layout: &FsLayout,
    state_path: &Path,
    journal_dir: &Path,
    scope: InstallationScope,
    now: &str,
    steps: &[Step],
    plan_labels: &[String],
    prior: OwnedArtifact,
    command: &str,
) -> Result<(), CliError> {
    // No root pre-check for an owned replay: `--prefix` may point at a
    // user-writable tree, and a genuine permission problem fails the exact
    // step and unwinds honestly instead of a blanket refusal.

    // Re-resolve the recorded package at the recorded version to recover the
    // artifact URL and its published sha256.
    let repo_config = common::load_repo_config(ctx, layout, command, RepoPersistPolicy::Require)?;
    let env = anolisa_env::EnvService::detect();
    let inputs = ResolveInputs {
        version: Some(prior.version.as_str()),
        ..resolve_raw_inputs_for_component(
            target.to_string(),
            "raw",
            prior.raw_package.as_deref(),
            &env,
            &repo_config,
            command,
        )?
    };
    let resolution = resolve_raw(ctx, layout, &env, inputs).map_err(|e| e.with_command(command))?;
    let resolve_warnings = resolution.warnings.clone();
    let package = resolution.package.clone();
    let version = prior.version.clone();

    // Lock, then re-read state under the lock: the file set to back up and
    // remove must reflect what is on disk now, not the pre-plan snapshot.
    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;
    let mut store = StateStore::load(state_path, privilege::effective_uid()).map_err(|err| {
        CliError::Runtime {
            command: command.to_string(),
            reason: format!("failed to load installed state: {err}"),
        }
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
                    "component '{target}' changed from {} to {} while this reinstall was resolving; nothing was changed — re-run `anolisa reinstall {target}`",
                    prior.version, artifact.version
                ),
            });
        }
        _ => {
            return Err(CliError::Runtime {
                command: command.to_string(),
                reason: format!(
                    "component '{target}' is no longer an owned installation; nothing was changed — re-run `anolisa reinstall {target}`"
                ),
            });
        }
    };

    let mut journal = Transaction::begin_with_subject(
        COMMAND,
        Some(target),
        state_path.to_path_buf(),
        journal_dir,
    )
    .map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to begin operation journal: {err}"),
    })?;
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
        );
        let result = execute_owned_steps(steps, &mut ops, &mut journal);
        if result.is_ok() {
            // Per-operation backups are rollback scratch; a failed plan keeps
            // them on disk for forensics.
            ops.discard_backups();
        }
        result
    }
    .map_err(|err| owned_error_to_cli(err, target, command))?;

    // Operation history is best-effort bookkeeping on top of the committed
    // record, exactly like the delegated path.
    store.operations.push(OperationRecord {
        id: operation_id.clone(),
        command: command.to_string(),
        status: "ok".to_string(),
        started_at: now.to_string(),
        finished_at: Some(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)),
        parent_operation_id: None,
    });
    if let Err(err) = store.save(state_path) {
        eprintln!("warning: failed to record operation history: {err}");
    }

    for warning in resolve_warnings.iter().chain(outcome.warnings.iter()) {
        eprintln!("warning: {warning}");
    }

    render_result(
        ctx,
        target,
        Some(&package),
        Some(&version),
        Some(&operation_id),
        false,
        plan_labels,
    )
}

/// Map an owned-executor failure to a CLI error that reports honestly what
/// happened to the host: cleanly restored, partially restored, or untouched.
fn owned_error_to_cli(err: OwnedExecutionError, target: &str, command: &str) -> CliError {
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
                format!(
                    "reinstall of '{target}' failed at '{at}': {source}; the host was not changed"
                )
            } else if rollback_warnings.is_empty() {
                format!(
                    "reinstall of '{target}' failed at '{at}': {source}; the previous files were restored"
                )
            } else {
                format!(
                    "reinstall of '{target}' failed at '{at}': {source}; restoring the previous files reported problems ({}) — run `anolisa repair {target}`",
                    rollback_warnings.join("; ")
                )
            }
        }
        other => format!("reinstall of '{target}' failed: {other}"),
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
        Step::ProvisionRuntimeDeps => "provision runtime dependencies".to_string(),
        Step::RunHook(kind) => format!(
            "run {} hooks",
            match kind {
                HookKind::PreInstall => "pre-install",
                HookKind::PostInstall => "post-install",
                HookKind::PreUninstall => "pre-uninstall",
                HookKind::PostUninstall => "post-uninstall",
            }
        ),
        Step::BackupFiles => "back up current files".to_string(),
        Step::PlaceFiles => "place files".to_string(),
        Step::SetCapabilities => "apply file capabilities".to_string(),
        Step::EnableServices => "enable services".to_string(),
        Step::RestartServices => "restart services".to_string(),
        Step::StopServices => "stop services".to_string(),
        Step::RemoveOwnedFiles => "remove owned files".to_string(),
        other => format!("{other:?}"),
    }
}

/// Map a planning refusal to an actionable CLI error. The planner names the
/// way out; this mapping only renders it.
fn plan_error_to_cli(err: PlanError, target: &str, command: &str) -> CliError {
    let command = command.to_string();
    match err {
        PlanError::NotInstalled => CliError::InvalidArgument {
            command,
            reason: format!(
                "component '{target}' is not installed; run `anolisa install {target}` instead"
            ),
        },
        PlanError::NotAdopted => CliError::InvalidArgument {
            command,
            reason: format!(
                "component '{target}' is only observed, not managed; run `anolisa adopt {target}` first, then reinstall"
            ),
        },
        PlanError::ExternallyRemoved => CliError::InvalidArgument {
            command,
            reason: format!(
                "the package backing '{target}' was removed outside ANOLISA; run `anolisa repair {target}` to reconcile or `anolisa forget {target}` to drop the record"
            ),
        },
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
        other => CliError::InvalidArgument {
            command,
            reason: format!("cannot reinstall '{target}': {other:?}"),
        },
    }
}

fn render_result(
    ctx: &CliContext,
    component: &str,
    package: Option<&str>,
    version: Option<&str>,
    operation_id: Option<&str>,
    dry_run: bool,
    plan_labels: &[String],
) -> Result<(), CliError> {
    if ctx.json {
        return render_json(
            COMMAND,
            ReinstallPayload {
                component: component.to_string(),
                package: package.map(str::to_string),
                version: version.map(str::to_string),
                operation_id: operation_id.map(str::to_string),
                dry_run,
                plan: plan_labels.to_vec(),
            },
        );
    }
    if dry_run {
        println!("reinstall {component} (dry-run):");
        for label in plan_labels {
            println!("  - {label}");
        }
        return Ok(());
    }
    match version {
        Some(version) => println!("reinstalled {component} {version}"),
        None => println!("reinstalled {component}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;

    use anolisa_core::domain::{
        Installation, LifecycleStatus, ManagementRelation, Observation, PackageIdentity,
    };
    use anolisa_core::state::InstallMode as StateInstallMode;
    use anolisa_core::state::{FileOwner, OwnedFile, OwnedFileKind};
    use anolisa_platform::pkg_query::{PackageInfo, PackageQueryError, PackageVersion};
    use anolisa_platform::pkg_transaction::PackageTransactionError;

    use crate::context::InstallMode;

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

    struct FakeHost {
        installed: RefCell<Option<PackageInfo>>,
        reinstall_calls: Cell<usize>,
        reinstall_succeeds: bool,
    }

    impl FakeHost {
        fn with_installed(info: Option<PackageInfo>) -> Self {
            Self {
                installed: RefCell::new(info),
                reinstall_calls: Cell::new(0),
                reinstall_succeeds: true,
            }
        }
    }

    impl PackageQuery for FakeHost {
        fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
            Ok(self
                .installed
                .borrow()
                .clone()
                .filter(|info| info.name == package))
        }
        fn query_available(&self, _package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
            Ok(Vec::new())
        }
    }

    impl PackageTransaction for FakeHost {
        fn install(&self, _packages: &[&str]) -> Result<(), PackageTransactionError> {
            panic!("reinstall must not delegate a dnf install");
        }
        fn update(&self, _packages: &[&str]) -> Result<(), PackageTransactionError> {
            panic!("reinstall must not delegate a dnf update");
        }
        fn reinstall(&self, _packages: &[&str]) -> Result<(), PackageTransactionError> {
            self.reinstall_calls.set(self.reinstall_calls.get() + 1);
            if !self.reinstall_succeeds {
                return Err(PackageTransactionError::TransactionFailed {
                    command: "dnf".to_string(),
                    operation: "reinstall".to_string(),
                    code: Some(1),
                    stderr: "payload unavailable".to_string(),
                });
            }
            Ok(())
        }
        fn remove(&self, _packages: &[&str]) -> Result<(), PackageTransactionError> {
            panic!("reinstall must not delegate a dnf remove");
        }
    }

    fn pkg_info(name: &str, version: &str) -> PackageInfo {
        PackageInfo {
            name: name.to_string(),
            version: PackageVersion {
                epoch: None,
                version: version.to_string(),
                release: Some("1.al4".to_string()),
            },
            arch: "x86_64".to_string(),
            origin: None,
        }
    }

    fn delegated_installation(name: &str, relation: ManagementRelation) -> Installation {
        Installation {
            kind: ObjectKind::Component,
            name: name.to_string(),
            scope: InstallationScope::System,
            binding: ProviderBinding::Delegated {
                pm: NativePm::Rpm,
                package: PackageIdentity::Resolved {
                    name: name.to_string(),
                },
                relation,
                last_observed: Some(Observation {
                    version: "2.6.0".to_string(),
                    evr: Some("2.6.0-1.al4".to_string()),
                    arch: Some("x86_64".to_string()),
                    source_repo: None,
                    observed_at: "2026-07-01T00:00:00Z".to_string(),
                }),
            },
            status: LifecycleStatus::Installed,
            installed_at: "2026-07-01T00:00:00Z".to_string(),
            last_operation_id: None,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            health: Vec::new(),
        }
    }

    /// Seed a v5 state file under the layout that `resolve_layout(ctx)`
    /// derives for the given prefix, returning the state path.
    fn seed_state(ctx: &CliContext, installations: Vec<Installation>) -> PathBuf {
        let layout = common::resolve_layout(ctx);
        let state_path = layout.state_dir.join("installed.toml");
        let mut store = StateStore::empty();
        store.install_mode = StateInstallMode::System;
        for installation in installations {
            store.upsert(installation);
        }
        store.save(&state_path).expect("seed state");
        state_path
    }

    #[test]
    fn managed_component_reinstalls_and_refreshes_the_observation() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let ctx = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let state_path = seed_state(
            &ctx,
            vec![delegated_installation(
                "cosh",
                ManagementRelation::Managed {
                    since: "2026-07-01T00:00:00Z".to_string(),
                },
            )],
        );
        let host = FakeHost::with_installed(Some(pkg_info("cosh", "2.7.0")));

        reinstall_with_host("cosh", &ctx, &host, &host, true).expect("reinstall ok");

        assert_eq!(host.reinstall_calls.get(), 1);
        // The observation cache absorbed the post-transaction probe.
        let store = StateStore::load(&state_path, 0).expect("reload");
        let record = store.find(ObjectKind::Component, "cosh").expect("record");
        match &record.binding {
            ProviderBinding::Delegated {
                relation: ManagementRelation::Managed { .. },
                last_observed,
                ..
            } => {
                assert_eq!(
                    last_observed.as_ref().map(|o| o.version.as_str()),
                    Some("2.7.0")
                );
            }
            other => panic!("expected managed binding, got {other:?}"),
        }
        assert_eq!(store.operations.len(), 1);
        assert!(store.operations[0].command.starts_with("reinstall"));
    }

    #[test]
    fn observed_component_requires_adoption_first() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let ctx = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed_state(
            &ctx,
            vec![delegated_installation("cosh", ManagementRelation::Observed)],
        );
        let host = FakeHost::with_installed(Some(pkg_info("cosh", "2.7.0")));

        let err = reinstall_with_host("cosh", &ctx, &host, &host, true).unwrap_err();

        assert!(matches!(err, CliError::InvalidArgument { .. }));
        assert!(err.to_string().contains("adopt"), "got: {err}");
        assert_eq!(host.reinstall_calls.get(), 0);
    }

    #[test]
    fn absent_component_points_at_install() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let ctx = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed_state(&ctx, Vec::new());
        let host = FakeHost::with_installed(None);

        let err = reinstall_with_host("cosh", &ctx, &host, &host, true).unwrap_err();

        assert!(err.to_string().contains("not installed"), "got: {err}");
    }

    #[test]
    fn externally_removed_package_points_at_repair() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let ctx = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed_state(
            &ctx,
            vec![delegated_installation(
                "cosh",
                ManagementRelation::Managed {
                    since: "2026-07-01T00:00:00Z".to_string(),
                },
            )],
        );
        // rpmdb no longer has the package.
        let host = FakeHost::with_installed(None);

        let err = reinstall_with_host("cosh", &ctx, &host, &host, true).unwrap_err();

        assert!(err.to_string().contains("repair"), "got: {err}");
        assert_eq!(host.reinstall_calls.get(), 0);
    }

    #[test]
    fn dry_run_previews_without_side_effects() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let ctx = ctx(tmp.path().to_path_buf(), InstallMode::System, true);
        let state_path = seed_state(
            &ctx,
            vec![delegated_installation(
                "cosh",
                ManagementRelation::Managed {
                    since: "2026-07-01T00:00:00Z".to_string(),
                },
            )],
        );
        let before = std::fs::read_to_string(&state_path).expect("read state");
        let host = FakeHost::with_installed(Some(pkg_info("cosh", "2.7.0")));

        // Dry-run needs no root: the plan is previewed, nothing executes.
        reinstall_with_host("cosh", &ctx, &host, &host, false).expect("dry-run ok");

        assert_eq!(host.reinstall_calls.get(), 0);
        assert_eq!(
            std::fs::read_to_string(&state_path).expect("re-read state"),
            before,
            "dry-run must not touch state"
        );
    }

    #[test]
    fn execution_requires_root() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let ctx = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed_state(
            &ctx,
            vec![delegated_installation(
                "cosh",
                ManagementRelation::Managed {
                    since: "2026-07-01T00:00:00Z".to_string(),
                },
            )],
        );
        let host = FakeHost::with_installed(Some(pkg_info("cosh", "2.7.0")));

        let err = reinstall_with_host("cosh", &ctx, &host, &host, false).unwrap_err();

        assert!(matches!(err, CliError::PermissionDenied { .. }));
        assert_eq!(host.reinstall_calls.get(), 0);
    }

    #[test]
    fn failed_transaction_is_forward_only_and_suggests_repair() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let ctx = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed_state(
            &ctx,
            vec![delegated_installation(
                "cosh",
                ManagementRelation::Managed {
                    since: "2026-07-01T00:00:00Z".to_string(),
                },
            )],
        );
        let host = FakeHost {
            installed: RefCell::new(Some(pkg_info("cosh", "2.7.0"))),
            reinstall_calls: Cell::new(0),
            reinstall_succeeds: false,
        };

        let err = reinstall_with_host("cosh", &ctx, &host, &host, true).unwrap_err();

        assert!(matches!(err, CliError::Runtime { .. }));
        assert!(err.to_string().contains("repair"), "got: {err}");
        assert_eq!(
            host.reinstall_calls.get(),
            1,
            "exactly one attempt, no retry"
        );
    }

    /// Fixture for the owned-replay tests: a local `file://` repo publishing
    /// `skillfs 1.0.0`, a repo.toml pointing at it, an "old" owned file on
    /// disk, and a v5 owned record claiming that file.
    fn seed_owned_replay(tmp: &Path, ctx: &CliContext) -> (PathBuf, PathBuf, PathBuf) {
        use crate::commands::tier1::install::tests::write_local_repo_component;

        let layout = common::resolve_layout(ctx);
        let repo_root = tmp.join("repo");
        let base_url = write_local_repo_component(&repo_root, "skillfs", "1.0.0", &["system"]);
        std::fs::create_dir_all(&layout.etc_dir).expect("etc dir");
        std::fs::write(
            layout.etc_dir.join("repo.toml"),
            format!(
                "schema_version = 1\ndefault_backend = \"raw\"\n\n[backends.raw]\nbase_url = \"{base_url}\"\n"
            ),
        )
        .expect("write repo.toml");

        let old_binary = layout.bin_dir.join("skillfs");
        std::fs::create_dir_all(&layout.bin_dir).expect("bindir");
        std::fs::write(&old_binary, "old payload\n").expect("seed old file");

        let owned = Installation {
            binding: ProviderBinding::Owned {
                artifact: OwnedArtifact {
                    version: "1.0.0".to_string(),
                    distribution_source: None,
                    raw_package: Some("skillfs".to_string()),
                    manifest_digest: None,
                    files: vec![OwnedFile {
                        path: old_binary.clone(),
                        owner: FileOwner::Anolisa,
                        sha256: None,
                        kind: OwnedFileKind::File,
                        referent: None,
                    }],
                    services: Vec::new(),
                    external_modified_files: Vec::new(),
                    provisioned_packages: vec!["libfoo".to_string()],
                },
            },
            ..delegated_installation("skillfs", ManagementRelation::Observed)
        };
        let state_path = seed_state(ctx, vec![owned]);
        (state_path, old_binary, repo_root)
    }

    #[test]
    fn owned_component_replays_files_end_to_end() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let ctx = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let (state_path, binary, _repo_root) = seed_owned_replay(tmp.path(), &ctx);
        let host = FakeHost::with_installed(None);

        reinstall_with_host("skillfs", &ctx, &host, &host, true).expect("owned replay ok");

        // The artifact's payload replaced the old bytes on disk.
        let placed = std::fs::read_to_string(&binary).expect("read replayed binary");
        assert!(placed.contains("echo skillfs"), "got: {placed}");

        // The record was rewritten from this run's execution state: placed
        // file + manifest snapshot, with the provisioning history preserved.
        let store = StateStore::load(&state_path, 0).expect("reload");
        let record = store
            .find(ObjectKind::Component, "skillfs")
            .expect("record");
        let artifact = match &record.binding {
            ProviderBinding::Owned { artifact } => artifact,
            other => panic!("expected owned binding, got {other:?}"),
        };
        assert_eq!(artifact.version, "1.0.0");
        assert_eq!(artifact.raw_package.as_deref(), Some("skillfs"));
        assert_eq!(artifact.files.len(), 2, "binary + manifest snapshot");
        let placed_row = artifact
            .files
            .iter()
            .find(|f| f.path == binary)
            .expect("binary row");
        assert!(placed_row.sha256.is_some(), "replay records fresh digests");
        assert_eq!(artifact.provisioned_packages, vec!["libfoo".to_string()]);
        assert_eq!(store.operations.len(), 1);
        assert!(store.operations[0].command.starts_with("reinstall"));

        // The per-operation backup scratch was discarded after commit.
        let layout = common::resolve_layout(&ctx);
        let leftover = std::fs::read_dir(&layout.backup_dir)
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(leftover, 0, "backup scratch must not survive a commit");
    }

    #[test]
    fn owned_replay_download_failure_restores_the_previous_files() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let ctx = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let (state_path, binary, repo_root) = seed_owned_replay(tmp.path(), &ctx);
        // The index still lists 1.0.0 but the artifact bytes are gone, so
        // download-verify fails after the backup step already ran.
        std::fs::remove_file(repo_root.join("v1").join("skillfs.tar.gz")).expect("remove artifact");
        let host = FakeHost::with_installed(None);

        let err = reinstall_with_host("skillfs", &ctx, &host, &host, true).unwrap_err();

        assert!(
            err.to_string().contains("previous files were restored"),
            "got: {err}"
        );
        // The old bytes survived and the record is untouched.
        assert_eq!(
            std::fs::read_to_string(&binary).expect("re-read binary"),
            "old payload\n"
        );
        let store = StateStore::load(&state_path, 0).expect("reload");
        let record = store
            .find(ObjectKind::Component, "skillfs")
            .expect("record");
        match &record.binding {
            ProviderBinding::Owned { artifact } => {
                assert_eq!(artifact.files.len(), 1, "record still lists the old file");
            }
            other => panic!("expected owned binding, got {other:?}"),
        }
        assert!(store.operations.is_empty(), "no ok history for a failure");
    }
}
