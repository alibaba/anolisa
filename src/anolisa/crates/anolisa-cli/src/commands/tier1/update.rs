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

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use clap::{Parser, Subcommand};
use serde::Serialize;

use anolisa_core::self_update::{self, ProgressFn, SelfUpdateOutcome};

use crate::color::Palette;
use crate::commands::common;
use crate::context::CliContext;
use crate::repo_config::RepoConfig;
use crate::response::{self, CliError};

const CLI_CHANGELOG_URL: &str = "https://agentic-os.sh/#anolisa-cli-changelog";

/// TEMPORARY bootstrap: published copy of `templates/repo.toml`.
///
/// Until install/register provisions the user-editable repo config,
/// `anolisa update` downloads this copy when `<etc_dir>/repo.toml` is
/// absent, so a host that has only the CLI binary still ends up with the
/// production backend configuration. Remove once repo.toml provisioning
/// moves into the install/register flow.
const DEFAULT_REPO_CONFIG_URL: &str =
    "https://anolisa.oss-cn-hangzhou.aliyuncs.com/anolisa-releases/anolisa/v1/repo.toml";

/// Hard cap on the downloaded config size; repo.toml is a few KiB, so
/// anything larger is a misconfigured URL, not a config.
const MAX_REPO_CONFIG_BYTES: u64 = 256 * 1024;

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
    bootstrap_repo_config(ctx);
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

/// TEMPORARY: make sure the user-editable repo config exists before any
/// update operation runs (see [`DEFAULT_REPO_CONFIG_URL`]).
///
/// Best-effort by design: every failure mode (network down, bad TOML,
/// unwritable etc dir) degrades to a stderr warning — `update self` and
/// component updates must not be blocked by config bootstrap. The
/// download is validated as a parseable [`RepoConfig`] before anything
/// lands on disk, and the write is tmp + rename so a crash cannot leave
/// a half-written config behind.
fn bootstrap_repo_config(ctx: &CliContext) {
    let layout = common::resolve_layout(ctx);
    let dest = layout.etc_dir.join("repo.toml");
    if dest.exists() {
        return;
    }
    let url = std::env::var("ANOLISA_REPO_CONFIG_URL")
        .unwrap_or_else(|_| DEFAULT_REPO_CONFIG_URL.to_string());
    if ctx.dry_run {
        if !ctx.quiet && !ctx.json {
            println!(
                "would download repo config from {url} to {} (not present locally)",
                dest.display()
            );
        }
        return;
    }
    match fetch_and_write_repo_config(&url, &dest) {
        Ok(()) => {
            if !ctx.quiet && !ctx.json {
                let color = Palette::new(ctx.no_color);
                println!(
                    "{} repo config was missing — downloaded {} to {}",
                    color.ok("✓"),
                    url,
                    color.path(dest.display().to_string()),
                );
            }
        }
        Err(reason) => {
            eprintln!("warning: repo config bootstrap skipped: {reason}");
        }
    }
}

/// Download, validate, and atomically install the repo config at `dest`.
fn fetch_and_write_repo_config(url: &str, dest: &Path) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .build();
    let response = agent
        .get(url)
        .call()
        .map_err(|err| format!("fetch {url}: {err}"))?;
    let mut body = String::new();
    response
        .into_reader()
        .take(MAX_REPO_CONFIG_BYTES)
        .read_to_string(&mut body)
        .map_err(|err| format!("read {url}: {err}"))?;

    // Refuse to install bytes that the CLI itself cannot parse — a bad
    // published config must not break every subsequent command.
    RepoConfig::from_toml_str(&body).map_err(|err| format!("downloaded config invalid: {err}"))?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("create {}: {err}", parent.display()))?;
    }
    let tmp = dest.with_extension("toml.tmp");
    std::fs::write(&tmp, &body).map_err(|err| format!("write {}: {err}", tmp.display()))?;
    std::fs::rename(&tmp, dest).map_err(|err| {
        let _ = std::fs::remove_file(&tmp);
        format!("rename to {}: {err}", dest.display())
    })?;
    Ok(())
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
                println!("  view the changelog at {}", color.path(CLI_CHANGELOG_URL));
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

    /// Serve one HTTP response on an ephemeral port and return its URL.
    fn serve_once(body: &'static str) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                use std::io::Write;
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
            }
        });
        format!("http://{addr}/repo.toml")
    }

    #[test]
    fn bootstrap_fetch_writes_valid_config() {
        let body = "schema_version = 1\ndefault_backend = \"raw\"\n\n[backends.raw]\nbase_url = \"https://example.com/v1/\"\n";
        let url = serve_once(body);
        let tmp = tempfile::tempdir().expect("tempdir");
        let dest = tmp.path().join("etc/repo.toml");

        fetch_and_write_repo_config(&url, &dest).expect("bootstrap ok");
        assert_eq!(std::fs::read_to_string(&dest).expect("read dest"), body);
        assert!(!dest.with_extension("toml.tmp").exists());
    }

    #[test]
    fn bootstrap_fetch_refuses_unparseable_config() {
        let url = serve_once("this is not a repo config");
        let tmp = tempfile::tempdir().expect("tempdir");
        let dest = tmp.path().join("etc/repo.toml");

        let err = fetch_and_write_repo_config(&url, &dest).expect_err("must refuse");
        assert!(err.contains("invalid"), "unexpected error: {err}");
        assert!(!dest.exists(), "invalid config must not land on disk");
    }

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
