use clap::Parser;

#[derive(Parser)]
pub struct InfoArgs {}

pub fn handle(_args: InfoArgs) -> anyhow::Result<()> {
    println!(
        "anolisa {}  (manifest schema v1)",
        env!("CARGO_PKG_VERSION")
    );
    println!();
    println!("Enabled capabilities (0):");
    println!("  → state file lookup not yet implemented");
    println!();
    println!("OS base:");
    println!("  → osbase status not yet implemented");
    Ok(())
}
