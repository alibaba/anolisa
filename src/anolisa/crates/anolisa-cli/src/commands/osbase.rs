use clap::{Parser, Subcommand, ValueEnum};

use anolisa_core::sandbox_install::{
    InstallPhase, PhaseStatus, SandboxBackendKind, SandboxInstallError, SandboxInstallOutcome,
    SandboxInstallRequest, build_dry_run_plan, execute_sandbox_install, validate_request,
};
use anolisa_platform::fs_layout::FsLayout;

use crate::context::CliContext;
use crate::response::{self, CliError};

#[derive(Parser)]
pub struct OsbaseArgs {
    #[command(subcommand)]
    pub command: OsbaseCommands,
}

#[derive(Subcommand)]
pub enum OsbaseCommands {
    /// Kernel modules and eBPF base management
    Kernel(KernelArgs),
    /// Sandbox substrate management (container, kata, firecracker, gvisor, vm, landlock)
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

/// Sandbox backend target (isolation engine)
#[derive(Clone, Debug, ValueEnum)]
pub enum SandboxTarget {
    /// OCI container runtime (runc/rund)
    Container,
    /// Kata Containers (KVM-based lightweight VM)
    Kata,
    /// Firecracker microVM (standard/e2b/kata-fc)
    Firecracker,
    /// gVisor user-space kernel (runsc)
    Gvisor,
    /// QEMU/KVM full virtual machine
    Vm,
    /// Landlock LSM filesystem access control
    Landlock,
}

impl std::fmt::Display for SandboxTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Container => write!(f, "container"),
            Self::Kata => write!(f, "kata"),
            Self::Firecracker => write!(f, "firecracker"),
            Self::Gvisor => write!(f, "gvisor"),
            Self::Vm => write!(f, "vm"),
            Self::Landlock => write!(f, "landlock"),
        }
    }
}

#[derive(Parser)]
pub struct SandboxArgs {
    #[command(subcommand)]
    pub command: SandboxCommands,
}

#[derive(Subcommand)]
pub enum SandboxCommands {
    /// Install a sandbox backend
    ///
    /// Runs the 5-phase install pipeline: Pre-flight → Packages → OS Primitives → Service → Verify
    Install {
        /// Backend to install
        target: SandboxTarget,

        /// Variant selection (container: runc|rund; firecracker: standard|e2b|kata-fc)
        #[arg(long)]
        variant: Option<String>,

        /// L2 runtime to register the engine into (gvisor: containerd|docker).
        /// Firecracker rejects this flag (direct KVM access).
        #[arg(long)]
        runtime: Option<String>,

        /// Control-panel data-plane overlay (gvisor: substrate).
        /// Requires --runtime=containerd.
        #[arg(long)]
        control_panel: Option<String>,

        /// Print install plan without executing
        #[arg(long)]
        dry_run: bool,

        /// Skip confirmation prompts (e.g. HugePages allocation)
        #[arg(long)]
        force: bool,

        /// Skip post-install verification (Phase 5)
        #[arg(long)]
        no_verify: bool,
    },

    /// Remove a sandbox backend
    ///
    /// Runs the reverse 3-phase pipeline: Pre-check → Service Teardown → Cleanup
    Remove {
        /// Backend to remove
        target: SandboxTarget,

        /// Variant selection (container: runc|rund; firecracker: standard|e2b|kata-fc)
        #[arg(long)]
        variant: Option<String>,

        /// Also remove ANOLISA-written config files and data directories
        #[arg(long)]
        purge: bool,

        /// Skip dependency checks (dangerous: may break kata/firecracker/gvisor substrate)
        #[arg(long)]
        force: bool,

        /// Print removal plan without executing
        #[arg(long)]
        dry_run: bool,
    },

    /// List all sandbox backends and their availability
    ///
    /// Performs real-time environment probing (does not read cache)
    List {
        /// Only show backends whose gate conditions pass
        #[arg(long)]
        available: bool,

        /// Output as structured JSON
        #[arg(long)]
        json: bool,
    },

    /// Show sandbox backend status
    ///
    /// Without target: summary of all backends. With target: detailed info.
    Status {
        /// Specific backend to query (omit for all)
        target: Option<SandboxTarget>,

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
    match command {
        SandboxCommands::Install {
            target,
            variant,
            runtime,
            control_panel,
            dry_run,
            force,
            no_verify,
        } => {
            let backend = sandbox_target_to_kind(&target);
            let variant_str = variant.unwrap_or_else(|| backend.default_variant().to_string());

            // sandbox install writes to /etc, /var/lib, /usr/lib and enables
            // systemd units — all of which are system-scoped. The default
            // global --install-mode is `user` (XDG), under which sandbox
            // install would silently route audit log + state to
            // ~/.local/state/anolisa/ while the actual writes still target
            // /etc and /var/lib (and need root). Reject that mismatch up
            // front with a clear error rather than letting it surface as a
            // permission-denied or `dnf` failure deep in Phase 2/3.
            if !matches!(ctx.install_mode, crate::context::InstallMode::System) {
                return Err(CliError::InvalidArgument {
                    command: format!(
                        "osbase sandbox install {target} --variant={variant_str}"
                    ),
                    reason:
                        "sandbox install is system-only; pass --install-mode=system (and run as root)"
                            .to_string(),
                });
            }

            let request = SandboxInstallRequest {
                backend,
                variant: variant_str,
                runtime,
                control_panel,
                dry_run: dry_run || ctx.dry_run,
                force,
                no_verify,
                json: ctx.json,
            };

            let layout = resolve_layout(ctx);

            // Dry-run: print plan and exit. Validate the backend/variant
            // first so that an unknown variant fails loudly instead of
            // returning a misleading "plan" the real install would reject.
            if request.dry_run {
                if let Err(e) = validate_request(&request) {
                    return Err(map_sandbox_err(e, &request));
                }
                let plan = build_dry_run_plan(&request);
                if ctx.json {
                    return response::render_json(
                        &format!(
                            "osbase sandbox install {} --variant={}",
                            request.backend, request.variant
                        ),
                        &plan,
                    );
                }
                println!(
                    "Install plan for: {} (variant={})",
                    plan.backend, plan.variant
                );
                println!();
                for phase in &plan.phases {
                    println!("Phase {}: {}", phase_number(phase.phase), phase.phase);
                    for action in &phase.actions {
                        println!("  - {action}");
                    }
                    println!();
                }
                return Ok(());
            }

            // Execute real install
            match execute_sandbox_install(&request, &layout) {
                Ok(outcome) => render_install_outcome(ctx, &outcome),
                Err(err) => Err(map_sandbox_err(err, &request)),
            }
        }
        SandboxCommands::Remove {
            target, variant, ..
        } => {
            let cmd = match variant {
                Some(v) => format!("osbase sandbox remove {target} --variant={v}"),
                None => format!("osbase sandbox remove {target}"),
            };
            Err(CliError::not_implemented(cmd))
        }
        SandboxCommands::List { .. } => Err(CliError::not_implemented("osbase sandbox list")),
        SandboxCommands::Status { target, .. } => {
            let cmd = match target {
                Some(t) => format!("osbase sandbox status {t}"),
                None => "osbase sandbox status".to_string(),
            };
            Err(CliError::not_implemented(cmd))
        }
    }
}

// ===========================================================================
// Helpers
// ===========================================================================

fn sandbox_target_to_kind(target: &SandboxTarget) -> SandboxBackendKind {
    match target {
        SandboxTarget::Container => SandboxBackendKind::Container,
        SandboxTarget::Kata => SandboxBackendKind::Kata,
        SandboxTarget::Firecracker => SandboxBackendKind::Firecracker,
        SandboxTarget::Gvisor => SandboxBackendKind::Gvisor,
        SandboxTarget::Vm => SandboxBackendKind::Vm,
        SandboxTarget::Landlock => SandboxBackendKind::Landlock,
    }
}

fn resolve_layout(ctx: &CliContext) -> FsLayout {
    match ctx.install_mode {
        crate::context::InstallMode::System => FsLayout::system(ctx.prefix.clone()),
        crate::context::InstallMode::User => {
            let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
            FsLayout::user(home)
        }
    }
}

fn render_install_outcome(
    ctx: &CliContext,
    outcome: &SandboxInstallOutcome,
) -> Result<(), CliError> {
    let cmd = format!(
        "osbase sandbox install {} --variant={}",
        outcome.backend, outcome.variant
    );

    // For non-zero outcomes (degraded / failed) the JSON envelope must
    // carry ok=false so machine callers don't see a success envelope
    // contradicting the non-zero exit code. Build the CliError up front
    // and let `render_error` (called by main on Err) emit the error
    // envelope on the JSON path. Phase details are still discoverable
    // via the central audit log; we keep the envelope shape consistent
    // with other commands instead of inventing a degraded JSON variant.
    let outcome_err: Option<CliError> = match outcome.exit_code {
        0 => None,
        2 => Some(CliError::Degraded {
            command: cmd.clone(),
            reason: format!(
                "sandbox backend '{}' (variant={}) installed with warnings",
                outcome.backend, outcome.variant
            ),
        }),
        // Phase-level Failed (3) or any other non-zero code: surface
        // as runtime failure so callers see exit 1.
        _ => Some(CliError::Runtime {
            command: cmd.clone(),
            reason: format!(
                "sandbox backend '{}' (variant={}) install failed (exit_code={})",
                outcome.backend, outcome.variant, outcome.exit_code
            ),
        }),
    };

    if ctx.json {
        if let Some(err) = outcome_err {
            return Err(err);
        }
        return response::render_json(&cmd, outcome);
    }

    // Human-readable output
    for (i, phase) in outcome.phases.iter().enumerate() {
        let icon = match phase.status {
            PhaseStatus::Success => "\u{2713}",
            PhaseStatus::Skipped => "\u{2298}",
            PhaseStatus::Warning => "\u{26A0}",
            PhaseStatus::Failed => "\u{2717}",
        };
        let phase_name = format!("{:<10}", phase.phase.to_string());
        println!(
            "[{}/{}] {} {}  ({})",
            i + 1,
            outcome.phases.len(),
            phase_name,
            icon,
            phase.message
        );
    }
    println!();

    if outcome.exit_code == 0 {
        println!(
            "sandbox backend '{}' (variant={}) installed successfully.",
            outcome.backend, outcome.variant
        );
    } else if outcome.exit_code == 2 {
        println!(
            "sandbox backend '{}' (variant={}) installed with warnings (degraded).",
            outcome.backend, outcome.variant
        );
    }

    if !outcome.warnings.is_empty() {
        eprintln!();
        for w in &outcome.warnings {
            eprintln!("warning: {w}");
        }
    }

    // Surface non-zero outcome.exit_code to the process exit. The
    // 5-phase pipeline returns Ok(outcome) even when phases emit
    // Warning / Failed (those are encoded as exit_code 2 / 3 inside
    // the outcome). Without this conversion the process always exits
    // 0 on Ok(outcome), masking degraded installs from CI / scripts.
    match outcome_err {
        None => Ok(()),
        Some(err) => Err(err),
    }
}

fn map_sandbox_err(err: SandboxInstallError, request: &SandboxInstallRequest) -> CliError {
    let command = format!(
        "osbase sandbox install {} --variant={}",
        request.backend, request.variant
    );
    match &err {
        SandboxInstallError::EnvNotSatisfied { .. }
        | SandboxInstallError::Unsupported { .. }
        | SandboxInstallError::NotRoot => CliError::InvalidArgument {
            command,
            reason: err.to_string(),
        },
        _ => CliError::Runtime {
            command,
            reason: err.to_string(),
        },
    }
}

fn phase_number(phase: InstallPhase) -> u8 {
    match phase {
        InstallPhase::Preflight => 1,
        InstallPhase::Packages => 2,
        InstallPhase::OsPrimitives => 3,
        InstallPhase::ServiceSetup => 4,
        InstallPhase::PostVerify => 5,
    }
}
