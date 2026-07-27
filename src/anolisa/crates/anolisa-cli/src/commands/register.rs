use anolisa_core::{
    ConsentState, HistoryAction, LegacyIlogtail, RegisterSource, RegistrationManager,
    TelemetryChannel, current_operator, require_root,
};
use clap::{Parser, Subcommand};
use std::io::IsTerminal;

use crate::commands::telemetry;
use crate::context::CliContext;
use crate::response::CliError;

#[derive(Parser)]
#[command(args_conflicts_with_subcommands = true)]
pub struct RegisterArgs {
    #[command(subcommand)]
    pub command: Option<RegisterCommands>,

    /// Skip interactive confirmation (for scripts / automation)
    #[arg(long)]
    pub yes: bool,
}

#[derive(Subcommand)]
pub enum RegisterCommands {
    /// Show registration status
    Status {
        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Parser)]
pub struct UnregisterArgs {
    /// Skip interactive confirmation
    #[arg(long)]
    pub force: bool,
}

/// Dispatch `register` subcommands or default register action
pub fn handle_register_group(args: RegisterArgs, ctx: &CliContext) -> Result<(), CliError> {
    match args.command {
        None => {
            // `--yes` is still accepted for script backward-compat but ignored:
            // the deprecated forwarder no longer prompts.
            let _ = args.yes;
            handle_register(ctx)
        }
        Some(RegisterCommands::Status { json }) => handle_status(&RegistrationManager::new(), json),
    }
}

/// Handle top-level `unregister` command
pub fn handle_unregister_cmd(args: UnregisterArgs, _ctx: &CliContext) -> Result<(), CliError> {
    let mgr = RegistrationManager::new();
    handle_unregister(&mgr, args.force)
}

// ── register ──────────────────────────────────────────────────────────────────

fn handle_register(ctx: &CliContext) -> Result<(), CliError> {
    // `register` is retired in the opt-out model: collection is on by default,
    // so an explicit opt-in verb is redundant. Keep it working as a thin,
    // deprecated alias that forwards to `telemetry enable` (the real,
    // reboot-persistent bring-up), then decommissions the legacy ilogtail
    // channel. Consent bookkeeping is intentionally left to sysom/console and
    // the `unregister` withdrawal path.
    eprintln!("warning: 'anolisa register' is deprecated and will be removed in a future release.");
    eprintln!(
        "         Collection is on by default; use 'anolisa telemetry enable' / 'telemetry link'."
    );

    telemetry::handle_enable(ctx)?;

    // Upgrade migration: an older ANOLISA registered through the shared ilogtail
    // daemon by writing SLS account files. Now that we ship via the self-hosted
    // uploader, decommission that legacy channel so it does not double-upload.
    decommission_legacy_ilogtail();

    Ok(())
}

// ── unregister ────────────────────────────────────────────────────────────────

fn handle_unregister(mgr: &RegistrationManager, force: bool) -> Result<(), CliError> {
    require_root().map_err(|e| CliError::Runtime {
        command: "unregister".to_string(),
        reason: e.to_string(),
    })?;

    let already_unregistered = mgr.read_state() == ConsentState::Unregistered;

    if already_unregistered && !force {
        println!("Not currently registered.");
        println!("  If telemetry teardown previously failed, run with --force to retry cleanup.");
        return Ok(());
    }

    if !already_unregistered {
        if !force {
            if !std::io::stdin().is_terminal() {
                return Err(CliError::Runtime {
                    command: "unregister".to_string(),
                    reason:
                        "non-interactive session detected; pass --force to confirm unregistration"
                            .to_string(),
                });
            }
            println!("You are about to unregister from the Agentic OS Co-Build Program.");
            println!(
                "Local logs are preserved; you can re-enable anytime with 'sudo anolisa telemetry enable'."
            );
            println!();
            if !prompt_yn("Unregister? [y/N]: ", false) {
                println!("Cancelled.");
                return Ok(());
            }
        }

        // Write consent state FIRST — user intent takes priority over cleanup.
        // Even if stop() fails below, the consent record must reflect "no".
        let operator = current_operator();
        mgr.do_unregister(&operator)
            .map_err(|e| CliError::Runtime {
                command: "unregister".to_string(),
                reason: e.to_string(),
            })?;
    }

    // Disable default collection: write the opt-out marker so components stop
    // writing and the uploader loop self-exits. Consent is already recorded
    // above; this is best-effort cleanup. Already-buffered data is preserved.
    if let Err(e) = TelemetryChannel::new().disable_collection() {
        eprintln!("error: consent recorded as UNREGISTERED, but disabling collection failed: {e}");
        eprintln!("  The system will NOT upload new data (consent denied),");
        eprintln!("  but the opt-out marker may not have been written.");
        eprintln!("  Retry with: sudo anolisa unregister --force");
        return Err(CliError::Runtime {
            command: "unregister".to_string(),
            reason: format!(
                "disabling collection failed: {e}. Consent is UNREGISTERED; retry with --force."
            ),
        });
    }

    println!("Unregistered. Data reporting stopped.");
    println!("  Local logs preserved. To re-enable: sudo anolisa telemetry enable");

    // Withdrawing consent must also cut the legacy ilogtail channel: older
    // ANOLISA versions uploaded through the shared daemon via SLS account
    // files, which would otherwise keep shipping after opt-out.
    decommission_legacy_ilogtail();

    Ok(())
}

/// Best-effort teardown of the pre-self-hosted ilogtail upload channel.
///
/// Removes the account files configured in `/etc/anolisa/legacy-accounts.json`
/// (idempotent). If the configuration file is missing, decommission is a
/// no-op so downstream distributions without the legacy channel are unaffected.
/// A warning is emitted on failure but the caller's outcome is unaffected.
fn decommission_legacy_ilogtail() {
    match LegacyIlogtail::new().decommission() {
        Ok(removed) if !removed.is_empty() => {
            eprintln!(
                "note: decommissioned legacy ilogtail upload channel ({} file(s)).",
                removed.len()
            );
        }
        Ok(_) => {}
        Err(e) => eprintln!("warn: could not fully decommission legacy ilogtail channel: {e}"),
    }
}

// ── status ────────────────────────────────────────────────────────────────────

fn handle_status(mgr: &RegistrationManager, json: bool) -> Result<(), CliError> {
    let (state, rec) = mgr.read_state_and_record();
    let product_type = mgr.detect_product_type();
    let sysom_active = mgr.is_sysom_registered();

    if json {
        print_status_json(&state, &rec, &product_type, sysom_active);
        return Ok(());
    }

    println!(
        "Note: 'anolisa register status' is deprecated; use 'anolisa telemetry status' for telemetry collection state."
    );
    println!("═══════════════════════════════════════");
    println!("  ANOLISA Registration Status");
    println!("═══════════════════════════════════════");
    println!("  Product:       {}", product_type.display_name());
    println!();
    println!();

    // sysom service registration (sysak_meta is active)
    if sysom_active {
        // Console source means the registration was done through sysom's web console.
        let registered_via_console = rec
            .as_ref()
            .and_then(|r| r.source.as_ref())
            .map(|s| *s == RegisterSource::Console)
            .unwrap_or(false);

        if state != ConsentState::Registered || registered_via_console {
            println!("  Consent State: REGISTERED");
            println!("  Data Reporting: active");
            println!("  Source:        console");
            if let Some(r) = &rec
                && let Some(entry) = last_register_entry(r)
            {
                println!(
                    "  Registered:    {}",
                    entry.timestamp.format("%Y-%m-%d %H:%M")
                );
                println!("  Operator:      {}", entry.operator);
            }
            return Ok(());
        }
    }

    match &state {
        ConsentState::InitFresh => {
            println!("  Consent State: INIT (not yet decided)");
            println!("  Data Reporting: disabled (local only)");
            println!();
            println!("  You haven't decided whether to enable data reporting.");
            println!("  Run 'sudo anolisa telemetry enable' to enable.");
        }
        ConsentState::Unregistered => {
            println!("  Consent State: UNREGISTERED");
            println!("  Data Reporting: disabled (local only)");
            if let Some(r) = &rec
                && let Some(entry) = last_register_entry(r)
            {
                let via = format_source(&r.source);
                println!(
                    "  Last Registered: {}{via}",
                    entry.timestamp.format("%Y-%m-%d %H:%M")
                );
            }
            println!();
            println!("  To enable: sudo anolisa telemetry enable");
        }
        ConsentState::Registered => {
            println!("  Consent State: REGISTERED");
            println!("  Data Reporting: active");
            if let Some(r) = &rec
                && let Some(entry) = last_register_entry(r)
            {
                let via = format_source(&r.source);
                println!(
                    "  Registered:    {}{via}",
                    entry.timestamp.format("%Y-%m-%d %H:%M")
                );
                println!("  Operator:      {}", entry.operator);
            }
        }
    }

    Ok(())
}

// ── JSON output ─────────────────────────────────────────────────────────────

fn print_status_json(
    state: &ConsentState,
    rec: &Option<anolisa_core::RegisterRecord>,
    product_type: &anolisa_core::ProductType,
    sysom_active: bool,
) {
    let state_str = if sysom_active && state != &ConsentState::Registered {
        "registered"
    } else {
        match state {
            ConsentState::InitFresh => "init",
            ConsentState::Unregistered => "unregistered",
            ConsentState::Registered => "registered",
        }
    };

    let upload_active = state == &ConsentState::Registered || sysom_active;

    let mut obj = serde_json::json!({
        "product_type": product_type.to_string(),
        "consent_state": state_str,
        "upload_active": upload_active,
    });

    if let Some(r) = rec
        && let Some(entry) = last_register_entry(r)
    {
        obj["registration_time"] =
            serde_json::Value::String(entry.timestamp.format("%Y-%m-%dT%H:%M:%SZ").to_string());
        obj["operator"] = serde_json::Value::String(entry.operator.clone());
    }
    if let Some(r) = rec
        && let Some(src) = &r.source
    {
        obj["source"] = serde_json::Value::String(src.to_string());
    }

    if sysom_active {
        obj["effective_source"] = serde_json::Value::String("sysom".to_string());
        obj["sysom_services_active"] = serde_json::Value::Bool(true);
    }

    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
}

// ── Utility functions ────────────────────────────────────────────────────────

/// Find the last `Register` entry in the history array.
fn last_register_entry(rec: &anolisa_core::RegisterRecord) -> Option<&anolisa_core::HistoryEntry> {
    rec.history
        .iter()
        .rev()
        .find(|e| e.action == HistoryAction::Register)
}

fn format_source(source: &Option<anolisa_core::RegisterSource>) -> String {
    match source {
        Some(s) => format!(" (via {s})"),
        None => String::new(),
    }
}

fn prompt_yn(prompt: &str, default: bool) -> bool {
    use std::io::{self, BufRead};
    print!("{prompt}");
    crate::output::flush_stdout();
    let line = io::stdin()
        .lock()
        .lines()
        .next()
        .and_then(|l| l.ok())
        .unwrap_or_default();
    match line.trim().to_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        "" => default,
        _ => false,
    }
}
