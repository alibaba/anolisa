//! Command-line surface.
//!
//! Two-tier structure (see design doc):
//! - **Tier 1** — capability-vocabulary verbs for everyday use (`tier1/`).
//! - **Tier 2** — independent management surfaces (subscription / adapter / self
//!   / runtime / osbase). Each surface uses its own appropriate vocabulary.

pub mod common;
pub mod tier1;

// Tier 2 surfaces
pub mod adapter;
pub mod osbase;
pub mod runtime;
pub mod self_;
pub mod subscription;

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use crate::context::{CliContext, InstallMode};
use crate::response::CliError;

#[derive(Parser)]
#[command(
    name = "anolisa",
    about = "ANOLISA — Agentic OS helper",
    version,
    propagate_version = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Install scope: user (~/.local) or system (/usr/local)
    #[arg(long, global = true, value_enum, default_value_t = InstallMode::User)]
    pub install_mode: InstallMode,

    /// Custom install prefix (system-mode only)
    #[arg(long, global = true, value_name = "PATH")]
    pub prefix: Option<PathBuf>,

    /// Output in JSON format
    #[arg(long, global = true)]
    pub json: bool,

    /// Print plan without executing
    #[arg(long, global = true)]
    pub dry_run: bool,

    /// Increase verbosity
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Suppress non-error output
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Disable colored output
    #[arg(long, global = true)]
    pub no_color: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    // ── Tier 1 — Capability commands ────────────────────────────────
    /// Generate a bug report with diagnostic information
    Bug(tier1::bug::BugArgs),
    /// List capabilities and their availability / enable status
    List(tier1::list::ListArgs),
    /// Enable one or more capabilities
    Enable(tier1::enable::EnableArgs),
    /// Disable a capability or one of its features
    Disable(tier1::disable::DisableArgs),
    /// Uninstall a capability (removes ANOLISA-owned files); `--purge` also drops config
    Uninstall(tier1::uninstall::UninstallArgs),
    /// Show capability health
    Status(tier1::status::StatusArgs),
    /// Diagnose capability issues
    Doctor(tier1::doctor::DoctorArgs),
    /// Central log query (operation/audit + component-reported logs)
    Logs(tier1::logs::LogsArgs),
    /// Restart the capability's underlying service
    Restart(tier1::restart::RestartArgs),
    /// Show environment detection results
    Env(tier1::env::EnvArgs),
    /// One-shot summary: anolisa version + enabled capabilities + components
    Info(tier1::info::InfoArgs),
    /// Update self, runtime components, or everything ANOLISA-managed
    Update(tier1::update::UpdateArgs),

    // ── Tier 2 — Management surfaces ────────────────────────────────
    /// Manage ANOLISA subscription
    Subscription(subscription::SubscriptionArgs),
    /// Manage agent-framework adapters
    Adapter(adapter::AdapterArgs),
    /// Manage anolisa CLI itself
    #[command(name = "self")]
    SelfCmd(self_::SelfArgs),
    /// Manage runtime-layer components directly
    Runtime(runtime::RuntimeArgs),
    /// Manage OS base layer (kernel / sandbox / security)
    Osbase(osbase::OsbaseArgs),
}

/// Dispatch parsed CLI arguments to their handlers.
///
/// Every handler receives the immutable [`CliContext`] so global flags
/// such as `--json`, `--dry-run`, `--install-mode` stay consistent across
/// surfaces. Handlers must not re-parse global flags from their own
/// `args` struct.
pub fn dispatch(cli: Cli, ctx: &CliContext) -> Result<(), CliError> {
    validate_global_args(ctx)?;
    match cli.command {
        // Tier 1
        Commands::Bug(args) => tier1::bug::handle(args, ctx),
        Commands::List(args) => tier1::list::handle(args, ctx),
        Commands::Enable(args) => tier1::enable::handle(args, ctx),
        Commands::Disable(args) => tier1::disable::handle(args, ctx),
        Commands::Uninstall(args) => tier1::uninstall::handle(args, ctx),
        Commands::Status(args) => tier1::status::handle(args, ctx),
        Commands::Doctor(args) => tier1::doctor::handle(args, ctx),
        Commands::Logs(args) => tier1::logs::handle(args, ctx),
        Commands::Restart(args) => tier1::restart::handle(args, ctx),
        Commands::Env(args) => tier1::env::handle(args, ctx),
        Commands::Info(args) => tier1::info::handle(args, ctx),
        Commands::Update(args) => tier1::update::handle(args, ctx),
        // Tier 2
        Commands::Subscription(args) => subscription::handle(args, ctx),
        Commands::Adapter(args) => adapter::handle(args, ctx),
        Commands::SelfCmd(args) => self_::handle(args, ctx),
        Commands::Runtime(args) => runtime::handle(args, ctx),
        Commands::Osbase(args) => osbase::handle(args, ctx),
    }
}

fn validate_global_args(ctx: &CliContext) -> Result<(), CliError> {
    if let Some(prefix) = &ctx.prefix
        && !is_safe_absolute_path(prefix)
    {
        return Err(CliError::InvalidArgument {
            command: "global".to_string(),
            reason: format!(
                "--prefix must be an absolute path without '.' or '..' segments, got {}",
                prefix.display()
            ),
        });
    }
    Ok(())
}

fn is_safe_absolute_path(path: &Path) -> bool {
    path.is_absolute() && !path.as_os_str().is_empty() && !has_dot_segment(path)
}

fn has_dot_segment(path: &Path) -> bool {
    let raw = path.to_string_lossy();
    raw.split(std::path::MAIN_SEPARATOR)
        .any(|segment| segment == "." || segment == "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with_prefix(prefix: PathBuf) -> CliContext {
        CliContext {
            install_mode: InstallMode::System,
            prefix: Some(prefix),
            json: false,
            dry_run: false,
            verbose: false,
            quiet: false,
            no_color: false,
        }
    }

    #[test]
    fn global_prefix_must_be_absolute() {
        let err = validate_global_args(&ctx_with_prefix(PathBuf::from("relative")))
            .expect_err("relative prefix must be rejected");

        assert_eq!(err.code(), "INVALID_ARGUMENT");
    }

    #[test]
    fn global_prefix_rejects_traversal_segments() {
        let err = validate_global_args(&ctx_with_prefix(PathBuf::from("/opt/../etc")))
            .expect_err("traversing prefix must be rejected");

        assert_eq!(err.code(), "INVALID_ARGUMENT");
    }
}
