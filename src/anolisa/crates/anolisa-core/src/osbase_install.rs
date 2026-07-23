//! Generic osbase install entry layer — TOML-manifest-driven.
//!
//! The install pipeline reads scenario definitions from `sandbox.toml`
//! (deployed by `anolisa system setup` to `/etc/anolisa/sandbox.toml`)
//! and executes a five-phase flow:
//!
//!   1. Preflight  — kernel version gate, KVM check if required
//!   2. Packages   — `dnf install -y <packages>` from manifest
//!   3. Services   — `systemctl enable --now` for each service
//!   4. Verify     — scenario-aware checks from `verify_commands` in manifest
//!   5. State      — persist to `installed.toml`
//!
//! Currently serves the "beginner" scenario only: zero optional
//! parameters, full-stack install from manifest.

use std::process::Command;

use anolisa_env::EnvFacts;
use anolisa_platform::fs_layout::FsLayout;
use chrono::{SecondsFormat, Utc};

use crate::domain::{
    Installation, InstallationScope, LifecycleStatus, ManagementRelation, NativePm,
    PackageIdentity, ProviderBinding,
};
use crate::lock::{InstallLock, LockError};
use crate::sandbox_manifest::{ManifestError, SandboxManifest, ScenarioConfig};
use crate::state::ObjectKind;
use crate::state_store::StateStore;

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
    /// Real degraded-verification or phase warnings.
    pub warnings: Vec<String>,
    /// Informational hints (e.g. optional packages available). Not counted
    /// as warnings and do not affect `exit_code`.
    pub hints: Vec<String>,
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
    let layout = FsLayout::system(None);
    let runtime = HostManifestInstallRuntime;
    sandbox_dispatch_with(request, env, &layout, &runtime)
}

fn sandbox_dispatch_with(
    request: &OsbaseInstallRequest,
    env: &EnvFacts,
    layout: &FsLayout,
    runtime: &impl ManifestInstallRuntime,
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
        load_state_for_layout(layout)?;
        eprintln!("[osbase] scenario: {}", scenario.name);
        let outcome = build_dry_run_outcome(request, &scenario);
        // Print phase plan in pipeline order so Direct and Helper paths
        // produce identical user-facing output.
        for phase in &outcome.phases {
            let msg = phase.message.as_deref().unwrap_or("");
            eprintln!("[osbase] [dry-run] {}: {msg}", phase.name);
        }
        for hint in &outcome.hints {
            eprintln!("[osbase] [dry-run] hint: {hint}");
        }
        return Ok(outcome);
    }

    run_manifest_install(request, env, &scenario, layout, runtime)
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

    // Services
    if scenario.services.is_empty() {
        phases.push(PhaseResult {
            name: "services".to_string(),
            status: PhaseStatus::Skipped,
            message: Some("no services for this scenario".to_string()),
            duration_ms: None,
        });
    } else {
        phases.push(PhaseResult {
            name: "services".to_string(),
            status: PhaseStatus::Skipped,
            message: Some(format!(
                "systemctl enable --now {}",
                scenario.services.join(" ")
            )),
            duration_ms: None,
        });
    }

    // Verify
    phases.push(PhaseResult {
        name: "verify".to_string(),
        status: PhaseStatus::Skipped,
        message: Some("post-install checks".to_string()),
        duration_ms: None,
    });

    // State
    phases.push(PhaseResult {
        name: "state".to_string(),
        status: PhaseStatus::Skipped,
        message: Some("persist to installed.toml".to_string()),
        duration_ms: None,
    });

    let mut hints = vec!["dry-run mode: no changes made".to_string()];
    if !scenario.packages_optional.is_empty() {
        hints.push(format!(
            "optional packages available: {}",
            scenario.packages_optional.join(" ")
        ));
    }

    OsbaseInstallOutcome {
        domain: request.domain,
        target: request.target.clone(),
        phases,
        exit_code: 0,
        warnings: vec![],
        hints,
    }
}

/// Enable and start systemd services.
fn run_enable_services(services: &[String]) -> Result<String, String> {
    let mut enabled = Vec::new();
    for svc in services {
        let output = Command::new("systemctl")
            .args(["enable", "--now", svc])
            .output()
            .map_err(|e| format!("failed to run systemctl: {e}"))?;
        if output.status.success() {
            eprintln!("[osbase] services: {svc}.service active \u{2713}");
            enabled.push(svc.clone());
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "systemctl enable --now {svc} failed: {}",
                stderr.trim()
            ));
        }
    }
    Ok(format!("enabled: {}", enabled.join(", ")))
}

/// Result of scenario-aware post-install verification.
enum VerifyOutcome {
    /// All verify commands passed.
    Passed(String),
    /// No verify commands or services defined; nothing to verify.
    NothingToVerify,
    /// One or more checks failed (degraded, not fatal).
    Failed(String),
}

trait ManifestInstallRuntime {
    fn install_packages(&self, packages: &[String]) -> Result<String, String>;
    fn enable_services(&self, services: &[String]) -> Result<String, String>;
    fn verify(&self, scenario: &ScenarioConfig) -> VerifyOutcome;
}

struct HostManifestInstallRuntime;

impl ManifestInstallRuntime for HostManifestInstallRuntime {
    fn install_packages(&self, packages: &[String]) -> Result<String, String> {
        run_dnf_install(packages)
    }

    fn enable_services(&self, services: &[String]) -> Result<String, String> {
        run_enable_services(services)
    }

    fn verify(&self, scenario: &ScenarioConfig) -> VerifyOutcome {
        run_post_verify(scenario)
    }
}

/// Scenario-aware post-install verification.
///
/// If `scenario.verify_commands` is non-empty, each entry is executed as a
/// shell-style command (split on whitespace). Otherwise, falls back to
/// `systemctl is-active` for each service declared in the scenario.
fn run_post_verify(scenario: &ScenarioConfig) -> VerifyOutcome {
    let mut checks = Vec::new();

    if !scenario.verify_commands.is_empty() {
        // Use explicit verify commands from manifest.
        for cmd_str in &scenario.verify_commands {
            let parts: Vec<&str> = cmd_str.split_whitespace().collect();
            if parts.is_empty() {
                continue;
            }
            let (bin, args) = (parts[0], &parts[1..]);
            if let Err(e) = run_verify_cmd(bin, args, cmd_str) {
                return VerifyOutcome::Failed(e);
            }
            checks.push(cmd_str.as_str());
        }
    } else if !scenario.services.is_empty() {
        // Fallback: check each service is active.
        for svc in &scenario.services {
            if let Err(e) =
                run_verify_cmd("systemctl", &["is-active", svc], &format!("{svc} active"))
            {
                return VerifyOutcome::Failed(e);
            }
            checks.push(svc.as_str());
        }
    } else {
        // No verify commands and no services — nothing to verify.
        return VerifyOutcome::NothingToVerify;
    }

    VerifyOutcome::Passed(format!("all checks passed: {}", checks.join(", ")))
}

/// Run a single verification command and report result.
fn run_verify_cmd(cmd: &str, args: &[&str], label: &str) -> Result<(), String> {
    let output = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| format!("{label}: command not found — is the package installed? ({e})"))?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let first_line = stdout.lines().next().unwrap_or("");
        eprintln!("[osbase] verify: {label} \u{2713} {first_line}");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let hint = stderr.lines().next().unwrap_or("").trim();
        if hint.is_empty() {
            Err(format!(
                "{label} failed (exit {})",
                output.status.code().unwrap_or(-1)
            ))
        } else {
            Err(format!(
                "{label} failed (exit {}): {hint}",
                output.status.code().unwrap_or(-1)
            ))
        }
    }
}

fn load_state_for_layout(layout: &FsLayout) -> Result<StateStore, OsbaseInstallError> {
    let state_path = layout.state_dir.join("installed.toml");
    StateStore::load_for_layout(
        &state_path,
        anolisa_platform::privilege::effective_uid(),
        layout,
    )
    .map_err(|error| OsbaseInstallError::PhaseFailed {
        phase: "state".to_string(),
        message: format!("failed to load state: {error}"),
    })
}

/// Execute the five-phase manifest-driven install:
/// 1. Preflight (kernel + KVM)
/// 2. Packages (full stack from manifest)
/// 3. Services (systemctl enable --now)
/// 4. Verify (scenario-aware: verify_commands from manifest, or service checks)
/// 5. State (persist to installed.toml)
fn run_manifest_install(
    request: &OsbaseInstallRequest,
    env: &EnvFacts,
    scenario: &ScenarioConfig,
    layout: &FsLayout,
    runtime: &impl ManifestInstallRuntime,
) -> Result<OsbaseInstallOutcome, OsbaseInstallError> {
    let mut phases = Vec::new();
    let mut warnings = Vec::new();

    eprintln!("[osbase] scenario: {}", scenario.name);

    // ─── Phase 1: Preflight ──────────────────────────────────────────────
    // Preflight is read-only (kernel/KVM checks); runs before the lock.
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
            eprintln!("[osbase] error: {reason}");
            phases.push(PhaseResult {
                name: "preflight".to_string(),
                status: PhaseStatus::Failed,
                message: Some(reason),
                duration_ms: None,
            });
            return Ok(OsbaseInstallOutcome {
                domain: request.domain,
                target: request.target.clone(),
                phases,
                exit_code: 1,
                warnings,
                hints: vec![],
            });
        }
    }

    // ─── Acquire InstallLock ─────────────────────────────────────────────
    // Lock covers the full mutation window: packages → services → state.
    // Held until the function returns (drop releases the lock).
    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|e| match e {
        LockError::Held { path } => OsbaseInstallError::PhaseFailed {
            phase: "lock".to_string(),
            message: format!(
                "install lock at {} is held by another process; try again later",
                path.display()
            ),
        },
        other => OsbaseInstallError::PhaseFailed {
            phase: "lock".to_string(),
            message: format!("failed to acquire install lock: {other}"),
        },
    })?;
    let state_path = layout.state_dir.join("installed.toml");
    let mut store = load_state_for_layout(layout)?;

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
        match runtime.install_packages(&scenario.packages) {
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
                    message: Some(reason),
                    duration_ms: None,
                });
                return Ok(OsbaseInstallOutcome {
                    domain: request.domain,
                    target: request.target.clone(),
                    phases,
                    exit_code: 1,
                    warnings,
                    hints: vec![],
                });
            }
        }
    }

    // ─── Phase 3: Services ───────────────────────────────────────────────
    if scenario.services.is_empty() {
        phases.push(PhaseResult {
            name: "services".to_string(),
            status: PhaseStatus::Skipped,
            message: Some("no services for this scenario".to_string()),
            duration_ms: None,
        });
    } else {
        eprintln!(
            "[osbase] enabling services: {}",
            scenario.services.join(", ")
        );
        match runtime.enable_services(&scenario.services) {
            Ok(msg) => {
                phases.push(PhaseResult {
                    name: "services".to_string(),
                    status: PhaseStatus::Success,
                    message: Some(msg),
                    duration_ms: None,
                });
            }
            Err(reason) => {
                eprintln!("[osbase] service enablement failed: {reason}");
                phases.push(PhaseResult {
                    name: "services".to_string(),
                    status: PhaseStatus::Failed,
                    message: Some(reason),
                    duration_ms: None,
                });
                return Ok(OsbaseInstallOutcome {
                    domain: request.domain,
                    target: request.target.clone(),
                    phases,
                    exit_code: 1,
                    warnings,
                    hints: vec![],
                });
            }
        }
    }

    // ─── Phase 4: Verify ─────────────────────────────────────────────────
    if !request.skip_verify {
        match runtime.verify(scenario) {
            VerifyOutcome::Passed(msg) => {
                phases.push(PhaseResult {
                    name: "verify".to_string(),
                    status: PhaseStatus::Success,
                    message: Some(msg),
                    duration_ms: None,
                });
            }
            VerifyOutcome::NothingToVerify => {
                phases.push(PhaseResult {
                    name: "verify".to_string(),
                    status: PhaseStatus::Skipped,
                    message: Some("no verify commands defined for this scenario".to_string()),
                    duration_ms: None,
                });
            }
            VerifyOutcome::Failed(reason) => {
                // Verify failure is degraded, not fatal
                eprintln!("[osbase] verify degraded: {reason}");
                warnings.push(format!("verify degraded: {reason}"));
                phases.push(PhaseResult {
                    name: "verify".to_string(),
                    status: PhaseStatus::Degraded,
                    message: Some(reason),
                    duration_ms: None,
                });
            }
        }
    } else {
        eprintln!("[osbase] verify: skipped (--no-verify)");
        phases.push(PhaseResult {
            name: "verify".to_string(),
            status: PhaseStatus::Skipped,
            message: Some("skipped by --no-verify".to_string()),
            duration_ms: None,
        });
    }

    // ─── Phase 5: State ─────────────────────────────────────────────────────
    // Lock is already held (acquired before Phase 2).
    let state_result = (|| -> Result<String, String> {
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
        // The scenario's packages are dnf-managed: record a delegated,
        // managed installation. The record marks existence; dnf owns the
        // package facts.
        let installation = Installation {
            kind: ObjectKind::Osbase,
            name: format!("sandbox-{}", scenario.name),
            scope: InstallationScope::System,
            binding: ProviderBinding::Delegated {
                pm: NativePm::Rpm,
                package: PackageIdentity::Unresolved {
                    component_hint: format!("sandbox-{}", scenario.name),
                },
                relation: ManagementRelation::Managed { since: now.clone() },
                last_observed: None,
            },
            status: LifecycleStatus::Installed,
            installed_at: now,
            last_operation_id: None,
            subscription_scope: Default::default(),
            enabled_features: vec![],
            health: vec![],
        };
        store.upsert(installation);
        store
            .save(&state_path)
            .map_err(|e| format!("failed to save state: {e}"))?;
        Ok(format!(
            "sandbox-{} recorded in {}",
            scenario.name,
            state_path.display()
        ))
    })();

    match state_result {
        Ok(msg) => {
            eprintln!("[osbase] state: {msg}");
            phases.push(PhaseResult {
                name: "state".to_string(),
                status: PhaseStatus::Success,
                message: Some(msg),
                duration_ms: None,
            });
        }
        Err(reason) => {
            // State persistence failure after packages/services were mutated
            // is a hard error: the machine has changed but we have no record.
            eprintln!("[osbase] state: FAILED: {reason}");
            warnings.push(format!(
                "state persistence failed after packages/services were modified: {reason}"
            ));
            phases.push(PhaseResult {
                name: "state".to_string(),
                status: PhaseStatus::Failed,
                message: Some(reason),
                duration_ms: None,
            });
            return Ok(OsbaseInstallOutcome {
                domain: request.domain,
                target: request.target.clone(),
                phases,
                exit_code: 1,
                warnings,
                hints: vec![],
            });
        }
    }

    eprintln!("[osbase] installed successfully");

    // Optional packages hint — informational only, not a warning.
    let mut hints = Vec::new();
    if !scenario.packages_optional.is_empty() {
        let hint = format!(
            "optional packages available: {}",
            scenario.packages_optional.join(" ")
        );
        eprintln!("[osbase] {hint}");
        hints.push(hint);
    }

    let exit_code = if phases.iter().any(|p| p.status == PhaseStatus::Degraded) {
        2
    } else {
        0
    };

    Ok(OsbaseInstallOutcome {
        domain: request.domain,
        target: request.target.clone(),
        phases,
        exit_code,
        warnings,
        hints,
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
    use std::cell::Cell;

    use super::*;
    use crate::state::OperationRecord;

    #[derive(Default)]
    struct CountingRuntime {
        package_calls: Cell<usize>,
        service_calls: Cell<usize>,
        verify_calls: Cell<usize>,
    }

    impl ManifestInstallRuntime for CountingRuntime {
        fn install_packages(&self, packages: &[String]) -> Result<String, String> {
            self.package_calls.set(self.package_calls.get() + 1);
            Ok(format!("installed: {}", packages.join(" ")))
        }

        fn enable_services(&self, services: &[String]) -> Result<String, String> {
            self.service_calls.set(self.service_calls.get() + 1);
            Ok(format!("enabled: {}", services.join(", ")))
        }

        fn verify(&self, _scenario: &ScenarioConfig) -> VerifyOutcome {
            self.verify_calls.set(self.verify_calls.get() + 1);
            VerifyOutcome::NothingToVerify
        }
    }

    fn write_operation_only_user_state(system_layout: &FsLayout, home: &std::path::Path) {
        let user_layout =
            FsLayout::user_with_overrides(home.to_path_buf(), None, None, None, None, None);
        let mut store = StateStore::empty_for_layout(&user_layout);
        store.operations.push(OperationRecord {
            id: "op-1".to_string(),
            command: "install cosh".to_string(),
            status: "started".to_string(),
            started_at: "2026-07-21T00:00:00Z".to_string(),
            finished_at: None,
            parent_operation_id: None,
        });
        store
            .save(&system_layout.state_dir.join("installed.toml"))
            .expect("save mismatched state");
    }

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
        let tmp = tempfile::tempdir().expect("tempdir");
        let layout = FsLayout::system(Some(tmp.path().join("system")));
        let runtime = CountingRuntime::default();
        for s in ["runc", "rund", "firecracker", "gvisor", "landlock"] {
            let r = req(OsbaseDomain::Sandbox, s);
            let outcome = sandbox_dispatch_with(&r, &env, &layout, &runtime)
                .unwrap_or_else(|_| panic!("scenario '{s}' should work"));
            assert_eq!(outcome.exit_code, 0);
            assert_eq!(outcome.target, s);

            // Every dry-run must produce exactly five phases in canonical order.
            let phase_names: Vec<&str> = outcome.phases.iter().map(|p| p.name.as_str()).collect();
            assert_eq!(
                phase_names,
                vec!["preflight", "packages", "services", "verify", "state"],
                "scenario '{s}' should produce exactly five phases in order"
            );
            // All phases must be Skipped in dry-run mode.
            for phase in &outcome.phases {
                assert_eq!(
                    phase.status,
                    PhaseStatus::Skipped,
                    "scenario '{s}' phase '{}' should be Skipped in dry-run, got {:?}",
                    phase.name,
                    phase.status
                );
            }
        }
    }

    #[test]
    fn dry_run_rejects_mismatched_state_scope() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let layout = FsLayout::system(Some(tmp.path().join("system")));
        write_operation_only_user_state(&layout, &tmp.path().join("home"));
        let runtime = CountingRuntime::default();

        let err = sandbox_dispatch_with(
            &req(OsbaseDomain::Sandbox, "runc"),
            &root_env(),
            &layout,
            &runtime,
        )
        .expect_err("dry-run must validate the executable state scope");

        assert!(matches!(
            err,
            OsbaseInstallError::PhaseFailed { ref phase, .. } if phase == "state"
        ));
        assert_eq!(runtime.package_calls.get(), 0);
        assert_eq!(runtime.service_calls.get(), 0);
        assert_eq!(runtime.verify_calls.get(), 0);
    }

    #[test]
    fn state_scope_is_validated_before_native_side_effects() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let layout = FsLayout::system(Some(tmp.path().join("system")));
        write_operation_only_user_state(&layout, &tmp.path().join("home"));
        let runtime = CountingRuntime::default();
        let mut request = req(OsbaseDomain::Sandbox, "runc");
        request.dry_run = false;

        let err = sandbox_dispatch_with(&request, &root_env(), &layout, &runtime)
            .expect_err("state mismatch must stop the install");

        assert!(matches!(
            err,
            OsbaseInstallError::PhaseFailed { ref phase, .. } if phase == "state"
        ));
        assert_eq!(runtime.package_calls.get(), 0);
        assert_eq!(runtime.service_calls.get(), 0);
        assert_eq!(runtime.verify_calls.get(), 0);
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
