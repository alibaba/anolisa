//! `anolisa adapter` sub-surface: scan, enable, disable, status.
//!
//! Adapters bridge ANOLISA-managed components into agent frameworks
//! (e.g. `tokenless/openclaw`). Component install lays each adapter's
//! resources under `{datadir}/adapters/<component>/<framework>/`; this
//! surface drives the framework side from those resources via the
//! [`AdapterManager`], which owns the install lock, receipts, and the
//! controlled framework-CLI boundary.
//!
//! ## `adapter scan`
//!
//! Read-only: reads installed component manifests, then overlays local
//! resource-directory, framework-detection, and receipt state.
//!
//! ## `adapter enable <component> [<framework>]`
//!
//! Runs the framework driver's enable (e.g. registers an OpenClaw plugin)
//! and writes a receipt. `--dry-run` (global flag) reports the plan
//! without touching framework state. `<framework>` may be omitted when the
//! component ships adapters for exactly one framework.
//!
//! ## `adapter disable <component> [<framework>]`
//!
//! Reverses enable using the receipt only, then removes it. Idempotent:
//! disabling something not enabled is a successful no-op. If cleanup
//! cannot complete, the receipt is kept (marked `cleanup_failed`) and the
//! command exits degraded so the state is not silently lost.
//!
//! ## `adapter status [<component>]`
//!
//! Read-only: reports each receipt's health summary and the individual
//! conditions behind it. Verification that cannot run reports `unknown`
//! rather than a faked healthy/absent verdict.

use clap::{Parser, Subcommand};
use serde::Serialize;

use anolisa_core::adapter::AdapterError;
use anolisa_core::adapter::claim::{AdapterClaim, ClaimStatus};
use anolisa_core::adapter::driver::{AdapterStatusReport, DriverPlan};
use anolisa_core::adapter::manager::{
    AdapterManager, DisableOutcome, EnableOutcome, ScanEntry, ScanReport, StatusReport,
};

use crate::commands::common;
use crate::context::CliContext;
use crate::response::{CliError, render_json};

/// CLI arguments for the `adapter` sub-surface.
#[derive(Parser)]
pub struct AdapterArgs {
    /// Adapter subcommand.
    #[command(subcommand)]
    pub command: AdapterCommands,
}

/// Subcommands under `anolisa adapter`.
#[derive(Subcommand)]
pub enum AdapterCommands {
    /// Discover installed adapter declarations and local resource/receipt state.
    Scan,
    /// Enable a component's adapter for a framework.
    Enable {
        /// Component name (e.g. `tokenless`).
        component: String,
        /// Target framework (e.g. `openclaw`). Omit when the component
        /// ships adapters for exactly one framework.
        framework: Option<String>,
    },
    /// Disable a previously enabled adapter.
    Disable {
        /// Component name.
        component: String,
        /// Target framework. Omit to disable the component's single
        /// enabled adapter.
        framework: Option<String>,
    },
    /// Report adapter receipt status.
    Status {
        /// Limit to one component; omit for all receipts.
        component: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// JSON payloads
// ---------------------------------------------------------------------------

/// One row of `adapter scan` JSON output.
#[derive(Serialize)]
struct ScanRow {
    component: String,
    framework: String,
    declared: bool,
    resource_present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource_root: Option<String>,
    driver_available: bool,
    framework_detected: bool,
    /// The `adapter_type` declared in the component manifest, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    adapter_type: Option<String>,
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    claim_status: Option<ClaimStatus>,
}

/// `adapter scan` JSON output.
#[derive(Serialize)]
struct ScanPayload {
    adapters: Vec<ScanRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

/// `adapter enable` JSON output.
#[derive(Serialize)]
struct EnablePayload {
    component: String,
    framework: String,
    dry_run: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan: Option<DriverPlan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    claim: Option<AdapterClaim>,
}

/// `adapter disable` JSON output.
#[derive(Serialize)]
struct DisablePayload {
    component: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    framework: Option<String>,
    claim_removed: bool,
    cleanup_complete: bool,
    messages: Vec<String>,
}

/// One row of `adapter status` JSON output.
#[derive(Serialize)]
struct StatusRow {
    component: String,
    framework: String,
    #[serde(flatten)]
    report: AdapterStatusReport,
}

/// `adapter status` JSON output.
#[derive(Serialize)]
struct StatusPayload {
    receipts: Vec<StatusRow>,
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Entry point for `anolisa adapter`.
pub fn handle(args: AdapterArgs, ctx: &CliContext) -> Result<(), CliError> {
    match args.command {
        AdapterCommands::Scan => handle_scan(ctx),
        AdapterCommands::Enable {
            component,
            framework,
        } => handle_enable(ctx, &component, framework.as_deref()),
        AdapterCommands::Disable {
            component,
            framework,
        } => handle_disable(ctx, &component, framework.as_deref()),
        AdapterCommands::Status { component } => handle_status(ctx, component.as_deref()),
    }
}

/// Build a manager for the active layout.
fn build_manager(ctx: &CliContext) -> AdapterManager {
    common::build_adapter_manager(ctx)
}

// ---------------------------------------------------------------------------
// scan
// ---------------------------------------------------------------------------

fn handle_scan(ctx: &CliContext) -> Result<(), CliError> {
    const COMMAND: &str = "adapter scan";
    let manager = build_manager(ctx);
    let report: ScanReport = manager.scan().map_err(|e| map_err(COMMAND, e))?;

    if ctx.json {
        let adapters = report.entries.iter().map(scan_row_from_entry).collect();
        return render_json(
            COMMAND,
            ScanPayload {
                adapters,
                warnings: report.warnings,
            },
        );
    }

    if !ctx.quiet {
        for warning in &report.warnings {
            eprintln!("warning: {warning}");
        }
    }

    if report.entries.is_empty() {
        println!("No adapter declarations or resources found.");
        return Ok(());
    }
    println!(
        "{:<16} {:<10} {:<14} {:<9} {:<9} {:<9} {:<8} STATE",
        "COMPONENT", "FRAMEWORK", "TYPE", "DECLARED", "RESOURCE", "DRIVER", "DETECTED"
    );
    for row in &report.entries {
        println!(
            "{:<16} {:<10} {:<14} {:<9} {:<9} {:<9} {:<8} {}",
            row.component,
            row.framework,
            row.adapter_type.as_deref().unwrap_or("-"),
            yes_no(row.declared),
            if row.resource_root.is_some() {
                "present"
            } else {
                "missing"
            },
            yes_no(row.driver_available),
            yes_no(row.framework_detected),
            scan_state_label(row),
        );
    }
    Ok(())
}

fn scan_row_from_entry(row: &ScanEntry) -> ScanRow {
    ScanRow {
        component: row.component.clone(),
        framework: row.framework.clone(),
        declared: row.declared,
        resource_present: row.resource_root.is_some(),
        resource_root: row
            .resource_root
            .as_ref()
            .map(|path| path.display().to_string()),
        driver_available: row.driver_available,
        framework_detected: row.framework_detected,
        adapter_type: row.adapter_type.clone(),
        enabled: row.enabled,
        claim_status: row.claim_status,
    }
}

fn scan_state_label(row: &ScanEntry) -> &'static str {
    match (row.enabled, row.claim_status) {
        (true, Some(ClaimStatus::Enabled)) => "enabled",
        (true, Some(ClaimStatus::CleanupFailed)) => "cleanup_failed",
        (true, None) => "enabled",
        (false, _) => "-",
    }
}

// ---------------------------------------------------------------------------
// enable
// ---------------------------------------------------------------------------

fn handle_enable(
    ctx: &CliContext,
    component: &str,
    framework: Option<&str>,
) -> Result<(), CliError> {
    const COMMAND: &str = "adapter enable";
    let manager = build_manager(ctx);
    let outcome = manager
        .enable(component, framework, ctx.dry_run)
        .map_err(|e| map_err(COMMAND, e))?;

    match outcome {
        EnableOutcome::Planned(plan) => {
            if ctx.json {
                let payload = EnablePayload {
                    component: plan.component.clone(),
                    framework: plan.framework.clone(),
                    dry_run: true,
                    plan: Some(plan),
                    claim: None,
                };
                return render_json(COMMAND, payload);
            }
            println!(
                "[dry-run] would enable {}/{}:",
                plan.component, plan.framework
            );
            for action in &plan.actions {
                println!("  - {action}");
            }
            if let Some(cmd) = &plan.register_command {
                println!("  command: {cmd}");
            }
            Ok(())
        }
        EnableOutcome::Enabled(claim) => {
            if ctx.json {
                let payload = EnablePayload {
                    component: claim.component.clone(),
                    framework: claim.framework.clone(),
                    dry_run: false,
                    plan: None,
                    claim: Some(*claim),
                };
                return render_json(COMMAND, payload);
            }
            println!("Enabled {}/{}.", claim.component, claim.framework);
            if let Some(pid) = &claim.plugin_id {
                println!("  plugin: {pid}");
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// disable
// ---------------------------------------------------------------------------

fn handle_disable(
    ctx: &CliContext,
    component: &str,
    framework: Option<&str>,
) -> Result<(), CliError> {
    const COMMAND: &str = "adapter disable";
    let manager = build_manager(ctx);
    let outcome: DisableOutcome = manager
        .disable(component, framework)
        .map_err(|e| map_err(COMMAND, e))?;

    // Cleanup that did not complete is a degraded outcome: the receipt was
    // kept (marked cleanup_failed) so the operator can retry. Surface a
    // non-zero exit rather than a silent success.
    let degraded = (!outcome.report.cleanup_complete).then(|| CliError::Degraded {
        command: COMMAND.to_string(),
        reason: format!(
            "adapter '{}' cleanup incomplete; receipt kept for retry",
            outcome.component
        ),
    });

    if ctx.json {
        if let Some(err) = degraded {
            return Err(err);
        }
        let payload = DisablePayload {
            component: outcome.component.clone(),
            framework: outcome.framework.clone(),
            claim_removed: outcome.claim_removed,
            cleanup_complete: outcome.report.cleanup_complete,
            messages: outcome.report.messages.clone(),
        };
        return render_json(COMMAND, payload);
    }

    let target = match &outcome.framework {
        Some(f) => format!("{}/{}", outcome.component, f),
        None => outcome.component.clone(),
    };
    if outcome.claim_removed {
        println!("Disabled {target}.");
    } else if outcome.report.cleanup_complete {
        println!("Nothing to disable for {target}.");
    } else {
        println!("Disable of {target} did not complete cleanly:");
    }
    for msg in &outcome.report.messages {
        println!("  - {msg}");
    }
    degraded.map_or(Ok(()), Err)
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

fn handle_status(ctx: &CliContext, component: Option<&str>) -> Result<(), CliError> {
    const COMMAND: &str = "adapter status";
    let manager = build_manager(ctx);
    let report: StatusReport = manager.status(component).map_err(|e| map_err(COMMAND, e))?;

    if ctx.json {
        let receipts = report
            .entries
            .into_iter()
            .map(|e| StatusRow {
                component: e.component,
                framework: e.framework,
                report: e.report,
            })
            .collect();
        return render_json(COMMAND, StatusPayload { receipts });
    }

    if report.entries.is_empty() {
        println!("No adapter receipts.");
        return Ok(());
    }
    for e in &report.entries {
        println!(
            "{}/{}: {}",
            e.component,
            e.framework,
            summary_label(&e.report.summary),
        );
        for cond in &e.report.conditions {
            let reason = cond
                .reason
                .as_deref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default();
            println!("  {:?} = {:?}{}", cond.kind, cond.status, reason);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map an [`AdapterError`] to the CLI error model. Input/environment
/// problems are `INVALID_ARGUMENT` (exit 2); machine-side failures (CLI
/// spawn, lock, state/log IO) are `EXECUTION_FAILED` (exit 1).
fn map_err(command: &str, err: AdapterError) -> CliError {
    match err {
        AdapterError::UnknownPlaceholder { .. }
        | AdapterError::UnknownFramework { .. }
        | AdapterError::AmbiguousFramework { .. }
        | AdapterError::UnsupportedAdapterType { .. }
        | AdapterError::ComponentNotInstalled { .. }
        | AdapterError::AdapterNotDeclared { .. }
        | AdapterError::ResourceRootNotFound { .. }
        | AdapterError::FrameworkNotDetected { .. }
        | AdapterError::BundleInvalid { .. }
        | AdapterError::ClaimValidation(_) => CliError::InvalidArgument {
            command: command.to_string(),
            reason: err.to_string(),
        },
        AdapterError::AdapterManifest { .. }
        | AdapterError::FrameworkCli { .. }
        | AdapterError::Lock(_)
        | AdapterError::State(_)
        | AdapterError::Log(_)
        | AdapterError::Io { .. } => CliError::Runtime {
            command: command.to_string(),
            reason: err.to_string(),
        },
    }
}

/// Human-facing one-line label for a status summary.
fn summary_label(summary: &anolisa_core::adapter::driver::AdapterSummary) -> &'static str {
    use anolisa_core::adapter::driver::AdapterSummary::*;
    match summary {
        Healthy => "healthy",
        Degraded => "degraded",
        CleanupFailed => "cleanup_failed",
        Unknown => "unknown",
    }
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// Wrapper so we can parse the adapter subcommand in isolation.
    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: AdapterCommands,
    }

    #[test]
    fn enable_parses_optional_framework() {
        let cli = TestCli::try_parse_from(["x", "enable", "tokenless"]).expect("parse");
        match cli.command {
            AdapterCommands::Enable {
                component,
                framework,
            } => {
                assert_eq!(component, "tokenless");
                assert!(framework.is_none());
            }
            _ => panic!("expected enable"),
        }

        let cli = TestCli::try_parse_from(["x", "enable", "tokenless", "openclaw"]).expect("parse");
        match cli.command {
            AdapterCommands::Enable { framework, .. } => {
                assert_eq!(framework.as_deref(), Some("openclaw"));
            }
            _ => panic!("expected enable"),
        }
    }

    #[test]
    fn status_component_is_optional() {
        let cli = TestCli::try_parse_from(["x", "status"]).expect("parse");
        assert!(matches!(
            cli.command,
            AdapterCommands::Status { component: None }
        ));
    }

    #[test]
    fn ambiguous_framework_maps_to_invalid_argument() {
        let err = map_err(
            "adapter enable",
            AdapterError::AmbiguousFramework {
                component: "x".to_string(),
                frameworks: vec!["a".to_string(), "b".to_string()],
            },
        );
        assert!(matches!(err, CliError::InvalidArgument { .. }));
    }

    #[test]
    fn framework_cli_failure_maps_to_runtime() {
        let err = map_err(
            "adapter enable",
            AdapterError::FrameworkCli {
                program: "openclaw".to_string(),
                reason: "boom".to_string(),
            },
        );
        assert!(matches!(err, CliError::Runtime { .. }));
    }

    #[test]
    fn unsupported_adapter_type_maps_to_invalid_argument() {
        let err = map_err(
            "adapter enable",
            AdapterError::UnsupportedAdapterType {
                component: "tokenless".to_string(),
                framework: "openclaw".to_string(),
                adapter_type: "skill_bundle".to_string(),
            },
        );
        assert!(matches!(err, CliError::InvalidArgument { .. }));
    }

    #[test]
    fn scan_row_includes_adapter_type() {
        let entry = ScanEntry {
            component: "tokenless".to_string(),
            framework: "openclaw".to_string(),
            declared: true,
            resource_root: None,
            driver_available: true,
            framework_detected: true,
            adapter_type: Some("plugin".to_string()),
            enabled: false,
            claim_status: None,
        };
        let row = scan_row_from_entry(&entry);
        assert_eq!(row.adapter_type.as_deref(), Some("plugin"));
    }

    #[test]
    fn scan_row_adapter_type_none_when_not_declared() {
        let entry = ScanEntry {
            component: "tokenless".to_string(),
            framework: "openclaw".to_string(),
            declared: false,
            resource_root: Some(std::path::PathBuf::from("/tmp/adapters/tokenless/openclaw")),
            driver_available: true,
            framework_detected: true,
            adapter_type: None,
            enabled: false,
            claim_status: None,
        };
        let row = scan_row_from_entry(&entry);
        assert!(row.adapter_type.is_none());
    }
}
