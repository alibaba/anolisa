use clap::{Parser, Subcommand};

use crate::context::CliContext;
use crate::response::CliError;

#[derive(Parser)]
pub struct AdapterArgs {
    #[command(subcommand)]
    pub command: AdapterCommands,
}

#[derive(Subcommand)]
pub enum AdapterCommands {
    /// List registered adapters
    List,
    /// Install an adapter for a component into a framework
    Install {
        /// Component name (e.g., tokenless)
        component: String,
        /// Target framework (e.g., openclaw, hermes)
        framework: String,
    },
    /// Remove an adapter
    Remove {
        component: String,
        framework: String,
    },
    /// Auto-detect available adapter integrations
    Scan,
}

pub fn handle(args: AdapterArgs, _ctx: &CliContext) -> Result<(), CliError> {
    let command = match &args.command {
        AdapterCommands::List => "adapter list".to_string(),
        AdapterCommands::Install {
            component,
            framework,
        } => format!("adapter install {component} {framework}"),
        AdapterCommands::Remove {
            component,
            framework,
        } => format!("adapter remove {component} {framework}"),
        AdapterCommands::Scan => "adapter scan".to_string(),
    };
    Err(CliError::not_implemented(command))
}
