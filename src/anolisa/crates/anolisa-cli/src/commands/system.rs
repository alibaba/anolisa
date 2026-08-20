//! `anolisa system` command surface — daemon lifecycle management.
//!
//! Subcommands:
//! - `serve` — start the system-helper daemon (foreground, for systemd).
//! - `setup` — one-time installation of the system helper daemon.
//! - `teardown` — remove system helper: stop service, delete unit + binary.
//! - `status` — check system helper health (read-only, no root required).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use clap::{Parser, Subcommand};
use serde::Serialize;

use anolisa_core::daemon_server::DaemonServer;
use anolisa_platform::command::CommandRunner;
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::ipc::SYSTEM_HELPER_SOCKET;
use anolisa_platform::privilege;
use anolisa_platform::systemd::{Systemd, SystemdError};

use crate::context::CliContext;
use crate::helper_client::{HandshakeResult, HelperClient, HelperClientError, HelperStatus};
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
    require_root(
        "system serve",
        "the system helper daemon must run as root (euid 0)",
        "run with sudo or as a systemd service",
    )?;

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
    ctx.visible_system_layout().clone()
}

fn require_root(command: &str, reason: &str, hint: &str) -> Result<(), CliError> {
    if privilege::is_root() {
        return Ok(());
    }

    Err(CliError::PermissionDenied {
        command: command.to_string(),
        reason: reason.to_string(),
        hint: Some(hint.to_string()),
    })
}

fn handle_setup(
    helper_path_override: Option<&str>,
    upgrade: bool,
    ctx: &CliContext,
) -> Result<(), CliError> {
    let cmd = "system setup";

    require_root(
        cmd,
        "system setup must be run as root (euid 0)",
        "run with: sudo anolisa system setup",
    )?;

    let layout = resolve_layout(ctx);
    let helper_path: PathBuf = match helper_path_override {
        Some(p) => PathBuf::from(p),
        None => layout.libexec_dir.join("anolisa-system-helper"),
    };
    let unit_path = layout.systemd_unit_dir.join(UNIT_FILENAME);
    let systemd = Systemd::system();

    // 2. Stop the service if it's running (avoids "Text file busy" on binary overwrite)
    stop_service_before_setup(&systemd);

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
    reload_and_start_service(cmd, upgrade, &systemd)?;

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

fn reload_and_start_service<R: CommandRunner>(
    cmd: &str,
    upgrade: bool,
    systemd: &Systemd<R>,
) -> Result<(), CliError> {
    systemd
        .daemon_reload()
        .map_err(|error| systemd_cli_error(cmd, &["daemon-reload"], error))?;
    systemd
        .enable_unit_file(SERVICE_NAME)
        .map_err(|error| systemd_cli_error(cmd, &["enable", SERVICE_NAME], error))?;

    if upgrade {
        systemd
            .restart_unit(SERVICE_NAME)
            .map_err(|error| systemd_cli_error(cmd, &["restart", SERVICE_NAME], error))?;
    } else {
        systemd
            .start_unit(SERVICE_NAME)
            .map_err(|error| systemd_cli_error(cmd, &["start", SERVICE_NAME], error))?;
    }
    eprintln!("[setup] service {SERVICE_NAME} active");
    Ok(())
}

fn stop_service_before_setup<R: CommandRunner>(systemd: &Systemd<R>) {
    let _ = systemd.stop_unit(SERVICE_NAME);
}

fn systemd_cli_error(cmd: &str, args: &[&str], error: SystemdError) -> CliError {
    let reason = match error {
        SystemdError::Spawn { source, .. } => {
            format!("failed to run systemctl {}: {source}", args.join(" "))
        }
        SystemdError::NonZeroExit(failure) => {
            format!("systemctl {} failed: {}", args.join(" "), failure.stderr)
        }
        SystemdError::NotFound(unit) => {
            format!(
                "systemctl {} failed: service not found: {unit}",
                args.join(" ")
            )
        }
    };
    CliError::Runtime {
        command: cmd.to_string(),
        reason,
    }
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

    verify_helper_connection(cmd, || HelperClient::connect(socket_path))
}

fn verify_helper_connection<F>(cmd: &str, connect: F) -> Result<(), CliError>
where
    F: FnOnce() -> Result<HelperClient, HelperClientError>,
{
    let mut client = connect().map_err(|error| CliError::Runtime {
        command: cmd.to_string(),
        reason: verify_connection_error(error),
    })?;
    let handshake = client
        .handshake(env!("CARGO_PKG_VERSION"))
        .map_err(|error| CliError::Runtime {
            command: cmd.to_string(),
            reason: verify_connection_error(error),
        })?;
    if !handshake.compatible {
        return Err(CliError::Runtime {
            command: cmd.to_string(),
            reason: "handshake succeeded but version is incompatible".to_string(),
        });
    }
    eprintln!("[setup] handshake verified — helper is operational");
    Ok(())
}

fn verify_connection_error(error: HelperClientError) -> String {
    match error {
        HelperClientError::Connect { path, source } => {
            format!("failed to connect to {}: {source}", path.display())
        }
        HelperClientError::Send { source, .. } => format!("handshake send failed: {source}"),
        HelperClientError::Receive { source, .. } => format!("handshake recv failed: {source}"),
        HelperClientError::Remote { code, message, .. } => format!(
            "unexpected handshake response: Error {{ code: {code:?}, message: {message:?} }}"
        ),
        HelperClientError::UnexpectedResponse { response, .. } => {
            format!("unexpected handshake response: {response:?}")
        }
    }
}

// ─── Teardown ────────────────────────────────────────────────────────────────

fn handle_teardown(ctx: &CliContext) -> Result<(), CliError> {
    let cmd = "system teardown";

    require_root(
        cmd,
        "system teardown must be run as root (euid 0)",
        "run with: sudo anolisa system teardown",
    )?;

    let layout = resolve_layout(ctx);
    let helper_path = layout.libexec_dir.join("anolisa-system-helper");
    let unit_path = layout.systemd_unit_dir.join(UNIT_FILENAME);
    let mut warnings: Vec<String> = Vec::new();
    let systemd = Systemd::system();

    // 2-3. Stop and disable service while retaining failures as warnings.
    stop_and_disable_service(cmd, &systemd, &mut warnings);

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
    reload_systemd_after_teardown(cmd, &systemd, &mut warnings);

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

fn stop_and_disable_service<R: CommandRunner>(
    cmd: &str,
    systemd: &Systemd<R>,
    warnings: &mut Vec<String>,
) {
    match systemd.stop_unit(SERVICE_NAME) {
        Ok(()) => eprintln!("[teardown] stopped {SERVICE_NAME}"),
        Err(SystemdError::NotFound(_)) => {
            warnings.push(format!(
                "service {SERVICE_NAME} was not loaded (already stopped)"
            ));
        }
        Err(error @ SystemdError::NonZeroExit(_)) => {
            if matches!(
                systemd.unit_status(SERVICE_NAME),
                Err(SystemdError::NotFound(_))
            ) {
                warnings.push(format!(
                    "service {SERVICE_NAME} was not loaded (already stopped)"
                ));
            } else {
                let error = systemd_cli_error(cmd, &["stop", SERVICE_NAME], error);
                warnings.push(format!("failed to stop {SERVICE_NAME}: {error}"));
            }
        }
        Err(error) => {
            let error = systemd_cli_error(cmd, &["stop", SERVICE_NAME], error);
            warnings.push(format!("failed to stop {SERVICE_NAME}: {error}"));
        }
    }

    match systemd.disable_unit_file(SERVICE_NAME) {
        Ok(()) => eprintln!("[teardown] disabled {SERVICE_NAME}"),
        Err(error) => {
            let error = systemd_cli_error(cmd, &["disable", SERVICE_NAME], error);
            warnings.push(format!("failed to disable {SERVICE_NAME}: {error}"));
        }
    }
}

fn reload_systemd_after_teardown<R: CommandRunner>(
    cmd: &str,
    systemd: &Systemd<R>,
    warnings: &mut Vec<String>,
) {
    if let Err(error) = systemd.daemon_reload() {
        let error = systemd_cli_error(cmd, &["daemon-reload"], error);
        warnings.push(format!("daemon-reload failed: {error}"));
    } else {
        eprintln!("[teardown] systemd daemon-reload complete");
    }
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
    let connection = if socket_exists {
        try_status_connection(&cli_version)
    } else {
        HelperConnectionStatus::disconnected()
    };

    // Derive fields.
    let helper_version = connection
        .handshake
        .as_ref()
        .map(|handshake| handshake.helper_version.clone());
    let version_compatible = connection
        .handshake
        .as_ref()
        .map(|handshake| handshake.compatible)
        .unwrap_or(false);

    let uptime_secs = connection.status.as_ref().map(|status| status.uptime_secs);
    let last_operation = connection
        .status
        .as_ref()
        .and_then(|status| status.last_operation.clone());
    let last_operation_time = connection
        .status
        .as_ref()
        .and_then(|status| status.last_operation_time.clone());

    let report = StatusReport {
        service_active: service_state == StatusServiceState::Active,
        socket_exists,
        socket_connectable: connection.connectable,
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
        connection.connectable,
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
    match Systemd::system().unit_status(STATUS_SERVICE_UNIT) {
        Ok(status) => {
            if status.failed {
                StatusServiceState::Failed
            } else if status.active {
                StatusServiceState::Active
            } else {
                StatusServiceState::Inactive
            }
        }
        Err(SystemdError::NotFound(_)) => StatusServiceState::NotInstalled,
        Err(_) => StatusServiceState::Unknown,
    }
}

#[derive(Debug)]
struct HelperConnectionStatus {
    connectable: bool,
    handshake: Option<HandshakeResult>,
    status: Option<HelperStatus>,
}

impl HelperConnectionStatus {
    fn disconnected() -> Self {
        Self {
            connectable: false,
            handshake: None,
            status: None,
        }
    }
}

/// Attempt to connect to the helper socket, perform handshake, and query
/// system status while retaining partial typed evidence.
fn try_status_connection(cli_version: &str) -> HelperConnectionStatus {
    try_status_connection_with(cli_version, || {
        HelperClient::connect(Path::new(SYSTEM_HELPER_SOCKET))
    })
}

fn try_status_connection_with<F>(cli_version: &str, connect: F) -> HelperConnectionStatus
where
    F: FnOnce() -> Result<HelperClient, HelperClientError>,
{
    let mut client = match connect() {
        Ok(client) => client,
        Err(_) => return HelperConnectionStatus::disconnected(),
    };

    let handshake = match client.handshake(cli_version) {
        Ok(handshake) => handshake,
        Err(_) => {
            return HelperConnectionStatus {
                connectable: true,
                handshake: None,
                status: None,
            };
        }
    };

    let status = if handshake.compatible {
        client.system_status().ok()
    } else {
        None
    };
    HelperConnectionStatus {
        connectable: true,
        handshake: Some(handshake),
        status,
    }
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io;
    use std::rc::Rc;

    use anolisa_core::system_helper::HelperResponse;
    use anolisa_platform::command::{CommandOutput, CommandRunner};

    use super::*;
    use crate::helper_client::ScriptedTransport;

    enum FakeOutcome {
        Output(CommandOutput),
        Spawn(io::ErrorKind),
    }

    type FakeCalls = Rc<RefCell<VecDeque<(Vec<String>, FakeOutcome)>>>;

    struct FakeSystemdRunner {
        calls: FakeCalls,
    }

    impl CommandRunner for FakeSystemdRunner {
        fn run(&self, program: &str, args: &[&str]) -> io::Result<CommandOutput> {
            assert_eq!(program, "systemctl");
            let (expected, outcome) = self
                .calls
                .borrow_mut()
                .pop_front()
                .expect("unexpected systemctl call");
            assert_eq!(args, expected);
            match outcome {
                FakeOutcome::Output(output) => Ok(output),
                FakeOutcome::Spawn(kind) => {
                    Err(io::Error::new(kind, "fake systemctl spawn failure"))
                }
            }
        }
    }

    fn fake_systemd(
        calls: Vec<(Vec<&str>, FakeOutcome)>,
    ) -> (Systemd<FakeSystemdRunner>, FakeCalls) {
        let calls = Rc::new(RefCell::new(
            calls
                .into_iter()
                .map(|(args, outcome)| (args.into_iter().map(str::to_string).collect(), outcome))
                .collect(),
        ));
        let runner = FakeSystemdRunner {
            calls: Rc::clone(&calls),
        };
        (Systemd::with_runner(runner), calls)
    }

    fn success() -> FakeOutcome {
        FakeOutcome::Output(CommandOutput {
            code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }

    fn non_zero(code: i32, stderr: &str) -> FakeOutcome {
        FakeOutcome::Output(CommandOutput {
            code: Some(code),
            stdout: String::new(),
            stderr: stderr.to_string(),
        })
    }

    fn loaded_status() -> FakeOutcome {
        FakeOutcome::Output(CommandOutput {
            code: Some(0),
            stdout: "LoadState=loaded\nActiveState=inactive\nUnitFileState=enabled\nDescription=ANOLISA\n"
                .to_string(),
            stderr: String::new(),
        })
    }

    fn missing_status() -> FakeOutcome {
        FakeOutcome::Output(CommandOutput {
            code: Some(0),
            stdout:
                "LoadState=not-found\nActiveState=inactive\nUnitFileState=\nDescription=missing\n"
                    .to_string(),
            stderr: String::new(),
        })
    }

    fn assert_systemd_finished(calls: &FakeCalls) {
        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn setup_service_lifecycle_preserves_non_upgrade_order() {
        let (systemd, calls) = fake_systemd(vec![
            (vec!["daemon-reload"], success()),
            (vec!["enable", SERVICE_NAME], success()),
            (vec!["start", SERVICE_NAME], success()),
        ]);

        reload_and_start_service("system setup", false, &systemd)
            .expect("setup lifecycle should succeed");

        assert_systemd_finished(&calls);
    }

    #[test]
    fn setup_best_effort_stop_never_probes_status() {
        let cases = [
            success(),
            non_zero(5, "missing\n"),
            FakeOutcome::Spawn(io::ErrorKind::NotFound),
        ];

        for outcome in cases {
            let (systemd, calls) = fake_systemd(vec![(vec!["stop", SERVICE_NAME], outcome)]);

            stop_service_before_setup(&systemd);

            assert_systemd_finished(&calls);
        }
    }

    #[test]
    fn setup_service_lifecycle_restarts_during_upgrade() {
        let (systemd, calls) = fake_systemd(vec![
            (vec!["daemon-reload"], success()),
            (vec!["enable", SERVICE_NAME], success()),
            (vec!["restart", SERVICE_NAME], success()),
        ]);

        reload_and_start_service("system setup", true, &systemd)
            .expect("upgrade lifecycle should succeed");

        assert_systemd_finished(&calls);
    }

    #[test]
    fn setup_service_lifecycle_preserves_spawn_failure_message() {
        let (systemd, calls) = fake_systemd(vec![(
            vec!["daemon-reload"],
            FakeOutcome::Spawn(io::ErrorKind::PermissionDenied),
        )]);

        let error = reload_and_start_service("system setup", false, &systemd)
            .expect_err("spawn should fail setup");

        assert_eq!(
            error.reason(),
            "failed to run systemctl daemon-reload: fake systemctl spawn failure"
        );
        assert_systemd_finished(&calls);
    }

    #[test]
    fn setup_service_lifecycle_preserves_non_zero_exit_message() {
        let (systemd, calls) = fake_systemd(vec![
            (vec!["daemon-reload"], success()),
            (
                vec!["enable", SERVICE_NAME],
                non_zero(1, "enable refused\n"),
            ),
        ]);

        let error = reload_and_start_service("system setup", false, &systemd)
            .expect_err("enable should fail setup");

        assert_eq!(
            error.reason(),
            format!("systemctl enable {SERVICE_NAME} failed: enable refused\n")
        );
        assert_systemd_finished(&calls);
    }

    #[test]
    fn teardown_missing_unit_uses_typed_status_and_continues() {
        let (systemd, calls) = fake_systemd(vec![
            (
                vec!["stop", SERVICE_NAME],
                non_zero(5, "translated missing-unit diagnostic\n"),
            ),
            (
                vec![
                    "show",
                    SERVICE_NAME,
                    "--no-pager",
                    "--property=LoadState,ActiveState,UnitFileState,Description",
                ],
                missing_status(),
            ),
            (vec!["disable", SERVICE_NAME], success()),
        ]);
        let mut warnings = Vec::new();

        stop_and_disable_service("system teardown", &systemd, &mut warnings);

        assert_eq!(
            warnings,
            vec![format!(
                "service {SERVICE_NAME} was not loaded (already stopped)"
            )]
        );
        assert_systemd_finished(&calls);
    }

    #[test]
    fn teardown_preserves_stop_disable_reload_order() {
        let (systemd, calls) = fake_systemd(vec![
            (vec!["stop", SERVICE_NAME], success()),
            (vec!["disable", SERVICE_NAME], success()),
            (vec!["daemon-reload"], success()),
        ]);
        let mut warnings = Vec::new();

        stop_and_disable_service("system teardown", &systemd, &mut warnings);
        reload_systemd_after_teardown("system teardown", &systemd, &mut warnings);

        assert!(warnings.is_empty());
        assert_systemd_finished(&calls);
    }

    #[test]
    fn teardown_preserves_disable_and_reload_failure_warnings() {
        let (systemd, calls) = fake_systemd(vec![
            (vec!["stop", SERVICE_NAME], success()),
            (
                vec!["disable", SERVICE_NAME],
                non_zero(1, "disable refused\n"),
            ),
            (vec!["daemon-reload"], non_zero(1, "reload refused\n")),
        ]);
        let mut warnings = Vec::new();

        stop_and_disable_service("system teardown", &systemd, &mut warnings);
        reload_systemd_after_teardown("system teardown", &systemd, &mut warnings);

        assert_eq!(
            warnings,
            vec![
                format!(
                    "failed to disable {SERVICE_NAME}: execution failed: systemctl disable \
                     {SERVICE_NAME} failed: disable refused\n"
                ),
                "daemon-reload failed: execution failed: systemctl daemon-reload failed: \
                 reload refused\n"
                    .to_string(),
            ]
        );
        assert_systemd_finished(&calls);
    }

    #[test]
    fn teardown_non_missing_stop_failure_warns_and_disables() {
        let (systemd, calls) = fake_systemd(vec![
            (vec!["stop", SERVICE_NAME], non_zero(1, "access denied\n")),
            (
                vec![
                    "show",
                    SERVICE_NAME,
                    "--no-pager",
                    "--property=LoadState,ActiveState,UnitFileState,Description",
                ],
                loaded_status(),
            ),
            (vec!["disable", SERVICE_NAME], success()),
        ]);
        let mut warnings = Vec::new();

        stop_and_disable_service("system teardown", &systemd, &mut warnings);

        assert_eq!(
            warnings,
            vec![format!(
                "failed to stop {SERVICE_NAME}: execution failed: systemctl stop {SERVICE_NAME} failed: access denied\n"
            )]
        );
        assert_systemd_finished(&calls);
    }

    fn client_with_responses(responses: Vec<HelperResponse>) -> HelperClient {
        let (transport, _) =
            ScriptedTransport::new(Vec::new(), responses.into_iter().map(Ok).collect());
        HelperClient::with_transport(transport)
    }

    fn connect_error() -> HelperClientError {
        HelperClientError::Connect {
            path: PathBuf::from(SYSTEM_HELPER_SOCKET),
            source: io::Error::new(io::ErrorKind::ConnectionRefused, "not listening"),
        }
    }

    #[test]
    fn setup_verification_uses_typed_handshake_result() {
        let compatible = client_with_responses(vec![HelperResponse::HandshakeOk {
            helper_version: env!("CARGO_PKG_VERSION").to_string(),
            compatible: true,
        }]);
        verify_helper_connection("system setup", || Ok(compatible)).expect("compatible helper");

        let incompatible = client_with_responses(vec![HelperResponse::HandshakeOk {
            helper_version: "0.0.1".to_string(),
            compatible: false,
        }]);
        let error = verify_helper_connection("system setup", || Ok(incompatible))
            .expect_err("incompatible helper");
        match error {
            CliError::Runtime { reason, .. } => {
                assert_eq!(reason, "handshake succeeded but version is incompatible");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn setup_verification_preserves_unexpected_response_messages() {
        let cases = [
            (
                HelperResponse::Error {
                    code: "DENIED".to_string(),
                    message: "no access".to_string(),
                },
                "unexpected handshake response: Error { code: \"DENIED\", message: \"no access\" }",
            ),
            (
                HelperResponse::Success {
                    message: "wrong response".to_string(),
                    exit_code: 0,
                },
                "unexpected handshake response: Success { message: \"wrong response\", exit_code: 0 }",
            ),
        ];

        for (response, expected) in cases {
            let client = client_with_responses(vec![response]);
            let error = verify_helper_connection("system setup", || Ok(client))
                .expect_err("unexpected response");

            match error {
                CliError::Runtime { reason, .. } => assert_eq!(reason, expected),
                other => panic!("unexpected error: {other:?}"),
            }
        }
    }

    #[test]
    fn status_marks_connection_failure_as_not_connectable() {
        let result = try_status_connection_with("0.3.2", || Err(connect_error()));

        assert!(!result.connectable);
        assert!(result.handshake.is_none());
        assert!(result.status.is_none());
    }

    #[test]
    fn status_retains_connectability_when_handshake_fails() {
        let (transport, _) = ScriptedTransport::new(
            vec![Err(io::Error::new(io::ErrorKind::BrokenPipe, "send"))],
            Vec::new(),
        );
        let client = HelperClient::with_transport(transport);

        let result = try_status_connection_with("0.3.2", || Ok(client));

        assert!(result.connectable);
        assert!(result.handshake.is_none());
        assert!(result.status.is_none());
    }

    #[test]
    fn status_skips_query_for_incompatible_helper() {
        let client = client_with_responses(vec![HelperResponse::HandshakeOk {
            helper_version: "0.0.1".to_string(),
            compatible: false,
        }]);

        let result = try_status_connection_with("0.3.2", || Ok(client));

        assert!(result.connectable);
        let handshake = result.handshake.expect("handshake evidence");
        assert_eq!(handshake.helper_version, "0.0.1");
        assert!(!handshake.compatible);
        assert!(result.status.is_none());
    }

    #[test]
    fn status_retains_handshake_when_status_query_fails() {
        let client = client_with_responses(vec![
            HelperResponse::HandshakeOk {
                helper_version: "0.3.2".to_string(),
                compatible: true,
            },
            HelperResponse::Error {
                code: "UNAVAILABLE".to_string(),
                message: "status unavailable".to_string(),
            },
        ]);

        let result = try_status_connection_with("0.3.2", || Ok(client));

        assert!(result.connectable);
        assert!(result.handshake.expect("handshake evidence").compatible);
        assert!(result.status.is_none());
    }

    #[test]
    fn status_returns_complete_typed_evidence() {
        let client = client_with_responses(vec![
            HelperResponse::HandshakeOk {
                helper_version: "0.3.2".to_string(),
                compatible: true,
            },
            HelperResponse::Status {
                running: true,
                version: "0.3.2".to_string(),
                uptime_secs: 75,
                last_operation: Some("install".to_string()),
                last_operation_time: Some("now".to_string()),
            },
        ]);

        let result = try_status_connection_with("0.3.2", || Ok(client));

        assert!(result.connectable);
        assert!(result.handshake.expect("handshake evidence").compatible);
        let status = result.status.expect("status evidence");
        assert_eq!(status.uptime_secs, 75);
        assert_eq!(status.last_operation.as_deref(), Some("install"));
        assert_eq!(status.last_operation_time.as_deref(), Some("now"));
    }
}
