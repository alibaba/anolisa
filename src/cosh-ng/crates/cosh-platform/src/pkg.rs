//! Package management backend — routes operations to dnf/apt/zypper.

use std::collections::HashSet;
use std::process::Command;

use cosh_types::error::{CoshError, ErrorCode};
use cosh_types::pkg::*;

use crate::detect::{Distro, PkgManager};
use crate::{run_command, PKG_TIMEOUT};

/// Execute a package install operation on the detected distro.
pub fn pkg_install(
    distro: &Distro,
    package: &str,
    dry_run: bool,
) -> Result<PkgInstallResult, CoshError> {
    let mgr = distro.pkg_manager();
    let (cmd, args) = match mgr {
        PkgManager::Dnf => ("dnf", build_dnf_install_args(package, dry_run)),
        PkgManager::Apt => ("apt-get", build_apt_install_args(package, dry_run)),
        PkgManager::Zypper => ("zypper", build_zypper_install_args(package, dry_run)),
        PkgManager::Brew => ("brew", vec!["install", package]),
        PkgManager::Unknown => {
            return Err(CoshError::new(
                ErrorCode::UnsupportedDistro,
                format!("No package manager detected for {}", distro),
                "pkg",
            )
            .with_hint("Specify --pkg-backend to override detection"));
        }
    };

    if dry_run {
        return Ok(PkgInstallResult {
            package: package.to_string(),
            version: "(dry-run)".to_string(),
            already_installed: false,
            dependencies_installed: vec![],
        });
    }

    let output = run_command(Command::new(cmd).args(&args), PKG_TIMEOUT, "pkg")?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        // apt/dnf return exit 0 even when package is already installed
        let already = is_already_installed(&stdout);
        Ok(PkgInstallResult {
            package: package.to_string(),
            version: parse_installed_version(package, mgr),
            already_installed: already,
            dependencies_installed: vec![],
        })
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stderr.contains("already installed")
            || stderr.contains("is already the newest")
            || stdout.contains("already installed")
            || stdout.contains("is already the newest")
        {
            Ok(PkgInstallResult {
                package: package.to_string(),
                version: String::new(),
                already_installed: true,
                dependencies_installed: vec![],
            })
        } else {
            Err(CoshError::new(
                ErrorCode::PkgBackendError,
                format!("{} install failed: {}", cmd, stderr.trim()),
                "pkg",
            )
            .recoverable(true)
            .with_hint(format!(
                "Try 'cosh pkg search {}' to check availability",
                package
            )))
        }
    }
}

/// Execute a package search operation using the selected backend's pattern semantics.
///
/// The query is passed as one argument without shell expansion. CLI callers use
/// [`crate::validate::validate_pkg_search_query`] to enforce a portable pattern
/// subset; direct callers are responsible for choosing their validation policy.
/// DNF glob matching and apt-cache regular-expression matching are not equivalent.
pub fn pkg_search(distro: &Distro, query: &str) -> Result<PkgSearchResult, CoshError> {
    let mgr = distro.pkg_manager();
    let (cmd, args) = match mgr {
        PkgManager::Dnf => ("dnf", vec!["search", "-q", query]),
        PkgManager::Apt => ("apt-cache", vec!["search", query]),
        PkgManager::Zypper => ("zypper", vec!["search", query]),
        PkgManager::Brew => ("brew", vec!["search", query]),
        PkgManager::Unknown => {
            return Err(CoshError::new(
                ErrorCode::UnsupportedDistro,
                format!("No package manager detected for {}", distro),
                "pkg",
            ));
        }
    };

    let output = run_command(Command::new(cmd).args(&args), PKG_TIMEOUT, "pkg")?;
    check_search_status(
        cmd,
        output.status.success(),
        output.status.code(),
        &output.stderr,
    )?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut packages = parse_search_output(&stdout, mgr);

    // Zypper natively includes install status in search output; for other
    // backends, cross-reference against the local installed package set.
    if mgr != PkgManager::Zypper {
        let installed = get_installed_names(mgr);
        for pkg in &mut packages {
            pkg.installed = installed.contains(&pkg.name);
        }
    }

    let total = packages.len();
    Ok(PkgSearchResult { packages, total })
}

fn check_search_status(
    cmd: &str,
    success: bool,
    status_code: Option<i32>,
    stderr: &[u8],
) -> Result<(), CoshError> {
    if success {
        return Ok(());
    }

    let status = status_code.map_or_else(
        || "terminated without an exit code".to_string(),
        |code| format!("exit code {code}"),
    );
    let stderr = String::from_utf8_lossy(stderr);
    let detail = stderr.trim();
    let message = if detail.is_empty() {
        format!("{cmd} failed to process the search query ({status})")
    } else {
        format!("{cmd} failed to process the search query ({status}): {detail}")
    };

    Err(CoshError::new(ErrorCode::PkgBackendError, message, "pkg")
        .recoverable(true)
        .with_hint("Review the search query for the selected package manager's pattern syntax"))
}

/// List installed packages on the detected distro.
///
/// When `installed_only` is true, only installed packages are returned.
/// Currently only the installed-only mode is supported.
pub fn pkg_list(distro: &Distro, installed_only: bool) -> Result<PkgListResult, CoshError> {
    let _ = installed_only; // reserved for future "all available" mode
    let mgr = distro.pkg_manager();
    let (cmd, args): (&str, Vec<&str>) = match mgr {
        PkgManager::Dnf => ("dnf", vec!["list", "installed", "-q"]),
        PkgManager::Apt => (
            "dpkg-query",
            vec!["-W", "-f", "${Package}\t${Version}\t${db:Status-Abbrev}\n"],
        ),
        PkgManager::Zypper => ("zypper", vec!["se", "--installed-only", "-s"]),
        PkgManager::Brew => ("brew", vec!["list", "--versions"]),
        PkgManager::Unknown => {
            return Err(CoshError::new(
                ErrorCode::UnsupportedDistro,
                format!("No package manager detected for {}", distro),
                "pkg",
            ));
        }
    };

    let output = run_command(Command::new(cmd).args(&args), PKG_TIMEOUT, "pkg")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let packages = match mgr {
        PkgManager::Dnf => parse_dnf_list_output(&stdout),
        PkgManager::Apt => parse_apt_list_output(&stdout),
        PkgManager::Zypper => parse_zypper_list_output(&stdout),
        PkgManager::Brew => parse_brew_list_output(&stdout),
        PkgManager::Unknown => vec![],
    };
    let total = packages.len();

    Ok(PkgListResult { packages, total })
}

/// Execute a package remove operation.
pub fn pkg_remove(
    distro: &Distro,
    package: &str,
    dry_run: bool,
) -> Result<PkgRemoveResult, CoshError> {
    let mgr = distro.pkg_manager();
    let (cmd, args) = match mgr {
        PkgManager::Dnf => ("dnf", build_dnf_remove_args(package, dry_run)),
        PkgManager::Apt => ("apt-get", build_apt_remove_args(package, dry_run)),
        PkgManager::Zypper => ("zypper", build_zypper_remove_args(package, dry_run)),
        PkgManager::Brew => ("brew", vec!["uninstall", package]),
        PkgManager::Unknown => {
            return Err(CoshError::new(
                ErrorCode::UnsupportedDistro,
                format!("No package manager detected for {}", distro),
                "pkg",
            ));
        }
    };

    if dry_run {
        return Ok(PkgRemoveResult {
            package: package.to_string(),
            version_removed: "(dry-run)".to_string(),
            dependencies_removed: vec![],
        });
    }

    let output = run_command(Command::new(cmd).args(&args), PKG_TIMEOUT, "pkg")?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        // dnf returns exit 0 even when no packages matched for removal
        if is_remove_not_found(&stdout) {
            Err(CoshError::new(
                ErrorCode::PkgBackendError,
                format!("Package '{}' is not installed", package),
                "pkg",
            )
            .recoverable(true)
            .with_hint("Check installed packages with 'cosh pkg list --installed'"))
        } else {
            Ok(PkgRemoveResult {
                package: package.to_string(),
                version_removed: String::new(),
                dependencies_removed: vec![],
            })
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(CoshError::new(
            ErrorCode::PkgBackendError,
            format!("{} remove failed: {}", cmd, stderr.trim()),
            "pkg",
        ))
    }
}

// --- Detection helpers (extracted for testability) ---

/// Detect whether install output indicates the package was already installed.
fn is_already_installed(stdout: &str) -> bool {
    stdout.contains("is already the newest")
        || stdout.contains("already installed")
        || stdout.contains("Nothing to do")
}

/// Detect whether remove output indicates the package was not found.
fn is_remove_not_found(stdout: &str) -> bool {
    stdout.contains("No match for argument") || stdout.contains("No packages marked for removal")
}

// --- Argument builders ---

fn build_dnf_install_args(package: &str, dry_run: bool) -> Vec<&str> {
    if dry_run {
        vec!["install", "-y", "--assumeno", package]
    } else {
        vec!["install", "-y", package]
    }
}

fn build_apt_install_args(package: &str, dry_run: bool) -> Vec<&str> {
    if dry_run {
        vec!["install", "--dry-run", package]
    } else {
        vec!["install", "-y", package]
    }
}

fn build_zypper_install_args(package: &str, dry_run: bool) -> Vec<&str> {
    if dry_run {
        vec!["install", "--dry-run", package]
    } else {
        vec!["install", "-y", package]
    }
}

fn build_dnf_remove_args(package: &str, dry_run: bool) -> Vec<&str> {
    if dry_run {
        vec!["remove", "-y", "--assumeno", package]
    } else {
        vec!["remove", "-y", package]
    }
}

fn build_apt_remove_args(package: &str, dry_run: bool) -> Vec<&str> {
    if dry_run {
        vec!["remove", "--dry-run", package]
    } else {
        vec!["remove", "-y", package]
    }
}

fn build_zypper_remove_args(package: &str, dry_run: bool) -> Vec<&str> {
    if dry_run {
        vec!["remove", "--dry-run", package]
    } else {
        vec!["remove", "-y", package]
    }
}

// --- Installed package name lookup (for search cross-reference) ---

/// Query the set of installed package names from the local package database.
/// Returns an empty set on failure (graceful degradation).
fn get_installed_names(mgr: PkgManager) -> HashSet<String> {
    let result = match mgr {
        PkgManager::Dnf => run_command(
            Command::new("rpm").args(["-qa", "--qf", "%{NAME}\n"]),
            PKG_TIMEOUT,
            "pkg",
        ),
        PkgManager::Apt => run_command(
            Command::new("dpkg-query").args(["-W", "-f", "${Package}\n"]),
            PKG_TIMEOUT,
            "pkg",
        ),
        PkgManager::Brew => run_command(
            Command::new("brew").args(["list", "--formula", "-1"]),
            PKG_TIMEOUT,
            "pkg",
        ),
        PkgManager::Zypper | PkgManager::Unknown => return HashSet::new(),
    };

    match result {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            parse_installed_names(&stdout)
        }
        _ => HashSet::new(),
    }
}

/// Parse one-package-per-line output into a name set.
fn parse_installed_names(output: &str) -> HashSet<String> {
    output
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect()
}

// --- Output parsers (minimal, to be extended) ---

/// Query the installed version of a package after a successful install.
/// Falls back to parsing install output if the query fails.
fn parse_installed_version(package: &str, mgr: PkgManager) -> String {
    let result = match mgr {
        PkgManager::Dnf => run_command(
            Command::new("rpm").args(["-q", "--qf", "%{VERSION}-%{RELEASE}", package]),
            PKG_TIMEOUT,
            "pkg",
        ),
        PkgManager::Apt => run_command(
            Command::new("dpkg-query").args(["-W", "-f", "${Version}", package]),
            PKG_TIMEOUT,
            "pkg",
        ),
        PkgManager::Zypper => run_command(
            Command::new("rpm").args(["-q", "--qf", "%{VERSION}-%{RELEASE}", package]),
            PKG_TIMEOUT,
            "pkg",
        ),
        PkgManager::Brew => run_command(
            Command::new("brew").args(["list", "--versions", package]),
            PKG_TIMEOUT,
            "pkg",
        ),
        PkgManager::Unknown => return String::new(),
    };
    match result {
        Ok(o) if o.status.success() => {
            let ver = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if mgr == PkgManager::Brew {
                // brew list --versions output: "name ver1 ver2", take the first version
                ver.split_whitespace().nth(1).unwrap_or("").to_string()
            } else {
                ver
            }
        }
        _ => String::new(),
    }
}

fn parse_search_output(stdout: &str, mgr: PkgManager) -> Vec<PkgSearchEntry> {
    let mut results = Vec::new();

    match mgr {
        PkgManager::Dnf => {
            // dnf search output: "name.arch : summary"
            for line in stdout.lines() {
                if let Some((name_part, summary)) = line.split_once(" : ") {
                    let name = name_part.split('.').next().unwrap_or(name_part).trim();
                    results.push(PkgSearchEntry {
                        name: name.to_string(),
                        version: String::new(),
                        summary: summary.trim().to_string(),
                        installed: false,
                    });
                }
            }
        }
        PkgManager::Apt => {
            // apt-cache search output: "name - description"
            for line in stdout.lines() {
                if let Some((name, desc)) = line.split_once(" - ") {
                    results.push(PkgSearchEntry {
                        name: name.trim().to_string(),
                        version: String::new(),
                        summary: desc.trim().to_string(),
                        installed: false,
                    });
                }
            }
        }
        PkgManager::Zypper => {
            // zypper search output is tabular, skip header
            for line in stdout.lines().skip(2) {
                let parts: Vec<&str> = line.split('|').collect();
                if parts.len() >= 3 {
                    results.push(PkgSearchEntry {
                        name: parts[1].trim().to_string(),
                        version: if parts.len() > 3 {
                            parts[3].trim().to_string()
                        } else {
                            String::new()
                        },
                        summary: if parts.len() > 2 {
                            parts[2].trim().to_string()
                        } else {
                            String::new()
                        },
                        installed: parts[0].trim() == "i",
                    });
                }
            }
        }
        PkgManager::Brew => {
            // brew search output: one package name per line
            for line in stdout.lines() {
                let name = line.trim();
                if !name.is_empty() && !name.starts_with("==>") {
                    results.push(PkgSearchEntry {
                        name: name.to_string(),
                        version: String::new(),
                        summary: String::new(),
                        installed: false,
                    });
                }
            }
        }
        PkgManager::Unknown => {}
    }

    results
}

/// Parse `dnf list installed -q` output.
///
/// Each line has the format: `package-name.arch  version  repo`
/// Skip header lines like "Installed Packages" or "Available Packages".
fn parse_dnf_list_output(output: &str) -> Vec<PkgListEntry> {
    let mut results = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Skip dnf section headers (e.g. "Installed Packages", "Available Packages")
        if line.ends_with("Packages") || line.ends_with("packages") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            // Validate that the first field contains a dot (name.arch format)
            if !parts[0].contains('.') {
                continue;
            }
            let (name, arch) = match parts[0].rsplit_once('.') {
                Some((n, a)) => (n.to_string(), Some(a.to_string())),
                None => (parts[0].to_string(), None),
            };
            let version = parts[1].to_string();
            let repo = parts.get(2).map(|s| s.to_string());
            results.push(PkgListEntry {
                name,
                version,
                arch,
                repo,
            });
        }
    }
    results
}

/// Parse `dpkg-query -W -f '${Package}\t${Version}\t${db:Status-Abbrev}\n'` output.
///
/// Only lines where the status field starts with "ii" are included.
fn parse_apt_list_output(output: &str) -> Vec<PkgListEntry> {
    let mut results = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 3 {
            let status = parts[2].trim();
            if !status.starts_with("ii") {
                continue;
            }
            results.push(PkgListEntry {
                name: parts[0].to_string(),
                version: parts[1].to_string(),
                arch: None,
                repo: None,
            });
        } else if parts.len() == 2 {
            // Fallback: some dpkg-query outputs may omit status
            results.push(PkgListEntry {
                name: parts[0].to_string(),
                version: parts[1].to_string(),
                arch: None,
                repo: None,
            });
        }
    }
    results
}

/// Parse `zypper se --installed-only -s` tabular output.
///
/// Skips the first 2 header lines, then parses pipe-separated columns:
/// `status | name | type | version | arch | repo`
fn parse_zypper_list_output(output: &str) -> Vec<PkgListEntry> {
    let mut results = Vec::new();
    for line in output.lines().skip(2) {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() >= 4 {
            let name = parts[1].trim().to_string();
            let version = parts[3].trim().to_string();
            let arch = if parts.len() > 4 {
                let a = parts[4].trim();
                if a.is_empty() {
                    None
                } else {
                    Some(a.to_string())
                }
            } else {
                None
            };
            let repo = if parts.len() > 5 {
                let r = parts[5].trim();
                if r.is_empty() {
                    None
                } else {
                    Some(r.to_string())
                }
            } else {
                None
            };
            if !name.is_empty() {
                results.push(PkgListEntry {
                    name,
                    version,
                    arch,
                    repo,
                });
            }
        }
    }
    results
}

/// Parse `brew list --versions` output.
///
/// Each line has the format: `package-name version1 [version2 ...]`
fn parse_brew_list_output(output: &str) -> Vec<PkgListEntry> {
    let mut results = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, ' ');
        let name = match parts.next() {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => continue,
        };
        let version = parts.next().unwrap_or("").trim().to_string();
        results.push(PkgListEntry {
            name,
            version,
            arch: None,
            repo: None,
        });
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::{Distro, PkgManager};

    // --- parse_installed_version tests ---

    #[test]
    fn test_parse_installed_version_bash() {
        // bash is installed on virtually every system
        let distro = Distro::detect();
        let mgr = distro.pkg_manager();
        if mgr == PkgManager::Unknown {
            return; // skip on unsupported platforms
        }
        let version = parse_installed_version("bash", mgr);
        assert!(!version.is_empty(), "Expected non-empty version for bash");
    }

    #[test]
    fn test_parse_installed_version_nonexistent() {
        let version = parse_installed_version("this-package-does-not-exist-xyz", PkgManager::Dnf);
        assert!(version.is_empty());
    }

    #[test]
    fn test_parse_installed_version_unknown_mgr() {
        let version = parse_installed_version("bash", PkgManager::Unknown);
        assert!(version.is_empty());
    }

    // --- dnf search output parsing ---

    #[test]
    fn test_parse_search_dnf() {
        let output = "nginx.x86_64 : A high performance web server\nnginx-filesystem.noarch : The basic directory layout for nginx";
        let results = parse_search_output(output, PkgManager::Dnf);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "nginx");
        assert_eq!(results[0].summary, "A high performance web server");
        assert_eq!(results[1].name, "nginx-filesystem");
    }

    #[test]
    fn test_parse_search_dnf_empty() {
        let output = "";
        let results = parse_search_output(output, PkgManager::Dnf);
        assert!(results.is_empty());
    }

    // --- apt search output parsing ---

    #[test]
    fn test_parse_search_apt() {
        let output = "nginx - small, powerful, scalable web/proxy server\nnginx-common - small, powerful, scalable web/proxy server - common files";
        let results = parse_search_output(output, PkgManager::Apt);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "nginx");
        assert_eq!(
            results[0].summary,
            "small, powerful, scalable web/proxy server"
        );
        assert_eq!(results[1].name, "nginx-common");
    }

    #[test]
    fn test_parse_search_apt_empty() {
        let output = "";
        let results = parse_search_output(output, PkgManager::Apt);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_search_apt_line_without_separator() {
        let output = "some random line without separator";
        let results = parse_search_output(output, PkgManager::Apt);
        assert!(results.is_empty());
    }

    // --- zypper search output parsing ---

    #[test]
    fn test_parse_search_zypper() {
        let output = "S | Name            | Summary                    | Type\n--+-----------------+-----------------------------+-------\ni | nginx           | A high performance web serv | package\n  | nginx-common    | Common files for nginx      | package";
        let results = parse_search_output(output, PkgManager::Zypper);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "nginx");
        assert!(results[0].installed); // 'i' marker
        assert_eq!(results[1].name, "nginx-common");
        assert!(!results[1].installed); // empty marker
    }

    #[test]
    fn test_parse_search_zypper_empty() {
        let output = "S | Name | Summary | Type\n--+------+------+------";
        let results = parse_search_output(output, PkgManager::Zypper);
        assert!(results.is_empty());
    }

    // --- Unknown package manager ---

    #[test]
    fn test_parse_search_unknown() {
        let output = "something";
        let results = parse_search_output(output, PkgManager::Unknown);
        assert!(results.is_empty());
    }

    // --- pkg_install with unsupported distro ---

    #[test]
    fn test_pkg_install_unsupported_distro() {
        let distro = Distro::Unknown("foobar".into());
        let result = pkg_install(&distro, "nginx", false);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsupportedDistro);
    }

    // --- pkg_search with unsupported distro ---

    #[test]
    fn test_pkg_search_unsupported_distro() {
        let distro = Distro::Unknown("foobar".into());
        let result = pkg_search(&distro, "nginx");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsupportedDistro);
    }

    #[test]
    fn test_pkg_search_backend_failure_is_propagated() {
        let err = check_search_status(
            "apt-cache",
            false,
            Some(100),
            b"Regex compilation error - Invalid regular expression",
        )
        .unwrap_err();

        assert_eq!(err.code, ErrorCode::PkgBackendError);
        assert!(err.recoverable);
        assert!(err.message.contains("apt-cache"));
        assert!(err.message.contains("search query"));
        assert!(err.message.contains("exit code 100"));
        assert!(err.message.contains("Regex compilation error"));
        assert!(err
            .hint
            .as_deref()
            .is_some_and(|hint| hint.contains("search query")));
    }

    #[test]
    fn test_pkg_search_backend_success_ignores_stderr() {
        assert!(check_search_status("dnf", true, Some(0), b"warning").is_ok());
    }

    // --- pkg_remove with unsupported distro ---

    #[test]
    fn test_pkg_remove_unsupported_distro() {
        let distro = Distro::Unknown("foobar".into());
        let result = pkg_remove(&distro, "nginx", false);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsupportedDistro);
    }

    // --- dry-run returns immediately ---

    #[test]
    fn test_pkg_install_dry_run() {
        let distro = Distro::Alinux {
            version: "3".into(),
        };
        let result = pkg_install(&distro, "nginx", true).unwrap();
        assert_eq!(result.package, "nginx");
        assert_eq!(result.version, "(dry-run)");
    }

    #[test]
    fn test_pkg_remove_dry_run() {
        let distro = Distro::Ubuntu {
            version: "22.04".into(),
        };
        let result = pkg_remove(&distro, "nginx", true).unwrap();
        assert_eq!(result.package, "nginx");
        assert_eq!(result.version_removed, "(dry-run)");
    }

    // --- argument builders ---

    #[test]
    fn test_build_dnf_install_args() {
        let args = build_dnf_install_args("nginx", false);
        assert_eq!(args, vec!["install", "-y", "nginx"]);
    }

    #[test]
    fn test_build_dnf_install_args_dry_run() {
        let args = build_dnf_install_args("nginx", true);
        assert_eq!(args, vec!["install", "-y", "--assumeno", "nginx"]);
    }

    #[test]
    fn test_build_apt_install_args() {
        let args = build_apt_install_args("nginx", false);
        assert_eq!(args, vec!["install", "-y", "nginx"]);
    }

    #[test]
    fn test_build_apt_install_args_dry_run() {
        let args = build_apt_install_args("nginx", true);
        assert_eq!(args, vec!["install", "--dry-run", "nginx"]);
    }

    #[test]
    fn test_build_zypper_install_args() {
        let args = build_zypper_install_args("nginx", false);
        assert_eq!(args, vec!["install", "-y", "nginx"]);
    }

    #[test]
    fn test_build_zypper_install_args_dry_run() {
        let args = build_zypper_install_args("nginx", true);
        assert_eq!(args, vec!["install", "--dry-run", "nginx"]);
    }

    #[test]
    fn test_build_dnf_remove_args() {
        let args = build_dnf_remove_args("nginx", false);
        assert_eq!(args, vec!["remove", "-y", "nginx"]);
    }

    #[test]
    fn test_build_apt_remove_args() {
        let args = build_apt_remove_args("nginx", false);
        assert_eq!(args, vec!["remove", "-y", "nginx"]);
    }

    #[test]
    fn test_build_zypper_remove_args() {
        let args = build_zypper_remove_args("nginx", false);
        assert_eq!(args, vec!["remove", "-y", "nginx"]);
    }

    // --- brew search output parsing ---

    #[test]
    fn test_parse_search_brew() {
        let output = "==> Formulae\nnginx\nnginx-full\n==> Casks\nnginxconfig";
        let results = parse_search_output(output, PkgManager::Brew);
        // Should skip "==> ..." header lines
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].name, "nginx");
        assert_eq!(results[1].name, "nginx-full");
        assert_eq!(results[2].name, "nginxconfig");
    }

    #[test]
    fn test_parse_search_brew_empty() {
        let output = "";
        let results = parse_search_output(output, PkgManager::Brew);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_search_brew_only_headers() {
        let output = "==> Formulae\n==> Casks";
        let results = parse_search_output(output, PkgManager::Brew);
        assert!(results.is_empty());
    }

    // --- brew dry-run tests ---

    #[test]
    fn test_pkg_install_dry_run_brew() {
        let distro = Distro::MacOS {
            version: "15.4".into(),
        };
        let result = pkg_install(&distro, "wget", true).unwrap();
        assert_eq!(result.package, "wget");
        assert_eq!(result.version, "(dry-run)");
    }

    #[test]
    fn test_pkg_remove_dry_run_brew() {
        let distro = Distro::MacOS {
            version: "15.4".into(),
        };
        let result = pkg_remove(&distro, "wget", true).unwrap();
        assert_eq!(result.package, "wget");
        assert_eq!(result.version_removed, "(dry-run)");
    }

    // --- pkg_list parse tests ---

    #[test]
    fn test_parse_dnf_list_output() {
        let output = "nginx.x86_64                      1.24.0-1.fc39          @fedora\nbash.x86_64                       5.2.15-3.fc39          @anaconda";
        let results = parse_dnf_list_output(output);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "nginx");
        assert_eq!(results[0].version, "1.24.0-1.fc39");
        assert_eq!(results[0].arch, Some("x86_64".to_string()));
        assert_eq!(results[0].repo, Some("@fedora".to_string()));
        assert_eq!(results[1].name, "bash");
        assert_eq!(results[1].version, "5.2.15-3.fc39");
        assert_eq!(results[1].arch, Some("x86_64".to_string()));
        assert_eq!(results[1].repo, Some("@anaconda".to_string()));
    }

    #[test]
    fn test_parse_dnf_list_output_with_header() {
        // dnf list installed may output a header even with -q in some versions
        let output = "Installed Packages\nnginx.x86_64                      1.24.0-1.fc39          @fedora\nbash.x86_64                       5.2.15-3.fc39          @anaconda";
        let results = parse_dnf_list_output(output);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "nginx");
        assert_eq!(results[1].name, "bash");
    }

    #[test]
    fn test_parse_apt_list_output() {
        let output = "bash\t5.2-2ubuntu2\tii \nnginx\t1.24.0-1\tii \ncurl\t8.1.2-1\trc ";
        let results = parse_apt_list_output(output);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "bash");
        assert_eq!(results[0].version, "5.2-2ubuntu2");
        assert_eq!(results[1].name, "nginx");
        assert_eq!(results[1].version, "1.24.0-1");
    }

    #[test]
    fn test_parse_zypper_list_output() {
        let output = "S | Name         | Type    | Version       | Arch   | Repository\n--+--------------+---------+---------------+--------+-----------\ni | bash         | package | 5.2-1.1       | x86_64 | repo-oss\ni | nginx        | package | 1.24.0-1.1    | x86_64 | repo-oss";
        let results = parse_zypper_list_output(output);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "bash");
        assert_eq!(results[0].version, "5.2-1.1");
        assert_eq!(results[0].arch, Some("x86_64".to_string()));
        assert_eq!(results[0].repo, Some("repo-oss".to_string()));
        assert_eq!(results[1].name, "nginx");
        assert_eq!(results[1].version, "1.24.0-1.1");
    }

    #[test]
    fn test_parse_brew_list_output() {
        let output = "nginx 1.25.4\nwget 1.21.4\ncurl 8.6.0 8.5.0";
        let results = parse_brew_list_output(output);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].name, "nginx");
        assert_eq!(results[0].version, "1.25.4");
        assert_eq!(results[1].name, "wget");
        assert_eq!(results[1].version, "1.21.4");
        assert_eq!(results[2].name, "curl");
        assert_eq!(results[2].version, "8.6.0 8.5.0");
    }

    #[test]
    fn test_parse_dnf_list_empty() {
        let output = "";
        let results = parse_dnf_list_output(output);
        assert!(results.is_empty());
    }

    #[test]
    fn test_pkg_list_unsupported() {
        let distro = Distro::Unknown("foobar".into());
        let result = pkg_list(&distro, true);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsupportedDistro);
    }

    // --- is_already_installed / is_remove_not_found detection tests ---

    #[test]
    fn test_detect_already_installed_apt() {
        let stdout = "Reading package lists... Done\nBuilding dependency tree... Done\nhtop is already the newest version (3.2.2-2).\n0 upgraded, 0 newly installed, 0 to remove and 0 not upgraded.";
        assert!(is_already_installed(stdout));
    }

    #[test]
    fn test_detect_already_installed_dnf() {
        let stdout = "Last metadata expiration check: 0:30:00 ago.\nPackage vim-minimal-3:9.0.2092-13.alnx4.x86_64 is already installed.\nNothing to do.\nComplete!";
        assert!(is_already_installed(stdout));
    }

    #[test]
    fn test_detect_remove_not_found_dnf_no_match() {
        let stdout = "No match for argument: nonexist-pkg\nNo packages marked for removal.\nDependencies resolved.\nNothing to do.\nComplete!";
        assert!(is_remove_not_found(stdout));
    }

    #[test]
    fn test_detect_remove_not_found_dnf_no_packages_marked() {
        let stdout = "No packages marked for removal.\nDependencies resolved.\nNothing to do.";
        assert!(is_remove_not_found(stdout));
    }

    // --- parse_installed_names tests ---

    #[test]
    fn test_parse_installed_names_rpm_output() {
        let output = "bash\ncoreutils\nnginx\ncurl\n";
        let names = parse_installed_names(output);
        assert_eq!(names.len(), 4);
        assert!(names.contains("bash"));
        assert!(names.contains("nginx"));
        assert!(!names.contains("wget"));
    }

    #[test]
    fn test_parse_installed_names_dpkg_output() {
        let output = "bash\napt\ndpkg\nlibssl3\n";
        let names = parse_installed_names(output);
        assert_eq!(names.len(), 4);
        assert!(names.contains("bash"));
        assert!(names.contains("dpkg"));
    }

    #[test]
    fn test_parse_installed_names_brew_output() {
        let output = "nginx\nwget\ncurl\n";
        let names = parse_installed_names(output);
        assert_eq!(names.len(), 3);
        assert!(names.contains("nginx"));
        assert!(names.contains("wget"));
    }

    #[test]
    fn test_parse_installed_names_empty() {
        let names = parse_installed_names("");
        assert!(names.is_empty());
    }

    #[test]
    fn test_parse_installed_names_with_blank_lines() {
        let output = "bash\n\n  \nnginx\n";
        let names = parse_installed_names(output);
        assert_eq!(names.len(), 2);
        assert!(names.contains("bash"));
        assert!(names.contains("nginx"));
    }

    #[test]
    fn test_search_marks_installed_dnf() {
        let search_output = "bash.x86_64 : The GNU Bourne Again shell\nnginx.x86_64 : A high performance web server\n";
        let mut packages = parse_search_output(search_output, PkgManager::Dnf);

        let mut installed_set = HashSet::new();
        installed_set.insert("bash".to_string());

        for pkg in &mut packages {
            pkg.installed = installed_set.contains(&pkg.name);
        }

        assert_eq!(packages.len(), 2);
        assert!(packages[0].installed); // bash is installed
        assert!(!packages[1].installed); // nginx is not
    }

    #[test]
    fn test_search_marks_installed_apt() {
        let search_output =
            "bash - The GNU Bourne Again shell\nnginx - A high performance web server\n";
        let mut packages = parse_search_output(search_output, PkgManager::Apt);

        let mut installed_set = HashSet::new();
        installed_set.insert("bash".to_string());
        installed_set.insert("nginx".to_string());

        for pkg in &mut packages {
            pkg.installed = installed_set.contains(&pkg.name);
        }

        assert_eq!(packages.len(), 2);
        assert!(packages[0].installed);
        assert!(packages[1].installed);
    }
}
