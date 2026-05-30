use clap::Parser;

#[derive(Parser)]
pub struct UpdateArgs {
    /// Capability to update, or `all`
    pub target: Option<String>,
}

pub fn handle(args: UpdateArgs) -> anyhow::Result<()> {
    let target = args.target.as_deref().unwrap_or("all");
    println!("Updating {target}...");
    println!("  → component update: not yet implemented");
    Ok(())
}
