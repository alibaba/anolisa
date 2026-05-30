use clap::Parser;

#[derive(Parser)]
pub struct StatusArgs {
    /// Show detail for a specific capability (omit for aggregate view)
    pub capability: Option<String>,
}

pub fn handle(args: StatusArgs) -> anyhow::Result<()> {
    match args.capability {
        Some(cap) => println!("Status for {cap}: not yet implemented"),
        None => println!("Aggregate capability status: not yet implemented"),
    }
    Ok(())
}
