use std::os::unix::net::UnixStream;

use clap::{Parser, Subcommand};

use anolisa_core::osbase_install::{
    self, OsbaseDomain, OsbaseInstallError, OsbaseInstallRequest, RegisterHandler,
};
use anolisa_core::system_helper::{HelperRequest, HelperResponse};
use anolisa_platform::ipc::{SYSTEM_HELPER_SOCKET, recv_message, send_message};
use anolisa_platform::privilege;

use crate::context::CliContext;
use crate::response::CliError;

#[derive(Parser)]
pub struct OsbaseArgs {
    #[command(subcommand)]
    pub command: OsbaseCommands,
}

#[derive(Subcommand)]
pub enum OsbaseCommands {
    /// Kernel modules and eBPF base management
    Kernel(KernelArgs),
    /// Sandbox substrate management
    Sandbox(SandboxArgs),
    /// Security overlay management (loongshield, seccomp-profiles)
    Security(SecurityArgs),
}

// --- Kernel ---

#[derive(Parser)]
pub struct KernelArgs {
    #[command(subcommand)]
    pub command: KernelCommands,
}

#[derive(Subcommand)]
pub enum KernelCommands {
    /// Install kernel modules and eBPF programs
    Install {
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove kernel modules
    Remove,
    /// Show kernel substrate status
    Status,
}

// --- Sandbox ---

#[derive(Parser)]
pub struct SandboxArgs {
    #[command(subcommand)]
    pub command: SandboxCommands,
}

#[derive(Subcommand)]
pub enum SandboxCommands {
    /// Install a sandbox scenario (reads from sandbox.toml manifest)
    ///
    /// Runs: Preflight → Packages → Done
    Install {
        /// Scenario name (e.g. runc, rund, gvisor, firecracker, landlock).
        /// Run `anolisa osbase sandbox list` to see available scenarios.
        target: String,

        /// Print install plan without executing
        #[arg(long)]
        dry_run: bool,

        /// Skip confirmation prompts and non-fatal gates
        #[arg(long)]
        force: bool,

        /// Skip post-install verification
        #[arg(long)]
        no_verify: bool,
    },

    /// Uninstall scenario packages (dnf remove)
    Uninstall {
        /// Scenario name (e.g. gvisor, firecracker).
        scenario: String,

        /// Print uninstall plan without executing
        #[arg(long)]
        dry_run: bool,
    },

    /// Remove a sandbox scenario
    Remove {
        /// Scenario to remove
        target: String,

        /// Also remove config files and data directories
        #[arg(long)]
        purge: bool,

        /// Print removal plan without executing
        #[arg(long)]
        dry_run: bool,
    },

    /// List all available sandbox scenarios (from sandbox.toml manifest)
    List {
        /// Output as structured JSON
        #[arg(long)]
        json: bool,
    },

    /// Show sandbox scenario status
    Status {
        /// Specific scenario to query (omit for all)
        target: Option<String>,

        /// Output as structured JSON
        #[arg(long)]
        json: bool,
    },
}

// --- Security ---

#[derive(Parser)]
pub struct SecurityArgs {
    #[command(subcommand)]
    pub command: SecurityCommands,
}

#[derive(Subcommand)]
pub enum SecurityCommands {
    /// Install a security overlay
    Install {
        /// Target: loongshield, seccomp-profiles
        target: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove a security overlay
    Remove { target: String },
    /// Show security overlay status
    Status { target: Option<String> },
}

pub fn handle(args: OsbaseArgs, ctx: &CliContext) -> Result<(), CliError> {
    match args.command {
        OsbaseCommands::Sandbox(s) => handle_sandbox(s.command, ctx),
        OsbaseCommands::Kernel(k) => {
            let command = match k.command {
                KernelCommands::Install { .. } => "osbase kernel install",
                KernelCommands::Remove => "osbase kernel remove",
                KernelCommands::Status => "osbase kernel status",
            };
            Err(CliError::not_implemented(command))
        }
        OsbaseCommands::Security(s) => {
            let command = match s.command {
                SecurityCommands::Install { target, .. } => {
                    format!("osbase security install {target}")
                }
                SecurityCommands::Remove { target } => format!("osbase security remove {target}"),
                SecurityCommands::Status { target } => match target {
                    Some(t) => format!("osbase security status {t}"),
                    None => "osbase security status".to_string(),
                },
            };
            Err(CliError::not_implemented(command))
        }
    }
}

fn handle_sandbox(command: SandboxCommands, ctx: &CliContext) -> Result<(), CliError> {
    // List only reads the manifest — no privilege or helper needed.
    if let SandboxCommands::List { json } = &command {
        return handle_sandbox_list(*json);
    }

    let mode = osbase_preflight()?;
    match command {
        SandboxCommands::Install {
            target,
            dry_run,
            force,
            no_verify,
        } => handle_sandbox_install(ctx, mode, target, dry_run, force, no_verify),
        SandboxCommands::Uninstall { scenario, dry_run } => match mode {
            ExecutionMode::ViaHelper(mut stream) => {
                eprintln!("[osbase] scenario: {scenario}");
                let req = HelperRequest::OsbaseUninstall { scenario, dry_run };
                send_helper_request(&mut stream, &req, "osbase sandbox uninstall")
            }
            ExecutionMode::Direct => {
                match osbase_install::execute_uninstall(&scenario, dry_run) {
                    Ok(_msg) => {
                        // Progress was already printed via eprintln! in the core.
                        Ok(())
                    }
                    Err(err) => Err(map_osbase_err(err, "uninstall", &scenario)),
                }
            }
        },
        SandboxCommands::Remove { target, purge, .. } => match mode {
            ExecutionMode::ViaHelper(mut stream) => {
                let req = HelperRequest::OsbaseRemove {
                    scenario: target,
                    purge,
                };
                send_helper_request(&mut stream, &req, "osbase sandbox remove")
            }
            ExecutionMode::Direct => Err(CliError::not_implemented(format!(
                "osbase sandbox remove {target}"
            ))),
        },
        SandboxCommands::List { .. } => unreachable!(),
        SandboxCommands::Status { target, .. } => match mode {
            ExecutionMode::ViaHelper(mut stream) => {
                let req = HelperRequest::OsbaseStatus { scenario: target };
                send_helper_request(&mut stream, &req, "osbase sandbox status")
            }
            ExecutionMode::Direct => Err(CliError::not_implemented("osbase sandbox status")),
        },
    }
}

fn handle_sandbox_install(
    ctx: &CliContext,
    mode: ExecutionMode,
    target: String,
    dry_run: bool,
    force: bool,
    no_verify: bool,
) -> Result<(), CliError> {
    match mode {
        ExecutionMode::ViaHelper(mut stream) => {
            let req = HelperRequest::OsbaseInstall {
                scenario: target.clone(),
                register_handler: "none".to_string(),
                register_runtimeclass: false,
                config_override: None,
                set_default: false,
                force,
                skip_verify: no_verify,
                dry_run: dry_run || ctx.dry_run,
            };
            eprintln!("[osbase] scenario: {target}");
            send_helper_request(&mut stream, &req, "osbase sandbox install")
        }
        ExecutionMode::Direct => {
            let request = OsbaseInstallRequest {
                domain: OsbaseDomain::Sandbox,
                target: target.clone(),
                register_handler: RegisterHandler::None,
                register_runtimeclass: false,
                config_override: None,
                set_default: false,
                force,
                skip_verify: no_verify,
                dry_run: dry_run || ctx.dry_run,
            };

            let env = anolisa_env::EnvService::detect();
            match osbase_install::execute_install(&request, &env) {
                Ok(outcome) => {
                    // Progress was already printed via eprintln! in the core.
                    if outcome.exit_code != 0 {
                        Err(CliError::Runtime {
                            command: format!("osbase sandbox install {}", outcome.target),
                            reason: format!("install failed (exit_code={})", outcome.exit_code),
                        })
                    } else {
                        Ok(())
                    }
                }
                Err(err) => Err(map_osbase_err(err, "install", &target)),
            }
        }
    }
}

fn handle_sandbox_list(json: bool) -> Result<(), CliError> {
    match osbase_install::list_scenarios() {
        Ok(names) => {
            if json {
                let data = serde_json::json!({ "scenarios": names });
                println!("{}", serde_json::to_string_pretty(&data).unwrap());
            } else {
                println!("Available sandbox scenarios (from sandbox.toml):");
                println!();
                for name in &names {
                    println!("  - {name}");
                }
                println!();
                println!("Install: anolisa osbase sandbox install <scenario>");
            }
            Ok(())
        }
        Err(e) => Err(CliError::Runtime {
            command: "osbase sandbox list".to_string(),
            reason: format!("{e}"),
        }),
    }
}

// ===========================================================================
// Preflight
// ===========================================================================

/// Execution path for osbase operations.
pub enum ExecutionMode {
    /// Proxy execution via the privileged system-helper daemon.
    ViaHelper(UnixStream),
    /// Direct execution (process already has root privileges).
    Direct,
}

fn osbase_preflight() -> Result<ExecutionMode, CliError> {
    // 1. Try connecting to the system-helper socket.
    match UnixStream::connect(SYSTEM_HELPER_SOCKET) {
        Ok(mut stream) => {
            // Perform version handshake.
            let req = HelperRequest::Handshake {
                cli_version: env!("CARGO_PKG_VERSION").to_string(),
            };
            send_message(&mut stream, &req).map_err(|e| CliError::Runtime {
                command: "osbase".to_string(),
                reason: format!("failed to send handshake to system-helper: {e}"),
            })?;
            let resp: HelperResponse =
                recv_message(&mut stream).map_err(|e| CliError::Runtime {
                    command: "osbase".to_string(),
                    reason: format!("failed to receive handshake from system-helper: {e}"),
                })?;
            match resp {
                HelperResponse::HandshakeOk {
                    compatible,
                    helper_version,
                } => {
                    if !compatible {
                        let cli_version = env!("CARGO_PKG_VERSION");
                        return Err(CliError::Runtime {
                            command: "osbase".to_string(),
                            reason: format!(
                                "anolisa-system-helper version mismatch \
                                 (installed: {helper_version}, required: {cli_version}); \
                                 run 'sudo anolisa system setup' to upgrade"
                            ),
                        });
                    }
                    Ok(ExecutionMode::ViaHelper(stream))
                }
                _ => Err(CliError::Runtime {
                    command: "osbase".to_string(),
                    reason: "system-helper returned unexpected handshake response".to_string(),
                }),
            }
        }
        Err(_) => {
            // 2. Socket not available — check if we already have root.
            if privilege::is_root() {
                Ok(ExecutionMode::Direct)
            } else {
                // 3. Non-root + no helper → actionable error.
                let exe = std::env::current_exe()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| "anolisa".into());
                Err(CliError::PermissionDenied {
                    command: "osbase".to_string(),
                    reason: "osbase requires root privileges and system-helper is not running"
                        .to_string(),
                    hint: Some(format!(
                        "Either:\n  1. Install helper: sudo {exe} system setup\n  \
                         2. Run directly: sudo {exe} osbase ..."
                    )),
                })
            }
        }
    }
}

// ===========================================================================
// Helper IPC utilities
// ===========================================================================

fn send_helper_request(
    stream: &mut UnixStream,
    req: &HelperRequest,
    command_label: &str,
) -> Result<(), CliError> {
    send_message(stream, req).map_err(|e| CliError::Runtime {
        command: command_label.to_string(),
        reason: format!("failed to send request to system-helper: {e}"),
    })?;

    let resp: HelperResponse = recv_message(stream).map_err(|e| CliError::Runtime {
        command: command_label.to_string(),
        reason: format!("failed to receive response from system-helper: {e}"),
    })?;

    match resp {
        HelperResponse::Success { message, exit_code } => {
            if exit_code == 0 {
                for line in message.lines() {
                    eprintln!("[osbase] {line}");
                }
                Ok(())
            } else {
                Err(CliError::Runtime {
                    command: command_label.to_string(),
                    reason: format!("{message} (exit_code={exit_code})"),
                })
            }
        }
        HelperResponse::Error { code, message } => Err(CliError::Runtime {
            command: command_label.to_string(),
            reason: format!("[{code}] {message}"),
        }),
        other => Err(CliError::Runtime {
            command: command_label.to_string(),
            reason: format!("unexpected response from system-helper: {other:?}"),
        }),
    }
}

// ===========================================================================
// Error mapping
// ===========================================================================

fn map_osbase_err(err: OsbaseInstallError, action: &str, target: &str) -> CliError {
    let command = format!("osbase sandbox {action} {target}");
    match &err {
        OsbaseInstallError::InvalidRequest { .. } | OsbaseInstallError::Unsupported(_) => {
            CliError::InvalidArgument {
                command,
                reason: err.to_string(),
            }
        }
        _ => CliError::Runtime {
            command,
            reason: err.to_string(),
        },
    }
}
