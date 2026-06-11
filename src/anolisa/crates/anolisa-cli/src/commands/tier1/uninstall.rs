//! `anolisa uninstall <COMPONENT>` (with optional `--purge`).
//!
//! The CLI face of [`anolisa_core::execute_plan`] for direct component
//! teardown. The legacy capability path is retained as a fallback while
//! the capability concept is being removed. Two surfaces:
//!
//!   * `--dry-run` — render the [`LifecyclePlan`] (human or JSON) and
//!     return without touching the filesystem.
//!   * default — execute the plan: only ANOLISA-owned files are
//!     removed; external residue is preserved and surfaced as
//!     warnings.
//!
//! `--purge` widens the scope from "uninstall" to "uninstall + drop
//! ANOLISA-owned config/cache/state fragments". External modifications
//! are always refused regardless of `--purge`.
//!
//! `--force` is parsed today as a wire stub (the spec calls it out)
//! but the executor does not yet branch on it. We surface a warning so
//! users see the boundary instead of getting silent semantics.
//!
//! Error routing mirrors `disable`:
//!
//! | `LifecycleError`            | CLI code           | exit |
//! |-----------------------------|--------------------|------|
//! | `CapabilityNotInstalled`    | `INVALID_ARGUMENT` | 2    |
//! | `UnsupportedOperation`      | `EXECUTION_FAILED` | 1    |
//! | `LockHeld`                  | `EXECUTION_FAILED` | 1    |
//! | `Lock`                      | `EXECUTION_FAILED` | 1    |
//! | `State`                     | `EXECUTION_FAILED` | 1    |
//! | `Log`                       | `EXECUTION_FAILED` | 1    |
//! | `Filesystem`                | `EXECUTION_FAILED` | 1    |
//! | `ExecuteGated`              | `NOT_IMPLEMENTED`  | 64   |
//!
//! The `ExecuteGated` mapping follows the prior art `disable.rs` uses
//! for `--feature` / `--purge`: when a CLI surface is wire-shipped but
//! the executor refuses to perform a destructive operation, the right
//! bucket is `NOT_IMPLEMENTED` (exit 64) — same convention as the
//! enable-policy and disable-flag gates. The gate itself lives in
//! `anolisa-core::lifecycle::check_destructive_execute_gate`; see the
//! docstring there for the P1-D lift conditions.

use clap::Parser;

use anolisa_core::{
    CapabilityManifestsView, LifecycleError, LifecycleOperation, LifecycleOutcome, LifecyclePlan,
    LifecycleTargetKind, ObjectKind, execute_plan,
};

use crate::color::Palette;
use crate::commands::common;
use crate::context::CliContext;
use crate::response::{CliError, render_json};

const COMMAND: &str = "uninstall";

#[derive(Parser)]
pub struct UninstallArgs {
    /// Component to uninstall; legacy capability names are accepted as fallback
    #[arg(value_name = "COMPONENT")]
    pub component: String,
    /// Also remove ANOLISA-owned config / cache / state fragments
    #[arg(long)]
    pub purge: bool,
    /// Reserved for forcing through warnings (spec only, no behavior change yet)
    #[arg(long)]
    pub force: bool,
}

pub fn handle(args: UninstallArgs, ctx: &CliContext) -> Result<(), CliError> {
    let operation = if args.purge {
        LifecycleOperation::Purge
    } else {
        LifecycleOperation::Uninstall
    };
    let target = args.component.as_str();
    let command = format!("{} {}", operation.as_str(), target);

    // Load installed state to plan against. Missing state is the same
    // as "target not installed" — surface that as INVALID_ARGUMENT so
    // the user sees the right exit code (matches disable behavior).
    let installed = common::load_installed_state(ctx, COMMAND)?;
    let target_kind = if installed
        .find_object(ObjectKind::Component, target)
        .is_some()
        || installed
            .find_object(ObjectKind::Capability, target)
            .is_none()
    {
        LifecycleTargetKind::Component
    } else {
        LifecycleTargetKind::Capability
    };
    let plan = match operation {
        LifecycleOperation::Uninstall => match target_kind {
            LifecycleTargetKind::Component => {
                LifecyclePlan::for_component_uninstall(target, &installed)
            }
            LifecycleTargetKind::Capability => {
                LifecyclePlan::for_uninstall(target, &installed, &CapabilityManifestsView::empty())
            }
        },
        LifecycleOperation::Purge => match target_kind {
            LifecycleTargetKind::Component => {
                LifecyclePlan::for_component_purge(target, &installed)
            }
            LifecycleTargetKind::Capability => {
                LifecyclePlan::for_purge(target, &installed, &CapabilityManifestsView::empty())
            }
        },
        // Disable goes through `anolisa disable`; the executor refuses
        // here but this branch keeps the match total without leaking
        // the surface.
        LifecycleOperation::Disable => unreachable!("uninstall handler never builds Disable plans"),
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

    if args.force {
        // Wire-only stub; flag it so users do not assume any
        // behavioral change.
        eprintln!("warning: --force is a spec stub today and has no behavioral effect yet");
    }

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

    let outcome = execute_plan(&plan, &layout, &actor, install_mode)
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
        LifecycleError::CapabilityNotInstalled { capability } => CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "capability '{capability}' is not installed — nothing to uninstall (run `anolisa status` to see what is installed)",
            ),
        },
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

#[derive(serde::Serialize)]
struct UninstallPayload {
    operation_id: String,
    operation: String,
    capability: String,
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
            capability: o.capability.clone(),
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
        plan.capability,
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
        outcome.capability,
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
    use std::path::PathBuf;
    use tempfile::tempdir;

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

    /// Asking to uninstall a capability that is not installed must
    /// surface `INVALID_ARGUMENT` (exit 2), not `EXECUTION_FAILED`.
    /// Symmetric with `disable_unknown_capability_*` so wrapping
    /// scripts can use the same routing for both verbs.
    #[test]
    fn uninstall_unknown_capability_routes_to_invalid_argument_exit_2() {
        let tmp = tempdir().expect("tmpdir");
        let err = handle(
            args("agent-observability", false),
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

    /// Dry-run path must not touch the filesystem even when the
    /// capability is not installed: the planner builds an empty plan
    /// and we return Ok(()). This is the same contract as
    /// `enable --dry-run` for absent capabilities once the policy gate
    /// is open.
    #[test]
    fn uninstall_dry_run_on_unknown_capability_returns_empty_plan() {
        let tmp = tempdir().expect("tmpdir");
        let result = handle(
            args("agent-observability", false),
            &ctx_with_prefix(
                false,
                true,
                InstallMode::System,
                Some(tmp.path().to_path_buf()),
            ),
        );
        result.expect("dry-run must produce a plan even for absent capabilities");
    }

    /// `lifecycle_err_to_cli` routing pin: every LifecycleError variant
    /// must surface as the documented CLI code. We test the buckets
    /// that route to EXECUTION_FAILED here so a future refactor cannot
    /// silently downgrade one to INVALID_ARGUMENT.
    #[test]
    fn lifecycle_err_lock_held_maps_to_execution_failed_exit_1() {
        let err = lifecycle_err_to_cli(
            "uninstall agent-observability",
            LifecycleError::LockHeld {
                path: PathBuf::from("/var/lib/anolisa/lock"),
            },
        );
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert_eq!(err.exit_code(), 1);
        assert!(err.reason().contains("/var/lib/anolisa/lock"));
    }

    #[test]
    fn lifecycle_err_capability_not_installed_maps_to_invalid_argument_exit_2() {
        let err = lifecycle_err_to_cli(
            "uninstall agent-observability",
            LifecycleError::CapabilityNotInstalled {
                capability: "agent-observability".to_string(),
            },
        );
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert_eq!(err.exit_code(), 2);
    }

    /// P1-C plan-only gate: `LifecycleError::ExecuteGated` must surface
    /// as `NOT_IMPLEMENTED` (exit 64) so wrapping scripts see the same
    /// boundary semantics as `disable --feature` / `disable --purge`.
    /// The gate's reason text MUST be plumbed through to the CLI hint
    /// so users can see why the executor refused and that `--dry-run`
    /// is the supported alternative.
    #[test]
    fn lifecycle_err_execute_gated_maps_to_not_implemented_exit_64() {
        let err = lifecycle_err_to_cli(
            "uninstall agent-observability",
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

    /// End-to-end success: when the capability IS installed, the
    /// executor must remove the ANOLISA-owned file, drop the capability
    /// + component objects from `installed.toml`, write started +
    ///   succeeded central-log entries, and return `Ok(())`. Pins the
    ///   "uninstall execute on installed capability succeeds" contract
    ///   that replaced the previous plan-only gate (P0-D).
    #[test]
    fn uninstall_execute_on_installed_capability_removes_owned_files_and_succeeds() {
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
            install_backend: Some("raw".to_string()),
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
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Capability,
            name: "agent-observability".to_string(),
            version: "stable".to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: None,
            distribution_source: None,
            install_backend: None,
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: Some("op-prior".to_string()),
            managed: true,
            adopted: false,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            component_refs: vec!["agentsight".to_string()],
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        });
        let state_path = layout.state_dir.join("installed.toml");
        state.save(&state_path).expect("seed state save");

        handle(
            args("agent-observability", false),
            &ctx_with_prefix(
                false,
                false,
                InstallMode::System,
                Some(tmp.path().to_path_buf()),
            ),
        )
        .expect("uninstall execute must succeed");

        assert!(
            !owned.exists(),
            "ANOLISA-owned file must be removed by uninstall execute",
        );

        let after = InstalledState::load(&state_path).expect("reload state");
        assert!(
            after
                .find_object(ObjectKind::Capability, "agent-observability")
                .is_none(),
            "capability object must be dropped from installed.toml",
        );
        assert!(
            after
                .find_object(ObjectKind::Component, "agentsight")
                .is_none(),
            "non-shared component object must be dropped from installed.toml",
        );
        assert!(
            layout.central_log.exists(),
            "uninstall execute must append a central-log record",
        );
    }

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
            install_backend: Some("raw".to_string()),
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
            install_backend: Some("raw".to_string()),
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
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Capability,
            name: "agent-observability".to_string(),
            version: "stable".to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: None,
            distribution_source: None,
            install_backend: None,
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: Some("op-prior".to_string()),
            managed: true,
            adopted: false,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            component_refs: vec!["agentsight".to_string()],
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        });
        let state_path = layout.state_dir.join("installed.toml");
        state.save(&state_path).expect("seed state save");
        let prior_bytes = std::fs::read(&state_path).expect("read prior");

        let err = handle(
            args("agent-observability", true),
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
}
