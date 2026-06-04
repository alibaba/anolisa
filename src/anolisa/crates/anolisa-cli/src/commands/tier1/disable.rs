//! `anolisa disable <capability>` — P1-I logical / control-plane disable.
//!
//! This handler is the CLI face of [`anolisa_core::execute_disable`]. It
//! is intentionally state-only: nothing on this path deletes files,
//! stops services, calls adapters, or rewrites third-party configuration.
//! See the module docs on [`anolisa_core::disable_execute`] for the
//! scope rationale.
//!
//! Scope guards mirrored from `enable`:
//!
//! * `--feature <NAME>` is explicitly `NOT_IMPLEMENTED` today; we
//!   reject it up front with a hint so users see a clear contract
//!   instead of a silently-ignored flag. `--feature` graduates with
//!   the per-feature lifecycle hook work.
//! * `--purge` is rejected with a `NOT_IMPLEMENTED` hint that points
//!   the user at `anolisa uninstall <cap> --purge` — the dedicated
//!   destructive surface. We deliberately do NOT alias `disable
//!   --purge` to `uninstall --purge` because a verb typo must never
//!   silently delete ANOLISA-owned files.
//!
//! Error routing:
//!
//! | `DisableError`              | CLI code           | exit |
//! |-----------------------------|--------------------|------|
//! | `CapabilityNotInstalled`    | `INVALID_ARGUMENT` | 2    |
//! | `LockHeld`                  | `EXECUTION_FAILED` | 1    |
//! | `Lock`                      | `EXECUTION_FAILED` | 1    |
//! | `State`                     | `EXECUTION_FAILED` | 1    |
//! | `Log`                       | `EXECUTION_FAILED` | 1    |
//!
//! The "not installed" case is bucketed with `INVALID_ARGUMENT` because
//! it tells the caller "your input is wrong" (typo, or the capability
//! was already uninstalled) — not "the machine refused". The rest are
//! runtime IO failures and follow the same EXECUTION_FAILED bucket the
//! `enable` handler uses for `State` / `Log` / `Lock` variants.

use clap::Parser;

use anolisa_core::{DisableError, DisableOutcome, execute_disable};

use crate::color::Palette;
use crate::commands::common;
use crate::context::CliContext;
use crate::response::{CliError, render_json};

const COMMAND: &str = "disable";

#[derive(Parser)]
pub struct DisableArgs {
    /// Capability to disable
    pub capability: String,
    /// Disable only the named sub-feature (capability stays enabled)
    #[arg(long, value_name = "NAME")]
    pub feature: Option<String>,
    /// Also remove installed files and config
    #[arg(long)]
    pub purge: bool,
}

pub fn handle(args: DisableArgs, ctx: &CliContext) -> Result<(), CliError> {
    let command = format!("disable {}", args.capability);

    // Scope guards — both options require lifecycle hooks that P1-I
    // explicitly does not ship. We reject explicitly so users see the
    // boundary instead of getting a silent "OK, did nothing extra".
    if args.feature.is_some() {
        return Err(CliError::not_implemented_with_hint(
            command,
            "--feature is not supported yet (per-feature disable requires manifest hooks)",
        ));
    }
    if args.purge {
        // `disable --purge` is intentionally NOT a silent passthrough to
        // uninstall: that would let a typo in the verb (`disable`
        // instead of `uninstall`) destroy ANOLISA-owned files. Surface
        // a NOT_IMPLEMENTED hint that points the user at the dedicated
        // `uninstall --purge` surface instead.
        return Err(CliError::not_implemented_with_hint(
            command,
            "--purge is not supported on `disable`; run `anolisa uninstall <capability> --purge` for the destructive teardown",
        ));
    }

    let layout = common::resolve_layout(ctx);
    let install_mode = ctx.install_mode.as_str();

    let actor = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "cli".to_string());

    let outcome = execute_disable(&layout, &args.capability, &actor, install_mode)
        .map_err(|err| disable_err_to_cli(&args.capability, err))?;

    if ctx.json {
        let payload = DisablePayload::from(&outcome);
        return render_json(COMMAND, &payload);
    }

    if !ctx.quiet {
        render_human(&outcome, ctx.no_color);
    }
    Ok(())
}

/// Route a [`DisableError`] to the CLI error surface.
///
/// `CapabilityNotInstalled` is `INVALID_ARGUMENT` (the caller named a
/// capability that isn't in `installed.toml`); everything else is a
/// runtime IO failure and falls into the `EXECUTION_FAILED` bucket so
/// scripts can distinguish "fix your input" (exit 2) from "the machine
/// refused" (exit 1) — same convention as `enable`.
fn disable_err_to_cli(capability: &str, err: DisableError) -> CliError {
    match &err {
        DisableError::CapabilityNotInstalled { capability } => CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!(
                "capability '{capability}' is not installed — nothing to disable (run `anolisa status` to see what is installed)",
            ),
        },
        DisableError::LockHeld { path } => CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "install lock at {} is held by another process — run again after the other invocation finishes",
                path.display(),
            ),
        },
        DisableError::Lock { source } => CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!("install lock io: {source}"),
        },
        DisableError::State { source } => CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!("installed state write failed for '{capability}': {source}"),
        },
        DisableError::Log { source } => CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!("central log write failed for '{capability}': {source}"),
        },
        DisableError::HookFailed {
            phase,
            component,
            summary,
            exit_code,
        } => CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "lifecycle hook {phase} for component '{component}' failed (exit {}): {summary} — inspect the central log (`anolisa logs --kind component --component {component}`) and the hook script before retrying",
                exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "?".to_string()),
            ),
        },
    }
}

/// Wire shape for the success envelope. Defined here so `anolisa-core`
/// does not need to derive `Serialize` on its outcome struct.
#[derive(serde::Serialize)]
struct DisablePayload {
    operation_id: String,
    capability: String,
    previous_status: String,
    status: String,
    changed: bool,
    components: Vec<String>,
    state_path: String,
    central_log_path: String,
}

impl From<&DisableOutcome> for DisablePayload {
    fn from(o: &DisableOutcome) -> Self {
        Self {
            operation_id: o.operation_id.clone(),
            capability: o.capability.clone(),
            previous_status: o.previous_status.clone(),
            status: o.status.clone(),
            changed: o.changed,
            components: o.components.clone(),
            state_path: o.state_path.display().to_string(),
            central_log_path: o.central_log_path.display().to_string(),
        }
    }
}

fn render_human(outcome: &DisableOutcome, no_color: bool) {
    let color = Palette::new(no_color);
    if outcome.changed {
        println!(
            "{} {} {}",
            color.command("disable"),
            outcome.capability,
            color.ok("succeeded")
        );
    } else {
        println!(
            "{} {} {} {}",
            color.command("disable"),
            outcome.capability,
            color.ok("succeeded"),
            color.muted("(already disabled - no state change)")
        );
    }
    println!(
        "{}    {}",
        color.label("operation_id:"),
        color.id(&outcome.operation_id)
    );
    println!(
        "{} {}",
        color.label("previous_status:"),
        color.status(&outcome.previous_status)
    );
    println!(
        "{}          {}",
        color.label("status:"),
        color.status(&outcome.status)
    );
    println!(
        "{}         {}",
        color.label("changed:"),
        color.bool_value(outcome.changed)
    );
    if !outcome.components.is_empty() {
        println!("{}", color.header("components:"));
        for c in &outcome.components {
            println!("  - {c}");
        }
    }
    println!(
        "{} {}",
        color.label("state:"),
        color.path(outcome.state_path.display())
    );
    println!(
        "{}   {}",
        color.label("log:"),
        color.path(outcome.central_log_path.display())
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::context::InstallMode;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn ctx_with_prefix(
        json: bool,
        install_mode: InstallMode,
        prefix: Option<PathBuf>,
    ) -> CliContext {
        CliContext {
            install_mode,
            prefix,
            json,
            dry_run: false,
            verbose: false,
            quiet: true,
            no_color: true,
        }
    }

    fn args(capability: &str) -> DisableArgs {
        DisableArgs {
            capability: capability.to_string(),
            feature: None,
            purge: false,
        }
    }

    /// `--feature` is gated off until lifecycle hooks land. The handler
    /// must reject it BEFORE touching the layout / lock / state so the
    /// hint is the only side effect a user sees.
    #[test]
    fn disable_with_feature_returns_not_implemented() {
        let mut a = args("agent-observability");
        a.feature = Some("ws-ckpt".to_string());
        let err =
            handle(a, &ctx_with_prefix(false, InstallMode::System, None)).expect_err("must error");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
        assert!(
            err.hint().unwrap_or("").contains("--feature"),
            "hint must name the rejected flag: {:?}",
            err.hint(),
        );
    }

    /// `--purge` is gated off until file/service teardown lands. Same
    /// rationale as `--feature`: state-only disable must not silently
    /// promote itself to purge semantics.
    #[test]
    fn disable_with_purge_returns_not_implemented() {
        let mut a = args("agent-observability");
        a.purge = true;
        let err =
            handle(a, &ctx_with_prefix(false, InstallMode::System, None)).expect_err("must error");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
        assert!(
            err.hint().unwrap_or("").contains("--purge"),
            "hint must name the rejected flag: {:?}",
            err.hint(),
        );
    }

    /// Asking to disable a capability that is not installed must surface
    /// `INVALID_ARGUMENT` (exit 2), not `EXECUTION_FAILED`. The handler
    /// must not write to the central log on this path — confirmed by
    /// `disable_capability_not_installed_returns_error_and_writes_nothing`
    /// in the core layer. Here we only pin the CLI routing.
    #[test]
    fn disable_unknown_capability_routes_to_invalid_argument_exit_2() {
        let tmp = tempdir().expect("tmpdir");
        let err = handle(
            args("agent-observability"),
            &ctx_with_prefix(false, InstallMode::System, Some(tmp.path().to_path_buf())),
        )
        .expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert_eq!(err.exit_code(), 2);
        assert!(
            err.reason().contains("not installed"),
            "reason must mention 'not installed': {}",
            err.reason(),
        );
    }

    /// Routing pin for the runtime IO bucket: every non-input
    /// `DisableError` variant must surface as `EXECUTION_FAILED`
    /// (exit 1). This mirrors the `execute_err_*_maps_to_*` family in
    /// `enable.rs` so a future refactor of `execute_disable` cannot
    /// silently flip a bucket without breaking a test.
    #[test]
    fn disable_lock_held_maps_to_execution_failed_exit_1() {
        let err = disable_err_to_cli(
            "agent-observability",
            DisableError::LockHeld {
                path: PathBuf::from("/var/lib/anolisa/lock"),
            },
        );
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert_eq!(err.exit_code(), 1);
        assert!(err.reason().contains("/var/lib/anolisa/lock"));
    }
}
