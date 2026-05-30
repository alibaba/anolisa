use clap::Parser;

#[derive(Parser)]
pub struct DisableArgs {
    /// Capability to disable
    pub capability: String,
    /// Disable only the named sub-feature (capability stays enabled)
    #[arg(long, value_name = "NAME")]
    pub feature: Option<String>,
    /// Also remove installed files and config
    #[arg(long)]
    pub purge: bool,
}

pub fn handle(args: DisableArgs) -> anyhow::Result<()> {
    if let Some(f) = args.feature {
        println!("Disabling feature {f} of {}...", args.capability);
    } else if args.purge {
        println!("Purging {} (files + config)...", args.capability);
    } else {
        println!("Disabling {}...", args.capability);
    }
    println!("  → Capability Resolver not yet wired");
    Ok(())
}
