use clap::Parser;

#[derive(Parser)]
pub struct LogsArgs {
    /// Capability whose service logs to show
    pub capability: String,
    /// Stream new log entries (like `tail -f`)
    #[arg(long)]
    pub follow: bool,
    /// Time window (e.g. `5m`, `1h`, `1d`)
    #[arg(long, value_name = "DURATION")]
    pub since: Option<String>,
    /// Number of trailing lines to show
    #[arg(long, value_name = "N")]
    pub lines: Option<u32>,
}

pub fn handle(args: LogsArgs) -> anyhow::Result<()> {
    println!(
        "Logs for {} (follow={}, since={}, lines={})",
        args.capability,
        args.follow,
        args.since.as_deref().unwrap_or("(default)"),
        args.lines
            .map(|n| n.to_string())
            .unwrap_or_else(|| "(default)".into()),
    );
    println!("  → log tailing: not yet implemented");
    Ok(())
}
