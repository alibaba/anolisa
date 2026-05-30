use clap::Parser;

#[derive(Parser)]
pub struct RestartArgs {
    /// Capability whose underlying service to restart
    pub capability: String,
}

pub fn handle(args: RestartArgs) -> anyhow::Result<()> {
    println!("Restarting service for {}...", args.capability);
    println!("  → service restart: not yet implemented");
    Ok(())
}
