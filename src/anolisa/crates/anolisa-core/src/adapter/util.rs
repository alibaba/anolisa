//! Pure, side-effect-free helpers shared by the built-in framework
//! drivers.
//!
//! These never spawn a process or mutate the filesystem, so they are safe to
//! call from `plan`/`status`/`prepare` paths. Built-in drivers share them here
//! rather than each re-declaring timestamp and formatting logic.

use super::driver::{CliOutput, ConditionStatus, FrameworkCommand};

/// ISO 8601 UTC timestamp, second precision.
pub(crate) fn now_iso8601() -> String {
    use chrono::{SecondsFormat, Utc};
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Map a bool to a [`ConditionStatus`] (`true` -> `True`, `false` -> `False`).
pub(crate) fn bool_status(b: bool) -> ConditionStatus {
    if b {
        ConditionStatus::True
    } else {
        ConditionStatus::False
    }
}

/// Compose a failure reason string from a non-success [`CliOutput`].
pub(crate) fn cli_failure_reason(verb: &str, output: &CliOutput) -> String {
    if output.timed_out {
        return format!("'{verb}' timed out");
    }
    let code = output
        .status
        .map(|c| c.to_string())
        .unwrap_or_else(|| "killed".to_string());
    let mut reason = format!("'{verb}' exited with {code}");
    let stderr = output.stderr.trim();
    if !stderr.is_empty() {
        reason.push_str(": ");
        reason.push_str(stderr);
    }
    reason
}

/// Human-readable form of a command for dry-run/preview output. Display
/// only — never parsed back into an argv.
pub(crate) fn display_command(cmd: &FrameworkCommand) -> String {
    let mut s = String::new();
    for (k, v) in &cmd.env_set {
        s.push_str(&format!("{k}={v} "));
    }
    s.push_str(&cmd.program);
    for a in &cmd.args {
        s.push(' ');
        s.push_str(a);
    }
    s
}
