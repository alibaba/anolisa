use anolisa_core::{
    ConsentState, RegistrationManager, UploadConfig, UploadStarter, current_operator, require_root,
};
use clap::{Parser, Subcommand};
use std::io::IsTerminal;
use unicode_width::UnicodeWidthStr;

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
        if !prompt_yn("Register? [Y/N]: ", false) {
            println!("Cancelled.");
            return Err(CliError::Runtime {
                command: "register".to_string(),
                reason: "user cancelled".to_string(),
            });
        }
    }

    let upload_cfg = build_upload_config();
    let starter = UploadStarter::new(upload_cfg);
    if let Err(e) = starter.start() {
        return Err(CliError::Runtime {
            command: "register".to_string(),
            reason: format!(
                "unable to start usage report service: {e}\n  Please check network connectivity and try again."
            ),
        });
    }

    if let Err(e) = mgr.do_register(&operator) {
        // Compensate: rollback the upload we just started, but only if the
        // system is NOT in Registered state.  Another process may have raced
        // us and successfully registered; in that case its upload config is
        // valid and we must NOT tear it down.
        if mgr.read_state() != ConsentState::Registered
            && let Err(rollback_err) = starter.stop()
        {
            eprintln!("warn: rollback of upload start also failed: {rollback_err}");
        }
        return Err(CliError::Runtime {
            command: "register".to_string(),
            reason: e.to_string(),
        });
    }

    println!();
    println!("Registered successfully.");
    println!("  Status:       registered");
    if let Some(rec) = mgr.read_record()
        && let Some(t) = rec.registration_time
    {
        println!("  Registered:   {}", t.format("%Y-%m-%dT%H:%M:%SZ"));
    }
    println!("  Usage Report: active");

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
        println!("  If upload teardown previously failed, run with --force to retry cleanup.");
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
            if !prompt_yn("Unregister? [y/N]: ", false) {
                println!("Cancelled.");
                return Err(CliError::Runtime {
                    command: "unregister".to_string(),
                    reason: "user cancelled".to_string(),
                });
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

    // Attempt to tear down upload infrastructure.
    // Consent is already recorded above; this is best-effort cleanup.
    let upload_cfg = build_upload_config();
    if let Err(e) = UploadStarter::new(upload_cfg).stop() {
        eprintln!("error: consent recorded as UNREGISTERED, but upload teardown failed: {e}");
        eprintln!("  The system will NOT upload new data (consent denied),");
        eprintln!("  but residual ilogtail configuration may remain.");
        eprintln!("  Retry with: sudo anolisa unregister --force");
        return Err(CliError::Runtime {
            command: "unregister".to_string(),
            reason: format!(
                "upload teardown failed: {e}. Consent is UNREGISTERED; retry with --force."
            ),
        });
    }

    println!("Unregistered. Usage report stopped.");
    println!("  To re-enable: sudo anolisa register");

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
        // Console source means the registration was done through sysom's web console
        let registered_via_console = rec
            .as_ref()
            .and_then(|r| r.source.as_ref())
            .map(|s| *s == anolisa_core::RegisterSource::Console)
            .unwrap_or(false);

        if state != ConsentState::Registered || registered_via_console {
            println!("  Consent State: REGISTERED");
            println!("  Usage Report:  active");
            println!("  Source:        console");
            if let Some(r) = &rec {
                if let Some(t) = r.registration_time {
                    println!("  Registered:    {}", t.format("%Y-%m-%d %H:%M"));
                }
                if let Some(op) = &r.operator {
                    println!("  Operator:      {op}");
                }
            }
            return Ok(());
        }
    }

    match &state {
        ConsentState::InitFresh => {
            println!("  Consent State: INIT (not yet decided)");
            println!("  Usage Report:  disabled (local only)");
            println!();
            println!("  You haven't decided whether to enable Token collection.");
            println!("  Run 'sudo anolisa register' to enable.");
        }
        ConsentState::Unregistered => {
            println!("  Consent State: UNREGISTERED");
            println!("  Usage Report:  disabled (local only)");
            if let Some(r) = &rec
                && let Some(t) = r.registration_time
            {
                let via = format_source(&r.source);
                println!("  Last Registered: {}{via}", t.format("%Y-%m-%d %H:%M"));
            }
            println!();
            println!("  To enable registration: sudo anolisa register");
        }
        ConsentState::Registered => {
            println!("  Consent State: REGISTERED");
            println!("  Usage Report:  active");
            if let Some(r) = &rec {
                if let Some(t) = r.registration_time {
                    let via = format_source(&r.source);
                    println!("  Registered:    {}{via}", t.format("%Y-%m-%d %H:%M"));
                }
                if let Some(op) = &r.operator {
                    println!("  Operator:      {op}");
                }
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

    if let Some(r) = rec {
        if let Some(t) = r.registration_time {
            obj["registration_time"] =
                serde_json::Value::String(t.format("%Y-%m-%dT%H:%M:%SZ").to_string());
        }
        if let Some(op) = &r.operator {
            obj["operator"] = serde_json::Value::String(op.clone());
        }
        if let Some(src) = &r.source {
            obj["source"] = serde_json::Value::String(src.to_string());
        }
    }

    if sysom_active {
        obj["effective_source"] = serde_json::Value::String("sysom".to_string());
        obj["sysom_services_active"] = serde_json::Value::Bool(true);
    }

    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
}

// ── Utility functions ────────────────────────────────────────────────────────

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

fn build_upload_config() -> UploadConfig {
    let mut cfg = UploadConfig::default();
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
    const BOX_INNER_WIDTH: usize = 75;

    let lines = [
        "  \u{1F331} Join the Agentic OS Co-Build Program",
        "  Welcome to Agentic OS \u{2014} the operating system for the Agent era.",
        "  We invite you to become a co-builder and help make this OS",
        "  smarter and more in tune with your needs.",
        "",
        "  By joining, you will get:",
        "    \u{2726} Smarter cosh \u{2014} learns from real user scenarios, more accurate",
        "    \u{2726} Cross-instance Token insights \u{2014} view costs & trends for all",
        "      instances under your account in one dashboard",
        "    \u{2726} Personalized optimization \u{2014} model selection, Token savings,",
        "      Skill recommendations, tailored for you",
        "    \u{2726} Early access to new features \u{2014} beta Skills / new model",
        "      adaptations delivered first",
        "    \u{2726} Product co-build vote \u{2014} your pain points become our next P0",
        "",
        "  Our commitments:",
        "    \u{00B7} Only upload desensitized aggregate statistics",
        "      (token counts, model ID, request counts, time window)",
        "    \u{00B7} Your prompts, conversations, keys, files \u{2014} never leave",
        "      this machine",
        "    \u{00B7} Uses Alibaba Cloud internal network, zero public network",
        "      cost, zero extra configuration",
        "    \u{00B7} You stay in control \u{2014} run 'anolisa unregister'",
        "      to opt out at any time",
        "",
        "  Help us make Agentic OS even better?",
    ];

    let border = "\u{2500}".repeat(BOX_INNER_WIDTH);
    println!("\u{256d}{border}\u{256e}");
    for line in &lines {
        let w = UnicodeWidthStr::width(*line);
        let pad = BOX_INNER_WIDTH.saturating_sub(w);
        println!("\u{2502}{}{}\u{2502}", line, " ".repeat(pad));
    }
    println!("\u{2570}{border}\u{256f}");
}
