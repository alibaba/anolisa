mod color;
mod commands;
mod context;
mod packaged;
mod response;

use std::process::ExitCode;

use clap::FromArgMatches as _;

use crate::commands::Cli;
use crate::context::CliContext;

fn main() -> ExitCode {
    let matches = commands::build_cli().get_matches();
    let cli = Cli::from_arg_matches(&matches).expect("clap mismatch");
    let ctx = CliContext::from_cli(&cli);
    match commands::dispatch(cli, &ctx) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => response::render_error(&ctx, &err),
    }
}
