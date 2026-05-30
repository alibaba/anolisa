//! Package manager abstraction (dnf/apt/zypper).

use thiserror::Error;

/// Errors returned by native package-manager backends.
#[derive(Debug, Error)]
pub enum PkgError {
    /// The native command exited unsuccessfully or could not be invoked.
    #[error("package manager command failed: {0}")]
    CommandFailed(String),
    /// No backend is available for the host package base.
    #[error("unsupported package base")]
    Unsupported,
}

/// Abstraction over system package managers.
pub trait PackageManager {
    /// Install every package listed by the caller.
    fn install(&self, packages: &[&str]) -> Result<(), PkgError>;
    /// Remove every package listed by the caller.
    fn remove(&self, packages: &[&str]) -> Result<(), PkgError>;
    /// Return whether a package is already present according to the native DB.
    fn is_installed(&self, package: &str) -> bool;
}

/// DNF/YUM-family package backend.
pub struct DnfBackend;

/// APT/dpkg-family package backend.
pub struct AptBackend;

impl PackageManager for DnfBackend {
    fn install(&self, _packages: &[&str]) -> Result<(), PkgError> {
        todo!("owner: platform-runtime; when native rpm installs ship; dnf install")
    }
    fn remove(&self, _packages: &[&str]) -> Result<(), PkgError> {
        todo!("owner: platform-runtime; when native rpm removal ships; dnf remove")
    }
    fn is_installed(&self, _package: &str) -> bool {
        todo!("owner: platform-runtime; when native rpm status probes ship; rpm -q")
    }
}

impl PackageManager for AptBackend {
    fn install(&self, _packages: &[&str]) -> Result<(), PkgError> {
        todo!("owner: platform-runtime; when native deb installs ship; apt install")
    }
    fn remove(&self, _packages: &[&str]) -> Result<(), PkgError> {
        todo!("owner: platform-runtime; when native deb removal ships; apt remove")
    }
    fn is_installed(&self, _package: &str) -> bool {
        todo!("owner: platform-runtime; when native deb status probes ship; dpkg -l")
    }
}
