use anolisa_core::{
    ConsentState, HistoryAction, RegisterSource, RegistrationManager, TelemetryConfig,
    TelemetryStarter, current_operator, require_root,
};
use clap::{Parser, Subcommand};
use std::io::IsTerminal;

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
pub fn handle_register_group(args: RegisterArgs, _ctx: &CliContext) -> Result<(), CliError> {
    let mgr = RegistrationManager::new();
    match args.command {
        None => handle_register(&mgr, args.yes),
        Some(RegisterCommands::Status { json }) => handle_status(&mgr, json),
    }
}

/// Handle top-level `unregister` command
pub fn handle_unregister_cmd(args: UnregisterArgs, _ctx: &CliContext) -> Result<(), CliError> {
    let mgr = RegistrationManager::new();
    handle_unregister(&mgr, args.force)
}

// ── register ──────────────────────────────────────────────────────────────────

fn handle_register(mgr: &RegistrationManager, yes: bool) -> Result<(), CliError> {
    require_root().map_err(|e| CliError::Runtime {
        command: "register".to_string(),
        reason: e.to_string(),
    })?;

    if mgr.read_state() == ConsentState::Registered {
        println!("Already registered.");
        println!("Use 'anolisa register status' to check.");
        return Ok(());
    }

    if mgr.is_sysom_registered() {
        println!("Already registered (via sysom).");
        println!("Use 'anolisa register status' to check.");
        return Ok(());
    }

    let operator = current_operator();

    if !yes {
        if !std::io::stdin().is_terminal() {
            return Err(CliError::Runtime {
                command: "register".to_string(),
                reason: "non-interactive session detected; pass --yes to confirm registration"
                    .to_string(),
            });
        }
        print_register_banner();
        println!();
        if !prompt_yn("Register? [y/N]: ", false) {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let telemetry_cfg = build_telemetry_config();
    let starter = TelemetryStarter::new(telemetry_cfg);
    if let Err(e) = starter.start() {
        return Err(CliError::Runtime {
            command: "register".to_string(),
            reason: format!(
                "unable to start usage report service: {e}\n  Please check network connectivity and try again."
            ),
        });
    }

    if let Err(e) = mgr.do_register(&operator, RegisterSource::Cli) {
        // Compensate: rollback the telemetry setup we just started, but only if the
        // system is NOT in Registered state.  Another process may have raced
        // us and successfully registered; in that case its telemetry config is
        // valid and we must NOT tear it down.
        if mgr.read_state() != ConsentState::Registered
            && let Err(rollback_err) = starter.stop()
        {
            eprintln!("warn: rollback of telemetry start also failed: {rollback_err}");
        }
        return Err(CliError::Runtime {
            command: "register".to_string(),
            reason: e.to_string(),
        });
    }

    println!();
    println!("Registered successfully.");
    println!("  Status:        registered");
    if let Some(rec) = mgr.read_record()
        && let Some(entry) = last_register_entry(&rec)
    {
        println!(
            "  Registered:    {}",
            entry.timestamp.format("%Y-%m-%dT%H:%M:%SZ")
        );
        println!("  Operator:      {}", entry.operator);
    }
    println!("  Data Reporting: active");

    if !mgr.is_agentsight_running() {
        println!();
        println!(
            "  Note: agentsight is not running. Usage report may not work until agentsight is started."
        );
    }

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
                "Local logs are preserved; you can re-register anytime with 'sudo anolisa register'."
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

    // Attempt to tear down telemetry infrastructure.
    // Consent is already recorded above; this is best-effort cleanup.
    let telemetry_cfg = build_telemetry_config();
    if let Err(e) = TelemetryStarter::new(telemetry_cfg).stop() {
        eprintln!("error: consent recorded as UNREGISTERED, but telemetry teardown failed: {e}");
        eprintln!("  The system will NOT upload new data (consent denied),");
        eprintln!("  but residual ilogtail configuration may remain.");
        eprintln!("  Retry with: sudo anolisa unregister --force");
        return Err(CliError::Runtime {
            command: "unregister".to_string(),
            reason: format!(
                "telemetry teardown failed: {e}. Consent is UNREGISTERED; retry with --force."
            ),
        });
    }

    println!("Unregistered. Data reporting stopped.");
    println!("  Local logs preserved. To re-enable: sudo anolisa register");

    Ok(())
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

    println!("═══════════════════════════════════════");
    println!("  ANOLISA Registration Status");
    println!("═══════════════════════════════════════");
    println!("  Product:       {}", product_type.display_name());
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
            println!("  Run 'sudo anolisa register' to enable.");
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
            println!("  To enable registration: sudo anolisa register");
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
    use std::io::{self, BufRead, Write};
    print!("{prompt}");
    io::stdout().flush().ok();
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

fn build_telemetry_config() -> TelemetryConfig {
    let mut cfg = TelemetryConfig::default();
    if let Some(id) = read_sls_account_id_override() {
        cfg.sls_account_id = id;
    }
    cfg
}

fn read_sls_account_id_override() -> Option<String> {
    if let Ok(val) = std::env::var("ANOLISA_SLS_ACCOUNT_ID") {
        let v = val.trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }
    let content = std::fs::read_to_string("/etc/anolisa/ilogtail.cfg").ok()?;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(val) = line.strip_prefix("SLS_ACCOUNT_ID=") {
            let v = val.trim().trim_matches('"').trim_matches('\'').to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

fn print_register_banner() {
    let lines = [
        "Join the Agentic OS Co-Build Program",
        "",
        "Agentic OS \u{2014} the operating system for the Agent era.",
        "Register to the central platform to unlock full observability",
        "and personalized intelligence for your instances.",
        "",
        "What you'll get:",
        "",
        "  \u{2726} Token dashboard on Alibaba Cloud Console \u{2014} cost",
        "    breakdown & usage trends for all instances under",
        "    your account, in one place",
        "  \u{2726} Smarter, personalized cosh \u{2014} learns from real-world",
        "    scenarios across the fleet; model routing, Token",
        "    compression & Skill recommendations tailored to",
        "    your workload",
        "  \u{2726} Co-build privileges \u{2014} early access to beta Skills",
        "    and new model adaptations, plus a direct vote on",
        "    our next P0 priorities",
        "",
        "Registration is lightweight: a single 'anolisa register'",
        "command. Component metrics and instance ID (with your",
        "explicit consent per PIPL) will be reported to help",
        "improve the platform. You can revoke anytime via",
        "'sudo anolisa unregister'.",
    ];

    for line in &lines {
        println!("{line}");
    }
}
