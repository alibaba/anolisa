//! Tier 2 surface — `anolisa self`: management of the anolisa CLI itself.
//!
//! `anolisa self update` delegates to the same self-update logic as
//! `anolisa update self` — both paths are supported as a user convenience.
//! Other subcommands: adopt, completions.

use clap::{Parser, Subcommand};

use crate::commands::tier1::update::handle_self_update;
use crate::context::CliContext;
use crate::response::CliError;

/// Arguments for `anolisa self`.
#[derive(Parser)]
pub struct SelfArgs {
    /// Selected CLI-management subcommand.
    #[command(subcommand)]
    pub command: SelfCommands,
}

/// CLI-management subcommands outside the unified component lifecycle surface.
#[derive(Subcommand)]
pub enum SelfCommands {
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
    /// Self-update the CLI binary (same as `anolisa update self`)
    #[command(name = "update")]
    Update,
}

/// Dispatches `anolisa self` subcommands.
///
/// # Errors
///
/// Returns [`CliError`] for subcommands that are not yet implemented, or
/// propagates errors from [`handle_self_update`].
pub fn handle(args: SelfArgs, ctx: &CliContext) -> Result<(), CliError> {
    match args.command {
        SelfCommands::Adopt { .. } => Err(CliError::not_implemented("self adopt")),
        SelfCommands::Completions { shell } => Err(CliError::not_implemented(format!(
            "self completions {shell}"
        ))),
        SelfCommands::Update => handle_self_update(ctx),
    }
}
