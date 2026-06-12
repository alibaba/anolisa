#![forbid(unsafe_code)]
//! cosh CLI — Computable Operating System Harness.
//!
//! Deterministic OS capability interface for Agents.

use std::path::PathBuf;
use std::time::Instant;

use clap::{Parser, Subcommand};

mod cmd;

use cosh_platform::detect::Distro;
use cosh_types::output::{CoshResponse, ResponseMeta};

#[derive(Parser)]
#[command(name = "cosh", version, about = "Computable Operating System Harness — deterministic OS capability interface")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Package management (cross-distro: dnf/apt/zypper)
    Pkg {
        #[command(subcommand)]
        action: cmd::pkg::PkgCommands,
    },
    /// Service management (systemd)
    Svc {
        #[command(subcommand)]
        action: cmd::svc::SvcCommands,
    },
    /// Workspace checkpoints (ws-ckpt integration)
    Checkpoint {
        #[command(subcommand)]
        action: cmd::checkpoint::CheckpointCommands,
    },
    /// Security audit
    Audit {
        #[command(subcommand)]
        action: cmd::audit::AuditCommands,
    },
}

/// Locate the `cosh-tui` binary: first check adjacent to the current executable,
/// then fall back to PATH lookup.
fn find_cosh_tui() -> Option<PathBuf> {
    // Try sibling directory of the current executable.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("cosh-tui");
            if sibling.is_file() {
                return Some(sibling);
            }
        }
    }
    // Fall back to PATH lookup via `which`-style resolution.
    which::which("cosh-tui").ok()
}

/// Attempt to exec into `cosh-tui` when invoked with no arguments.
fn dispatch_tui() -> ! {
    match find_cosh_tui() {
        Some(tui_path) => {
            exec_tui(&tui_path);
        }
        None => {
            eprintln!("cosh: no subcommand provided and `cosh-tui` was not found.");
            eprintln!("  Install cosh-tui or run `cosh --help` for available subcommands.");
            std::process::exit(127);
        }
    }
}

/// Platform-specific exec into the TUI binary.
#[cfg(unix)]
fn exec_tui(path: &PathBuf) -> ! {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(path).exec();
    eprintln!("cosh: failed to exec cosh-tui: {err}");
    std::process::exit(1);
}

#[cfg(not(unix))]
fn exec_tui(path: &PathBuf) -> ! {
    let status = std::process::Command::new(path)
        .status()
        .unwrap_or_else(|e| {
            eprintln!("cosh: failed to launch cosh-tui: {e}");
            std::process::exit(1);
        });
    std::process::exit(status.code().unwrap_or(1));
}

fn main() {
    // If invoked with zero arguments (only the binary name), dispatch to cosh-tui.
    if std::env::args().count() == 1 {
        dispatch_tui();
    }

    let cli = Cli::parse();
    let distro = Distro::detect();
    let start = Instant::now();

    let exit_code = match cli.command {
        Commands::Pkg { action } => cmd::pkg::run(action, &distro, start),
        Commands::Svc { action } => cmd::svc::run(action, &distro, start),
        Commands::Checkpoint { action } => cmd::checkpoint::run(action, &distro, start),
        Commands::Audit { action } => cmd::audit::run(action, &distro, start),
    };

    std::process::exit(exit_code);
}

/// Print a successful CoshResponse as JSON and return exit code 0.
pub fn print_success<T: serde::Serialize>(data: T, meta: ResponseMeta) -> i32 {
    let resp = CoshResponse::success(data, meta);
    match serde_json::to_string_pretty(&resp) {
        Ok(json) => println!("{}", json),
        Err(e) => {
            eprintln!("{{\"ok\":false,\"error\":\"serialization failed: {}\"}}", e);
            return 1;
        }
    }
    0
}

/// Print a failure CoshResponse as JSON and return exit code 1.
pub fn print_failure(error: cosh_types::error::CoshError, meta: ResponseMeta) -> i32 {
    let resp: CoshResponse<()> = CoshResponse::failure(error, meta);
    match serde_json::to_string_pretty(&resp) {
        Ok(json) => println!("{}", json),
        Err(e) => {
            eprintln!("{{\"ok\":false,\"error\":\"serialization failed: {}\"}}", e);
        }
    }
    1
}

/// Build ResponseMeta from common parameters.
pub fn build_meta(subsystem: &str, distro: &Distro, start: Instant, dry_run: bool) -> ResponseMeta {
    ResponseMeta {
        subsystem: subsystem.to_string(),
        duration_ms: start.elapsed().as_millis() as u64,
        distro: Some(distro.id_str().to_string()),
        dry_run,
        warning: None,
    }
}

/// Build ResponseMeta with a warning message.
pub fn build_meta_with_warning(subsystem: &str, distro: &Distro, start: Instant, dry_run: bool, warning: &str) -> ResponseMeta {
    ResponseMeta {
        subsystem: subsystem.to_string(),
        duration_ms: start.elapsed().as_millis() as u64,
        distro: Some(distro.id_str().to_string()),
        dry_run,
        warning: Some(warning.to_string()),
    }
}
