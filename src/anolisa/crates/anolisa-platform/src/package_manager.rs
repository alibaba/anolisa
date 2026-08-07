//! Package manager abstraction (dnf/apt/zypper).

use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::process::{Command, ExitStatus, Stdio};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PkgError {
    #[error("package manager command failed: {0}")]
    CommandFailed(String),
    #[error("unsupported package base: {0}")]
    Unsupported(String),
}

/// Abstraction over system package managers.
pub trait PackageManager {
    fn install(&self, packages: &[&str]) -> Result<(), PkgError>;
    fn remove(&self, packages: &[&str]) -> Result<(), PkgError>;
    fn is_installed(&self, package: &str) -> bool;
}

/// DNF/YUM backend for RPM-based distros (Anolis, ALINUX, RHEL, Fedora).
pub struct DnfBackend;

/// APT backend for DEB-based distros (Ubuntu, Debian).
pub struct AptBackend;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PackageManagerKind {
    Dnf,
    Apt,
}

impl PackageManagerKind {
    fn into_manager(self) -> Box<dyn PackageManager> {
        match self {
            Self::Dnf => Box::new(DnfBackend),
            Self::Apt => Box::new(AptBackend),
        }
    }
}

impl PackageManager for DnfBackend {
    fn install(&self, packages: &[&str]) -> Result<(), PkgError> {
        if packages.is_empty() {
            return Ok(());
        }
        let status = run_with_progress(
            Command::new("dnf")
                .args(["install", "-y", "--setopt=install_weak_deps=False"])
                .args(packages),
        )
        .map_err(|e| PkgError::CommandFailed(format!("failed to spawn dnf: {e}")))?;
        if !status.success() {
            return Err(PkgError::CommandFailed(format!(
                "dnf install exited with {status}"
            )));
        }
        Ok(())
    }

    fn remove(&self, packages: &[&str]) -> Result<(), PkgError> {
        if packages.is_empty() {
            return Ok(());
        }
        let status = run_with_progress(Command::new("dnf").args(["remove", "-y"]).args(packages))
            .map_err(|e| PkgError::CommandFailed(format!("failed to spawn dnf: {e}")))?;
        if !status.success() {
            return Err(PkgError::CommandFailed(format!(
                "dnf remove exited with {status}"
            )));
        }
        Ok(())
    }

    fn is_installed(&self, package: &str) -> bool {
        Command::new("rpm")
            .args(["-q", package])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

impl PackageManager for AptBackend {
    fn install(&self, packages: &[&str]) -> Result<(), PkgError> {
        if packages.is_empty() {
            return Ok(());
        }
        let status = run_with_progress(
            Command::new("apt-get")
                .args(["install", "-y", "--no-install-recommends"])
                .args(packages)
                .env("DEBIAN_FRONTEND", "noninteractive"),
        )
        .map_err(|e| PkgError::CommandFailed(format!("failed to spawn apt-get: {e}")))?;
        if !status.success() {
            return Err(PkgError::CommandFailed(format!(
                "apt-get install exited with {status}"
            )));
        }
        Ok(())
    }

    fn remove(&self, packages: &[&str]) -> Result<(), PkgError> {
        if packages.is_empty() {
            return Ok(());
        }
        let status = run_with_progress(
            Command::new("apt-get")
                .args(["remove", "-y"])
                .args(packages)
                .env("DEBIAN_FRONTEND", "noninteractive"),
        )
        .map_err(|e| PkgError::CommandFailed(format!("failed to spawn apt-get: {e}")))?;
        if !status.success() {
            return Err(PkgError::CommandFailed(format!(
                "apt-get remove exited with {status}"
            )));
        }
        Ok(())
    }

    fn is_installed(&self, package: &str) -> bool {
        Command::new("dpkg")
            .args(["-s", package])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

fn run_with_progress(command: &mut Command) -> io::Result<ExitStatus> {
    let stderr = io::stderr();
    let progress = stderr.as_fd().try_clone_to_owned()?;
    run_with_progress_to(command, progress)
}

fn run_with_progress_to(command: &mut Command, progress: OwnedFd) -> io::Result<ExitStatus> {
    command
        .stdout(Stdio::from(progress))
        .stderr(Stdio::inherit())
        .status()
}

/// Detect the appropriate package manager for the current system.
///
/// Uses `pkg_base` from `EnvFacts` to select the backend. Falls back to
/// checking binary availability when the hint is absent or unrecognized.
pub fn detect_package_manager(pkg_base: Option<&str>) -> Result<Box<dyn PackageManager>, PkgError> {
    if let Some(kind) = pkg_base.and_then(package_manager_kind) {
        return Ok(kind.into_manager());
    }

    // Unknown families may still be installable when the host exposes a
    // supported package manager under a nonstandard distro identifier.
    if command_exists("dnf") || command_exists("yum") {
        Ok(Box::new(DnfBackend))
    } else if command_exists("apt-get") {
        Ok(Box::new(AptBackend))
    } else {
        Err(PkgError::Unsupported(
            pkg_base.unwrap_or("unknown").to_string(),
        ))
    }
}

fn package_manager_kind(pkg_base: &str) -> Option<PackageManagerKind> {
    if pkg_base == "rpm"
        || pkg_base.starts_with("anolis")
        || pkg_base.starts_with("alinux")
        || pkg_base.starts_with("rhel")
        || pkg_base.starts_with("centos")
        || pkg_base.starts_with("fedora")
    {
        Some(PackageManagerKind::Dnf)
    } else if pkg_base == "deb" || pkg_base.starts_with("ubuntu") || pkg_base.starts_with("debian")
    {
        Some(PackageManagerKind::Apt)
    } else {
        None
    }
}

fn command_exists(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::io::Read;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::*;

    struct Fixture(PathBuf);

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn unique_fixture(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("anolisa-pkg-{name}-{}", std::process::id()))
    }

    #[test]
    fn package_family_hints_select_expected_backends() {
        assert_eq!(package_manager_kind("rpm"), Some(PackageManagerKind::Dnf));
        assert_eq!(package_manager_kind("deb"), Some(PackageManagerKind::Apt));
    }

    #[test]
    fn distro_hints_keep_selecting_expected_backends() {
        for pkg_base in ["anolis23", "alinux4", "rhel9", "centos9", "fedora42"] {
            assert_eq!(
                package_manager_kind(pkg_base),
                Some(PackageManagerKind::Dnf)
            );
        }
        for pkg_base in ["ubuntu24", "debian12"] {
            assert_eq!(
                package_manager_kind(pkg_base),
                Some(PackageManagerKind::Apt)
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn package_command_writes_progress_to_supplied_fd() {
        let progress_fixture = Fixture(unique_fixture("progress"));
        let progress_file =
            File::create(&progress_fixture.0).expect("progress fixture must be created");
        let progress: OwnedFd = progress_file.into();
        let mut command = Command::new("sh");
        command.args(["-c", "printf progress-marker"]);

        assert!(
            run_with_progress_to(&mut command, progress)
                .expect("package command must complete")
                .success()
        );
        let mut marker = String::new();
        File::open(&progress_fixture.0)
            .expect("progress fixture must be readable")
            .read_to_string(&mut marker)
            .expect("progress fixture must contain UTF-8 text");

        assert_eq!(marker, "progress-marker");
    }

    #[cfg(unix)]
    #[test]
    fn package_command_does_not_wait_for_background_descendant_holding_progress_fd() {
        let progress = Fixture(unique_fixture("retained-fd"));
        let progress_file = File::create(&progress.0).expect("progress fixture must be created");
        let completion = Fixture(unique_fixture("descendant-complete"));
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "(sleep 1; printf complete > \"$1\") &",
            "sh",
            completion
                .0
                .to_str()
                .expect("completion fixture path must be UTF-8"),
        ]);
        let progress: OwnedFd = progress_file.into();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result = run_with_progress_to(&mut command, progress);
            let _ = sender.send(result);
        });
        let result = receiver.recv_timeout(Duration::from_millis(500));

        let status = result
            .expect("direct child completion must not depend on its descendant")
            .expect("package command must complete");
        assert!(status.success());
        assert!(!completion.0.exists());

        let deadline = Instant::now() + Duration::from_secs(3);
        while !completion.0.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_eq!(
            fs::read_to_string(&completion.0)
                .expect("descendant must complete within three seconds"),
            "complete"
        );
    }
}
