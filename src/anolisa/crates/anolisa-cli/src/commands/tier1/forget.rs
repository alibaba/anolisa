//! `anolisa forget <component>` — drop a component's ANOLISA state record
//! without touching the underlying package or files.
//!
//! `forget` is the escape hatch for stale state: after a manual `rpm -e` (the
//! `missing` case from `anolisa status`), or whenever the operator wants ANOLISA
//! to stop tracking a component, `forget` removes the state record and records
//! the operation. It also resolves quarantined records — legacy state the
//! migration refused to classify — when the operator decides they are not
//! worth repairing. It performs **no** package operation — no `dnf remove`,
//! no `rpm -e` — and leaves package/component files on disk. An
//! observed/managed RPM stays installed in rpmdb; an owned component's files
//! stay on disk (use `anolisa uninstall` to remove those).

use chrono::{SecondsFormat, Utc};
use clap::Parser;
use serde::Serialize;

use anolisa_core::central_log::{CentralLog, LogKind, LogRecord, LogStatus, Severity};
use anolisa_core::domain::ProviderBinding;
use anolisa_core::facts::{JournalEvidence, pending_journal_for};
use anolisa_core::lock::InstallLock;
use anolisa_core::state::{ObjectKind, OperationRecord};
use anolisa_core::state_store::StateStore;
use anolisa_platform::privilege;

use crate::color::Palette;
use crate::commands::common;
use crate::commands::tier1::rpm_install;
use crate::context::CliContext;
use crate::response::{CliError, render_json};

/// Command label for JSON envelopes and error routing.
const COMMAND: &str = "forget";

/// Arguments for `anolisa forget <component>`.
#[derive(Debug, Parser)]
pub struct ForgetArgs {
    /// Component whose ANOLISA state record should be dropped
    #[arg(value_name = "COMPONENT")]
    pub component: String,
}

/// Wire shape for a `forget <component>` result (`--json`) and its dry-run
/// preview.
#[derive(Serialize)]
struct ForgetPayload {
    component: String,
    /// Provenance of the dropped record, for the audit trail:
    /// `owned` | `managed` | `adopted` | `observed` | `quarantined`.
    provenance: &'static str,
    install_mode: String,
    /// Whether the state record was actually removed (false on dry-run).
    forgotten: bool,
    dry_run: bool,
    /// `None` on dry-run (nothing recorded).
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
}

/// Dispatch `forget <component>`: drop the ANOLISA state record, run no package
/// operation.
///
/// # Errors
///
/// Returns [`CliError`] when the component is absent, still has enabled adapter
/// receipts, or the state write fails.
pub fn handle(args: ForgetArgs, ctx: &CliContext) -> Result<(), CliError> {
    let input = args.component.as_str();
    let command = format!("forget {input}");
    let layout = common::resolve_layout(ctx);
    let (resolved, view) = common::resolve_mutation_target(input, ctx, &command)?;
    let store = view.writable.state;
    let target = resolved.as_str();

    // Forget also resolves quarantined records: it is the documented exit for
    // legacy state the migration refused to classify and repair cannot
    // recover.
    let provenance = record_provenance(&store, target);
    let provenance = provenance.ok_or_else(|| CliError::InvalidArgument {
        command: command.clone(),
        reason: format!(
            "component '{target}' is not installed — nothing to forget (run `anolisa status` to see what is tracked)"
        ),
    })?;
    let journal_dir = rpm_install::journal_dir(&layout);
    ensure_no_pending_journal(
        JournalEvidence::new(&journal_dir, &store.operations),
        target,
        &command,
    )?;

    // Adapter receipts must be released before the component is dropped:
    // silently orphaning a registered plugin is worse than refusing. This guard
    // is a fast-fail and the dry-run preview; `persist_forget` re-checks
    // authoritatively under the lock. Mirrors `uninstall`, pointing at
    // `adapter disable`.
    if !ctx.dry_run {
        ensure_no_adapter_claims(&store, target, &command)?;
    }

    if ctx.dry_run {
        let payload = ForgetPayload {
            component: target.to_string(),
            provenance,
            install_mode: ctx.install_mode.as_str().to_string(),
            forgotten: false,
            dry_run: true,
            operation_id: None,
        };
        render_forget(ctx, &payload);
        return Ok(());
    }

    let (operation_id, provenance) = persist_forget(ctx, target, &command)?;
    let payload = ForgetPayload {
        component: target.to_string(),
        provenance,
        install_mode: ctx.install_mode.as_str().to_string(),
        forgotten: true,
        dry_run: false,
        operation_id: Some(operation_id),
    };
    render_forget(ctx, &payload);
    Ok(())
}

/// Provenance label for the record `forget` would drop — active or
/// quarantined — or `None` when nothing is tracked under this name.
fn record_provenance(store: &StateStore, component: &str) -> Option<&'static str> {
    if let Some(installation) = store.find(ObjectKind::Component, component) {
        return Some(match &installation.binding {
            ProviderBinding::Owned { .. } => "owned",
            ProviderBinding::Delegated { relation, .. } => relation.label(),
        });
    }
    store
        .quarantined
        .iter()
        .any(|q| q.record.kind == ObjectKind::Component && q.record.name == component)
        .then_some("quarantined")
}

/// Refuse to drop a component that still has enabled adapter receipts.
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
            "'{target}' has enabled adapters ({}); run `anolisa adapter disable {target}` for each framework before forgetting",
            frameworks.join(", ")
        ),
    })
}

/// Remove the component's state record and local manifest snapshot under the
/// install lock, then append an audit record. No package/component files are
/// removed. Returns the operation id and the provenance the lock observed.
fn persist_forget(
    ctx: &CliContext,
    component: &str,
    command: &str,
) -> Result<(String, &'static str), CliError> {
    let layout = common::resolve_layout(ctx);
    let state_path = layout.state_dir.join("installed.toml");
    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;
    let mut store = StateStore::load_for_layout(&state_path, privilege::effective_uid(), &layout)
        .map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to load installed state: {err}"),
    })?;

    // The planner treats pending recovery as a global precondition for every
    // lifecycle mutation except repair. Forget bypasses the planner, so it
    // must enforce the same rule under the authoritative lock; otherwise the
    // surviving journal could later recreate the record just removed here.
    let journal_dir = rpm_install::journal_dir(&layout);
    ensure_no_pending_journal(
        JournalEvidence::new(&journal_dir, &store.operations),
        component,
        command,
    )?;

    // Authoritative adapter-claim guard, under the lock. The check in `handle`
    // is only a fast-fail / dry-run preview: a concurrent `adapter enable`
    // landing between that read and this removal would otherwise orphan its
    // receipt once the component record is gone. Re-checking the freshly
    // reloaded state here closes that window.
    ensure_no_adapter_claims(&store, component, command)?;

    // Re-validate record presence under the lock (a concurrent
    // uninstall/forget may have dropped it), and report the provenance the
    // lock actually observed rather than the pre-lock read.
    let provenance = record_provenance(&store, component).ok_or_else(|| CliError::Runtime {
        command: command.to_string(),
        reason: format!(
            "component '{component}' disappeared from state during forget; nothing removed"
        ),
    })?;
    store.remove(ObjectKind::Component, component);
    remove_component_manifest_snapshot(&layout, component, command)?;

    let now = now_iso8601();
    let lock_ts = Utc::now();
    let operation_id = format!(
        "op-forget-{}-{}",
        lock_ts.format("%Y%m%d%H%M%S"),
        lock_ts.timestamp_subsec_nanos()
    );
    store.operations.push(OperationRecord {
        id: operation_id.clone(),
        command: command.to_string(),
        status: "ok".to_string(),
        started_at: now.clone(),
        finished_at: Some(now.clone()),
        parent_operation_id: None,
    });

    store.save(&state_path).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to save state: {err}"),
    })?;

    // Audit log is best-effort: the state already persisted, so a log failure
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
            "forgot ANOLISA state for component {component}; no package operation performed"
        ),
        actor: "cli".to_string(),
        install_mode: Some(ctx.install_mode.as_str().to_string()),
        started_at: now.clone(),
        finished_at: Some(now),
        status: Some(LogStatus::Ok),
        objects: vec![component.to_string()],
        backup_ids: Vec::new(),
        warnings: Vec::new(),
        details: serde_json::Value::Null,
    };
    if let Err(err) = log.append(&record) {
        eprintln!("warning: failed to write central log: {err}");
    }
    Ok((operation_id, provenance))
}

fn ensure_no_pending_journal(
    evidence: JournalEvidence<'_>,
    component: &str,
    command: &str,
) -> Result<(), CliError> {
    let pending = pending_journal_for(evidence, component).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to inspect operation journals: {err}"),
    })?;
    if let Some(path) = pending {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "component '{component}' has a pending operation journal at {}; run `anolisa repair {component}` before forgetting its state",
                path.display()
            ),
        });
    }
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

/// Human/JSON renderer for a forget result.
fn render_forget(ctx: &CliContext, payload: &ForgetPayload) {
    if ctx.json {
        // Errors here are unreachable for a plain Serialize struct; ignore the
        // Result so an (already-persisted) forget is not reported as failed.
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
            color.command("forget"),
            payload.component,
            color.muted(format!("({})", payload.provenance)),
            color.muted("(dry-run — ANOLISA state not modified)"),
        );
        println!(
            "  {}",
            color.muted("no package operation would be performed")
        );
        return;
    }
    println!(
        "{} {} {}",
        color.ok("✓ forgot"),
        payload.component,
        color.muted(format!("({})", payload.provenance)),
    );
    println!(
        "    {} ANOLISA stopped tracking this component; no package operation was performed",
        color.label("note:"),
    );
    // Tailor the residue reminder to what forget deliberately left behind.
    match payload.provenance {
        "owned" => println!(
            "    {} ANOLISA-owned files remain on disk; forget dropped their inventory, so 'anolisa uninstall' can no longer remove them — delete them manually (next time, run 'anolisa uninstall' instead of 'forget' when you want ANOLISA to remove files)",
            color.label("note:"),
        ),
        "quarantined" => println!(
            "    {} whatever backed the quarantined record — files or a package — remains on the system untouched",
            color.label("note:"),
        ),
        _ => println!(
            "    {} the RPM package remains installed; use dnf/rpm directly if you want to remove it",
            color.label("note:"),
        ),
    }
}

/// RFC3339 UTC timestamp, seconds precision (matches the install/update paths).
fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::PathBuf;

    use anolisa_core::adapter::claim::{AdapterClaim, ClaimStatus, DriverPayload, OpenClawClaim};
    use anolisa_core::state::{
        InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectStatus, Ownership,
        RpmMetadata,
    };
    use anolisa_core::transaction::Transaction;

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

    /// An adopted rpm-observed component object (legacy v4 shape; loading it
    /// exercises the migration into the v5 store).
    fn rpm_observed_object(component: &str, package: &str, evr: &str) -> InstalledObject {
        InstalledObject {
            kind: ObjectKind::Component,
            name: component.to_string(),
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
                arch: Some("x86_64".to_string()),
                source_repo: Some("@System".to_string()),
            }),
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: Some("op-prior".to_string()),
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

    /// A legacy object with no classifiable evidence: no backend, no
    /// ownership, no rpm metadata, no source, no files. The migration
    /// quarantines it (rule R4h).
    fn unclassifiable_object(component: &str) -> InstalledObject {
        InstalledObject {
            kind: ObjectKind::Component,
            name: component.to_string(),
            version: "0.0.1".to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: None,
            distribution_source: None,
            raw_package: None,
            install_backend: None,
            ownership: None,
            rpm_metadata: None,
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: None,
            managed: false,
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

    fn sample_claim(component: &str, framework: &str) -> AdapterClaim {
        AdapterClaim {
            claim_schema: 1,
            component: component.to_string(),
            framework: framework.to_string(),
            plugin_id: None,
            adapter_type: None,
            enabled_at: "2026-06-01T10:00:00Z".to_string(),
            resource_root: PathBuf::from("/tmp/anolisa-forget-test"),
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
            install_mode: match ctx.install_mode {
                InstallMode::System => StateInstallMode::System,
                InstallMode::User => StateInstallMode::User,
            },
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

    fn load_store(ctx: &CliContext) -> StateStore {
        let layout = common::resolve_layout(ctx);
        StateStore::load(&layout.state_dir.join("installed.toml"), 0).expect("load store")
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

    /// forget drops the state record and records the operation; no package
    /// operation is involved (there is no package query/transaction at all).
    #[test]
    fn forget_drops_object_and_records_operation() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            vec![rpm_observed_object(
                "copilot-shell",
                "copilot-shell",
                "2.2.0-1.al8",
            )],
            Vec::new(),
        );
        let snapshot_dir = seed_manifest_snapshot(&c, "copilot-shell");

        handle(
            ForgetArgs {
                component: "copilot-shell".to_string(),
            },
            &c,
        )
        .expect("forget ok");

        let after = load_store(&c);
        assert!(
            after.find(ObjectKind::Component, "copilot-shell").is_none(),
            "state record must be dropped",
        );
        assert!(
            after
                .operations
                .iter()
                .any(|o| o.command == "forget copilot-shell"),
            "an operation record must be appended",
        );
        assert!(
            !snapshot_dir.exists(),
            "component manifest snapshot dir must be removed",
        );
    }

    /// forget is the documented exit for quarantined records: it drops the
    /// quarantine entry like any other record.
    #[test]
    fn forget_drops_quarantined_record() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(&c, vec![unclassifiable_object("mystery")], Vec::new());
        // Sanity: the migration must have quarantined the seed.
        assert!(
            load_store(&c)
                .quarantined
                .iter()
                .any(|q| q.record.name == "mystery"),
            "seed must migrate into quarantine",
        );

        handle(
            ForgetArgs {
                component: "mystery".to_string(),
            },
            &c,
        )
        .expect("forget of a quarantined record ok");

        let after = load_store(&c);
        assert!(
            after.quarantined.iter().all(|q| q.record.name != "mystery"),
            "quarantined record must be dropped",
        );
        assert!(
            after
                .operations
                .iter()
                .any(|o| o.command == "forget mystery"),
            "an operation record must be appended",
        );
    }

    /// Forgetting an absent component routes to INVALID_ARGUMENT (exit 2).
    #[test]
    fn forget_unknown_component_routes_to_invalid_argument() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let err = handle(
            ForgetArgs {
                component: "ghost".to_string(),
            },
            &c,
        )
        .expect_err("absent component must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert_eq!(err.exit_code(), 2);
        assert!(err.reason().contains("not installed"));
    }

    /// A component with an adapter receipt is refused until the adapter is
    /// disabled — forget must not silently orphan a registered plugin.
    #[test]
    fn forget_refuses_with_enabled_adapter_claim() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            vec![rpm_observed_object(
                "copilot-shell",
                "copilot-shell",
                "2.2.0-1.al8",
            )],
            vec![sample_claim("copilot-shell", "openclaw")],
        );
        let err = handle(
            ForgetArgs {
                component: "copilot-shell".to_string(),
            },
            &c,
        )
        .expect_err("enabled adapter must block forget");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("adapter disable"),
            "reason must point at adapter disable: {}",
            err.reason()
        );
        // The component must still be present — forget refused.
        assert!(
            load_store(&c)
                .find(ObjectKind::Component, "copilot-shell")
                .is_some(),
        );
    }

    /// `persist_forget` enforces the adapter-claim guard under the lock, not only
    /// in `handle`. Calling it directly — bypassing the pre-lock fast-fail, as a
    /// concurrent `adapter enable` effectively would — on a state that already
    /// holds a claim must refuse and leave the record intact. This is what closes
    /// the enable-during-forget race; a regression that drops the locked check
    /// fails here while the `handle`-level test above would still pass.
    #[test]
    fn persist_forget_rechecks_adapter_claim_under_lock() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            vec![rpm_observed_object(
                "copilot-shell",
                "copilot-shell",
                "2.2.0-1.al8",
            )],
            vec![sample_claim("copilot-shell", "openclaw")],
        );
        let err = persist_forget(&c, "copilot-shell", "forget copilot-shell")
            .expect_err("locked claim check must refuse");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("adapter disable"),
            "reason must point at adapter disable: {}",
            err.reason()
        );
        assert!(
            load_store(&c)
                .find(ObjectKind::Component, "copilot-shell")
                .is_some(),
            "record must remain when the locked claim check refuses",
        );
    }

    #[test]
    fn pending_journal_blocks_forget_without_dropping_state_or_snapshot() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            vec![rpm_observed_object(
                "copilot-shell",
                "copilot-shell",
                "2.2.0-1.al8",
            )],
            Vec::new(),
        );
        let snapshot_dir = seed_manifest_snapshot(&c, "copilot-shell");
        let layout = common::resolve_layout(&c);
        let journal_dir = rpm_install::journal_dir(&layout);
        let journal = Transaction::begin_with_subject(
            "update",
            Some("copilot-shell"),
            layout.state_dir.join("installed.toml"),
            &journal_dir,
        )
        .expect("pending journal");

        let err = persist_forget(&c, "copilot-shell", "forget copilot-shell")
            .expect_err("locked pending recovery check must block forget");

        assert!(err.reason().contains("anolisa repair copilot-shell"));
        assert!(
            load_store(&c)
                .find(ObjectKind::Component, "copilot-shell")
                .is_some(),
            "record must remain",
        );
        assert!(snapshot_dir.exists(), "snapshot must remain");
        assert!(
            Transaction::load_journal(&journal.journal_path)
                .expect("reload journal")
                .is_pending(),
            "forget must not settle the journal",
        );
    }

    /// Dry-run leaves the state record in place.
    #[test]
    fn forget_dry_run_leaves_state_untouched() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, true);
        seed(
            &c,
            vec![rpm_observed_object(
                "copilot-shell",
                "copilot-shell",
                "2.2.0-1.al8",
            )],
            Vec::new(),
        );
        let snapshot_dir = seed_manifest_snapshot(&c, "copilot-shell");
        handle(
            ForgetArgs {
                component: "copilot-shell".to_string(),
            },
            &c,
        )
        .expect("dry-run ok");
        assert!(
            load_store(&c)
                .find(ObjectKind::Component, "copilot-shell")
                .is_some(),
            "dry-run must not remove the state record",
        );
        assert!(
            snapshot_dir.exists(),
            "dry-run must not remove the manifest snapshot dir",
        );
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

    /// CLI surface: `forget <component>` parses to the positional.
    #[test]
    fn forget_parses_positional_component() {
        use clap::Parser as _;
        let a = ForgetArgs::try_parse_from(["forget", "copilot-shell"]).expect("parse");
        assert_eq!(a.component, "copilot-shell");
    }

    /// Forget by package alias (e.g., "copilot-shell") must resolve to the
    /// canonical component name ("cosh") before addressing state.
    #[test]
    fn forget_via_package_alias_succeeds() {
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
            vec![rpm_observed_object("cosh", "copilot-shell", "2.2.0-1.al8")],
            Vec::new(),
        );
        let _snapshot_dir = seed_manifest_snapshot(&c, "cosh");

        handle(
            ForgetArgs {
                component: "copilot-shell".to_string(),
            },
            &c,
        )
        .expect("forget via alias");

        let after = load_store(&c);
        assert!(
            after.find(ObjectKind::Component, "cosh").is_none(),
            "state record for 'cosh' must be dropped",
        );
    }

    #[test]
    fn quarantined_exact_name_wins_over_repo_alias() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed_component_index(
            &c,
            r#"
schema_version = 1

[[components]]
name = "cosh"

[[components.aliases]]
kind = "rpm-package"
name = "legacy-name"
"#,
        );
        seed(
            &c,
            vec![
                unclassifiable_object("legacy-name"),
                rpm_observed_object("cosh", "copilot-shell", "2.2.0-1.al8"),
            ],
            Vec::new(),
        );

        handle(
            ForgetArgs {
                component: "legacy-name".to_string(),
            },
            &c,
        )
        .expect("forget exact quarantine");

        let after = load_store(&c);
        assert!(
            after
                .quarantined
                .iter()
                .all(|entry| entry.record.name != "legacy-name")
        );
        assert!(
            after.find(ObjectKind::Component, "cosh").is_some(),
            "the alias target must not be forgotten",
        );
    }
}
