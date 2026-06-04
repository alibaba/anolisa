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
pub fn unit_status(unit: &str) -> Result<UnitStatus, SystemdError> {
    if unit.trim().is_empty() {
        return Err(SystemdError::NotFound("<empty>".to_string()));
    }
    // TODO(owner: platform-runtime, when: status/restart need live unit state):
    // invoke `systemctl show` and parse active/enabled/description fields.
    Err(SystemdError::CommandFailed(
        "systemd unit status query is not implemented".to_string(),
    ))
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
pub fn enable_unit(unit: &str) -> Result<(), SystemdError> {
    if unit.trim().is_empty() {
        return Err(SystemdError::NotFound("<empty>".to_string()));
    }
    // TODO(owner: platform-runtime, when: service execute path ships):
    // invoke `systemctl enable --now <unit>` and surface command status.
    Err(SystemdError::CommandFailed(
        "systemd unit enable is not implemented".to_string(),
    ))
}

/// Stop and disable a systemd unit.
pub fn disable_unit(unit: &str) -> Result<(), SystemdError> {
    if unit.trim().is_empty() {
        return Err(SystemdError::NotFound("<empty>".to_string()));
    }
    // TODO(owner: platform-runtime, when: service execute path ships):
    // invoke `systemctl disable --now <unit>` and surface command status.
    Err(SystemdError::CommandFailed(
        "systemd unit disable is not implemented".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unimplemented_unit_operations_return_errors_instead_of_panicking() {
        assert!(unit_status("agentsight.service").is_err());
        assert!(enable_unit("agentsight.service").is_err());
        assert!(disable_unit("agentsight.service").is_err());
    }
}
