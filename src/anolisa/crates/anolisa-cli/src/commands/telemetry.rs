//! `anolisa telemetry` management surface.
//!
//! Independent command surface for the self-hosted telemetry channel: toggling
//! the collection sentinel, managing the named-reporting link id, showing
//! status, and running the uploader. It deliberately does **not** touch the
//! `register` / `unregister` flow — that stays orthogonal.

use anolisa_core::execution::{CommandOutcomeStatus, ExecutionIntent};
use anolisa_core::{RegistrationManager, TelemetryChannel};
use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::context::CliContext;
use crate::response::{CliError, render_json};

use self::application::{
    TelemetryApplicationOutcome, TelemetryApplied, TelemetryPreview, TelemetryRequest,
};

mod application;

/// systemd unit that runs the resident upload loop.
const SERVICE_NAME: &str = "anolisa-telemetry";
/// Filename of the unit written into the system unit directory.
const UNIT_FILENAME: &str = "anolisa-telemetry.service";
/// User-facing command for inspecting telemetry state.
const STATUS_COMMAND: &str = "anolisa telemetry status";

/// Returns the management command for a telemetry systemd unit target.
pub(crate) fn status_command_for_service_target(target: &str) -> Option<&'static str> {
    matches!(target, SERVICE_NAME | UNIT_FILENAME).then_some(STATUS_COMMAND)
}

#[derive(Parser)]
pub struct TelemetryArgs {
    #[command(subcommand)]
    pub command: TelemetryCommands,
}

#[derive(Subcommand)]
pub enum TelemetryCommands {
    /// Enable default telemetry collection (requires root/sudo)
    Enable,
    /// Disable telemetry collection (requires root/sudo)
    Disable,
    /// Show telemetry collection and link status
    Status {
        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Link this instance to named reporting (requires root/sudo)
    Link,
    /// Remove the named-reporting link (requires root/sudo)
    Unlink,
    /// Run the uploader once, or as a loop with `--loop` (internal)
    #[command(hide = true)]
    Upload {
        /// Run the continuous upload loop (daemon mode)
        #[arg(long = "loop")]
        loop_flag: bool,
    },
    /// Self-heal the ops channel without touching consent (internal, boot)
    #[command(hide = true)]
    Init,
}

/// Dispatch `telemetry` subcommands.
pub fn handle(args: TelemetryArgs, ctx: &CliContext) -> Result<(), CliError> {
    match args.command {
        TelemetryCommands::Status { json } => handle_status(json),
        command => handle_mutation(command, ctx),
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct TelemetryPreviewPayload {
    dry_run: bool,
    message: &'static str,
}

fn handle_mutation(command: TelemetryCommands, ctx: &CliContext) -> Result<(), CliError> {
    let request = mutation_request(command, execution_intent(ctx.dry_run));
    render_mutation(application::run(request, ctx)?, ctx)
}

fn mutation_request(command: TelemetryCommands, intent: ExecutionIntent) -> TelemetryRequest {
    match command {
        TelemetryCommands::Enable => TelemetryRequest::Enable { intent },
        TelemetryCommands::Disable => TelemetryRequest::Disable { intent },
        TelemetryCommands::Link => TelemetryRequest::Link { intent },
        TelemetryCommands::Unlink => TelemetryRequest::Unlink { intent },
        TelemetryCommands::Upload { loop_flag } => TelemetryRequest::Upload { loop_flag, intent },
        TelemetryCommands::Init => TelemetryRequest::Init { intent },
        TelemetryCommands::Status { .. } => {
            unreachable!("status is dispatched through the read-only handler")
        }
    }
}

fn execution_intent(dry_run: bool) -> ExecutionIntent {
    if dry_run {
        ExecutionIntent::Plan
    } else {
        ExecutionIntent::Apply
    }
}

/// Enable default collection through the shared telemetry application path.
///
/// The deprecated `register` command uses this apply-only compatibility entry
/// before decommissioning its legacy ilogtail channel.
pub(crate) fn handle_enable(ctx: &CliContext) -> Result<(), CliError> {
    render_mutation(
        application::run(
            TelemetryRequest::Enable {
                intent: ExecutionIntent::Apply,
            },
            ctx,
        )?,
        ctx,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TelemetryOutput {
    Stdout(String),
    Stderr(String),
}

fn render_mutation(result: TelemetryApplicationOutcome, ctx: &CliContext) -> Result<(), CliError> {
    match result {
        TelemetryApplicationOutcome::Preview(TelemetryPreview { command, message }) => {
            if ctx.json {
                return render_json(
                    command,
                    TelemetryPreviewPayload {
                        dry_run: true,
                        message,
                    },
                );
            }
            if !ctx.quiet {
                println!("[dry-run] {message}");
            }
            Ok(())
        }
        TelemetryApplicationOutcome::Applied { result, outcome } => {
            match outcome.status() {
                CommandOutcomeStatus::Completed => {}
                CommandOutcomeStatus::Partial { reason } => {
                    return Err(CliError::Degraded {
                        command: result.command().to_string(),
                        reason: reason.clone(),
                    });
                }
                CommandOutcomeStatus::Failed { reason } => {
                    return Err(CliError::Runtime {
                        command: result.command().to_string(),
                        reason: reason.clone(),
                    });
                }
            }
            for output in applied_output(&result, outcome.warnings()) {
                match output {
                    TelemetryOutput::Stdout(line) => println!("{line}"),
                    TelemetryOutput::Stderr(line) => eprintln!("{line}"),
                }
            }
            Ok(())
        }
    }
}

fn applied_output(result: &TelemetryApplied, warnings: &[String]) -> Vec<TelemetryOutput> {
    let warning_lines = || {
        warnings
            .iter()
            .map(|warning| TelemetryOutput::Stderr(format!("warn: {warning}")))
            .collect::<Vec<_>>()
    };
    match result {
        TelemetryApplied::Enabled => warning_lines()
            .into_iter()
            .chain([TelemetryOutput::Stdout(
                "Telemetry collection enabled.".to_string(),
            )])
            .collect(),
        TelemetryApplied::Disabled => warning_lines()
            .into_iter()
            .chain([
                TelemetryOutput::Stdout("Telemetry collection disabled.".to_string()),
                TelemetryOutput::Stdout(
                    "  The uploader stops shortly; buffered logs are preserved.".to_string(),
                ),
            ])
            .collect(),
        TelemetryApplied::Linked {
            link_id,
            already_linked: true,
        } => vec![TelemetryOutput::Stdout(format!(
            "Already linked (link id: {link_id})."
        ))],
        TelemetryApplied::Linked {
            link_id,
            already_linked: false,
        } => [
            TelemetryOutput::Stdout("Linked to named reporting.".to_string()),
            TelemetryOutput::Stdout(format!("  link id: {link_id}")),
        ]
        .into_iter()
        .chain(warning_lines())
        .collect(),
        TelemetryApplied::Unlinked => warning_lines()
            .into_iter()
            .chain([TelemetryOutput::Stdout(
                "Unlinked from named reporting.".to_string(),
            )])
            .collect(),
        TelemetryApplied::Uploaded { .. } | TelemetryApplied::Initialized => warning_lines(),
    }
}

// ── status ──────────────────────────────────────────────────────────

fn handle_status(json: bool) -> Result<(), CliError> {
    let enabled = TelemetryChannel::new().is_enabled();
    let link_id = RegistrationManager::new().read_link_id();
    let linked = link_id.is_some();

    if json {
        return render_json(
            "telemetry status",
            serde_json::json!({
                "collection_enabled": enabled,
                "linked": linked,
                "link_id": link_id,
            }),
        );
    }

    println!(
        "Telemetry collection: {}",
        if enabled { "enabled" } else { "disabled" }
    );
    match &link_id {
        Some(id) => println!("Named reporting:      linked ({id})"),
        None => println!("Named reporting:      not linked"),
    }
    Ok(())
}

// ── Unit tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: TelemetryCommands,
    }

    fn parse(args: &[&str]) -> TelemetryCommands {
        TestCli::parse_from(args).command
    }

    #[test]
    fn test_parse_enable_disable() {
        assert!(matches!(parse(&["t", "enable"]), TelemetryCommands::Enable));
        assert!(matches!(
            parse(&["t", "disable"]),
            TelemetryCommands::Disable
        ));
    }

    #[test]
    fn test_parse_status_json() {
        assert!(matches!(
            parse(&["t", "status", "--json"]),
            TelemetryCommands::Status { json: true }
        ));
        assert!(matches!(
            parse(&["t", "status"]),
            TelemetryCommands::Status { json: false }
        ));
    }

    #[test]
    fn service_targets_share_the_canonical_status_command() {
        for target in [SERVICE_NAME, UNIT_FILENAME] {
            assert_eq!(
                status_command_for_service_target(target),
                Some("anolisa telemetry status"),
            );
        }
        assert_eq!(status_command_for_service_target("telemetry"), None);
    }

    #[test]
    fn test_parse_link_unlink() {
        assert!(matches!(parse(&["t", "link"]), TelemetryCommands::Link));
        assert!(matches!(parse(&["t", "unlink"]), TelemetryCommands::Unlink));
    }

    #[test]
    fn test_parse_init() {
        assert!(matches!(parse(&["t", "init"]), TelemetryCommands::Init));
    }

    #[test]
    fn test_parse_upload_loop_flag() {
        assert!(matches!(
            parse(&["t", "upload", "--loop"]),
            TelemetryCommands::Upload { loop_flag: true }
        ));
        assert!(matches!(
            parse(&["t", "upload"]),
            TelemetryCommands::Upload { loop_flag: false }
        ));
    }

    #[test]
    fn global_dry_run_maps_to_execution_intent() {
        assert_eq!(execution_intent(true), ExecutionIntent::Plan);
        assert_eq!(execution_intent(false), ExecutionIntent::Apply);
    }

    #[test]
    fn mutation_commands_map_to_typed_requests() {
        assert!(matches!(
            mutation_request(TelemetryCommands::Enable, ExecutionIntent::Plan),
            TelemetryRequest::Enable {
                intent: ExecutionIntent::Plan
            }
        ));
        assert!(matches!(
            mutation_request(
                TelemetryCommands::Upload { loop_flag: true },
                ExecutionIntent::Apply
            ),
            TelemetryRequest::Upload {
                loop_flag: true,
                intent: ExecutionIntent::Apply
            }
        ));
    }

    #[test]
    fn preview_payload_is_machine_readable() {
        let payload = TelemetryPreviewPayload {
            dry_run: true,
            message: "would link this instance to named reporting",
        };
        let value = serde_json::to_value(payload).expect("serialize telemetry preview");

        assert_eq!(value["dry_run"], true);
        assert_eq!(
            value["message"],
            "would link this instance to named reporting"
        );
    }

    #[test]
    fn applied_output_preserves_warning_and_success_order() {
        let warnings = vec!["first warning".to_string(), "second warning".to_string()];
        assert_eq!(
            applied_output(&TelemetryApplied::Enabled, &warnings),
            vec![
                TelemetryOutput::Stderr("warn: first warning".to_string()),
                TelemetryOutput::Stderr("warn: second warning".to_string()),
                TelemetryOutput::Stdout("Telemetry collection enabled.".to_string()),
            ]
        );
        assert_eq!(
            applied_output(
                &TelemetryApplied::Linked {
                    link_id: "link-1".to_string(),
                    already_linked: false,
                },
                &warnings,
            ),
            vec![
                TelemetryOutput::Stdout("Linked to named reporting.".to_string()),
                TelemetryOutput::Stdout("  link id: link-1".to_string()),
                TelemetryOutput::Stderr("warn: first warning".to_string()),
                TelemetryOutput::Stderr("warn: second warning".to_string()),
            ]
        );
        assert_eq!(
            applied_output(&TelemetryApplied::Unlinked, &warnings),
            vec![
                TelemetryOutput::Stderr("warn: first warning".to_string()),
                TelemetryOutput::Stderr("warn: second warning".to_string()),
                TelemetryOutput::Stdout("Unlinked from named reporting.".to_string()),
            ]
        );
    }

    #[test]
    fn already_linked_output_keeps_existing_message() {
        assert_eq!(
            applied_output(
                &TelemetryApplied::Linked {
                    link_id: "existing".to_string(),
                    already_linked: true,
                },
                &[],
            ),
            vec![TelemetryOutput::Stdout(
                "Already linked (link id: existing).".to_string()
            )]
        );
    }

    #[test]
    fn test_unit_template_renders_exec_and_wantedby() {
        const UNIT_TEMPLATE: &str =
            include_str!("../../../../systemd/anolisa-telemetry.service.in");
        let rendered = UNIT_TEMPLATE.replace("@@ANOLISA_BIN@@", "/usr/bin/anolisa");
        assert!(rendered.contains("ExecStartPre=/usr/bin/anolisa telemetry init"));
        assert!(rendered.contains("ExecStart=/usr/bin/anolisa telemetry upload --loop"));
        assert!(rendered.contains("WantedBy=multi-user.target"));
        // Placeholder must be fully substituted.
        assert!(!rendered.contains("@@ANOLISA_BIN@@"));
    }
}
