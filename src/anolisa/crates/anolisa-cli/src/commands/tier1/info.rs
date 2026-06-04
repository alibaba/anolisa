use clap::Parser;

use crate::context::CliContext;
use crate::response::CliError;

#[derive(Parser)]
pub struct InfoArgs {}

pub fn handle(_args: InfoArgs, _ctx: &CliContext) -> Result<(), CliError> {
    Err(CliError::not_implemented_with_hint(
        "info",
        "info summary needs catalog/state/env wiring; not implemented yet",
    ))
}
