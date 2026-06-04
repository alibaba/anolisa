use clap::{Parser, Subcommand};

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

pub fn handle(args: SubscriptionArgs) -> anyhow::Result<()> {
    match args.command {
        SubscriptionCommands::Register { org, key, server } => {
            println!(
                "Registering with org={}, key={}, server={}",
                org.as_deref().unwrap_or("<interactive>"),
                key.as_deref().unwrap_or("<interactive>"),
                server.as_deref().unwrap_or("<interactive>")
            );
            println!("  → subscription register: not yet implemented");
        }
        SubscriptionCommands::Unregister { force } => {
            println!("Unregistering (force={force}): not yet implemented");
        }
        SubscriptionCommands::Status => {
            println!("Subscription: unregistered");
        }
        SubscriptionCommands::Refresh => {
            println!("Refreshing entitlements: not yet implemented");
        }
    }
    Ok(())
}
