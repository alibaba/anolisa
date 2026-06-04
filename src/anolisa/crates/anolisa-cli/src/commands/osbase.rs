use clap::{Parser, Subcommand};

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
    /// Sandbox substrate management (container, kata, firecracker, vm, landlock)
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
    /// Install a sandbox runtime
    Install {
        /// Target: container, kata, firecracker, vm, landlock
        target: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove a sandbox runtime
    Remove { target: String },
    /// List available sandbox runtimes
    List {
        #[arg(long)]
        available: bool,
    },
    /// Show sandbox status
    Status { target: Option<String> },
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

pub fn handle(args: OsbaseArgs, _ctx: &CliContext) -> Result<(), CliError> {
    let command = match args.command {
        OsbaseCommands::Kernel(k) => match k.command {
            KernelCommands::Install { .. } => "osbase kernel install".to_string(),
            KernelCommands::Remove => "osbase kernel remove".to_string(),
            KernelCommands::Status => "osbase kernel status".to_string(),
        },
        OsbaseCommands::Sandbox(s) => match s.command {
            SandboxCommands::Install { target, .. } => format!("osbase sandbox install {target}"),
            SandboxCommands::Remove { target } => format!("osbase sandbox remove {target}"),
            SandboxCommands::List { .. } => "osbase sandbox list".to_string(),
            SandboxCommands::Status { target } => match target {
                Some(t) => format!("osbase sandbox status {t}"),
                None => "osbase sandbox status".to_string(),
            },
        },
        OsbaseCommands::Security(s) => match s.command {
            SecurityCommands::Install { target, .. } => {
                format!("osbase security install {target}")
            }
            SecurityCommands::Remove { target } => format!("osbase security remove {target}"),
            SecurityCommands::Status { target } => match target {
                Some(t) => format!("osbase security status {t}"),
                None => "osbase security status".to_string(),
            },
        },
    };
    Err(CliError::not_implemented(command))
}
