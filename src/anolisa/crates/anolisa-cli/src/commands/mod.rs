//! Command-line surface.
//!
//! Two-tier structure (see design doc):
//! - **Tier 1** — capability-vocabulary verbs for everyday use (`tier1/`).
//! - **Tier 2** — independent management surfaces (subscription / adapter / self
//!   / runtime / osbase). Each surface uses its own appropriate vocabulary.

pub mod tier1;

// Tier 2 surfaces
pub mod adapter;
pub mod osbase;
pub mod runtime;
pub mod self_;
pub mod subscription;

use clap::{Parser, Subcommand};

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
    #[arg(long, global = true, default_value = "user")]
    pub install_mode: String,

    /// Custom install prefix (system-mode only)
    #[arg(long, global = true)]
    pub prefix: Option<String>,

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
    /// List capabilities and their availability / enable status
    List(tier1::list::ListArgs),
    /// Enable one or more capabilities
    Enable(tier1::enable::EnableArgs),
    /// Disable a capability or one of its features
    Disable(tier1::disable::DisableArgs),
    /// Show capability health
    Status(tier1::status::StatusArgs),
    /// Diagnose capability issues
    Doctor(tier1::doctor::DoctorArgs),
    /// Show service logs for a capability
    Logs(tier1::logs::LogsArgs),
    /// Restart the capability's underlying service
    Restart(tier1::restart::RestartArgs),
    /// Show environment detection results
    Env(tier1::env::EnvArgs),
    /// One-shot summary: anolisa version + enabled capabilities + components
    Info(tier1::info::InfoArgs),
    /// Update components behind a capability
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
pub fn dispatch(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        // Tier 1
        Commands::List(args) => tier1::list::handle(args),
        Commands::Enable(args) => tier1::enable::handle(args),
        Commands::Disable(args) => tier1::disable::handle(args),
        Commands::Status(args) => tier1::status::handle(args),
        Commands::Doctor(args) => tier1::doctor::handle(args),
        Commands::Logs(args) => tier1::logs::handle(args),
        Commands::Restart(args) => tier1::restart::handle(args),
        Commands::Env(args) => tier1::env::handle(args),
        Commands::Info(args) => tier1::info::handle(args),
        Commands::Update(args) => tier1::update::handle(args),
        // Tier 2
        Commands::Subscription(args) => subscription::handle(args),
        Commands::Adapter(args) => adapter::handle(args),
        Commands::SelfCmd(args) => self_::handle(args),
        Commands::Runtime(args) => runtime::handle(args),
        Commands::Osbase(args) => osbase::handle(args),
    }
}
