//! `anolisa update` — unified update surface (launch spec §7.3).
//!
//! Three subcommands:
//! - `update self` - update the `anolisa` CLI binary only.
//! - `update runtime <COMP|all>` - update one or all ANOLISA-managed
//!   runtime components.
//! - `update all` - update every ANOLISA-managed runtime, osbase, and
//!   adapter object.
//!
//! Explicit invariant (spec §7.3, decision §11.2): `update all` does
//! **not** include `self`. CLI self-update lives only behind `update
//! self` so the CLI binary swap never shares a transaction with
//! component updates.
//!
//! Long-term `self update` / `runtime update` aliases (see `self_.rs` and
//! `runtime.rs`) point users at this surface.

use clap::{Parser, Subcommand};

use crate::context::CliContext;
use crate::response::CliError;

#[derive(Parser)]
pub struct UpdateArgs {
    #[command(subcommand)]
    pub command: UpdateCommands,
}

#[derive(Subcommand)]
pub enum UpdateCommands {
    /// Update the anolisa CLI binary only
    #[command(name = "self")]
    SelfBin,
    /// Update one or all ANOLISA-managed runtime components
    Runtime {
        /// Component name, or `all`
        target: String,
    },
    /// Update every ANOLISA-managed runtime, osbase, and adapter object.
    ///
    /// Does NOT include the CLI binary itself — use `anolisa update self`
    /// for that.
    All,
}

pub fn handle(args: UpdateArgs, _ctx: &CliContext) -> Result<(), CliError> {
    let command = match &args.command {
        UpdateCommands::SelfBin => "update self".to_string(),
        UpdateCommands::Runtime { target } => format!("update runtime {target}"),
        UpdateCommands::All => "update all".to_string(),
    };
    Err(CliError::not_implemented_with_hint(
        command,
        "update planner / distribution resolver not implemented yet",
    ))
}
