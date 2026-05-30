//! Systemd service management bridge.

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

/// Query the status of a systemd unit.
pub fn unit_status(_unit: &str) -> Result<UnitStatus, SystemdError> {
    // TODO(owner: platform-runtime, when: status/restart need live unit state):
    // invoke `systemctl show` and parse active/enabled/description fields.
    Ok(UnitStatus {
        active: false,
        enabled: false,
        description: String::new(),
    })
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

/// Enable and start a systemd unit.
pub fn enable_unit(_unit: &str) -> Result<(), SystemdError> {
    todo!("owner: platform-runtime; when service execute path ships; systemctl enable --now")
}

/// Stop and disable a systemd unit.
pub fn disable_unit(_unit: &str) -> Result<(), SystemdError> {
    todo!("owner: platform-runtime; when service execute path ships; systemctl disable --now")
}
