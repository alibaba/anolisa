//! Tier 2 surface — `anolisa runtime`: direct management of runtime-layer
//! components.
//!
//! `runtime update` is retained as a long-term compatibility alias for
//! `anolisa update runtime` (launch spec §7.3); handler hint redirects
//! callers to the unified update surface.

use clap::{Parser, Subcommand};

use crate::context::CliContext;
use crate::response::CliError;

#[derive(Parser)]
pub struct RuntimeArgs {
    #[command(subcommand)]
    pub command: RuntimeCommands,
}

#[derive(Subcommand)]
pub enum RuntimeCommands {
    /// Install a runtime component
    Install {
        /// Component name or "all"
        component: String,
        /// Install from source (build locally)
        #[arg(long)]
        from_source: bool,
        /// Install from RPM/DEB repository
        #[arg(long)]
        from_rpm: bool,
        /// Specific component version to install (e.g. 0.3.2).
        /// Renamed from --version to avoid colliding with the global -V/--version flag.
        #[arg(long = "component-version", value_name = "VERSION")]
        component_version: Option<String>,
    },
    /// Remove a runtime component
    Remove {
        component: String,
        /// Also remove configuration and data
        #[arg(long)]
        purge: bool,
    },
    /// Update a runtime component (alias of `anolisa update runtime <COMP>`)
    Update {
        /// Component name or "all"
        component: String,
    },
    /// Build a component from source
    Build {
        /// Component name or "all"
        component: String,
        /// Build in release mode (default)
        #[arg(long)]
        release: bool,
        /// Build in debug mode
        #[arg(long)]
        debug: bool,
        /// Build only, do not install
        #[arg(long)]
        no_install: bool,
    },
    /// List runtime components
    List {
        /// Show all available (not just installed)
        #[arg(long)]
        available: bool,
    },
    /// Show component status
    Status {
        /// Specific component (omit for all)
        component: Option<String>,
    },
}

pub fn handle(args: RuntimeArgs, _ctx: &CliContext) -> Result<(), CliError> {
    match args.command {
        RuntimeCommands::Install { component, .. } => Err(CliError::not_implemented(format!(
            "runtime install {component}"
        ))),
        RuntimeCommands::Remove { component, .. } => Err(CliError::not_implemented(format!(
            "runtime remove {component}"
        ))),
        RuntimeCommands::Update { component } => Err(CliError::not_implemented_with_hint(
            format!("runtime update {component}"),
            "long-term alias of `anolisa update runtime <COMPONENT>`; use that instead",
        )),
        RuntimeCommands::Build { component, .. } => Err(CliError::not_implemented(format!(
            "runtime build {component}"
        ))),
        RuntimeCommands::List { .. } => Err(CliError::not_implemented("runtime list")),
        RuntimeCommands::Status { component } => {
            let cmd = match component {
                Some(c) => format!("runtime status {c}"),
                None => "runtime status".to_string(),
            };
            Err(CliError::not_implemented(cmd))
        }
    }
}
