#![forbid(unsafe_code)]
//! cosh CLI — Computable Operating System Harness.
//!
//! Deterministic OS capability interface for Agents.

use std::time::Instant;

use clap::{error::ErrorKind, Parser, Subcommand};

mod cmd;

use cosh_platform::detect::Distro;
use cosh_types::error::{CoshError, ErrorCode};
use cosh_types::output::{CoshResponse, ResponseMeta};

#[derive(Parser)]
#[command(
    name = "cosh-cli",
    version,
    about = "Computable Operating System Harness — deterministic OS capability interface"
)]
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

fn main() {
    // Initialize tracing (stderr-only, controlled by COSH_LOG or RUST_LOG)
    let filter = std::env::var("COSH_LOG")
        .or_else(|_| std::env::var("RUST_LOG"))
        .unwrap_or_else(|_| "warn".to_string());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_new(&filter)
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .with_target(true)
        .try_init();

    let start = Instant::now();
    let distro = Distro::detect();

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            // --help and --version print to stdout and exit 0 — keep clap's
            // native behaviour for these, do not wrap in the JSON envelope.
            if matches!(
                e.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                e.exit();
            }
            // All other clap errors (missing args, unknown subcommand, etc.)
            // must honour the unified JSON envelope contract: emit the envelope
            // on STDOUT and exit 1 instead of clap's default (STDERR text, exit 2).
            let error = CoshError::new(ErrorCode::InvalidInput, e.to_string(), "cli")
                .with_details(serde_json::json!({"kind": "clap"}));
            let meta = ResponseMeta {
                subsystem: "cli".to_string(),
                duration_ms: start.elapsed().as_millis() as u64,
                distro: Some(distro.id_str().to_string()),
                dry_run: false,
                warning: None,
            };
            let exit_code = print_failure(error, meta);
            std::process::exit(exit_code);
        }
    };

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
pub fn build_meta_with_warning(
    subsystem: &str,
    distro: &Distro,
    start: Instant,
    dry_run: bool,
    warning: &str,
) -> ResponseMeta {
    ResponseMeta {
        subsystem: subsystem.to_string(),
        duration_ms: start.elapsed().as_millis() as u64,
        distro: Some(distro.id_str().to_string()),
        dry_run,
        warning: Some(warning.to_string()),
    }
}
