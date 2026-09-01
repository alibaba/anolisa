//! Command-line adapter for the `AgentSecCore` system daemon.

#![forbid(unsafe_code)]

mod args;
mod error;
mod request;
mod run;

pub use args::Cli;
pub use error::CliError;
pub use run::execute;
