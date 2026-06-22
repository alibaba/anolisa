//! Command-line surface.
//!
//! Two-tier structure (see design doc):
//! - **Tier 1** — component lifecycle verbs for everyday use (`tier1/`).
//! - **Tier 2** — independent management surfaces (register / adapter / self
//!   / runtime / osbase). Each surface uses its own appropriate vocabulary.

pub mod common;
pub mod tier1;

// Tier 2 surfaces
pub mod adapter;
pub mod osbase;
pub mod register;
pub mod system;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use clap::{CommandFactory, Parser, Subcommand};

use crate::context::{CliContext, InstallMode};
use crate::response::CliError;

const HELP_TEMPLATE: &str = "\
{before-help}{name} {version}\n\
{about-with-newline}\n\
{usage-heading} {usage}\
{after-help}\
\nOptions:\n{options}";

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

    /// Install scope: user (~/.local) or system (/usr/local).
    /// When omitted, defaults to system if running as root, user otherwise.
    #[arg(long, global = true, value_enum)]
    pub install_mode: Option<InstallMode>,

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
    #[command(flatten)]
    Component(ComponentCommands),

    #[command(flatten)]
    Management(ManagementCommands),
}

/// Primary commands — component lifecycle and operations.
#[derive(Subcommand)]
pub enum ComponentCommands {
    /// List available components from remote catalog
    #[command(visible_alias = "ls")]
    List(tier1::list::ListArgs),
    /// Install a component from a configured backend (raw today; rpm/npm planned)
    Install(tier1::install::InstallArgs),
    /// Uninstall a component
    Uninstall(tier1::uninstall::UninstallArgs),
    /// Show component health
    Status(tier1::status::StatusArgs),
    /// Diagnose component issues
    Doctor(tier1::doctor::DoctorArgs),
    /// Query logs, optionally filtered by component and level
    Logs(tier1::logs::LogsArgs),
    /// Restart a component's service
    Restart(tier1::restart::RestartArgs),
    /// Update a component (`update <component>`), the CLI binary (`self`), or everything (`all`)
    Update(tier1::update::UpdateArgs),
    /// Reconcile a component's ANOLISA state with rpmdb after manual RPM changes
    Repair(tier1::repair::RepairArgs),
    /// Drop a component's ANOLISA state record without any package operation
    Forget(tier1::forget::ForgetArgs),
    /// Record an already-installed system RPM as rpm-observed (system scope)
    Adopt(tier1::adopt::AdoptArgs),
    /// Manage component-to-framework adapters
    Adapter(adapter::AdapterArgs),
}

/// Management commands.
#[derive(Subcommand)]
pub enum ManagementCommands {
    /// Join the Agentic OS Co-Build Program (requires root/sudo)
    Register(register::RegisterArgs),
    /// Leave the Agentic OS Co-Build Program (requires root/sudo)
    Unregister(register::UnregisterArgs),
    /// Show environment detection results
    Env(tier1::env::EnvArgs),
    /// Generate a bug report
    Bug(tier1::bug::BugArgs),
    /// Manage OS base layer (kernel / sandbox / security)
    Osbase(osbase::OsbaseArgs),
    /// System helper daemon management
    System(system::SystemArgs),
}

/// Build the top-level [`clap::Command`] with grouped help rendering.
///
/// Generates the "Component Commands / Management Commands / Other"
/// sections dynamically from the registered subcommands so that adding
/// a new variant to [`ComponentCommands`] or [`ManagementCommands`]
/// automatically updates `--help` without maintaining a separate const.
pub fn build_cli() -> clap::Command {
    let cmd = Cli::command();
    let help_text = generate_grouped_help();
    cmd.help_template(HELP_TEMPLATE).after_help(help_text)
}

fn generate_grouped_help() -> String {
    let cap = subcommand_rows::<ComponentCommands>();
    let mgmt = subcommand_rows::<ManagementCommands>();
    render_grouped_help(&cap, &mgmt)
}

fn subcommand_rows<T: Subcommand>() -> Vec<(String, String)> {
    let cmd = T::augment_subcommands(clap::Command::new("group"));
    let mut rows = Vec::new();
    for sub in cmd.get_subcommands() {
        let name = sub.get_name();
        let aliases: Vec<&str> = sub.get_visible_aliases().collect();
        let display = if aliases.is_empty() {
            name.to_string()
        } else {
            format!("{name}, {}", aliases.join(", "))
        };
        let about = sub.get_about().map(|s| s.to_string()).unwrap_or_default();

        rows.push((display, about));
    }
    rows
}

fn render_grouped_help(cap: &[(String, String)], mgmt: &[(String, String)]) -> String {
    let longest = cap
        .iter()
        .chain(mgmt)
        .map(|(d, _)| d.len())
        .max()
        .unwrap_or(0)
        .max(4); // "help" length

    let mut out = String::from("Commands:\n");
    for (display, about) in cap {
        let _ = writeln!(out, "  {display:<longest$}  {about}");
    }
    out.push_str("\nManagement Commands:\n");
    for (display, about) in mgmt {
        let _ = writeln!(out, "  {display:<longest$}  {about}");
    }
    out.push_str("\nOther:\n");
    let _ = writeln!(
        out,
        "  {:<longest$}  Print this message or the help of the given subcommand(s)",
        "help"
    );
    out
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
        Commands::Component(cmd) => match cmd {
            ComponentCommands::List(args) => tier1::list::handle(args, ctx),
            ComponentCommands::Install(args) => tier1::install::handle(args, ctx),
            ComponentCommands::Uninstall(args) => tier1::uninstall::handle(args, ctx),
            ComponentCommands::Status(args) => tier1::status::handle(args, ctx),
            ComponentCommands::Doctor(args) => tier1::doctor::handle(args, ctx),
            ComponentCommands::Logs(args) => tier1::logs::handle(args, ctx),
            ComponentCommands::Restart(args) => tier1::restart::handle(args, ctx),
            ComponentCommands::Update(args) => tier1::update::handle(args, ctx),
            ComponentCommands::Repair(args) => tier1::repair::handle(args, ctx),
            ComponentCommands::Forget(args) => tier1::forget::handle(args, ctx),
            ComponentCommands::Adopt(args) => tier1::adopt::handle(args, ctx),
            ComponentCommands::Adapter(args) => adapter::handle(args, ctx),
        },
        Commands::Management(cmd) => match cmd {
            ManagementCommands::Register(args) => register::handle_register_group(args, ctx),
            ManagementCommands::Unregister(args) => register::handle_unregister_cmd(args, ctx),
            ManagementCommands::Env(args) => tier1::env::handle(args, ctx),
            ManagementCommands::Bug(args) => tier1::bug::handle(args, ctx),
            ManagementCommands::Osbase(args) => osbase::handle(args, ctx),
            ManagementCommands::System(args) => system::handle(args, ctx),
        },
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
    use clap::FromArgMatches as _;

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

    #[test]
    fn generated_help_includes_all_subcommands() {
        let mut cmd = build_cli();
        let help = cmd.render_help().to_string();

        for sub in Cli::command().get_subcommands() {
            let name = sub.get_name();
            assert!(
                help.contains(name),
                "subcommand `{name}` missing from generated help output"
            );
        }
    }

    #[test]
    fn generated_help_keeps_management_commands_in_management_section() {
        let mut cmd = build_cli();
        let help = cmd.render_help().to_string();
        let management = help
            .split("Management Commands:\n")
            .nth(1)
            .and_then(|rest| rest.split("\nOther:\n").next())
            .expect("management help section must exist");

        for sub in
            ManagementCommands::augment_subcommands(clap::Command::new("group")).get_subcommands()
        {
            let name = sub.get_name();
            assert!(
                management.contains(name),
                "management subcommand `{name}` missing from Management Commands section"
            );
        }
    }

    #[test]
    fn alias_appears_in_help() {
        let mut cmd = build_cli();
        let help = cmd.render_help().to_string();
        assert!(
            help.contains("ls"),
            "visible alias `ls` should appear in help output"
        );
    }

    #[test]
    fn install_mode_is_none_when_omitted() {
        let cmd = build_cli();
        let matches = cmd.try_get_matches_from(["anolisa", "status"]).unwrap();
        let cli = Cli::from_arg_matches(&matches).unwrap();
        assert_eq!(cli.install_mode, None);
    }

    #[test]
    fn install_mode_is_some_when_explicitly_set() {
        let cmd = build_cli();
        let matches = cmd
            .try_get_matches_from(["anolisa", "--install-mode=system", "status"])
            .unwrap();
        let cli = Cli::from_arg_matches(&matches).unwrap();
        assert_eq!(cli.install_mode, Some(InstallMode::System));
    }
}
