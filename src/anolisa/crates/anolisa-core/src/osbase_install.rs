//! Generic osbase install entry layer — TOML-manifest-driven.
//!
//! The install pipeline reads scenario definitions from `sandbox.toml`
//! (deployed by `anolisa system setup` to `/etc/anolisa/sandbox.toml`)
//! and executes a simplified 3-step flow:
//!
//!   1. Preflight — kernel version gate, KVM check if required
//!   2. Packages  — `dnf install -y <packages>` from manifest
//!   3. Hint      — print optional packages if any
//!
//! The old 5-phase pipeline in `sandbox_install.rs` is no longer invoked
//! from this path.  `Kernel` and `Security` domains remain stubs.

use std::process::Command;

use anolisa_env::EnvFacts;

use crate::sandbox_manifest::{ManifestError, SandboxManifest, ScenarioConfig};

// ===========================================================================
// Public types
// ===========================================================================

/// The three osbase domains. Each domain owns a distinct install pipeline;
/// dispatch happens in [`execute_install`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsbaseDomain {
    /// Linux kernel variants (e.g. `agentic`, `vanilla`).
    Kernel,
    /// Sandbox engines (runc / rund / firecracker / gvisor / landlock).
    Sandbox,
    /// Security primitives (LSMs, audit, seccomp profiles).
    Security,
}

impl OsbaseDomain {
    /// Stable lower-case identifier used in logs and error strings.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kernel => "kernel",
            Self::Sandbox => "sandbox",
            Self::Security => "security",
        }
    }
}

/// Whether to register the engine into a containerd handler entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RegisterHandler {
    /// Register with containerd via the appropriate shim.
    #[default]
    Containerd,
    /// Standalone install — no L2 runtime wiring.
    None,
}

/// Generic install request for any osbase domain.
#[derive(Debug, Clone)]
pub struct OsbaseInstallRequest {
    /// Which domain pipeline to dispatch to.
    pub domain: OsbaseDomain,
    /// Scenario name (Sandbox) or variant (Kernel/Security). Must be
    /// non-empty; matched against the manifest.
    pub target: String,
    /// L2 handler registration mode.
    pub register_handler: RegisterHandler,
    /// Additionally create a Kubernetes `RuntimeClass` after handler
    /// registration.
    pub register_runtimeclass: bool,
    /// Optional `--config` override path.
    pub config_override: Option<String>,
    /// Mark the installed engine as the default runtime for its handler.
    pub set_default: bool,
    /// Bypass non-fatal pre-flight gates.
    pub force: bool,
    /// Skip the post-install verify phase.
    pub skip_verify: bool,
    /// Produce a plan without side effects.
    pub dry_run: bool,
}

/// Aggregate outcome of a generic install.
#[derive(Debug, Clone)]
pub struct OsbaseInstallOutcome {
    pub domain: OsbaseDomain,
    pub target: String,
    pub phases: Vec<PhaseResult>,
    /// `0` success, `1` failed, `2` degraded.
    pub exit_code: i32,
    pub warnings: Vec<String>,
}

/// Per-phase result.
#[derive(Debug, Clone)]
pub struct PhaseResult {
    pub name: String,
    pub status: PhaseStatus,
    pub message: Option<String>,
    pub duration_ms: Option<u64>,
}

/// Status of a single phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseStatus {
    Success,
    Skipped,
    Degraded,
    Failed,
}

/// Errors surfaced by the generic install entry.
#[derive(Debug, thiserror::Error)]
pub enum OsbaseInstallError {
    #[error("unsupported: {0}")]
    Unsupported(String),

    #[error("invalid request: {reason}")]
    InvalidRequest { reason: String },

    #[error("phase '{phase}' failed: {message}")]
    PhaseFailed { phase: String, message: String },

    #[error("manifest error: {0}")]
    Manifest(#[from] ManifestError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

// ===========================================================================
// Entry point
// ===========================================================================

/// Validate the request and dispatch to the appropriate domain pipeline.
pub fn execute_install(
    request: &OsbaseInstallRequest,
    env: &EnvFacts,
) -> Result<OsbaseInstallOutcome, OsbaseInstallError> {
    validate_request(request, env)?;

    match request.domain {
        OsbaseDomain::Sandbox => sandbox_dispatch(request, env),
        OsbaseDomain::Kernel => Err(OsbaseInstallError::InvalidRequest {
            reason: "kernel install not yet implemented".to_string(),
        }),
        OsbaseDomain::Security => Err(OsbaseInstallError::InvalidRequest {
            reason: "security install not yet implemented".to_string(),
        }),
    }
}

/// List all available scenarios from the manifest.
pub fn list_scenarios() -> Result<Vec<String>, OsbaseInstallError> {
    let manifest = SandboxManifest::load()?;
    Ok(manifest
        .scenario_names()
        .into_iter()
        .map(String::from)
        .collect())
}

/// Uninstall packages for a given scenario via `dnf remove -y`.
///
/// - If the scenario is not found in the manifest → error
/// - If the scenario has no packages (e.g. landlock) → "nothing to uninstall"
/// - Otherwise → `dnf remove -y <packages>`
pub fn execute_uninstall(scenario: &str, dry_run: bool) -> Result<String, OsbaseInstallError> {
    let manifest = SandboxManifest::load()?;

    let config = manifest.find_scenario(scenario).ok_or_else(|| {
        let available = manifest.scenario_names().join(", ");
        OsbaseInstallError::InvalidRequest {
            reason: format!("unknown sandbox scenario '{scenario}'; available: [{available}]"),
        }
    })?;

    eprintln!("[osbase] scenario: {scenario}");

    if config.packages.is_empty() {
        return Ok(format!(
            "scenario '{scenario}': nothing to uninstall (no packages defined)"
        ));
    }

    let pkg_list = config.packages.join(" ");

    if dry_run {
        eprintln!("[osbase] [dry-run] would remove packages: {pkg_list}");
        eprintln!("[osbase] [dry-run] no packages will be removed in dry-run mode");
        return Ok(format!("dry-run: would uninstall: {pkg_list}"));
    }

    eprintln!("[osbase] removing packages: {pkg_list}");

    match run_dnf_remove(&config.packages) {
        Ok(msg) => {
            eprintln!("[osbase] dnf remove completed (exit_code=0)");
            eprintln!("[osbase] removed successfully");
            Ok(msg)
        }
        Err(msg) => {
            eprintln!("[osbase] dnf remove failed");
            Err(OsbaseInstallError::PhaseFailed {
                phase: "uninstall".to_string(),
                message: msg,
            })
        }
    }
}

/// Execute `dnf remove -y -q <packages>`.
fn run_dnf_remove(packages: &[String]) -> Result<String, String> {
    let mut cmd = Command::new("dnf");
    cmd.arg("remove").arg("-y").arg("-q");
    for pkg in packages {
        cmd.arg(pkg);
    }

    let output = cmd
        .output()
        .map_err(|e| format!("failed to execute dnf: {e}"))?;

    if output.status.success() {
        Ok(format!("uninstalled: {}", packages.join(" ")))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = format!("{stdout}\n{stderr}");
        // "No match" or already not installed is not a real failure
        if combined.contains("No packages marked for removal")
            || combined.contains("No match for argument")
        {
            Ok(format!("packages already absent: {}", packages.join(" ")))
        } else {
            // Print stderr on failure for diagnostics
            let stderr_str = stderr.trim();
            if !stderr_str.is_empty() {
                eprintln!("[osbase] dnf stderr:\n{stderr_str}");
            }
            Err(format!(
                "dnf remove failed (exit={}): {}",
                output.status.code().unwrap_or(-1),
                stderr.lines().take(5).collect::<Vec<_>>().join("\n")
            ))
        }
    }
}

/// Lightweight request validation.
pub fn validate_request(
    request: &OsbaseInstallRequest,
    env: &EnvFacts,
) -> Result<(), OsbaseInstallError> {
    if request.target.trim().is_empty() {
        return Err(OsbaseInstallError::InvalidRequest {
            reason: "target must not be empty".to_string(),
        });
    }

    if request.register_runtimeclass && request.register_handler == RegisterHandler::None {
        return Err(OsbaseInstallError::InvalidRequest {
            reason: "--register-runtimeclass requires a non-None --register-handler".to_string(),
        });
    }

    if env.uid != 0 {
        return Err(OsbaseInstallError::InvalidRequest {
            reason: "osbase requires root (uid=0); re-run with sudo".to_string(),
        });
    }

    Ok(())
}

// ===========================================================================
// Sandbox dispatch — manifest-driven
// ===========================================================================

/// Load the manifest, find the scenario, and run the simplified install.
fn sandbox_dispatch(
    request: &OsbaseInstallRequest,
    env: &EnvFacts,
) -> Result<OsbaseInstallOutcome, OsbaseInstallError> {
    let manifest = SandboxManifest::load()?;

    let scenario = manifest.find_scenario(&request.target).ok_or_else(|| {
        let available = manifest.scenario_names().join(", ");
        OsbaseInstallError::InvalidRequest {
            reason: format!(
                "unknown sandbox scenario '{}'; available: [{}]",
                request.target, available
            ),
        }
    })?;

    // Clone what we need before running phases (avoid borrow issues)
    let scenario = scenario.clone();

    if request.dry_run {
        eprintln!("[osbase] scenario: {}", scenario.name);
        if !scenario.packages.is_empty() {
            let pkg_list = scenario.packages.join(" ");
            eprintln!("[osbase] [dry-run] would install packages: {pkg_list}");
        }
        eprintln!(
            "[osbase] [dry-run] preflight: kernel {} \u{2713}",
            scenario.requires_kernel
        );
        eprintln!("[osbase] [dry-run] no packages will be installed in dry-run mode");
        return Ok(build_dry_run_outcome(request, &scenario));
    }

    run_manifest_install(request, env, &scenario)
}

/// Build a dry-run outcome showing what would happen.
fn build_dry_run_outcome(
    request: &OsbaseInstallRequest,
    scenario: &ScenarioConfig,
) -> OsbaseInstallOutcome {
    let mut phases = Vec::new();

    // Preflight
    let mut preflight_msg = format!("check kernel {}", scenario.requires_kernel);
    if scenario.requires_kvm {
        preflight_msg.push_str("; check /dev/kvm");
    }
    phases.push(PhaseResult {
        name: "preflight".to_string(),
        status: PhaseStatus::Skipped,
        message: Some(preflight_msg),
        duration_ms: None,
    });

    // Packages
    let pkg_msg = if scenario.packages.is_empty() {
        "no packages to install".to_string()
    } else {
        format!("dnf install -y {}", scenario.packages.join(" "))
    };
    phases.push(PhaseResult {
        name: "packages".to_string(),
        status: PhaseStatus::Skipped,
        message: Some(pkg_msg),
        duration_ms: None,
    });

    // Optional hint
    if !scenario.packages_optional.is_empty() {
        phases.push(PhaseResult {
            name: "optional_hint".to_string(),
            status: PhaseStatus::Skipped,
            message: Some(format!(
                "optional: {}",
                scenario.packages_optional.join(" ")
            )),
            duration_ms: None,
        });
    }

    OsbaseInstallOutcome {
        domain: request.domain,
        target: request.target.clone(),
        phases,
        exit_code: 0,
        warnings: vec!["dry-run mode: no changes made".to_string()],
    }
}

/// Execute the simplified manifest-driven install:
/// 1. Preflight (kernel + KVM)
/// 2. dnf install packages
/// 3. Optional packages hint
fn run_manifest_install(
    request: &OsbaseInstallRequest,
    env: &EnvFacts,
    scenario: &ScenarioConfig,
) -> Result<OsbaseInstallOutcome, OsbaseInstallError> {
    let mut phases = Vec::new();
    let mut warnings = Vec::new();

    eprintln!("[osbase] scenario: {}", scenario.name);

    // ─── Phase 1: Preflight ──────────────────────────────────────────────
    let preflight_result = run_preflight(env, scenario, request.force);
    match preflight_result {
        Ok(msg) => {
            phases.push(PhaseResult {
                name: "preflight".to_string(),
                status: PhaseStatus::Success,
                message: Some(msg),
                duration_ms: None,
            });
        }
        Err(reason) => {
            phases.push(PhaseResult {
                name: "preflight".to_string(),
                status: PhaseStatus::Failed,
                message: Some(reason.clone()),
                duration_ms: None,
            });
            eprintln!("[osbase] error: {reason}");
            return Err(OsbaseInstallError::PhaseFailed {
                phase: "preflight".to_string(),
                message: reason,
            });
        }
    }

    // ─── Phase 2: Packages ───────────────────────────────────────────────
    if scenario.packages.is_empty() {
        phases.push(PhaseResult {
            name: "packages".to_string(),
            status: PhaseStatus::Skipped,
            message: Some("no packages required for this scenario".to_string()),
            duration_ms: None,
        });
    } else {
        let pkg_list = scenario.packages.join(" ");
        eprintln!("[osbase] installing packages: {pkg_list}");
        match run_dnf_install(&scenario.packages) {
            Ok(msg) => {
                eprintln!("[osbase] dnf install completed (exit_code=0)");
                phases.push(PhaseResult {
                    name: "packages".to_string(),
                    status: PhaseStatus::Success,
                    message: Some(msg),
                    duration_ms: None,
                });
            }
            Err(reason) => {
                eprintln!("[osbase] dnf install failed");
                phases.push(PhaseResult {
                    name: "packages".to_string(),
                    status: PhaseStatus::Failed,
                    message: Some(reason.clone()),
                    duration_ms: None,
                });
                return Err(OsbaseInstallError::PhaseFailed {
                    phase: "packages".to_string(),
                    message: reason,
                });
            }
        }
    }

    eprintln!("[osbase] installed successfully");

    // ─── Phase 3: Optional packages hint ─────────────────────────────────
    if !scenario.packages_optional.is_empty() {
        let hint = format!(
            "optional packages available: {}",
            scenario.packages_optional.join(" ")
        );
        eprintln!("[osbase] {hint}");
        warnings.push(hint.clone());
        phases.push(PhaseResult {
            name: "optional_hint".to_string(),
            status: PhaseStatus::Success,
            message: Some(hint),
            duration_ms: None,
        });
    } else {
        eprintln!("[osbase] optional packages available: (none)");
    }

    Ok(OsbaseInstallOutcome {
        domain: request.domain,
        target: request.target.clone(),
        phases,
        exit_code: 0,
        warnings,
    })
}

// ===========================================================================
// Phase implementations
// ===========================================================================

/// Preflight: check kernel version and KVM availability.
fn run_preflight(env: &EnvFacts, scenario: &ScenarioConfig, force: bool) -> Result<String, String> {
    let mut checks_passed = Vec::new();

    // Kernel version check
    match scenario.check_kernel(env.kernel.as_deref()) {
        Ok(()) => {
            eprintln!(
                "[osbase] preflight: kernel {} \u{2713}",
                scenario.requires_kernel
            );
            checks_passed.push(format!(
                "kernel {} satisfies {}",
                env.kernel.as_deref().unwrap_or("unknown"),
                scenario.requires_kernel
            ));
        }
        Err(reason) => {
            if force {
                eprintln!(
                    "[osbase] preflight: kernel {} \u{2713} (forced)",
                    scenario.requires_kernel
                );
                checks_passed.push(format!("kernel check FORCED (would fail: {reason})"));
            } else {
                eprintln!(
                    "[osbase] preflight: kernel {} \u{2717}",
                    scenario.requires_kernel
                );
                return Err(reason);
            }
        }
    }

    // KVM check
    if scenario.requires_kvm {
        if std::path::Path::new("/dev/kvm").exists() {
            eprintln!("[osbase] preflight: KVM required \u{2014} checking /dev/kvm... \u{2713}");
            checks_passed.push("/dev/kvm available".to_string());
        } else if force {
            eprintln!(
                "[osbase] preflight: KVM required \u{2014} checking /dev/kvm... \u{2713} (forced)"
            );
            checks_passed.push("/dev/kvm NOT found (forced)".to_string());
        } else {
            eprintln!("[osbase] preflight: KVM required \u{2014} checking /dev/kvm... \u{2717}");
            return Err("KVM not available (required by this scenario)".to_string());
        }
    }

    Ok(checks_passed.join("; "))
}

/// Execute `dnf install -y -q <packages>`.
fn run_dnf_install(packages: &[String]) -> Result<String, String> {
    let mut cmd = Command::new("dnf");
    cmd.arg("install").arg("-y").arg("-q");
    for pkg in packages {
        cmd.arg(pkg);
    }

    let output = cmd
        .output()
        .map_err(|e| format!("failed to execute dnf: {e}"))?;

    if output.status.success() {
        Ok(format!("installed: {}", packages.join(" ")))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Check if packages are already installed (dnf exits 0 for already-installed,
        // but let's handle the "nothing to do" case gracefully)
        let combined = format!("{stdout}\n{stderr}");
        if combined.contains("Nothing to do") || combined.contains("already installed") {
            Ok(format!(
                "packages already installed: {}",
                packages.join(" ")
            ))
        } else {
            // Print stderr on failure for diagnostics
            let stderr_str = stderr.trim();
            if !stderr_str.is_empty() {
                eprintln!("[osbase] dnf stderr:\n{stderr_str}");
            }
            Err(format!(
                "dnf install failed (exit={}): {}",
                output.status.code().unwrap_or(-1),
                stderr.lines().take(5).collect::<Vec<_>>().join("\n")
            ))
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn req(domain: OsbaseDomain, target: &str) -> OsbaseInstallRequest {
        OsbaseInstallRequest {
            domain,
            target: target.to_string(),
            register_handler: RegisterHandler::Containerd,
            register_runtimeclass: false,
            config_override: None,
            set_default: false,
            force: false,
            skip_verify: false,
            dry_run: true,
        }
    }

    #[test]
    fn validate_rejects_empty_target() {
        let r = req(OsbaseDomain::Sandbox, "  ");
        assert!(matches!(
            validate_request(&r, &root_env()),
            Err(OsbaseInstallError::InvalidRequest { .. })
        ));
    }

    #[test]
    fn validate_rejects_runtimeclass_without_handler() {
        let mut r = req(OsbaseDomain::Sandbox, "runc");
        r.register_handler = RegisterHandler::None;
        r.register_runtimeclass = true;
        assert!(matches!(
            validate_request(&r, &root_env()),
            Err(OsbaseInstallError::InvalidRequest { .. })
        ));
    }

    #[test]
    fn validate_accepts_minimal_request() {
        assert!(validate_request(&req(OsbaseDomain::Sandbox, "runc"), &root_env()).is_ok());
    }

    #[test]
    fn validate_rejects_non_root_uid() {
        let r = req(OsbaseDomain::Sandbox, "runc");
        let env = test_env(); // uid=1000
        match validate_request(&r, &env) {
            Err(OsbaseInstallError::InvalidRequest { reason }) => {
                assert!(
                    reason.contains("sudo"),
                    "expected hint pointing at sudo, got: {reason}"
                );
            }
            other => panic!("expected InvalidRequest for non-root uid, got {other:?}"),
        }
    }

    #[test]
    fn kernel_domain_is_stub() {
        let r = req(OsbaseDomain::Kernel, "agentic");
        let env = root_env();
        let err = execute_install(&r, &env).expect_err("kernel stub");
        assert!(matches!(err, OsbaseInstallError::InvalidRequest { .. }));
    }

    #[test]
    fn security_domain_is_stub() {
        let r = req(OsbaseDomain::Security, "selinux");
        let env = root_env();
        let err = execute_install(&r, &env).expect_err("security stub");
        assert!(matches!(err, OsbaseInstallError::InvalidRequest { .. }));
    }

    #[test]
    fn unknown_sandbox_scenario_is_invalid_request() {
        let r = req(OsbaseDomain::Sandbox, "nope-not-a-scenario");
        let env = root_env();
        let err = execute_install(&r, &env).expect_err("unknown scenario");
        match err {
            OsbaseInstallError::InvalidRequest { reason } => {
                assert!(reason.contains("nope-not-a-scenario"));
                assert!(reason.contains("available"));
            }
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
    }

    #[test]
    fn known_scenarios_resolve_dry_run() {
        let env = root_env();
        for s in ["runc", "rund", "firecracker", "gvisor", "landlock"] {
            let r = req(OsbaseDomain::Sandbox, s);
            let outcome =
                execute_install(&r, &env).unwrap_or_else(|_| panic!("scenario '{s}' should work"));
            assert_eq!(outcome.exit_code, 0);
            assert_eq!(outcome.target, s);
        }
    }

    #[test]
    fn list_scenarios_returns_all() {
        let names = list_scenarios().expect("should load");
        assert!(names.contains(&"runc".to_string()));
        assert!(names.contains(&"gvisor".to_string()));
        assert!(names.contains(&"landlock".to_string()));
    }

    fn test_env() -> EnvFacts {
        EnvFacts {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            libc: None,
            kernel: Some("6.6.30".to_string()),
            pkg_base: None,
            os_id: Some("alinux".to_string()),
            os_version: Some("4".to_string()),
            btf: None,
            cap_bpf: None,
            container: None,
            user: "tester".to_string(),
            uid: 1000,
            home: std::path::PathBuf::from("/home/tester"),
        }
    }

    fn root_env() -> EnvFacts {
        EnvFacts {
            uid: 0,
            user: "root".to_string(),
            ..test_env()
        }
    }
}
