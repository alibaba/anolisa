//! `anolisa update` — unified update surface (launch spec §7.3).
//!
//! Three subcommands:
//! - `update self` - update the `anolisa` CLI binary only.
//! - `update runtime <COMP|all>` - update one or all ANOLISA-managed
//!   runtime components.
//! - `update all` - update every ANOLISA-managed runtime, osbase, and
//!   adapter object.
//!
//! Explicit invariant (spec §7.3, decision §11.2): `update all` does
//! **not** include CLI self-update. The binary swap never shares a
//! transaction with component updates. Self-update is reachable via
//! both `anolisa update self` and `anolisa self update`.

use clap::{Parser, Subcommand};
use serde::Serialize;

use anolisa_core::self_update::{self, ProgressFn, SelfUpdateOutcome};

use crate::color::Palette;
use crate::context::CliContext;
use crate::response::{self, CliError};

/// Arguments for the unified update command surface.
#[derive(Parser)]
pub struct UpdateArgs {
    /// Selected update operation.
    #[command(subcommand)]
    pub command: UpdateCommands,
}

/// Update operations that intentionally keep CLI self-update separate from
/// component updates.
#[derive(Subcommand)]
pub enum UpdateCommands {
    /// Update the anolisa CLI binary only
    #[command(name = "self")]
    SelfBin,
    /// Update one or all ANOLISA-managed runtime components
    Runtime {
        /// Component name, or `all`
        target: String,
    },
    /// Update every ANOLISA-managed runtime, osbase, and adapter object.
    ///
    /// Does NOT include the CLI binary itself — use `anolisa update self`
    /// for that.
    All,
}

/// Dispatches the selected `anolisa update` subcommand.
///
/// # Errors
///
/// Returns [`CliError`] when the selected update operation fails or is not
/// implemented yet.
pub fn handle(args: UpdateArgs, ctx: &CliContext) -> Result<(), CliError> {
    match args.command {
        UpdateCommands::SelfBin => handle_self_update(ctx),
        UpdateCommands::Runtime { target } => Err(CliError::not_implemented_with_hint(
            format!("update runtime {target}"),
            "update planner / distribution resolver not implemented yet",
        )),
        UpdateCommands::All => Err(CliError::not_implemented_with_hint(
            "update all",
            "update planner / distribution resolver not implemented yet",
        )),
    }
}

/// Execute CLI self-update: fetch release manifest, compare versions,
/// download and atomically replace the running binary.
///
/// Also called from `anolisa self update` as a convenience alias.
///
/// # Errors
///
/// Returns [`CliError::Runtime`] when the manifest fetch, version check,
/// download, or binary replacement fails.
pub(in crate::commands) fn handle_self_update(ctx: &CliContext) -> Result<(), CliError> {
    let url = self_update::update_url();
    let current_version = env!("CARGO_PKG_VERSION");

    let progress_cb: Option<ProgressFn> = if !ctx.json && !ctx.quiet {
        Some(Box::new(move |downloaded: u64, total: Option<u64>| {
            render_progress(downloaded, total);
        }))
    } else {
        None
    };

    let result =
        self_update::check_and_update(&url, current_version, ctx.dry_run, progress_cb.as_ref());

    // Clear the progress line before any output (success or error).
    if progress_cb.is_some() {
        eprint!("\r\x1b[2K");
    }

    let outcome = result.map_err(|e| CliError::Runtime {
        command: "update self".to_string(),
        reason: e.to_string(),
    })?;

    if ctx.json {
        return render_json_outcome(&outcome, ctx.dry_run);
    }

    if ctx.quiet {
        return Ok(());
    }

    let color = Palette::new(ctx.no_color);
    match &outcome {
        SelfUpdateOutcome::AlreadyLatest { version } => {
            println!(
                "{} anolisa {} is already the latest version",
                color.ok("✓"),
                version
            );
        }
        SelfUpdateOutcome::UpdateAvailable { from, to } => {
            if ctx.dry_run {
                println!("{} update available: {} → {}", color.warn("⬆"), from, to);
                println!("  run without --dry-run to apply");
            } else {
                println!("{} anolisa updated: {} → {}", color.ok("✓"), from, to);
                eprintln!(
                    "  {} signature verification not yet implemented; \
                     update trust relies on HTTPS only",
                    color.warn("⚠")
                );
            }
        }
    }

    Ok(())
}

fn render_progress(downloaded: u64, total: Option<u64>) {
    match total {
        Some(t) if t > 0 => {
            let pct = (downloaded as f64 / t as f64 * 100.0).min(100.0);
            eprint!(
                "\r  downloading ... {:.1} / {:.1} MiB ({:.0}%)",
                downloaded as f64 / 1_048_576.0,
                t as f64 / 1_048_576.0,
                pct,
            );
        }
        _ => {
            eprint!(
                "\r  downloading ... {:.1} MiB",
                downloaded as f64 / 1_048_576.0,
            );
        }
    }
}

#[derive(Serialize)]
struct SelfUpdateData {
    current_version: String,
    latest_version: String,
    update_available: bool,
    updated: bool,
}

fn build_json_data(outcome: &SelfUpdateOutcome, dry_run: bool) -> SelfUpdateData {
    match outcome {
        SelfUpdateOutcome::AlreadyLatest { version } => SelfUpdateData {
            current_version: version.clone(),
            latest_version: version.clone(),
            update_available: false,
            updated: false,
        },
        SelfUpdateOutcome::UpdateAvailable { from, to } => SelfUpdateData {
            current_version: from.clone(),
            latest_version: to.clone(),
            update_available: true,
            updated: !dry_run,
        },
    }
}

fn render_json_outcome(outcome: &SelfUpdateOutcome, dry_run: bool) -> Result<(), CliError> {
    response::render_json("update self", build_json_data(outcome, dry_run))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_dry_run_reports_available_but_not_updated() {
        let outcome = SelfUpdateOutcome::UpdateAvailable {
            from: "0.1.0".into(),
            to: "0.2.0".into(),
        };
        let data = build_json_data(&outcome, true);
        assert!(data.update_available);
        assert!(!data.updated);
    }

    #[test]
    fn json_real_update_reports_both_true() {
        let outcome = SelfUpdateOutcome::UpdateAvailable {
            from: "0.1.0".into(),
            to: "0.2.0".into(),
        };
        let data = build_json_data(&outcome, false);
        assert!(data.update_available);
        assert!(data.updated);
    }

    #[test]
    fn json_already_latest_reports_both_false() {
        let outcome = SelfUpdateOutcome::AlreadyLatest {
            version: "0.1.0".into(),
        };
        let data = build_json_data(&outcome, false);
        assert!(!data.update_available);
        assert!(!data.updated);
    }
}
