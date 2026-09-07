//! Process bootstrap and composition root for the `AgentSecCore` V2 daemon.
//!
//! The binary installs PAP handlers with a root-managed authorization policy
//! and a replaceable process-local Repository. Durable persistence remains a
//! later composition change.

#![forbid(unsafe_code)]

mod bootstrap;
mod cli;
mod runtime;
mod signals;

pub use bootstrap::{BootstrapConfig, BootstrapError, default_service_config, serve};
pub use cli::{Cli, CliError, ParseOutcome};
pub use runtime::{RuntimeError, run_with_shutdown_timeout};
pub use signals::{ProcessSignals, SignalError};
