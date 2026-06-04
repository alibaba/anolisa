mod color;
mod commands;
mod context;
mod execution_policy;
mod packaged;
mod response;

use std::process::ExitCode;

use clap::Parser;

use crate::commands::Cli;
use crate::context::CliContext;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let ctx = CliContext::from_cli(&cli);
    match commands::dispatch(cli, &ctx) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => response::render_error(&ctx, &err),
    }
}
