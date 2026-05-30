use clap::Parser;

#[derive(Parser)]
pub struct EnableArgs {
    /// Capability name(s) to enable
    #[arg(required = true)]
    pub capabilities: Vec<String>,
    /// Only enable a specific sub-feature (capability must already be enabled)
    #[arg(long, value_name = "NAME")]
    pub feature: Option<String>,
    /// Adapter framework selection: explicit list ("cosh,openclaw"), `auto`, or omit for first-party only
    #[arg(long, value_name = "FRAMEWORKS|auto")]
    pub with_adapter: Option<String>,
    /// Build component(s) from source instead of installing prebuilt
    #[arg(long)]
    pub from_source: bool,
}

pub fn handle(args: EnableArgs) -> anyhow::Result<()> {
    for cap in &args.capabilities {
        println!("Resolving {cap}...");
        if let Some(ref f) = args.feature {
            println!("  → feature override: {f}");
        }
        match args.with_adapter.as_deref() {
            Some("auto") => println!("  → adapter mode: autoprobe + install all detected"),
            Some(list) => println!("  → adapter mode: explicit ({list})"),
            None => println!("  → adapter mode: first-party only"),
        }
        if args.from_source {
            println!("  → source build requested");
        }
        println!("  → Capability Resolver not yet wired");
    }
    Ok(())
}
