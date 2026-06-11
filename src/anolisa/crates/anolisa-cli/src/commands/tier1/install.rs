//! `anolisa install <COMPONENT>` — install a component.

use clap::Parser;

use crate::context::CliContext;
use crate::response::CliError;

#[derive(Parser)]
pub struct InstallArgs {
    /// Component name to install.
    pub component: String,
}

pub fn handle(_args: InstallArgs, _ctx: &CliContext) -> Result<(), CliError> {
    Err(CliError::not_implemented_with_hint(
        "install",
        "component install is not implemented yet",
    ))
}
