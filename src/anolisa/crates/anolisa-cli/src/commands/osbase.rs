use std::path::Path;

use clap::{Parser, Subcommand};

use anolisa_core::osbase_install::{
    self, OsbaseDomain, OsbaseInstallError, OsbaseInstallRequest, RegisterHandler,
};
use anolisa_core::system_helper::HelperRequest;
use anolisa_platform::ipc::SYSTEM_HELPER_SOCKET;
use anolisa_platform::privilege;

use crate::context::CliContext;
use crate::helper_client::{HelperClient, HelperClientError};
use crate::response::{CliError, render_json};

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
    /// Runs: Preflight → Packages → Services → Verify → State
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
            ExecutionMode::ViaHelper(mut client) => {
                eprintln!("[osbase] scenario: {scenario}");
                let req = HelperRequest::OsbaseUninstall { scenario, dry_run };
                send_helper_request(&mut client, &req, "osbase sandbox uninstall")
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
            ExecutionMode::ViaHelper(mut client) => {
                let req = HelperRequest::OsbaseRemove {
                    scenario: target,
                    purge,
                };
                send_helper_request(&mut client, &req, "osbase sandbox remove")
            }
            ExecutionMode::Direct => Err(CliError::not_implemented(format!(
                "osbase sandbox remove {target}"
            ))),
        },
        SandboxCommands::List { .. } => unreachable!(),
        SandboxCommands::Status { target, .. } => match mode {
            ExecutionMode::ViaHelper(mut client) => {
                let req = HelperRequest::OsbaseStatus { scenario: target };
                send_helper_request(&mut client, &req, "osbase sandbox status")
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
        ExecutionMode::ViaHelper(mut client) => {
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
            send_helper_request(&mut client, &req, "osbase sandbox install")
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
                    if outcome.exit_code == 1 {
                        // Phase failure — phases already printed to stderr by
                        // the core.  Surface the failed phase in the error.
                        let failed_phase = outcome
                            .phases
                            .iter()
                            .rev()
                            .find(|p| p.status == osbase_install::PhaseStatus::Failed);
                        let reason = match failed_phase {
                            Some(p) => format!(
                                "phase '{}' failed: {}",
                                p.name,
                                p.message.as_deref().unwrap_or("unknown error")
                            ),
                            None => "install failed".to_string(),
                        };
                        for w in &outcome.warnings {
                            eprintln!("[osbase] warning: {w}");
                        }
                        return Err(CliError::Runtime {
                            command: format!("osbase sandbox install {target}"),
                            reason,
                        });
                    }
                    // exit_code 2 = degraded (non-fatal warnings already
                    // printed to stderr by the core). CLI still returns
                    // success so the user sees "install ok".
                    if !outcome.warnings.is_empty() {
                        eprintln!(
                            "[osbase] install completed with {} warning(s)",
                            outcome.warnings.len()
                        );
                    }
                    // Print informational hints (not counted as warnings).
                    for hint in &outcome.hints {
                        eprintln!("[osbase] hint: {hint}");
                    }
                    Ok(())
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
                render_json("osbase sandbox list", data)?;
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
enum ExecutionMode {
    /// Proxy execution via the privileged system-helper daemon.
    ViaHelper(HelperClient),
    /// Direct execution (process already has root privileges).
    Direct,
}

fn osbase_preflight() -> Result<ExecutionMode, CliError> {
    osbase_preflight_with(
        || HelperClient::connect(Path::new(SYSTEM_HELPER_SOCKET)),
        privilege::is_root(),
    )
}

fn osbase_preflight_with<F>(connect: F, is_root: bool) -> Result<ExecutionMode, CliError>
where
    F: FnOnce() -> Result<HelperClient, HelperClientError>,
{
    // 1. Try connecting to the system-helper socket.
    match connect() {
        Ok(mut client) => {
            // Perform version handshake.
            let handshake = client
                .handshake(env!("CARGO_PKG_VERSION"))
                .map_err(|error| CliError::Runtime {
                    command: "osbase".to_string(),
                    reason: preflight_handshake_error(error),
                })?;
            if !handshake.compatible {
                let cli_version = env!("CARGO_PKG_VERSION");
                return Err(CliError::Runtime {
                    command: "osbase".to_string(),
                    reason: format!(
                        "anolisa-system-helper version mismatch \
                         (installed: {}, required: {cli_version}); \
                         run 'sudo anolisa system setup' to upgrade",
                        handshake.helper_version
                    ),
                });
            }
            Ok(ExecutionMode::ViaHelper(client))
        }
        Err(_) => {
            // 2. Socket not available — check if we already have root.
            if is_root {
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

fn preflight_handshake_error(error: HelperClientError) -> String {
    match error {
        HelperClientError::Send { source, .. } => {
            format!("failed to send handshake to system-helper: {source}")
        }
        HelperClientError::Receive { source, .. } => {
            format!("failed to receive handshake from system-helper: {source}")
        }
        HelperClientError::Remote { .. } | HelperClientError::UnexpectedResponse { .. } => {
            "system-helper returned unexpected handshake response".to_string()
        }
        HelperClientError::Connect { path, source } => {
            format!("failed to connect to {}: {source}", path.display())
        }
    }
}

// ===========================================================================
// Helper IPC utilities
// ===========================================================================

fn send_helper_request(
    client: &mut HelperClient,
    req: &HelperRequest,
    command_label: &str,
) -> Result<(), CliError> {
    let outcome = client.execute(req).map_err(|error| CliError::Runtime {
        command: command_label.to_string(),
        reason: helper_operation_error(error),
    })?;

    if outcome.exit_code == 0 || outcome.exit_code == 2 {
        // exit_code 0 = full success, 2 = degraded (non-fatal
        // verify/state warnings). Both are reported as success;
        // the core already printed diagnostics to stderr.
        for line in outcome.message.lines() {
            eprintln!("[osbase] {line}");
        }
        if outcome.exit_code == 2 {
            eprintln!("[osbase] install completed with degraded status (non-fatal)");
        }
        Ok(())
    } else {
        // exit_code = 1 → phase failure.  Print the phase summary
        // (from the helper response) before reporting the error so
        // the user sees which phases passed and which failed.
        for line in outcome.message.lines() {
            eprintln!("[osbase] {line}");
        }
        Err(CliError::Runtime {
            command: command_label.to_string(),
            reason: format!("install failed (exit_code={})", outcome.exit_code),
        })
    }
}

fn helper_operation_error(error: HelperClientError) -> String {
    match error {
        HelperClientError::Send { source, .. } => {
            format!("failed to send request to system-helper: {source}")
        }
        HelperClientError::Receive { source, .. } => {
            format!("failed to receive response from system-helper: {source}")
        }
        HelperClientError::Remote { code, message, .. } => format!("[{code}] {message}"),
        HelperClientError::UnexpectedResponse { response, .. } => {
            format!("unexpected response from system-helper: {response:?}")
        }
        HelperClientError::Connect { path, source } => {
            format!("failed to connect to {}: {source}", path.display())
        }
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

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::PathBuf;

    use anolisa_core::system_helper::HelperResponse;

    use super::*;
    use crate::helper_client::ScriptedTransport;

    fn client_with_responses(responses: Vec<HelperResponse>) -> HelperClient {
        let (transport, _) =
            ScriptedTransport::new(Vec::new(), responses.into_iter().map(Ok).collect());
        HelperClient::with_transport(transport)
    }

    fn connect_error() -> HelperClientError {
        HelperClientError::Connect {
            path: PathBuf::from(SYSTEM_HELPER_SOCKET),
            source: io::Error::new(io::ErrorKind::NotFound, "missing helper socket"),
        }
    }

    #[test]
    fn preflight_preserves_root_fallback_when_connection_fails() {
        let root = osbase_preflight_with(|| Err(connect_error()), true).expect("root fallback");
        assert!(matches!(root, ExecutionMode::Direct));

        let error = match osbase_preflight_with(|| Err(connect_error()), false) {
            Ok(_) => panic!("non-root connection failure must not fall back"),
            Err(error) => error,
        };
        assert!(matches!(error, CliError::PermissionDenied { .. }));
    }

    #[test]
    fn preflight_preserves_handshake_compatibility() {
        let compatible = client_with_responses(vec![HelperResponse::HandshakeOk {
            helper_version: env!("CARGO_PKG_VERSION").to_string(),
            compatible: true,
        }]);
        let mode = osbase_preflight_with(|| Ok(compatible), false).expect("compatible helper");
        assert!(matches!(mode, ExecutionMode::ViaHelper(_)));

        let incompatible = client_with_responses(vec![HelperResponse::HandshakeOk {
            helper_version: "0.0.1".to_string(),
            compatible: false,
        }]);
        let error = match osbase_preflight_with(|| Ok(incompatible), false) {
            Ok(_) => panic!("incompatible helper must fail"),
            Err(error) => error,
        };
        match error {
            CliError::Runtime { reason, .. } => {
                assert!(reason.contains("version mismatch"));
                assert!(reason.contains("0.0.1"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn helper_operation_preserves_success_degraded_and_failure_codes() {
        for (exit_code, succeeds) in [(0, true), (2, true), (1, false)] {
            let mut client = client_with_responses(vec![HelperResponse::Success {
                message: "phase summary".to_string(),
                exit_code,
            }]);

            let result = send_helper_request(
                &mut client,
                &HelperRequest::OsbaseStatus { scenario: None },
                "osbase sandbox status",
            );

            assert_eq!(result.is_ok(), succeeds, "exit code {exit_code}");
            if let Err(CliError::Runtime { reason, .. }) = result {
                assert_eq!(reason, "install failed (exit_code=1)");
            }
        }
    }

    #[test]
    fn helper_operation_maps_remote_and_unexpected_responses() {
        let cases = [
            (
                HelperResponse::Error {
                    code: "DENIED".to_string(),
                    message: "no access".to_string(),
                },
                "[DENIED] no access",
            ),
            (
                HelperResponse::HandshakeOk {
                    helper_version: "0.3.2".to_string(),
                    compatible: true,
                },
                "unexpected response from system-helper",
            ),
        ];

        for (response, expected) in cases {
            let mut client = client_with_responses(vec![response]);
            let error = send_helper_request(
                &mut client,
                &HelperRequest::OsbaseStatus { scenario: None },
                "osbase sandbox status",
            )
            .expect_err("response must fail");

            match error {
                CliError::Runtime { reason, .. } => assert!(reason.contains(expected)),
                other => panic!("unexpected error: {other:?}"),
            }
        }
    }
}
