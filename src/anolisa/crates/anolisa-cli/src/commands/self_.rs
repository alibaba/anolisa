//! Tier 2 surface — `anolisa self`: management of the anolisa CLI itself.
//!
//! `self update` is retained as a long-term compatibility alias for
//! `anolisa update self` (launch spec §7.3). Handlers return a hint that
//! redirects the user to the unified update surface.

use clap::{Parser, Subcommand};

use crate::context::CliContext;
use crate::response::CliError;

#[derive(Parser)]
pub struct SelfArgs {
    #[command(subcommand)]
    pub command: SelfCommands,
}

#[derive(Subcommand)]
pub enum SelfCommands {
    /// Update the anolisa CLI binary (alias of `anolisa update self`)
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

pub fn handle(args: SelfArgs, _ctx: &CliContext) -> Result<(), CliError> {
    match args.command {
        SelfCommands::Update => Err(CliError::not_implemented_with_hint(
            "self update",
            "long-term alias of `anolisa update self`; use that instead",
        )),
        SelfCommands::Adopt { .. } => Err(CliError::not_implemented("self adopt")),
        SelfCommands::Completions { shell } => Err(CliError::not_implemented(format!(
            "self completions {shell}"
        ))),
    }
}
