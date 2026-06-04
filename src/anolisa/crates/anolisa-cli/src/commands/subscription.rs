use clap::{Parser, Subcommand};

use crate::context::CliContext;
use crate::response::CliError;

#[derive(Parser)]
pub struct SubscriptionArgs {
    #[command(subcommand)]
    pub command: SubscriptionCommands,
}

#[derive(Subcommand)]
pub enum SubscriptionCommands {
    /// Register this machine with ANOLISA subscription service
    Register {
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Unregister this machine
    Unregister {
        #[arg(long)]
        force: bool,
    },
    /// Show subscription status
    Status,
    /// Refresh entitlements from server
    Refresh,
}

pub fn handle(args: SubscriptionArgs, _ctx: &CliContext) -> Result<(), CliError> {
    let command = match args.command {
        SubscriptionCommands::Register { .. } => "subscription register",
        SubscriptionCommands::Unregister { .. } => "subscription unregister",
        SubscriptionCommands::Status => "subscription status",
        SubscriptionCommands::Refresh => "subscription refresh",
    };
    Err(CliError::not_implemented(command))
}
