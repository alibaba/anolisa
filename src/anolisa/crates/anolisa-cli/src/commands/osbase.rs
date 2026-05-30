use clap::{Parser, Subcommand};

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

pub fn handle(args: OsbaseArgs) -> anyhow::Result<()> {
    match args.command {
        OsbaseCommands::Kernel(k) => match k.command {
            KernelCommands::Install { dry_run } => {
                println!("osbase kernel install (dry_run={dry_run}): not yet implemented");
            }
            KernelCommands::Remove => {
                println!("osbase kernel remove: not yet implemented");
            }
            KernelCommands::Status => {
                println!("osbase kernel status: not yet implemented");
            }
        },
        OsbaseCommands::Sandbox(s) => match s.command {
            SandboxCommands::Install { target, dry_run } => {
                println!(
                    "osbase sandbox install {target} (dry_run={dry_run}): not yet implemented"
                );
            }
            SandboxCommands::Remove { target } => {
                println!("osbase sandbox remove {target}: not yet implemented");
            }
            SandboxCommands::List { available } => {
                if available {
                    println!("Available sandbox targets:");
                } else {
                    println!("Installed sandbox targets:");
                }
                println!("  container   runc/crun container runtime");
                println!("  kata        Kata Containers (microVM)");
                println!("  firecracker Firecracker microVM");
                println!("  vm          KVM/QEMU full VM");
                println!("  landlock    Landlock LSM policies");
            }
            SandboxCommands::Status { target } => {
                println!(
                    "osbase sandbox status {}: not yet implemented",
                    target.as_deref().unwrap_or("(all)")
                );
            }
        },
        OsbaseCommands::Security(s) => match s.command {
            SecurityCommands::Install { target, dry_run } => {
                println!(
                    "osbase security install {target} (dry_run={dry_run}): not yet implemented"
                );
            }
            SecurityCommands::Remove { target } => {
                println!("osbase security remove {target}: not yet implemented");
            }
            SecurityCommands::Status { target } => {
                println!(
                    "osbase security status {}: not yet implemented",
                    target.as_deref().unwrap_or("(all)")
                );
            }
        },
    }
    Ok(())
}
