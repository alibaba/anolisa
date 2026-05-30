//! Tier 2 surface — `anolisa self`: management of the anolisa CLI itself.

use clap::{Parser, Subcommand};

#[derive(Parser)]
pub struct SelfArgs {
    #[command(subcommand)]
    pub command: SelfCommands,
}

#[derive(Subcommand)]
pub enum SelfCommands {
    /// Update the anolisa CLI binary
    Update,
    /// Scan and register pre-existing components (build-all.sh migration path)
    Adopt {
        /// Run a probe-only scan
        #[arg(long)]
        scan: bool,
        /// Confirm and persist into installed.toml
        #[arg(long)]
        confirm: bool,
    },
    /// Generate shell completion script
    Completions {
        /// Target shell (bash, zsh, fish)
        shell: String,
    },
}

pub fn handle(args: SelfArgs) -> anyhow::Result<()> {
    match args.command {
        SelfCommands::Update => {
            println!("anolisa self update: not yet implemented");
        }
        SelfCommands::Adopt { scan, confirm } => {
            if scan && !confirm {
                println!("Scanning for existing ANOLISA components...");
                println!("  → adopt scan not yet implemented");
                println!();
                println!("Re-run with --confirm to register the findings.");
            } else if confirm {
                println!("Adopting and registering existing components...");
                println!("  → adopt confirm not yet implemented");
            } else {
                println!("Usage: anolisa self adopt --scan  |  --scan --confirm");
            }
        }
        SelfCommands::Completions { shell } => {
            println!("Shell completions for {shell}: not yet implemented");
        }
    }
    Ok(())
}
