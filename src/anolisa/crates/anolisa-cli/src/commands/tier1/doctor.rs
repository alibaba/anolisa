use clap::Parser;

#[derive(Parser)]
pub struct DoctorArgs {
    /// Diagnose a specific capability (default: all enabled)
    pub capability: Option<String>,
    /// Apply suggested fixes automatically
    #[arg(long)]
    pub fix: bool,
}

pub fn handle(args: DoctorArgs) -> anyhow::Result<()> {
    println!("ANOLISA capability health check");
    match args.capability {
        Some(cap) => println!("[{cap}] checking..."),
        None => println!("[all enabled capabilities] checking..."),
    }
    if args.fix {
        println!("  (auto-fix enabled)");
    }
    println!("  → doctor diagnostics: not yet implemented");
    Ok(())
}
