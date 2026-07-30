//! Systemd service management bridge.
//!
//! Thin wrapper around `systemctl(1)`. Each operation spawns the binary,
//! captures stdout/stderr, and maps the result onto [`SystemdError`].
//! `NotFound` is reserved for the well-known "unit X not loaded" error
//! pattern; everything else (binary missing, non-zero exit, parse failure)
//! collapses to `CommandFailed` with a human-readable message.

use std::process::{Command, Output};

use thiserror::Error;

/// Errors returned by systemd service operations.
#[derive(Debug, Error)]
pub enum SystemdError {
    /// `systemctl` returned a non-zero status or malformed output.
    #[error("systemctl command failed: {0}")]
    CommandFailed(String),
    /// The requested unit is not known to systemd.
    #[error("service not found: {0}")]
    NotFound(String),
}

/// Snapshot of systemd unit state used by status/restart flows.
#[derive(Debug)]
pub struct UnitStatus {
    /// Whether systemd currently reports the unit as active.
    pub active: bool,
    /// Whether the unit is enabled for automatic start.
    pub enabled: bool,
    /// Human-readable unit description from systemd metadata.
    pub description: String,
}

fn run_systemctl(args: &[&str]) -> Result<Output, SystemdError> {
    Command::new("systemctl")
        .args(args)
        .output()
        .map_err(|e| SystemdError::CommandFailed(format!("failed to spawn systemctl: {e}")))
}

fn classify_error(unit: &str, output: &Output) -> SystemdError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stderr}{stdout}");
    let lower = combined.to_lowercase();
    if lower.contains("could not be found")
        || lower.contains("not loaded")
        || lower.contains("no such file or directory")
        || lower.contains("unit file") && lower.contains("does not exist")
    {
        return SystemdError::NotFound(unit.to_string());
    }
    let trimmed = combined.trim();
    let detail = if trimmed.is_empty() {
        format!(
            "systemctl exited with {}",
            output
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string())
        )
    } else {
        trimmed.to_string()
    };
    SystemdError::CommandFailed(detail)
}

/// Query the status of a systemd unit via `systemctl show`.
pub fn unit_status(unit: &str) -> Result<UnitStatus, SystemdError> {
    if unit.trim().is_empty() {
        return Err(SystemdError::NotFound("<empty>".to_string()));
    }
    let out = run_systemctl(&[
        "show",
        unit,
        "--no-pager",
        "--property=LoadState,ActiveState,UnitFileState,Description",
    ])?;
    if !out.status.success() {
        return Err(classify_error(unit, &out));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut load_state = String::new();
    let mut active_state = String::new();
    let mut unit_file_state = String::new();
    let mut description = String::new();
    for line in stdout.lines() {
        if let Some(v) = line.strip_prefix("LoadState=") {
            load_state = v.to_string();
        } else if let Some(v) = line.strip_prefix("ActiveState=") {
            active_state = v.to_string();
        } else if let Some(v) = line.strip_prefix("UnitFileState=") {
            unit_file_state = v.to_string();
        } else if let Some(v) = line.strip_prefix("Description=") {
            description = v.to_string();
        }
    }
    if load_state == "not-found" || (load_state == "masked" && unit_file_state.is_empty()) {
        return Err(SystemdError::NotFound(unit.to_string()));
    }
    Ok(UnitStatus {
        active: active_state == "active" || active_state == "reloading",
        enabled: matches!(
            unit_file_state.as_str(),
            "enabled" | "enabled-runtime" | "alias" | "static" | "indirect"
        ),
        description,
    })
}

/// Enable and start a systemd unit (`systemctl enable --now <unit>`).
pub fn enable_unit(unit: &str) -> Result<(), SystemdError> {
    if unit.trim().is_empty() {
        return Err(SystemdError::NotFound("<empty>".to_string()));
    }
    let out = run_systemctl(&["enable", "--now", unit])?;
    if out.status.success() {
        return Ok(());
    }
    Err(classify_error(unit, &out))
}

/// Stop and disable a systemd unit (`systemctl disable --now <unit>`).
pub fn disable_unit(unit: &str) -> Result<(), SystemdError> {
    if unit.trim().is_empty() {
        return Err(SystemdError::NotFound("<empty>".to_string()));
    }
    let out = run_systemctl(&["disable", "--now", unit])?;
    if out.status.success() {
        return Ok(());
    }
    Err(classify_error(unit, &out))
}

/// Disable a unit without blocking on its stop sequence.
///
/// Unlike [`disable_unit`] (`disable --now`), this never waits for the unit to
/// drain its shutdown: `stop --no-block` enqueues termination and returns
/// immediately, then plain `disable` removes the `WantedBy` boot symlink. An
/// explicit stop also suppresses `Restart=`, so the unit will not churn after
/// it goes down. Use this when the caller has already flipped an authoritative
/// off-switch (e.g. a persistent opt-out marker) and only needs systemd to
/// stop relaunching the unit — without paying the unit's `TimeoutStopSec`.
pub fn disable_unit_deferred(unit: &str) -> Result<(), SystemdError> {
    if unit.trim().is_empty() {
        return Err(SystemdError::NotFound("<empty>".to_string()));
    }
    // Best-effort, non-blocking stop; the subsequent `disable` reports the
    // authoritative outcome, so a not-loaded/inactive unit here is ignored.
    let _ = run_systemctl(&["stop", "--no-block", unit]);
    let out = run_systemctl(&["disable", unit])?;
    if out.status.success() {
        return Ok(());
    }
    Err(classify_error(unit, &out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unimplemented_unit_operations_return_errors_instead_of_panicking() {
        assert!(unit_status("agentsight.service").is_err());
        assert!(enable_unit("agentsight.service").is_err());
        assert!(disable_unit("agentsight.service").is_err());
        assert!(disable_unit_deferred("agentsight.service").is_err());
    }
}
