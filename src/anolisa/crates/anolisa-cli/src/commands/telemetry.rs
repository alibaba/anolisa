//! `anolisa telemetry` management surface.
//!
//! Independent command surface for the self-hosted telemetry channel: toggling
//! the collection sentinel, managing the named-reporting link id, showing
//! status, and running the uploader. It deliberately does **not** touch the
//! `register` / `unregister` flow — that stays orthogonal.

use std::fs;
use std::path::Path;

use anolisa_core::{
    RegistrationManager, TelemetryChannel, Uploader, generate_link_id, require_root,
};
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::systemd::{self, SystemdError};
use clap::{Parser, Subcommand};

use crate::context::CliContext;
use crate::response::{CliError, render_json};

/// systemd unit that runs the resident upload loop.
const SERVICE_NAME: &str = "anolisa-telemetry";
/// Filename of the unit written into the system unit directory.
const UNIT_FILENAME: &str = "anolisa-telemetry.service";

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
        TelemetryCommands::Enable => handle_enable(ctx),
        TelemetryCommands::Disable => handle_disable(ctx),
        TelemetryCommands::Status { json } => handle_status(json),
        TelemetryCommands::Link => handle_link(),
        TelemetryCommands::Unlink => handle_unlink(),
        TelemetryCommands::Upload { loop_flag } => handle_upload(loop_flag),
        TelemetryCommands::Init => handle_init(ctx),
    }
}

fn require_root_for(command: &str) -> Result<(), CliError> {
    require_root().map_err(|e| CliError::Runtime {
        command: command.to_string(),
        reason: e.to_string(),
    })
}

fn runtime(command: &str, e: impl std::fmt::Display) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: e.to_string(),
    }
}

// ── enable / disable ────────────────────────────────────────────────

/// Enable default collection and bring up the resident uploader.
///
/// `pub(crate)` so the deprecated `register` command can forward here instead of
/// duplicating an inferior (non-persistent) copy of this bring-up.
pub(crate) fn handle_enable(ctx: &CliContext) -> Result<(), CliError> {
    require_root_for("telemetry enable")?;
    // The instance snapshot records personal identifiers only when the user has
    // already authorized named reporting; otherwise it stays anonymous (L2).
    let linked = RegistrationManager::new().read_link_id().is_some();
    let channel = TelemetryChannel::new();
    channel
        .enable_collection(linked)
        .map_err(|e| runtime("telemetry enable", e))?;

    // Bring up the uploader. On systemd hosts install + enable the resident
    // loop service so default collection survives reboot (persistent WantedBy
    // symlink); otherwise, and on any systemd failure, fall back to the lazy
    // detached spawn. The uploader's flock keeps a single instance even if
    // both paths race.
    if systemd_available() {
        if let Err(e) = install_and_enable_service(ctx) {
            eprintln!("warn: could not enable {UNIT_FILENAME} ({e}); falling back to lazy start");
            spawn_uploader_best_effort();
        }
    } else {
        spawn_uploader_best_effort();
    }

    println!("Telemetry collection enabled.");
    Ok(())
}

fn handle_disable(_ctx: &CliContext) -> Result<(), CliError> {
    require_root_for("telemetry disable")?;
    TelemetryChannel::new()
        .disable_collection()
        .map_err(|e| runtime("telemetry disable", e))?;

    // The persistent opt-out marker written above is the authoritative "off":
    // components stop writing and the running loop no-ops then self-exits on
    // its next round. So the systemd teardown must not block — `disable --now`
    // would wait for the loop to drain an in-flight HTTP round (up to the
    // unit's TimeoutStopSec), which makes `disable` appear to hang. Instead we
    // queue a non-blocking stop (an explicit stop also suppresses Restart=, so
    // the unit does not churn) and drop the boot symlink so it stays off across
    // reboots.
    if systemd_available() {
        disable_service();
    }

    println!("Telemetry collection disabled.");
    println!("  The uploader stops shortly; buffered logs are preserved.");
    Ok(())
}

// ── systemd service wiring ──────────────────────────────────

/// Whether this host is booted with systemd (canonical `sd_booted` check).
fn systemd_available() -> bool {
    Path::new("/run/systemd/system").exists()
}

/// Materialize the telemetry unit into the system unit dir and enable it.
///
/// Mirrors `anolisa system setup`: substitute the running binary path into the
/// embedded `.in` template, write it under the system unit dir, reload the
/// daemon, then `systemctl enable --now`. This makes collection persistent
/// across reboots instead of relying on a spawned process that dies on
/// shutdown.
fn install_and_enable_service(ctx: &CliContext) -> Result<(), CliError> {
    const UNIT_TEMPLATE: &str = include_str!("../../../../systemd/anolisa-telemetry.service.in");

    let cmd = "telemetry enable";
    let exe = std::env::current_exe().map_err(|e| runtime(cmd, e))?;
    let unit_content = UNIT_TEMPLATE.replace("@@ANOLISA_BIN@@", &exe.display().to_string());

    let layout = FsLayout::system(ctx.prefix.clone());
    let unit_path = layout.systemd_unit_dir.join(UNIT_FILENAME);
    if let Some(parent) = unit_path.parent() {
        fs::create_dir_all(parent).map_err(|e| runtime(cmd, e))?;
    }
    fs::write(&unit_path, unit_content).map_err(|e| runtime(cmd, e))?;

    // Reload so systemd sees the freshly written unit before enabling it.
    let out = std::process::Command::new("systemctl")
        .arg("daemon-reload")
        .output()
        .map_err(|e| runtime(cmd, e))?;
    if !out.status.success() {
        return Err(runtime(
            cmd,
            format!(
                "systemctl daemon-reload failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }

    systemd::enable_unit(SERVICE_NAME).map_err(|e| runtime(cmd, e))
}

/// Best-effort, non-blocking teardown so `telemetry disable` never hangs.
///
/// See [`handle_disable`] for why we avoid `disable --now`. A missing unit
/// (never installed) is treated as success.
fn disable_service() {
    match systemd::disable_unit_deferred(SERVICE_NAME) {
        Ok(()) | Err(SystemdError::NotFound(_)) => {}
        Err(e) => eprintln!("warn: failed to disable {UNIT_FILENAME}: {e}"),
    }
}

/// Lazy detached uploader spawn; warns on failure but never fails the command.
fn spawn_uploader_best_effort() {
    if let Err(e) = Uploader::default().ensure_running() {
        eprintln!("warn: telemetry enabled, but uploader failed to start: {e}");
    }
}

// ── init (internal, boot self-heal) ─────────────────────────────────

/// Materialize the ops channel without touching the opt-out marker.
///
/// Wired as the telemetry service's `ExecStartPre`: on every boot it ensures
/// the ops directory / `.jsonl` files / logrotate exist so the upload loop can
/// tail them, but never clears a user's opt-out (that stays persistent).
/// Collection remains on by default (no marker); disabled hosts self-exit in
/// the loop's next round.
fn handle_init(_ctx: &CliContext) -> Result<(), CliError> {
    require_root_for("telemetry init")?;
    let linked = RegistrationManager::new().read_link_id().is_some();
    TelemetryChannel::new()
        .ensure_ops_channel(linked)
        .map_err(|e| runtime("telemetry init", e))
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

// ── link / unlink ───────────────────────────────────────────────────

fn handle_link() -> Result<(), CliError> {
    require_root_for("telemetry link")?;
    let mgr = RegistrationManager::new();
    if let Some(id) = mgr.read_link_id() {
        println!("Already linked (link id: {id}).");
        return Ok(());
    }
    let id = generate_link_id();
    mgr.do_link(&id).map_err(|e| runtime("telemetry link", e))?;
    println!("Linked to named reporting.");
    println!("  link id: {id}");

    // Now authorized: record the instance identity so the named endpoint
    // receives it. Skip when collection is off (the next `enable` writes it)
    // and never enable collection as a side effect of linking.
    let channel = TelemetryChannel::new();
    if channel.is_enabled()
        && let Err(e) = channel.append_instance_snapshot(true)
    {
        eprintln!("warn: linked, but failed to record instance snapshot: {e}");
    }
    Ok(())
}

fn handle_unlink() -> Result<(), CliError> {
    require_root_for("telemetry unlink")?;
    RegistrationManager::new()
        .do_unlink()
        .map_err(|e| runtime("telemetry unlink", e))?;

    // Withdrawing consent must also erase the on-disk identity so the uploader
    // stops attaching it. Best-effort: the link_id is already gone, so the
    // uploader would omit the fields regardless.
    if let Err(e) = TelemetryChannel::new().forget_identity() {
        eprintln!("warn: unlinked, but failed to erase identity cache: {e}");
    }

    println!("Unlinked from named reporting.");
    Ok(())
}

// ── upload (internal) ───────────────────────────────────────────────

fn handle_upload(loop_flag: bool) -> Result<(), CliError> {
    let uploader = Uploader::default();
    let result = if loop_flag {
        uploader.run_loop()
    } else {
        uploader.run_once()
    };
    result.map_err(|e| runtime("telemetry upload", e))
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
