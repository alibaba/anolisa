//! Tier 2 surface — `anolisa self`: management of the anolisa CLI itself.
//!
//! Self-update lives under `anolisa update self`. A compatibility entry
//! `anolisa self update` is kept here but returns a hint directing users
//! to the canonical path. Other subcommands: adopt, completions.

use clap::{Parser, Subcommand};

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
    /// Self-update the CLI binary (redirects to `anolisa update self`)
    #[command(name = "update")]
    Update,
}

/// Dispatches `anolisa self` subcommands.
///
/// # Errors
///
/// Returns [`CliError`] for subcommands that are intentionally not implemented
/// yet, including the compatibility `self update` redirect.
pub fn handle(args: SelfArgs, _ctx: &CliContext) -> Result<(), CliError> {
    match args.command {
        SelfCommands::Adopt { .. } => Err(CliError::not_implemented("self adopt")),
        SelfCommands::Completions { shell } => Err(CliError::not_implemented(format!(
            "self completions {shell}"
        ))),
        SelfCommands::Update => Err(CliError::not_implemented_with_hint(
            "self update",
            "`anolisa self update` has moved — use `anolisa update self` instead",
        )),
    }
}
