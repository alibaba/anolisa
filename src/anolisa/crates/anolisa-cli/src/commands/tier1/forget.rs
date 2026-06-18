//! `anolisa forget <component>` — drop a component's ANOLISA state record
//! without touching the underlying package or files.
//!
//! `forget` is the escape hatch for stale state: after a manual `rpm -e` (the
//! `missing` case from `anolisa status`), or whenever the operator wants ANOLISA
//! to stop tracking a component, `forget` removes the `installed.toml` object
//! and records the operation. It performs **no** package operation — no
//! `dnf remove`, no `rpm -e` — and deletes no files. An observed/managed RPM
//! stays installed in rpmdb; a raw component's owned files stay on disk (use
//! `anolisa uninstall` to remove those).

use chrono::{SecondsFormat, Utc};
use clap::Parser;
use serde::Serialize;

use anolisa_core::central_log::{CentralLog, LogKind, LogRecord, LogStatus, Severity};
use anolisa_core::lock::InstallLock;
use anolisa_core::state::{ObjectKind, OperationRecord};

use crate::color::Palette;
use crate::commands::common;
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
    /// Provenance label of the dropped record, for the audit trail.
    ownership: &'static str,
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
    let target = args.component.as_str();
    let command = format!("forget {target}");
    let installed = common::load_installed_state(ctx, COMMAND)?;

    let obj = installed
        .find_object(ObjectKind::Component, target)
        .ok_or_else(|| CliError::InvalidArgument {
            command: command.clone(),
            reason: format!(
                "component '{target}' is not installed — nothing to forget (run `anolisa status` to see what is tracked)"
            ),
        })?;
    let ownership_label = obj.effective_ownership().label();

    // Adapter receipts must be released before the component is dropped:
    // silently orphaning a registered plugin is worse than refusing. This guard
    // is a fast-fail and the dry-run preview; `persist_forget` re-checks
    // authoritatively under the lock. Mirrors `uninstall`, pointing at
    // `adapter disable`.
    if !ctx.dry_run {
        let claims = installed.adapter_claims_for_component(target);
        if !claims.is_empty() {
            let mut frameworks: Vec<&str> = claims.iter().map(|c| c.framework.as_str()).collect();
            frameworks.sort_unstable();
            frameworks.dedup();
            return Err(CliError::InvalidArgument {
                command,
                reason: format!(
                    "'{target}' has enabled adapters ({}); run `anolisa adapter disable {target}` for each framework before forgetting",
                    frameworks.join(", ")
                ),
            });
        }
    }

    if ctx.dry_run {
        let payload = ForgetPayload {
            component: target.to_string(),
            ownership: ownership_label,
            install_mode: ctx.install_mode.as_str().to_string(),
            forgotten: false,
            dry_run: true,
            operation_id: None,
        };
        render_forget(ctx, &payload);
        return Ok(());
    }

    let (operation_id, ownership_label) = persist_forget(ctx, target, &command)?;
    let payload = ForgetPayload {
        component: target.to_string(),
        ownership: ownership_label,
        install_mode: ctx.install_mode.as_str().to_string(),
        forgotten: true,
        dry_run: false,
        operation_id: Some(operation_id),
    };
    render_forget(ctx, &payload);
    Ok(())
}

/// Remove the component's state object under the install lock and append an
/// audit record. No package operation, no file deletion. Returns the operation
/// id.
fn persist_forget(
    ctx: &CliContext,
    component: &str,
    command: &str,
) -> Result<(String, &'static str), CliError> {
    let layout = common::resolve_layout(ctx);
    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;
    let mut state = common::load_installed_state(ctx, command)?;

    // Authoritative adapter-claim guard, under the lock. The check in `handle`
    // is only a fast-fail / dry-run preview: a concurrent `adapter enable`
    // landing between that read and this removal would otherwise orphan its
    // receipt once the component object is gone. Re-checking the freshly
    // reloaded state here closes that window.
    let claims = state.adapter_claims_for_component(component);
    if !claims.is_empty() {
        let mut frameworks: Vec<&str> = claims.iter().map(|c| c.framework.as_str()).collect();
        frameworks.sort_unstable();
        frameworks.dedup();
        return Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "'{component}' has enabled adapters ({}); run `anolisa adapter disable {component}` for each framework before forgetting",
                frameworks.join(", ")
            ),
        });
    }

    // Re-validate object presence under the lock (a concurrent uninstall/forget
    // may have dropped it), and take ownership of the removed object so the
    // response reports the provenance the lock actually observed rather than the
    // pre-lock read.
    let removed = state
        .remove_object(ObjectKind::Component, component)
        .ok_or_else(|| CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "component '{component}' disappeared from state during forget; nothing removed"
            ),
        })?;
    let ownership_label = removed.effective_ownership().label();

    let now = now_iso8601();
    let lock_ts = Utc::now();
    let operation_id = format!(
        "op-forget-{}-{}",
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

    Ok((operation_id, ownership_label))
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
            color.muted(format!("({})", payload.ownership)),
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
        color.muted(format!("({})", payload.ownership)),
    );
    println!(
        "    {} ANOLISA stopped tracking this component; no package operation was performed",
        color.label("note:"),
    );
    // Tailor the residue reminder to what forget deliberately left behind.
    if payload.ownership == "raw-managed" {
        println!(
            "    {} ANOLISA-owned files remain on disk; forget dropped their inventory, so 'anolisa uninstall' can no longer remove them — delete them manually (next time, run 'anolisa uninstall' instead of 'forget' when you want ANOLISA to remove files)",
            color.label("note:"),
        );
    } else {
        println!(
            "    {} the RPM package remains installed; use dnf/rpm directly if you want to remove it",
            color.label("note:"),
        );
    }
}

/// RFC3339 UTC timestamp, seconds precision (matches the install/update paths).
fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use anolisa_core::adapter::claim::{AdapterClaim, ClaimStatus, DriverPayload, OpenClawClaim};
    use anolisa_core::state::{
        InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectStatus, Ownership,
        RpmMetadata,
    };

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

    /// An adopted rpm-observed component object.
    fn rpm_observed_object(component: &str, package: &str, evr: &str) -> InstalledObject {
        InstalledObject {
            kind: ObjectKind::Component,
            name: component.to_string(),
            version: evr.to_string(),
            status: ObjectStatus::Adopted,
            manifest_digest: None,
            distribution_source: None,
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
        }
    }

    fn sample_claim(component: &str, framework: &str) -> AdapterClaim {
        AdapterClaim {
            claim_schema: 1,
            component: component.to_string(),
            framework: framework.to_string(),
            plugin_id: None,
            enabled_at: "2026-06-01T10:00:00Z".to_string(),
            resource_root: PathBuf::from("/tmp/anolisa-forget-test"),
            bundle_digest: None,
            driver_schema: 1,
            status: ClaimStatus::Enabled,
            resources: Vec::new(),
            driver_payload: DriverPayload::OpenClaw(OpenClawClaim {
                state_dir_resource: "state".to_string(),
                plugin_resource: "plugin".to_string(),
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

    fn load_state(ctx: &CliContext) -> InstalledState {
        let layout = common::resolve_layout(ctx);
        InstalledState::load(&layout.state_dir.join("installed.toml")).expect("load state")
    }

    /// forget drops the state object and records the operation; no package
    /// operation is involved (there is no package query/transaction at all).
    #[test]
    fn forget_drops_object_and_records_operation() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            vec![rpm_observed_object(
                "copilot-shell",
                "anolisa-copilot-shell",
                "2.2.0-1.al8",
            )],
            Vec::new(),
        );

        handle(
            ForgetArgs {
                component: "copilot-shell".to_string(),
            },
            &c,
        )
        .expect("forget ok");

        let after = load_state(&c);
        assert!(
            after
                .find_object(ObjectKind::Component, "copilot-shell")
                .is_none(),
            "state object must be dropped",
        );
        assert!(
            after
                .operations
                .iter()
                .any(|o| o.command == "forget copilot-shell"),
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
                "anolisa-copilot-shell",
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
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .is_some(),
        );
    }

    /// `persist_forget` enforces the adapter-claim guard under the lock, not only
    /// in `handle`. Calling it directly — bypassing the pre-lock fast-fail, as a
    /// concurrent `adapter enable` effectively would — on a state that already
    /// holds a claim must refuse and leave the object intact. This is what closes
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
                "anolisa-copilot-shell",
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
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .is_some(),
            "object must remain when the locked claim check refuses",
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
                "anolisa-copilot-shell",
                "2.2.0-1.al8",
            )],
            Vec::new(),
        );
        handle(
            ForgetArgs {
                component: "copilot-shell".to_string(),
            },
            &c,
        )
        .expect("dry-run ok");
        assert!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .is_some(),
            "dry-run must not remove the state object",
        );
    }

    /// CLI surface: `forget <component>` parses to the positional.
    #[test]
    fn forget_parses_positional_component() {
        use clap::Parser as _;
        let a = ForgetArgs::try_parse_from(["forget", "copilot-shell"]).expect("parse");
        assert_eq!(a.component, "copilot-shell");
    }
}
