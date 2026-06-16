//! `anolisa system` command surface — daemon lifecycle management.
//!
//! Subcommands:
//! - `serve` — start the system-helper daemon (foreground, for systemd).
//! - `setup` — one-time installation of the system helper daemon.
//! - `teardown` — remove system helper: stop service, delete unit + binary.
//! - `status` — check system helper health (read-only, no root required).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use clap::{Parser, Subcommand};
use serde::Serialize;

use anolisa_core::daemon_server::DaemonServer;
use anolisa_core::system_helper::{HelperRequest, HelperResponse};
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::ipc::{self, SYSTEM_HELPER_SOCKET};
use anolisa_platform::privilege;
use anolisa_platform::systemd::{self, SystemdError};

use crate::context::CliContext;
use crate::response::{self, CliError};

#[derive(Parser)]
pub struct SystemArgs {
    #[command(subcommand)]
    pub command: SystemCommands,
}

#[derive(Subcommand)]
pub enum SystemCommands {
    /// Start the system helper daemon (foreground, for systemd)
    Serve {
        /// Socket path override
        #[arg(long, default_value = SYSTEM_HELPER_SOCKET)]
        socket: String,
    },
    /// One-time setup: install system helper daemon
    Setup {
        /// Override helper binary destination (defaults to FsLayout libexec_dir)
        #[arg(long)]
        helper_path: Option<String>,

        /// Upgrade existing installation
        #[arg(long)]
        upgrade: bool,
    },
    /// Remove system helper: stop service, delete unit + binary
    Teardown,
    /// Check system helper health
    Status {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

pub fn handle(args: SystemArgs, ctx: &CliContext) -> Result<(), CliError> {
    match args.command {
        SystemCommands::Serve { socket } => handle_serve(&socket),
        SystemCommands::Setup {
            helper_path,
            upgrade,
        } => handle_setup(helper_path.as_deref(), upgrade, ctx),
        SystemCommands::Teardown => handle_teardown(ctx),
        SystemCommands::Status { json } => handle_status(json, ctx),
    }
}

fn handle_serve(socket: &str) -> Result<(), CliError> {
    if !privilege::is_root() {
        return Err(CliError::PermissionDenied {
            command: "system serve".to_string(),
            reason: "the system helper daemon must run as root (euid 0)".to_string(),
            hint: Some("run with sudo or as a systemd service".to_string()),
        });
    }

    let server = DaemonServer::new(socket);
    server.run().map_err(|e| CliError::Runtime {
        command: "system serve".to_string(),
        reason: format!("daemon exited with error: {e}"),
    })
}

// ─── Setup ───────────────────────────────────────────────────────────────────

const SERVICE_NAME: &str = "anolisa-system-helper";
const UNIT_FILENAME: &str = "anolisa-system-helper.service";
const RUNTIME_DIR: &str = "/run/anolisa";
const ANOLISA_GROUP: &str = "anolisa";

/// Resolve the system-mode FsLayout from context.
fn resolve_layout(ctx: &CliContext) -> FsLayout {
    FsLayout::system(ctx.prefix.clone())
}

fn handle_setup(
    helper_path_override: Option<&str>,
    upgrade: bool,
    ctx: &CliContext,
) -> Result<(), CliError> {
    let cmd = "system setup";

    // 1. Check root
    if !privilege::is_root() {
        return Err(CliError::PermissionDenied {
            command: cmd.to_string(),
            reason: "system setup must be run as root (euid 0)".to_string(),
            hint: Some("run with: sudo anolisa system setup".to_string()),
        });
    }

    let layout = resolve_layout(ctx);
    let helper_path: PathBuf = match helper_path_override {
        Some(p) => PathBuf::from(p),
        None => layout.libexec_dir.join("anolisa-system-helper"),
    };
    let unit_path = layout.systemd_unit_dir.join(UNIT_FILENAME);

    // 2. Stop the service if it's running (avoids "Text file busy" on binary overwrite)
    let _ = Command::new("systemctl")
        .args(["stop", SERVICE_NAME])
        .output();

    // 3. Copy current exe to helper_path
    let current_exe = std::env::current_exe().map_err(|e| CliError::Runtime {
        command: cmd.to_string(),
        reason: format!("failed to determine current executable path: {e}"),
    })?;

    // Ensure parent directory exists
    if let Some(parent) = helper_path.parent() {
        fs::create_dir_all(parent).map_err(|e| CliError::Runtime {
            command: cmd.to_string(),
            reason: format!("failed to create directory {}: {e}", parent.display()),
        })?;
    }

    fs::copy(&current_exe, &helper_path).map_err(|e| CliError::Runtime {
        command: cmd.to_string(),
        reason: format!("failed to copy binary to {}: {e}", helper_path.display()),
    })?;
    eprintln!(
        "[setup] installed helper binary → {}",
        helper_path.display()
    );

    // 4. Set helper permissions (0755)
    fs::set_permissions(&helper_path, fs::Permissions::from_mode(0o755)).map_err(|e| {
        CliError::Runtime {
            command: cmd.to_string(),
            reason: format!(
                "failed to set permissions on {}: {e}",
                helper_path.display()
            ),
        }
    })?;

    if !upgrade {
        // 5. Create anolisa system group (ignore if already exists)
        setup_group(cmd)?;

        // 6. Add calling user to anolisa group
        setup_user_membership(cmd)?;
    }

    // 7. Create /run/anolisa/ directory
    setup_runtime_dir(cmd)?;

    // 8. Generate systemd unit file
    write_unit_file(cmd, &helper_path, &unit_path)?;

    // 9. Deploy sandbox.toml configuration file
    deploy_sandbox_config(cmd, &layout)?;

    // 10. systemctl daemon-reload + enable + start/restart
    reload_and_start_service(cmd, upgrade)?;

    // 11. Verify socket
    verify_socket(cmd)?;

    // 12. Success
    eprintln!("[setup] anolisa system helper is running and verified.");
    Ok(())
}

fn setup_group(cmd: &str) -> Result<(), CliError> {
    let output = Command::new("groupadd")
        .args(["-r", ANOLISA_GROUP])
        .output()
        .map_err(|e| CliError::Runtime {
            command: cmd.to_string(),
            reason: format!("failed to run groupadd: {e}"),
        })?;

    // Exit code 9 means group already exists — not an error.
    if !output.status.success() && output.status.code() != Some(9) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::Runtime {
            command: cmd.to_string(),
            reason: format!("groupadd -r {ANOLISA_GROUP} failed: {stderr}"),
        });
    }
    eprintln!("[setup] system group '{ANOLISA_GROUP}' ensured");
    Ok(())
}

fn setup_user_membership(cmd: &str) -> Result<(), CliError> {
    let user = std::env::var("SUDO_USER").unwrap_or_default();
    if user.is_empty() {
        eprintln!("[setup] warning: $SUDO_USER not set, skipping group membership");
        return Ok(());
    }

    let output = Command::new("usermod")
        .args(["-aG", ANOLISA_GROUP, &user])
        .output()
        .map_err(|e| CliError::Runtime {
            command: cmd.to_string(),
            reason: format!("failed to run usermod: {e}"),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::Runtime {
            command: cmd.to_string(),
            reason: format!("usermod -aG {ANOLISA_GROUP} {user} failed: {stderr}"),
        });
    }
    eprintln!("[setup] user '{user}' added to group '{ANOLISA_GROUP}'");
    Ok(())
}

fn setup_runtime_dir(cmd: &str) -> Result<(), CliError> {
    fs::create_dir_all(RUNTIME_DIR).map_err(|e| CliError::Runtime {
        command: cmd.to_string(),
        reason: format!("failed to create {RUNTIME_DIR}: {e}"),
    })?;

    // chgrp anolisa /run/anolisa
    let output = Command::new("chgrp")
        .args([ANOLISA_GROUP, RUNTIME_DIR])
        .output()
        .map_err(|e| CliError::Runtime {
            command: cmd.to_string(),
            reason: format!("failed to run chgrp: {e}"),
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::Runtime {
            command: cmd.to_string(),
            reason: format!("chgrp {ANOLISA_GROUP} {RUNTIME_DIR} failed: {stderr}"),
        });
    }

    // chmod 0750 /run/anolisa
    fs::set_permissions(RUNTIME_DIR, fs::Permissions::from_mode(0o750)).map_err(|e| {
        CliError::Runtime {
            command: cmd.to_string(),
            reason: format!("failed to chmod {RUNTIME_DIR}: {e}"),
        }
    })?;
    eprintln!("[setup] runtime directory {RUNTIME_DIR} ready");
    Ok(())
}

/// Determine the sandbox.toml deployment path.
///
/// - System-level (euid==0): `<layout.etc_dir>/sandbox.toml`
/// - User-level: `$XDG_CONFIG_HOME/anolisa/sandbox.toml`
fn resolve_sandbox_config_path(layout: &FsLayout) -> PathBuf {
    if privilege::is_root() {
        layout.etc_dir.join("sandbox.toml")
    } else {
        let config_home = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            format!("{home}/.config")
        });
        PathBuf::from(config_home)
            .join("anolisa")
            .join("sandbox.toml")
    }
}

fn deploy_sandbox_config(cmd: &str, layout: &FsLayout) -> Result<(), CliError> {
    const SANDBOX_TOML_TEMPLATE: &str = include_str!("../../../../manifests/sandbox.toml");

    let config_path = resolve_sandbox_config_path(layout);

    if config_path.exists() {
        eprintln!(
            "[setup] sandbox.toml already exists, skipping (remove the file manually to regenerate)"
        );
        return Ok(());
    }

    // Ensure parent directory exists
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).map_err(|e| CliError::Runtime {
            command: cmd.to_string(),
            reason: format!("failed to create directory {}: {e}", parent.display()),
        })?;
    }

    fs::write(&config_path, SANDBOX_TOML_TEMPLATE).map_err(|e| CliError::Runtime {
        command: cmd.to_string(),
        reason: format!(
            "failed to write sandbox.toml to {}: {e}",
            config_path.display()
        ),
    })?;

    eprintln!(
        "[setup] sandbox.toml deployed \u{2192} {}",
        config_path.display()
    );
    Ok(())
}

fn write_unit_file(cmd: &str, helper_path: &Path, unit_path: &Path) -> Result<(), CliError> {
    const UNIT_TEMPLATE: &str =
        include_str!("../../../../systemd/anolisa-system-helper.service.in");

    let unit_content = UNIT_TEMPLATE
        .replace("@@HELPER_PATH@@", &helper_path.display().to_string())
        .replace("@@SOCKET_PATH@@", SYSTEM_HELPER_SOCKET);

    // Ensure unit directory exists
    if let Some(parent) = unit_path.parent() {
        fs::create_dir_all(parent).map_err(|e| CliError::Runtime {
            command: cmd.to_string(),
            reason: format!("failed to create directory {}: {e}", parent.display()),
        })?;
    }

    fs::write(unit_path, &unit_content).map_err(|e| CliError::Runtime {
        command: cmd.to_string(),
        reason: format!("failed to write unit file {}: {e}", unit_path.display()),
    })?;
    eprintln!("[setup] systemd unit written → {}", unit_path.display());
    Ok(())
}

fn reload_and_start_service(cmd: &str, upgrade: bool) -> Result<(), CliError> {
    run_systemctl(cmd, &["daemon-reload"])?;
    run_systemctl(cmd, &["enable", SERVICE_NAME])?;

    if upgrade {
        run_systemctl(cmd, &["restart", SERVICE_NAME])?;
    } else {
        run_systemctl(cmd, &["start", SERVICE_NAME])?;
    }
    eprintln!("[setup] service {SERVICE_NAME} active");
    Ok(())
}

fn run_systemctl(cmd: &str, args: &[&str]) -> Result<(), CliError> {
    let output = Command::new("systemctl")
        .args(args)
        .output()
        .map_err(|e| CliError::Runtime {
            command: cmd.to_string(),
            reason: format!("failed to run systemctl {}: {e}", args.join(" ")),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::Runtime {
            command: cmd.to_string(),
            reason: format!("systemctl {} failed: {stderr}", args.join(" ")),
        });
    }
    Ok(())
}

fn verify_socket(cmd: &str) -> Result<(), CliError> {
    // Wait briefly for the socket to appear (daemon may take a moment to start).
    let socket_path = Path::new(SYSTEM_HELPER_SOCKET);
    let mut attempts = 0;
    while !socket_path.exists() && attempts < 10 {
        thread::sleep(Duration::from_millis(300));
        attempts += 1;
    }

    if !socket_path.exists() {
        return Err(CliError::Runtime {
            command: cmd.to_string(),
            reason: format!("socket {SYSTEM_HELPER_SOCKET} did not appear within 3 seconds"),
        });
    }

    // Try a handshake to validate the daemon is responding.
    let mut stream =
        std::os::unix::net::UnixStream::connect(SYSTEM_HELPER_SOCKET).map_err(|e| {
            CliError::Runtime {
                command: cmd.to_string(),
                reason: format!("failed to connect to {SYSTEM_HELPER_SOCKET}: {e}"),
            }
        })?;

    let handshake = HelperRequest::Handshake {
        cli_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    ipc::send_message(&mut stream, &handshake).map_err(|e| CliError::Runtime {
        command: cmd.to_string(),
        reason: format!("handshake send failed: {e}"),
    })?;

    let resp: HelperResponse = ipc::recv_message(&mut stream).map_err(|e| CliError::Runtime {
        command: cmd.to_string(),
        reason: format!("handshake recv failed: {e}"),
    })?;

    match resp {
        HelperResponse::HandshakeOk { compatible, .. } if compatible => {
            eprintln!("[setup] handshake verified — helper is operational");
            Ok(())
        }
        HelperResponse::HandshakeOk { compatible, .. } if !compatible => Err(CliError::Runtime {
            command: cmd.to_string(),
            reason: "handshake succeeded but version is incompatible".to_string(),
        }),
        other => Err(CliError::Runtime {
            command: cmd.to_string(),
            reason: format!("unexpected handshake response: {other:?}"),
        }),
    }
}

// ─── Teardown ────────────────────────────────────────────────────────────────

fn handle_teardown(ctx: &CliContext) -> Result<(), CliError> {
    let cmd = "system teardown";

    // 1. Check root
    if !privilege::is_root() {
        return Err(CliError::PermissionDenied {
            command: cmd.to_string(),
            reason: "system teardown must be run as root (euid 0)".to_string(),
            hint: Some("run with: sudo anolisa system teardown".to_string()),
        });
    }

    let layout = resolve_layout(ctx);
    let helper_path = layout.libexec_dir.join("anolisa-system-helper");
    let unit_path = layout.systemd_unit_dir.join(UNIT_FILENAME);
    let mut warnings: Vec<String> = Vec::new();

    // 2. Stop service (ignore "not loaded" errors)
    if let Err(e) = run_systemctl(cmd, &["stop", SERVICE_NAME]) {
        let msg = format!("{e}");
        if msg.contains("not loaded") || msg.contains("not found") {
            warnings.push(format!(
                "service {SERVICE_NAME} was not loaded (already stopped)"
            ));
        } else {
            warnings.push(format!("failed to stop {SERVICE_NAME}: {msg}"));
        }
    } else {
        eprintln!("[teardown] stopped {SERVICE_NAME}");
    }

    // 3. Disable service (ignore errors)
    if let Err(e) = run_systemctl(cmd, &["disable", SERVICE_NAME]) {
        warnings.push(format!("failed to disable {SERVICE_NAME}: {e}"));
    } else {
        eprintln!("[teardown] disabled {SERVICE_NAME}");
    }

    // 4. Delete unit file
    if unit_path.exists() {
        if let Err(e) = fs::remove_file(&unit_path) {
            warnings.push(format!(
                "failed to remove unit file {}: {e}",
                unit_path.display()
            ));
        } else {
            eprintln!("[teardown] removed unit file {}", unit_path.display());
        }
    } else {
        warnings.push(format!("unit file {} already removed", unit_path.display()));
    }

    // 5. Reload systemd
    if let Err(e) = run_systemctl(cmd, &["daemon-reload"]) {
        warnings.push(format!("daemon-reload failed: {e}"));
    } else {
        eprintln!("[teardown] systemd daemon-reload complete");
    }

    // 6. Delete helper binary
    if helper_path.exists() {
        if let Err(e) = fs::remove_file(&helper_path) {
            warnings.push(format!(
                "failed to remove helper binary {}: {e}",
                helper_path.display()
            ));
        } else {
            eprintln!("[teardown] removed helper binary {}", helper_path.display());
        }
    } else {
        warnings.push(format!(
            "helper binary {} already removed",
            helper_path.display()
        ));
    }

    // 7. Remove sandbox.toml config file
    let sandbox_config_path = resolve_sandbox_config_path(&layout);
    if sandbox_config_path.exists() {
        if let Err(e) = fs::remove_file(&sandbox_config_path) {
            warnings.push(format!(
                "failed to remove sandbox.toml {}: {e}",
                sandbox_config_path.display()
            ));
        } else {
            eprintln!("[teardown] removed sandbox.toml");
        }
    }

    // 8. Optionally remove /run/anolisa/
    let runtime_path = Path::new(RUNTIME_DIR);
    if runtime_path.exists() {
        if let Err(e) = fs::remove_dir_all(runtime_path) {
            warnings.push(format!("failed to remove {RUNTIME_DIR}: {e}"));
        } else {
            eprintln!("[teardown] removed runtime directory {RUNTIME_DIR}");
        }
    }

    // 9. Print warnings and success
    for w in &warnings {
        eprintln!("[teardown] warning: {w}");
    }
    eprintln!("[teardown] system helper teardown complete.");
    Ok(())
}

// ─── Status command ─────────────────────────────────────────────────────────────────

const STATUS_SERVICE_UNIT: &str = "anolisa-system-helper.service";

/// JSON output payload for `system status --json`.
#[derive(Debug, Serialize)]
struct StatusReport {
    service_active: bool,
    socket_exists: bool,
    socket_connectable: bool,
    helper_version: Option<String>,
    cli_version: String,
    version_compatible: bool,
    uptime_secs: Option<u64>,
    last_operation: Option<String>,
    last_operation_time: Option<String>,
}

fn handle_status(json: bool, ctx: &CliContext) -> Result<(), CliError> {
    let cli_version = env!("CARGO_PKG_VERSION").to_string();

    // 1. Check systemd service state.
    let service_state = check_service_state();

    // 2. Check socket file existence.
    let socket_exists = Path::new(SYSTEM_HELPER_SOCKET).exists();

    // 3. Try connect + handshake + SystemStatus.
    let (socket_connectable, handshake_info, status_info) = if socket_exists {
        try_status_connection(&cli_version)
    } else {
        (false, None, None)
    };

    // Derive fields.
    let helper_version = handshake_info.as_ref().map(|(v, _)| v.clone());
    let version_compatible = handshake_info
        .as_ref()
        .map(|(_, compat)| *compat)
        .unwrap_or(false);

    let uptime_secs = status_info.as_ref().map(|s| s.0);
    let last_operation = status_info.as_ref().and_then(|s| s.1.clone());
    let last_operation_time = status_info.as_ref().and_then(|s| s.2.clone());

    let report = StatusReport {
        service_active: service_state == StatusServiceState::Active,
        socket_exists,
        socket_connectable,
        helper_version: helper_version.clone(),
        cli_version: cli_version.clone(),
        version_compatible,
        uptime_secs,
        last_operation: last_operation.clone(),
        last_operation_time: last_operation_time.clone(),
    };

    if json || ctx.json {
        return response::render_json("system status", report);
    }

    // Human-readable output.
    print_status_human(
        &service_state,
        socket_exists,
        socket_connectable,
        helper_version.as_deref(),
        &cli_version,
        version_compatible,
        uptime_secs,
        last_operation.as_deref(),
        last_operation_time.as_deref(),
    );

    Ok(())
}

// ─── Status helpers ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusServiceState {
    Active,
    Inactive,
    Failed,
    NotInstalled,
    Unknown,
}

impl StatusServiceState {
    fn label(self) -> &'static str {
        match self {
            Self::Active => "active (running)",
            Self::Inactive => "inactive (stopped)",
            Self::Failed => "failed",
            Self::NotInstalled => "not installed",
            Self::Unknown => "unknown",
        }
    }
}

fn check_service_state() -> StatusServiceState {
    match systemd::unit_status(STATUS_SERVICE_UNIT) {
        Ok(status) => {
            if status.active {
                StatusServiceState::Active
            } else {
                StatusServiceState::Inactive
            }
        }
        Err(SystemdError::NotFound(_)) => StatusServiceState::NotInstalled,
        Err(SystemdError::CommandFailed(ref msg)) if msg.to_lowercase().contains("failed") => {
            StatusServiceState::Failed
        }
        Err(_) => StatusServiceState::Unknown,
    }
}

/// Handshake result: (helper_version, compatible)
type HandshakeInfo = (String, bool);
/// Status info: (uptime_secs, last_operation, last_operation_time)
type StatusInfo = (u64, Option<String>, Option<String>);

/// Attempt to connect to the helper socket, perform handshake, and query
/// system status. Returns (connectable, handshake_info, status_info).
fn try_status_connection(cli_version: &str) -> (bool, Option<HandshakeInfo>, Option<StatusInfo>) {
    let mut stream = match UnixStream::connect(SYSTEM_HELPER_SOCKET) {
        Ok(s) => s,
        Err(_) => return (false, None, None),
    };

    // Handshake.
    let handshake_req = HelperRequest::Handshake {
        cli_version: cli_version.to_string(),
    };
    if ipc::send_message(&mut stream, &handshake_req).is_err() {
        return (true, None, None);
    }
    let handshake_resp: HelperResponse = match ipc::recv_message(&mut stream) {
        Ok(r) => r,
        Err(_) => return (true, None, None),
    };
    let handshake_info = match &handshake_resp {
        HelperResponse::HandshakeOk {
            helper_version,
            compatible,
        } => Some((helper_version.clone(), *compatible)),
        _ => None,
    };

    // SystemStatus query.
    if ipc::send_message(&mut stream, &HelperRequest::SystemStatus).is_err() {
        return (true, handshake_info, None);
    }
    let status_resp: HelperResponse = match ipc::recv_message(&mut stream) {
        Ok(r) => r,
        Err(_) => return (true, handshake_info, None),
    };
    let status_info = match status_resp {
        HelperResponse::Status {
            uptime_secs,
            last_operation,
            last_operation_time,
            ..
        } => Some((uptime_secs, last_operation, last_operation_time)),
        _ => None,
    };

    (true, handshake_info, status_info)
}

#[allow(clippy::too_many_arguments)]
fn print_status_human(
    service_state: &StatusServiceState,
    socket_exists: bool,
    socket_connectable: bool,
    helper_version: Option<&str>,
    cli_version: &str,
    version_compatible: bool,
    uptime_secs: Option<u64>,
    last_operation: Option<&str>,
    last_operation_time: Option<&str>,
) {
    println!("anolisa system helper:");
    println!("  Status:      {}", service_state.label());

    let socket_label = if socket_connectable {
        format!("{SYSTEM_HELPER_SOCKET} [connected]")
    } else if socket_exists {
        format!("{SYSTEM_HELPER_SOCKET} [not connectable]")
    } else {
        format!("{SYSTEM_HELPER_SOCKET} [missing]")
    };
    println!("  Socket:      {socket_label}");

    if let Some(hv) = helper_version {
        let compat_mark = if version_compatible {
            "\u{2713}"
        } else {
            "\u{26a0} version mismatch"
        };
        println!("  Version:     {hv} (CLI: {cli_version}) {compat_mark}");
    }

    if let Some(secs) = uptime_secs {
        println!("  Uptime:      {}", format_status_uptime(secs));
    }

    if let Some(op) = last_operation {
        let time_suffix = last_operation_time
            .map(|t| format!(" ({t})"))
            .unwrap_or_default();
        println!("  Last op:     {op}{time_suffix}");
    }

    println!();
    if *service_state == StatusServiceState::NotInstalled || !socket_exists {
        println!("  hint: run 'sudo anolisa system setup' to install");
    } else if socket_connectable && version_compatible {
        println!("  All checks passed.");
    } else if !version_compatible && helper_version.is_some() {
        println!("  warning: CLI and helper versions differ; consider restarting the helper.");
    }
}

fn format_status_uptime(secs: u64) -> String {
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {mins:02}m")
    } else {
        format!("{mins}m")
    }
}
