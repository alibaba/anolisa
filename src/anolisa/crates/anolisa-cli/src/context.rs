//! Process-wide CLI context constructed from global flags.
//!
//! Global flags (`--install-mode`, `--prefix`, `--json`, `--dry-run`,
//! `--verbose`, `--quiet`, `--no-color`) are parsed once on the top-level
//! `Cli` struct, projected into [`CliContext`], and then threaded through
//! every command handler. Handlers must not re-parse globals from the args
//! struct; instead they read from the shared context so that semantics stay
//! consistent across surfaces.

use std::path::PathBuf;

use clap::ValueEnum;

/// Where ANOLISA installs files: user-mode (`file-hierarchy(7)` under `$HOME`)
/// or system-mode (FHS under `/usr/local`, redirectable via `--prefix`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum InstallMode {
    User,
    System,
}

impl InstallMode {
    #[allow(dead_code)]
    pub fn as_str(&self) -> &'static str {
        match self {
            InstallMode::User => "user",
            InstallMode::System => "system",
        }
    }
}

/// Snapshot of global CLI flags, immutable for the lifetime of the process.
///
/// Several fields are not consumed yet by skeleton handlers; they are
/// kept on the context so that the dispatcher contract stays stable as
/// real implementations land.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CliContext {
    pub install_mode: InstallMode,
    pub prefix: Option<PathBuf>,
    pub json: bool,
    pub dry_run: bool,
    pub verbose: bool,
    pub quiet: bool,
    pub no_color: bool,
}

impl CliContext {
    /// Build a context from the parsed top-level [`crate::commands::Cli`].
    ///
    /// Borrows the CLI so the caller can still consume `cli.command` after.
    pub fn from_cli(cli: &crate::commands::Cli) -> Self {
        Self {
            install_mode: cli.install_mode,
            prefix: cli.prefix.clone(),
            json: cli.json,
            dry_run: cli.dry_run,
            verbose: cli.verbose,
            quiet: cli.quiet,
            no_color: cli.no_color,
        }
    }
}
