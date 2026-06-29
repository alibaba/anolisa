//! Process-wide CLI context constructed from global flags.
//!
//! Global flags (`--install-mode`, `--prefix`, `--json`, `--dry-run`,
//! `--verbose`, `--quiet`, `--no-color`) are parsed once on the top-level
//! `Cli` struct, projected into [`CliContext`], and then threaded through
//! every command handler. Handlers must not re-parse globals from the args
//! struct; instead they read from the shared context so that semantics stay
//! consistent across surfaces.
//!
//! When `--install-mode` is omitted, the effective scope is inferred from
//! the process's effective UID: root defaults to system, non-root to user.

use std::path::PathBuf;

use anolisa_platform::privilege;
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

/// Resolve the effective install mode from the explicit CLI value and a
/// privilege flag.
///
/// When the user passes `--install-mode`, that value wins unconditionally.
/// Otherwise the default is inferred from the process's effective UID:
/// root → [`InstallMode::System`], non-root → [`InstallMode::User`].
fn resolve_install_mode(explicit: Option<InstallMode>, effective_uid: u32) -> InstallMode {
    match explicit {
        Some(mode) => mode,
        None if effective_uid == 0 => InstallMode::System,
        None => InstallMode::User,
    }
}

impl CliContext {
    /// Build a context from the parsed top-level [`crate::commands::Cli`].
    ///
    /// Borrows the CLI so the caller can still consume `cli.command` after.
    /// The effective [`InstallMode`] is inferred from euid when
    /// `--install-mode` is not provided on the command line.
    pub fn from_cli(cli: &crate::commands::Cli) -> Self {
        let effective_uid = privilege::effective_uid();
        let effective_mode = resolve_install_mode(cli.install_mode, effective_uid);

        Self {
            install_mode: effective_mode,
            prefix: cli.prefix.clone(),
            json: cli.json,
            dry_run: cli.dry_run,
            verbose: cli.verbose,
            quiet: cli.quiet,
            no_color: cli.no_color,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_as_root_resolves_to_system() {
        assert_eq!(resolve_install_mode(None, 0), InstallMode::System);
    }

    #[test]
    fn omitted_as_non_root_resolves_to_user() {
        assert_eq!(resolve_install_mode(None, 1000), InstallMode::User);
    }

    #[test]
    fn explicit_user_stays_user_even_as_root() {
        assert_eq!(
            resolve_install_mode(Some(InstallMode::User), 0),
            InstallMode::User,
        );
    }

    #[test]
    fn explicit_system_stays_system_even_as_non_root() {
        assert_eq!(
            resolve_install_mode(Some(InstallMode::System), 1000),
            InstallMode::System,
        );
    }
}
