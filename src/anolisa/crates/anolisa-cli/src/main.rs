mod commands;

use clap::Parser;
use commands::Cli;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    commands::dispatch(cli)
}
