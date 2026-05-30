use clap::Parser;

#[derive(Parser)]
pub struct ListArgs {
    /// Show only capabilities available on this machine
    #[arg(long)]
    pub available: bool,
    /// Show only currently enabled capabilities
    #[arg(long)]
    pub enabled: bool,
}

pub fn handle(args: ListArgs) -> anyhow::Result<()> {
    let filter = match (args.available, args.enabled) {
        (true, _) => "available",
        (_, true) => "enabled",
        _ => "all",
    };
    println!("CAPABILITY              STATUS       NOTE");
    println!("(filter: {filter}) — Capability Resolver not yet wired");
    Ok(())
}
