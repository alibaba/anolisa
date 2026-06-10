//! Sandbox install pipeline — 5-phase orchestrator.
//!
//! Implements the install pipeline defined in sandbox-subsystem-design.md §11.3:
//!
//!   Phase 1: Pre-flight (environment gate + Strategy B probe)
//!   Phase 2: Packages (RPM/DEB install + dependency linkage)
//!   Phase 3: OS Primitives (sysctl / udev / kernel modules / systemd units)
//!   Phase 4: Service Setup (systemd enable + start, if applicable)
//!   Phase 5: Post-verify (health check via lifecycle.health_hook equivalent)
//!
//! Each phase is a transaction step. Rollback is best-effort here; full
//! transactional guarantees are tracked via [`crate::transaction`] integration
//! (see sandbox-subsystem-design §11.6.4).

use std::fmt;
use std::fs;
use std::path::Path;
use std::process::Command;

use serde::Serialize;

use anolisa_env::{EnvFacts, EnvService};
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::package_manager::detect_package_manager;
use anolisa_platform::privilege;

use crate::central_log::{CentralLog, CentralLogError, LogKind, LogRecord, LogStatus, Severity};
use crate::lock::InstallLock;
use crate::state::{
    InstalledObject, InstalledState, ObjectKind, ObjectStatus, OperationRecord, StateError,
};

// ===========================================================================
// Package lists (single source of truth, shared by execute & dry-run paths)
// ===========================================================================

/// Required RPMs for the firecracker `standard` / `default` variant. Used by
/// both [`firecracker_packages`] (real install) and
/// [`firecracker_dry_run_phases`] (dry-run text), so the two cannot drift.
const FIRECRACKER_STANDARD_PACKAGES: &[&str] = &[
    "firecracker-standard",
    "firecracker-e2b-kernel",
    "firecracker-e2b-rootfs",
];

/// Required RPMs for the firecracker `e2b` variant. Used by both
/// [`e2b_packages`] (real install) and [`firecracker_dry_run_phases`]
/// (dry-run text), so the two cannot drift.
const FIRECRACKER_E2B_PACKAGES: &[&str] = &[
    "firecracker-e2b",
    "firecracker-e2b-busybox",
    "firecracker-e2b-jailer",
    "firecracker-e2b-tools",
    "firecracker-e2b-kernel",
    "firecracker-e2b-rootfs",
    "e2b-orchestrator",
    "e2b-orchestrator-cli",
    "e2b-envd",
    "e2b-system-config",
];

/// Required RPMs for gVisor standalone mode (`install gvisor`).
pub(crate) const GVISOR_STANDALONE_PACKAGES: &[&str] = &["gvisor-runsc"];

/// Required RPMs for gVisor shim mode (`install gvisor --runtime=containerd`).
pub(crate) const GVISOR_SHIM_PACKAGES: &[&str] = &["gvisor-runsc", "containerd-shim-runsc-v1"];

/// Required RPMs for gVisor Docker mode (`install gvisor --runtime=docker`).
pub(crate) const GVISOR_DOCKER_PACKAGES: &[&str] = &["gvisor-runsc"];

/// Required RPMs for gVisor + Substrate data-plane mode.
pub(crate) const GVISOR_SUBSTRATE_PACKAGES: &[&str] = &[
    "gvisor-runsc",
    "containerd-shim-runsc-v1",
    "atelet",
    "ateom-gvisor",
];

/// Logical RPM repository ID surfaced in error messages and `anolisa doctor`.
///
/// MUST stay in sync with `[dependencies.repository] id` in
/// `manifests/osbase/sandbox-gvisor.toml`. The 4 RPMs in `GVISOR_*_PACKAGES`
/// are NOT in upstream Anolis/AlibabaCloudLinux repos — ANOLISA must publish
/// them via this repo. See `sandbox-rpm-packaging.md` for details.
pub(crate) const ANOLISA_SANDBOX_REPO_ID: &str = "anolisa-sandbox";

/// Path to the packaging design document, embedded in error messages so
/// operators can reach it without leaving the terminal.
pub(crate) const SANDBOX_RPM_PACKAGING_DOC: &str =
    "ANOLISA-design/docs/anolisa/osbase/sandbox/sandbox-rpm-packaging.md";

/// Probe whether a package is *available* in any configured dnf repository
/// (without installing it). Returns:
/// - `Some(true)`  — package is in a repo and dnf is functioning
/// - `Some(false)` — dnf functioned but the package was not found
/// - `None`        — dnf could not be invoked / probe was inconclusive (treat
///                   as "don't gate" — fall through to the actual install
///                   attempt so we don't generate spurious errors on quirky
///                   environments)
///
/// Implementation note: we use `dnf repoquery --quiet --qf '%{name}'` rather
/// than `dnf list --available` because the latter writes to stderr on miss
/// and exits non-zero, while `repoquery` exits 0 with empty stdout on miss.
#[allow(clippy::doc_overindented_list_items)]
fn dnf_repoquery_available(package: &str) -> Option<bool> {
    let output = Command::new("dnf")
        .args(["repoquery", "--quiet", "--qf", "%{name}", package])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Some(stdout.lines().any(|line| line.trim() == package))
}

/// Build the human-readable error message used when one or more required RPMs
/// are missing from the configured dnf repos. Centralised so that both the
/// pre-flight repoquery check and the post-install fallback path produce the
/// same actionable text — operators always see the same remediation.
fn gvisor_missing_rpm_error(missing: &[&str], required: &[&str]) -> String {
    format!(
        "required ANOLISA sandbox RPM(s) not found in any configured repo: {missing}. \
         These packages are NOT shipped by upstream Anolis/AlibabaCloudLinux — \
         ANOLISA must publish them via the `{repo}` repo. \
         Required for this install mode: {required}. \
         Action: enable repo `{repo}` (build via spec.in templates under \
         anolisa/src/anolisa/packaging/sandbox/, then push to the internal \
         dnf repository). See {doc} for the full playbook.",
        missing = missing.join(", "),
        required = required.join(", "),
        repo = ANOLISA_SANDBOX_REPO_ID,
        doc = SANDBOX_RPM_PACKAGING_DOC,
    )
}

/// Probe whether `dnf` is available on the host. Sandbox install fixes the
/// package-manager command to `dnf` (see [`detect_package_manager`] /
/// [`DnfBackend`]); a yum-only environment would pass the platform-level
/// detection but fail the moment we exec `dnf`. Reject early with a clear
/// error so the failure is attributable to env, not to the install pipeline.
fn dnf_command_exists() -> bool {
    if let Some(paths) = std::env::var_os("PATH") {
        for p in std::env::split_paths(&paths) {
            if p.join("dnf").is_file() {
                return true;
            }
        }
    }
    false
}

// ===========================================================================
// Public types
// ===========================================================================

/// Sandbox backend kind (mirrors CLI `SandboxTarget`).
///
/// Each variant identifies an isolation engine. The string forms returned by
/// [`Display`](fmt::Display) are stable identifiers used in CLI args, log
/// records, and the installed-state file — adding a variant must keep these in
/// sync with [`default_variant`](Self::default_variant).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxBackendKind {
    /// OCI container runtime (runc / rund).
    Container,
    /// Kata Containers — KVM-backed lightweight VM.
    Kata,
    /// Firecracker microVM (standard / e2b / kata-fc variants).
    Firecracker,
    /// gVisor user-space kernel (runsc).
    Gvisor,
    /// QEMU/KVM full virtual machine.
    Vm,
    /// Landlock LSM filesystem access control.
    Landlock,
}

impl fmt::Display for SandboxBackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Container => write!(f, "container"),
            Self::Kata => write!(f, "kata"),
            Self::Firecracker => write!(f, "firecracker"),
            Self::Gvisor => write!(f, "gvisor"),
            Self::Vm => write!(f, "vm"),
            Self::Landlock => write!(f, "landlock"),
        }
    }
}

impl SandboxBackendKind {
    /// Return the default variant for this backend.
    pub fn default_variant(&self) -> &'static str {
        match self {
            Self::Container => "runc",
            Self::Kata => "default",
            Self::Firecracker => "standard",
            Self::Gvisor => "default",
            Self::Vm => "default",
            Self::Landlock => "default",
        }
    }
}

/// Install pipeline request parameters.
#[derive(Debug, Clone)]
pub struct SandboxInstallRequest {
    pub backend: SandboxBackendKind,
    /// Variant identifier validated by [`validate_request`]. For firecracker:
    /// `"standard"` | `"default"` | `"e2b"`. For gvisor: `"default"` only
    /// (gVisor has no fork — see gvisor-substrate-design-note §5.1). Unknown
    /// values surface as [`SandboxInstallError::Unsupported`].
    pub variant: String,
    /// L2 runtime to register the engine into. For gvisor:
    /// `Some("containerd")` (shim mode) | `Some("docker")` (Docker daemon.json)
    /// | `None` (standalone). Firecracker rejects any value (it bypasses L2).
    /// See gvisor-substrate-design-note §5.1.
    pub runtime: Option<String>,
    /// Optional control-panel data-plane overlay layered on top of an
    /// engine+runtime install. Currently only `Some("substrate")` paired
    /// with `runtime=Some("containerd")` for the gvisor backend; all other
    /// combinations are rejected by [`validate_request`].
    pub control_panel: Option<String>,
    /// Produce a [`SandboxInstallDryRun`] without side effects: no install
    /// lock, no state write, no central-log entry.
    pub dry_run: bool,
    /// Bypass non-fatal pre-flight gates (e.g. missing UFFD WP_ASYNC for e2b,
    /// existing hugepages mount). Hard gates (no `/dev/kvm`, wrong arch) are
    /// not bypassed.
    pub force: bool,
    /// Skip Phase 5 (post-verify). The pipeline still records a `Skipped`
    /// [`PhaseResult`] so downstream tooling can distinguish "not run" from
    /// "failed".
    pub no_verify: bool,
    /// Output channel hint for the CLI; library code does not consume it.
    pub json: bool,
}

/// Which install phase we are in.
///
/// Three string forms exist with distinct consumers — keep them in sync when
/// adding a phase:
/// - variant name (`OsPrimitives`): code-internal
/// - [`Serialize`] (`os_primitives`, snake_case): JSON output / state file
/// - [`fmt::Display`] (`"OS Config"`): human progress lines
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallPhase {
    /// Phase 1 — environment gate (OS / arch / kernel / KVM / WP_ASYNC).
    Preflight,
    /// Phase 2 — RPM/DEB install via the detected package manager.
    Packages,
    /// Phase 3 — sysctl / udev / sysusers / tmpfiles / kernel modules /
    /// hugepages / default microVM assets.
    OsPrimitives,
    /// Phase 4 — systemd unit enable + start (skipped when none applies).
    ServiceSetup,
    /// Phase 5 — health check (binary version, gRPC probe, etc.).
    PostVerify,
}

impl fmt::Display for InstallPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Preflight => write!(f, "Pre-flight"),
            Self::Packages => write!(f, "Packages"),
            Self::OsPrimitives => write!(f, "OS Config"),
            Self::ServiceSetup => write!(f, "Service"),
            Self::PostVerify => write!(f, "Verify"),
        }
    }
}

/// Status of a single phase execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseStatus {
    /// Phase ran and met all hard requirements.
    Success,
    /// Phase intentionally not executed (dry-run, `--no-verify`, or no work
    /// applies — e.g. firecracker standard has no service to enable).
    Skipped,
    /// Phase ran but produced non-fatal warnings; install continues.
    Warning,
    /// Phase aborted with an error; install pipeline stops and surfaces a
    /// [`SandboxInstallError`].
    Failed,
}

/// Result of a single phase execution.
#[derive(Debug, Clone, Serialize)]
pub struct PhaseResult {
    pub phase: InstallPhase,
    pub status: PhaseStatus,
    /// Human-readable summary, also rendered into JSON output. Multiple
    /// actions are joined with `"; "` for compactness on a single line.
    pub message: String,
}

/// Aggregate outcome of the install pipeline.
#[derive(Debug, Clone, Serialize)]
pub struct SandboxInstallOutcome {
    /// Stable backend identifier (matches [`SandboxBackendKind::Display`](fmt::Display)).
    pub backend: String,
    pub variant: String,
    /// Per-phase results in execution order; always 5 entries on success.
    pub phases: Vec<PhaseResult>,
    /// Mapped per sandbox-subsystem-design §11.6.3:
    /// `0` ok, `2` degraded (warnings), `3` failed (rolled back).
    pub exit_code: u8,
    /// Aggregated non-fatal warnings collected across phases.
    pub warnings: Vec<String>,
    /// Version string parsed from `firecracker --version` during Phase 5.
    /// `None` when verify was skipped or the binary was not in PATH.
    /// Persisted into `installed.toml` so the state file reflects the actual
    /// runtime version rather than a hard-coded literal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
}

/// Dry-run plan item for a single phase.
#[derive(Debug, Clone, Serialize)]
pub struct DryRunPhase {
    pub phase: InstallPhase,
    /// Ordered list of side-effect descriptions that would be performed if
    /// `dry_run` were `false`. Strings are intended for human display only —
    /// they are not stable identifiers.
    pub actions: Vec<String>,
}

/// Dry-run output (no side effects).
#[derive(Debug, Clone, Serialize)]
pub struct SandboxInstallDryRun {
    pub backend: String,
    pub variant: String,
    /// Per-phase plan in execution order; always 5 entries for supported
    /// backend/variant combinations.
    pub phases: Vec<DryRunPhase>,
}

/// Errors from the sandbox install pipeline.
#[derive(Debug, thiserror::Error)]
pub enum SandboxInstallError {
    #[error("environment not satisfied: {reason}")]
    EnvNotSatisfied {
        reason: String,
        remediation: Option<String>,
    },

    #[error("package installation failed: {0}")]
    PackageFailed(String),

    #[error("OS configuration failed: {0}")]
    OsConfigFailed(String),

    #[error("service setup failed: {0}")]
    ServiceFailed(String),

    #[error("post-install verification failed: {0}")]
    VerifyFailed(String),

    #[error("install lock held by another process")]
    LockHeld,

    #[error("state write failed: {0}")]
    StateFailed(String),

    #[error("unsupported backend/variant: {backend} --variant={variant}")]
    Unsupported { backend: String, variant: String },

    #[error("must run as root for system-mode install")]
    NotRoot,
}

impl SandboxInstallError {
    /// Map to sandbox-subsystem-design §11.6.3 exit codes.
    //
    // The CLI handler currently derives the exit code from `phases` rather
    // than from this error directly; the table is kept colocated with the
    // variants so future callers (and tests) can rely on it without
    // re-deriving the mapping. Allow until a caller lands.
    #[allow(dead_code)]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::EnvNotSatisfied { .. } => 1, // blocked
            Self::PackageFailed(_) => 3,       // install failed, rollback success
            Self::OsConfigFailed(_) => 3,
            Self::ServiceFailed(_) => 3,
            Self::VerifyFailed(_) => 2, // installed but verify failed (degraded)
            Self::LockHeld => 1,
            Self::StateFailed(_) => 4, // state inconsistent
            Self::Unsupported { .. } => 1,
            Self::NotRoot => 1,
        }
    }
}

// ===========================================================================
// Main entry point
// ===========================================================================

/// Execute the sandbox install pipeline (or produce dry-run output).
///
/// Single entry point called by the CLI handler.
pub fn execute_sandbox_install(
    request: &SandboxInstallRequest,
    layout: &FsLayout,
) -> Result<SandboxInstallOutcome, SandboxInstallError> {
    // --- Validate backend/variant support ---
    validate_request(request)?;

    // --- Dry-run: just produce the plan ---
    if request.dry_run {
        // We produce a "success" outcome with the dry-run plan as messages.
        let plan = build_dry_run_plan(request);
        let phases: Vec<PhaseResult> = plan
            .phases
            .iter()
            .map(|p| PhaseResult {
                phase: p.phase,
                status: PhaseStatus::Skipped,
                message: p.actions.join("; "),
            })
            .collect();
        return Ok(SandboxInstallOutcome {
            backend: request.backend.to_string(),
            variant: request.variant.clone(),
            phases,
            exit_code: 0,
            warnings: vec!["dry-run mode: no changes made".to_string()],
            installed_version: None,
        });
    }

    // --- Root check for system-mode ---
    if matches!(
        layout.mode,
        anolisa_platform::fs_layout::InstallMode::System
    ) && !privilege::is_root()
    {
        return Err(SandboxInstallError::NotRoot);
    }

    // --- Acquire install lock ---
    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|e| match e {
        crate::lock::LockError::Held { .. } => SandboxInstallError::LockHeld,
        crate::lock::LockError::Io { path, source } => SandboxInstallError::StateFailed(format!(
            "lock IO error at {}: {source}",
            path.display()
        )),
    })?;

    // --- Log: started ---
    let operation_id = generate_operation_id();
    let cmd_str = format!(
        "osbase sandbox install {} --variant={}",
        request.backend, request.variant
    );
    let _ = log_operation(layout, &operation_id, &cmd_str, None, Severity::Info);

    // --- Probe environment exactly once ---
    //
    // EnvService::detect() reads /proc, /sys and /etc/os-release on every
    // invocation; calling it inside each phase produced 4 redundant probes.
    // Take a single snapshot here and pass it down so the install observes a
    // consistent view of the host (matters when the OS is rebooting / the
    // package manager is mutating /etc/os-release mid-install).
    let env_facts = EnvService::detect();

    // --- Execute 5-phase pipeline ---
    let result = run_pipeline(request, layout, &env_facts);

    // --- Log + state write based on result ---
    match &result {
        Ok(outcome) => {
            let _ = log_operation(
                layout,
                &operation_id,
                &cmd_str,
                Some(LogStatus::Ok),
                Severity::Info,
            );
            let _ = write_installed_state(layout, request, outcome, &operation_id);
        }
        Err(_) => {
            let _ = log_operation(
                layout,
                &operation_id,
                &cmd_str,
                Some(LogStatus::Failed),
                Severity::Error,
            );
        }
    }

    // Lock released on drop
    result
}

// ===========================================================================
// Pipeline runner
// ===========================================================================

fn run_pipeline(
    request: &SandboxInstallRequest,
    layout: &FsLayout,
    env_facts: &EnvFacts,
) -> Result<SandboxInstallOutcome, SandboxInstallError> {
    let mut phases = Vec::with_capacity(5);
    let mut warnings = Vec::new();
    let mut installed_version: Option<String> = None;

    // Dispatch to backend-specific pipeline
    match request.backend {
        SandboxBackendKind::Firecracker => match request.variant.as_str() {
            "e2b" => run_firecracker_e2b(
                request,
                layout,
                env_facts,
                &mut phases,
                &mut warnings,
                &mut installed_version,
            )?,
            _ => run_firecracker_standard(
                request,
                layout,
                env_facts,
                &mut phases,
                &mut warnings,
                &mut installed_version,
            )?,
        },
        SandboxBackendKind::Gvisor => run_gvisor(
            request,
            env_facts,
            &mut phases,
            &mut warnings,
            &mut installed_version,
        )?,
        other => {
            return Err(SandboxInstallError::Unsupported {
                backend: other.to_string(),
                variant: request.variant.clone(),
            });
        }
    }

    // Determine exit code from phases
    let exit_code = if phases.iter().any(|p| p.status == PhaseStatus::Failed) {
        3
    } else if phases.iter().any(|p| p.status == PhaseStatus::Warning) {
        2
    } else {
        0
    };

    Ok(SandboxInstallOutcome {
        backend: request.backend.to_string(),
        variant: request.variant.clone(),
        phases,
        exit_code,
        warnings,
        installed_version,
    })
}

// ===========================================================================
// Firecracker Standard variant
// ===========================================================================

fn run_firecracker_standard(
    request: &SandboxInstallRequest,
    layout: &FsLayout,
    env_facts: &EnvFacts,
    phases: &mut Vec<PhaseResult>,
    warnings: &mut Vec<String>,
    installed_version: &mut Option<String>,
) -> Result<(), SandboxInstallError> {
    // --- Phase 1: Pre-flight ---
    let preflight = firecracker_preflight(env_facts)?;
    phases.push(preflight);

    // --- Phase 2: Packages ---
    let packages = firecracker_packages(env_facts)?;
    phases.push(packages);

    // --- Phase 3: OS Primitives ---
    let os_config = firecracker_os_primitives(layout, warnings)?;
    phases.push(os_config);

    // --- Phase 4: Service (none for standard) ---
    phases.push(PhaseResult {
        phase: InstallPhase::ServiceSetup,
        status: PhaseStatus::Skipped,
        message: "skipped: no persistent service for firecracker standard".to_string(),
    });

    // --- Phase 5: Post-verify ---
    if !request.no_verify {
        let verify = firecracker_verify(warnings, installed_version)?;
        phases.push(verify);
    } else {
        phases.push(PhaseResult {
            phase: InstallPhase::PostVerify,
            status: PhaseStatus::Skipped,
            message: "skipped by --no-verify".to_string(),
        });
    }

    Ok(())
}

/// Phase 1: Pre-flight checks for firecracker.
fn firecracker_preflight(facts: &EnvFacts) -> Result<PhaseResult, SandboxInstallError> {
    // Check OS
    if facts.os != "linux" {
        return Err(SandboxInstallError::EnvNotSatisfied {
            reason: format!("firecracker requires Linux, got '{}'", facts.os),
            remediation: None,
        });
    }

    // Check arch
    if facts.arch != "x86_64" && facts.arch != "aarch64" {
        return Err(SandboxInstallError::EnvNotSatisfied {
            reason: format!(
                "firecracker requires x86_64 or aarch64, got '{}'",
                facts.arch
            ),
            remediation: None,
        });
    }

    // Check kernel >= 4.14
    if let Some(ref kernel) = facts.kernel
        && !kernel_version_at_least(kernel, 4, 14)
    {
        return Err(SandboxInstallError::EnvNotSatisfied {
            reason: format!("firecracker requires kernel >= 4.14, got '{kernel}'"),
            remediation: Some("upgrade your kernel to >= 4.14".to_string()),
        });
    }

    // Check KVM availability
    if !Path::new("/dev/kvm").exists() {
        return Err(SandboxInstallError::EnvNotSatisfied {
            reason: "/dev/kvm not found — KVM not available".to_string(),
            remediation: Some(
                "ensure KVM is enabled: load kvm_intel/kvm_amd module, check BIOS virtualization settings".to_string(),
            ),
        });
    }

    let kernel_str = facts.kernel.as_deref().unwrap_or("unknown");
    Ok(PhaseResult {
        phase: InstallPhase::Preflight,
        status: PhaseStatus::Success,
        message: format!("kvm=true, kernel={kernel_str}, arch={}", facts.arch),
    })
}

/// Phase 2: Package installation for firecracker standard.
fn firecracker_packages(facts: &EnvFacts) -> Result<PhaseResult, SandboxInstallError> {
    if !dnf_command_exists() {
        return Err(SandboxInstallError::PackageFailed(
            "dnf binary not found in PATH; firecracker sandbox install requires dnf (yum-only environments are not supported)"
                .to_string(),
        ));
    }
    let pkg_mgr = detect_package_manager(facts.pkg_base.as_deref()).map_err(|e| {
        SandboxInstallError::PackageFailed(format!("cannot detect package manager: {e}"))
    })?;

    // RPM packaging note: the Anolisa firecracker repository ships variant-prefixed
    // packages (`firecracker-standard*`, `firecracker-e2b*`). The `firecracker-standard`
    // main RPM provides the firecracker binary; the kernel (Guest vmlinux) and rootfs
    // (base.ext4) live in variant-neutral data-only sub-packages `firecracker-e2b-kernel`
    // and `firecracker-e2b-rootfs` shared across both variants — we depend on them
    // explicitly so the standard install ships a runnable default microVM.
    let required_packages: &[&str] = FIRECRACKER_STANDARD_PACKAGES;
    let mut installed = Vec::new();
    let mut to_install = Vec::new();

    for pkg in required_packages {
        if pkg_mgr.is_installed(pkg) {
            installed.push(*pkg);
        } else {
            to_install.push(*pkg);
        }
    }

    if !to_install.is_empty() {
        pkg_mgr.install(&to_install).map_err(|e| {
            SandboxInstallError::PackageFailed(format!(
                "failed to install {}: {e}",
                to_install.join(", ")
            ))
        })?;
    }

    let msg = if to_install.is_empty() {
        format!("already installed: {}", required_packages.join(", "))
    } else {
        format!("installed: {}", to_install.join(", "))
    };

    Ok(PhaseResult {
        phase: InstallPhase::Packages,
        status: PhaseStatus::Success,
        message: msg,
    })
}

/// Phase 3: OS-level configuration for firecracker.
fn firecracker_os_primitives(
    layout: &FsLayout,
    warnings: &mut Vec<String>,
) -> Result<PhaseResult, SandboxInstallError> {
    let mut actions = Vec::new();
    let warnings_before = warnings.len();

    // Write modules-load.d config for KVM modules
    let modules_conf_path = Path::new("/etc/modules-load.d/anolisa-sandbox-fc.conf");
    let modules_content = "# ANOLISA sandbox: ensure KVM modules are loaded\nkvm_intel\nkvm_amd\n";

    if let Some(parent) = modules_conf_path.parent() {
        if parent.exists() {
            match fs::write(modules_conf_path, modules_content) {
                Ok(()) => {
                    actions.push(format!("wrote {}", modules_conf_path.display()));
                }
                Err(e) => {
                    // Non-fatal: the system may still work if KVM is already loaded
                    warnings.push(format!(
                        "could not write {}: {e} (KVM modules may need manual loading)",
                        modules_conf_path.display()
                    ));
                }
            }
        } else {
            warnings.push(format!(
                "{} does not exist; skipping modules-load.d config",
                parent.display()
            ));
        }
    }

    // Best-effort: modprobe KVM modules
    let _ = Command::new("modprobe")
        .arg("kvm_intel")
        .stderr(std::process::Stdio::null())
        .status();
    let _ = Command::new("modprobe")
        .arg("kvm_amd")
        .stderr(std::process::Stdio::null())
        .status();
    actions.push("modprobe kvm_intel kvm_amd (best effort)".to_string());

    // Provision default microVM assets (kernel + rootfs symlinks + vm-config.json)
    // so that `firecracker --config-file` works out of the box.
    firecracker_provision_default_assets(layout, &mut actions, warnings);

    // Verify /dev/kvm permissions
    check_kvm_permissions(warnings);

    let msg = if actions.is_empty() {
        "no OS primitives needed".to_string()
    } else {
        actions.join("; ")
    };

    let had_warnings = warnings.len() > warnings_before;
    Ok(PhaseResult {
        phase: InstallPhase::OsPrimitives,
        status: if had_warnings {
            PhaseStatus::Warning
        } else {
            PhaseStatus::Success
        },
        message: msg,
    })
}

/// Phase 5: Post-install verification.
fn firecracker_verify(
    warnings: &mut Vec<String>,
    installed_version: &mut Option<String>,
) -> Result<PhaseResult, SandboxInstallError> {
    // Check firecracker --version
    let fc_version = run_version_command("firecracker");
    let jailer_version = run_version_command("jailer");

    match fc_version {
        Some(version) => {
            *installed_version = parse_firecracker_version(&version);
            let mut msg = format!("firecracker {version}");
            if let Some(jv) = jailer_version {
                msg.push_str(&format!(", jailer {jv}"));
            } else {
                warnings.push("jailer --version failed; jailer may not be in PATH".to_string());
            }
            Ok(PhaseResult {
                phase: InstallPhase::PostVerify,
                status: PhaseStatus::Success,
                message: msg,
            })
        }
        None => {
            // Verification failed — installed but not working (degraded)
            Err(SandboxInstallError::VerifyFailed(
                "firecracker --version returned non-zero or not in PATH".to_string(),
            ))
        }
    }
}

// ===========================================================================
// Firecracker E2B variant
// ===========================================================================

fn run_firecracker_e2b(
    request: &SandboxInstallRequest,
    layout: &FsLayout,
    env_facts: &EnvFacts,
    phases: &mut Vec<PhaseResult>,
    warnings: &mut Vec<String>,
    installed_version: &mut Option<String>,
) -> Result<(), SandboxInstallError> {
    // --- Phase 1: Pre-flight (base + WP_ASYNC) ---
    let preflight = e2b_preflight(request, env_facts, warnings)?;
    phases.push(preflight);

    // --- Phase 2: Packages ---
    let packages = e2b_packages(env_facts)?;
    phases.push(packages);

    // --- Phase 3: OS Primitives ---
    let os_config = e2b_os_primitives(layout, request, warnings)?;
    phases.push(os_config);

    // --- Phase 4: Service ---
    //
    // From this point onward Phase 3 has already written artifacts to the
    // host (sysctl/udev/sysusers/tmpfiles drop-ins + mnt-hugepages.mount).
    // If Phase 4 or Phase 5 fails we roll those artifacts back so the host
    // does not retain half-installed config that would confuse a retry or a
    // subsequent --variant=standard install.
    let service = match e2b_service() {
        Ok(r) => r,
        Err(e) => {
            cleanup_e2b_phase3_artifacts(warnings);
            return Err(e);
        }
    };
    phases.push(service);

    // --- Phase 5: Post-verify ---
    if !request.no_verify {
        let verify = match e2b_verify(warnings, installed_version) {
            Ok(r) => r,
            Err(e) => {
                cleanup_e2b_phase3_artifacts(warnings);
                // Best-effort: also disable+stop the orchestrator unit we
                // just enabled in Phase 4 so retry starts from a clean slate.
                let _ = Command::new("systemctl")
                    .args(["disable", "--now", "e2b-orchestrator.service"])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
                return Err(e);
            }
        };
        phases.push(verify);
    } else {
        phases.push(PhaseResult {
            phase: InstallPhase::PostVerify,
            status: PhaseStatus::Skipped,
            message: "skipped by --no-verify".to_string(),
        });
    }

    Ok(())
}

/// Best-effort rollback of Phase 3 artifacts written by `e2b_os_primitives`.
///
/// Invoked when Phase 4 (service enable) or Phase 5 (post-verify) fails so
/// that a subsequent retry starts from a clean state. Each step is
/// independent and any failure is recorded as a warning rather than
/// propagated, since the install has already failed.
fn cleanup_e2b_phase3_artifacts(warnings: &mut Vec<String>) {
    // Disable+stop the persistent hugepages mount (if it was enabled).
    let _ = Command::new("systemctl")
        .args(["disable", "--now", "mnt-hugepages.mount"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    // Best-effort: unmount the hugetlbfs (ignore failure if not mounted).
    let _ = Command::new("umount")
        .arg("/mnt/hugepages")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    // Remove the drop-ins ANOLISA wrote in Phase 3. We never touch files
    // outside this fixed allow-list to avoid clobbering distro-shipped
    // configuration with the same suffix.
    let artifacts: &[&str] = &[
        "/etc/sysctl.d/90-e2b.conf",
        "/etc/udev/rules.d/90-e2b-userfaultfd.rules",
        "/usr/lib/sysusers.d/e2b.conf",
        "/usr/lib/tmpfiles.d/e2b.conf",
        "/etc/modules-load.d/anolisa-sandbox-e2b.conf",
        "/etc/modprobe.d/e2b-nbd.conf",
        "/etc/systemd/system/mnt-hugepages.mount",
    ];
    for path in artifacts {
        let p = Path::new(path);
        if p.exists()
            && let Err(e) = fs::remove_file(p)
        {
            warnings.push(format!("rollback: could not remove {path}: {e}"));
        }
    }

    // Reload systemd / re-apply sysctl so the host reflects the cleanup.
    let _ = Command::new("systemctl")
        .arg("daemon-reload")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let _ = Command::new("sysctl")
        .arg("--system")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let _ = Command::new("udevadm")
        .args(["control", "--reload-rules"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Phase 1: Pre-flight checks for firecracker e2b.
///
/// Runs the base firecracker preflight (OS/arch/kernel/KVM) plus an
/// additional check for UFFD WP_ASYNC support (kernel >= 6.7 or
/// `userfaultfd_wp_async` symbol in /proc/kallsyms).
fn e2b_preflight(
    request: &SandboxInstallRequest,
    facts: &EnvFacts,
    warnings: &mut Vec<String>,
) -> Result<PhaseResult, SandboxInstallError> {
    // Run base firecracker checks first
    let base_result = firecracker_preflight(facts)?;

    // Check WP_ASYNC support
    let has_wp_async = check_wp_async_support(facts);

    if has_wp_async {
        Ok(PhaseResult {
            phase: InstallPhase::Preflight,
            status: PhaseStatus::Success,
            message: format!("{}, uffd_wp_async=true", base_result.message),
        })
    } else if request.force {
        warnings.push(
            "UFFD WP_ASYNC not available; incremental snapshot disabled. Continuing due to --force."
                .to_string(),
        );
        Ok(PhaseResult {
            phase: InstallPhase::Preflight,
            status: PhaseStatus::Warning,
            message: format!("{}, uffd_wp_async=false (forced)", base_result.message),
        })
    } else {
        Err(SandboxInstallError::EnvNotSatisfied {
            reason: "UFFD WP_ASYNC not available — required for e2b incremental snapshot".to_string(),
            remediation: Some(
                "kernel >= 6.7 or ANCK-6.6 with WP_ASYNC patch required. \
                 Use --variant=standard for basic firecracker, or --force to install e2b without incremental snapshot."
                    .to_string(),
            ),
        })
    }
}

/// Detect UFFD WP_ASYNC support via kernel version or /proc/kallsyms.
fn check_wp_async_support(facts: &EnvFacts) -> bool {
    // Check kernel >= 6.7
    if let Some(ref kernel) = facts.kernel
        && kernel_version_at_least(kernel, 6, 7)
    {
        return true;
    }

    // Check /proc/kallsyms for userfaultfd_wp_async symbol
    check_kallsyms_wp_async()
}

/// Grep /proc/kallsyms for the userfaultfd_wp_async symbol.
fn check_kallsyms_wp_async() -> bool {
    let kallsyms = Path::new("/proc/kallsyms");
    if !kallsyms.exists() {
        return false;
    }
    // Use grep for efficiency — kallsyms can be large
    Command::new("grep")
        .args(["-q", "userfaultfd_wp_async", "/proc/kallsyms"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Phase 2: Package installation for firecracker e2b.
fn e2b_packages(facts: &EnvFacts) -> Result<PhaseResult, SandboxInstallError> {
    if !dnf_command_exists() {
        return Err(SandboxInstallError::PackageFailed(
            "dnf binary not found in PATH; firecracker sandbox install requires dnf (yum-only environments are not supported)"
                .to_string(),
        ));
    }
    let pkg_mgr = detect_package_manager(facts.pkg_base.as_deref()).map_err(|e| {
        SandboxInstallError::PackageFailed(format!("cannot detect package manager: {e}"))
    })?;

    // RPM packaging note: the Anolisa firecracker repository ships fine-grained
    // sub-packages with `firecracker-e2b*` / `firecracker-standard*` prefixes. Both
    // variants share the variant-neutral data-only payload `firecracker-e2b-kernel`
    // (Guest vmlinux) and `firecracker-e2b-rootfs` (base.ext4) — the e2b control
    // plane (orchestrator / envd / system-config) does NOT pull them in, so anolisa
    // must request them explicitly here for the install to be self-sufficient.
    //
    // The e2b variant additionally needs the firecracker-e2b fork's auxiliary
    // binaries — `firecracker-e2b-busybox` (static busybox shipped into guest
    // rootfs at build time), `firecracker-e2b-jailer` (jailer wrapper used by
    // the orchestrator to confine each microVM) and `firecracker-e2b-tools`
    // (snapshot-editor / cpu-template-helper) — none of which are pulled in by
    // the main `firecracker-e2b` package, so list them explicitly.
    let required_packages: &[&str] = FIRECRACKER_E2B_PACKAGES;
    let mut to_install = Vec::new();

    for pkg in required_packages {
        if !pkg_mgr.is_installed(pkg) {
            to_install.push(*pkg);
        }
    }

    if !to_install.is_empty() {
        pkg_mgr.install(&to_install).map_err(|e| {
            SandboxInstallError::PackageFailed(format!(
                "failed to install {}: {e}",
                to_install.join(", ")
            ))
        })?;
    }

    let msg = if to_install.is_empty() {
        format!("already installed: {}", required_packages.join(", "))
    } else {
        format!("installed: {}", to_install.join(", "))
    };

    Ok(PhaseResult {
        phase: InstallPhase::Packages,
        status: PhaseStatus::Success,
        message: msg,
    })
}

/// Phase 3: OS-level configuration for firecracker e2b.
///
/// Writes sysctl, udev, sysusers, tmpfiles configs; loads kernel modules;
/// mounts HugePages.
fn e2b_os_primitives(
    layout: &FsLayout,
    request: &SandboxInstallRequest,
    warnings: &mut Vec<String>,
) -> Result<PhaseResult, SandboxInstallError> {
    let mut actions = Vec::new();
    let warnings_before = warnings.len();

    // 1. sysctl
    e2b_write_sysctl(&mut actions, warnings);

    // 2. sysusers (create the `e2b` user/group BEFORE udev so the rule below
    //    can safely reference GROUP="e2b" without a transient missing-group
    //    window during udevadm trigger).
    e2b_write_sysusers(&mut actions, warnings);

    // 3. udev rules
    e2b_write_udev(&mut actions, warnings);

    // 4. tmpfiles
    e2b_write_tmpfiles(&mut actions, warnings);

    // 5. Kernel modules
    e2b_load_modules(&mut actions, warnings);

    // 6. HugePages mount
    e2b_mount_hugepages(request, &mut actions, warnings);

    // 7. Default microVM assets (kernel + rootfs + vm-config.json)
    firecracker_provision_default_assets(layout, &mut actions, warnings);

    let msg = if actions.is_empty() {
        "no OS primitives configured".to_string()
    } else {
        actions.join("; ")
    };

    let had_warnings = warnings.len() > warnings_before;
    Ok(PhaseResult {
        phase: InstallPhase::OsPrimitives,
        status: if had_warnings {
            PhaseStatus::Warning
        } else {
            PhaseStatus::Success
        },
        message: msg,
    })
}

fn e2b_write_sysctl(actions: &mut Vec<String>, warnings: &mut Vec<String>) {
    let path = Path::new("/etc/sysctl.d/90-e2b.conf");
    let content = "\
# ANOLISA sandbox: e2b orchestrator sysctl settings
vm.nr_hugepages = 2048
net.ipv4.ip_forward = 1
net.ipv4.conf.all.rp_filter = 0
fs.file-max = 2097152
vm.unprivileged_userfaultfd = 0
";
    match write_config_file(path, content) {
        Ok(()) => {
            actions.push(format!("wrote {}", path.display()));
            // Apply sysctl
            let _ = Command::new("sysctl")
                .arg("--system")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            actions.push("sysctl --system".to_string());
        }
        Err(e) => {
            warnings.push(format!("could not write {}: {e}", path.display()));
        }
    }
}

fn e2b_write_udev(actions: &mut Vec<String>, warnings: &mut Vec<String>) {
    let path = Path::new("/etc/udev/rules.d/90-e2b-userfaultfd.rules");
    // Restrict /dev/userfaultfd to the `e2b` group rather than world (0666).
    // The `e2b` user/group is provisioned by `e2b_write_sysusers` which
    // runs immediately before this function. Service units that need raw
    // access to /dev/userfaultfd must run as `e2b` or list `e2b` in their
    // SupplementaryGroups=. This is consistent with `vm.unprivileged_userfaultfd=0`
    // in the sysctl block: only the `e2b` group obtains the privilege.
    let content = "\
# ANOLISA sandbox: e2b userfaultfd access
KERNEL==\"userfaultfd\", MODE=\"0660\", GROUP=\"e2b\"
";
    match write_config_file(path, content) {
        Ok(()) => {
            actions.push(format!("wrote {}", path.display()));
            let _ = Command::new("udevadm")
                .args(["control", "--reload-rules"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            actions.push("udevadm control --reload-rules".to_string());
        }
        Err(e) => {
            warnings.push(format!("could not write {}: {e}", path.display()));
        }
    }
}

fn e2b_write_sysusers(actions: &mut Vec<String>, warnings: &mut Vec<String>) {
    let path = Path::new("/usr/lib/sysusers.d/e2b.conf");
    let content = "\
# ANOLISA sandbox: e2b service account
u e2b - \"E2B Orchestrator\" /var/e2b
";
    match write_config_file(path, content) {
        Ok(()) => {
            actions.push(format!("wrote {}", path.display()));
            let _ = Command::new("systemd-sysusers")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            actions.push("systemd-sysusers".to_string());
        }
        Err(e) => {
            warnings.push(format!("could not write {}: {e}", path.display()));
        }
    }
}

fn e2b_write_tmpfiles(actions: &mut Vec<String>, warnings: &mut Vec<String>) {
    let path = Path::new("/usr/lib/tmpfiles.d/e2b.conf");
    let content = "\
# ANOLISA sandbox: e2b required directories
d /var/e2b 0755 e2b e2b -
d /var/e2b/storage 0755 e2b e2b -
d /var/e2b/storage/templates 0755 e2b e2b -
d /var/e2b/storage/orchestrator 0755 e2b e2b -
d /var/e2b/storage/logs 0755 e2b e2b -
";
    match write_config_file(path, content) {
        Ok(()) => {
            actions.push(format!("wrote {}", path.display()));
            let _ = Command::new("systemd-tmpfiles")
                .arg("--create")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            actions.push("systemd-tmpfiles --create".to_string());
        }
        Err(e) => {
            warnings.push(format!("could not write {}: {e}", path.display()));
        }
    }
}

fn e2b_load_modules(actions: &mut Vec<String>, warnings: &mut Vec<String>) {
    // Persist module loading
    let modules_conf = Path::new("/etc/modules-load.d/anolisa-sandbox-e2b.conf");
    let modules_content = "\
# ANOLISA sandbox: e2b required kernel modules
nbd
overlay
tun
erofs
";
    match write_config_file(modules_conf, modules_content) {
        Ok(()) => actions.push(format!("wrote {}", modules_conf.display())),
        Err(e) => warnings.push(format!("could not write {}: {e}", modules_conf.display())),
    }

    // nbd options
    let nbd_conf = Path::new("/etc/modprobe.d/e2b-nbd.conf");
    let nbd_content = "# ANOLISA sandbox: e2b nbd options\noptions nbd nbds_max=64\n";
    match write_config_file(nbd_conf, nbd_content) {
        Ok(()) => actions.push(format!("wrote {}", nbd_conf.display())),
        Err(e) => warnings.push(format!("could not write {}: {e}", nbd_conf.display())),
    }

    // Load modules now
    for module in &["nbd", "overlay", "tun", "erofs"] {
        let _ = Command::new("modprobe")
            .arg(module)
            .stderr(std::process::Stdio::null())
            .status();
    }
    actions.push("modprobe nbd overlay tun erofs".to_string());
}

fn e2b_mount_hugepages(
    request: &SandboxInstallRequest,
    actions: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    // Persistence boundary with the e2b-system-config RPM:
    //   * `e2b-system-config` provides the *runtime* sysctl knob
    //     (vm.nr_hugepages) but does NOT ship a systemd .mount unit for
    //     /mnt/hugepages — it relies on the orchestrator's caller (us)
    //     to mount hugetlbfs at the expected path.
    //   * Therefore ANOLISA owns the mount-unit lifecycle here. We write a
    //     dedicated `mnt-hugepages.mount` (escaped from /mnt/hugepages)
    //     and `systemctl enable --now` it so the mount survives reboot.
    //   * If a future e2b-system-config release adds its own
    //     mnt-hugepages.mount, the rollback path in
    //     [`cleanup_e2b_phase3_artifacts`] removes ours by absolute path,
    //     letting the RPM-shipped unit take over.
    let mount_point = Path::new("/mnt/hugepages");

    // Persist the mount via a systemd .mount unit so the hugetlbfs survives
    // reboot. The unit name is the escaped mount path: /mnt/hugepages -> mnt-hugepages.mount.
    let mount_unit_path = Path::new("/etc/systemd/system/mnt-hugepages.mount");
    let mount_unit_content = "\
# ANOLISA sandbox: persistent hugetlbfs for e2b
[Unit]
Description=ANOLISA sandbox hugetlbfs mount (/mnt/hugepages)
DefaultDependencies=no
Before=local-fs.target

[Mount]
What=hugetlbfs
Where=/mnt/hugepages
Type=hugetlbfs
Options=mode=01755

[Install]
WantedBy=local-fs.target
";
    match write_config_file(mount_unit_path, mount_unit_content) {
        Ok(()) => actions.push(format!("wrote {}", mount_unit_path.display())),
        Err(e) => {
            warnings.push(format!(
                "could not write {}: {e}",
                mount_unit_path.display()
            ));
        }
    }

    // Make sure the mount point exists before enabling the unit
    let _ = fs::create_dir_all(mount_point);

    // Reload systemd so the new unit is visible
    let _ = Command::new("systemctl")
        .arg("daemon-reload")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    // Check existing mount
    let already_mounted = Command::new("mountpoint")
        .arg("-q")
        .arg(mount_point)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if already_mounted && !request.force {
        warnings.push(
            "/mnt/hugepages already mounted; mnt-hugepages.mount written but not re-mounted (use --force to remount)"
                .to_string(),
        );
        // Still enable the unit so the mount persists across reboot.
        let _ = Command::new("systemctl")
            .args(["enable", "mnt-hugepages.mount"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        actions.push("systemctl enable mnt-hugepages.mount".to_string());
        return;
    }

    // Enable + start the mount unit (replaces the previous one-shot `mount` call
    // so the mount survives reboot).
    let status = Command::new("systemctl")
        .args(["enable", "--now", "mnt-hugepages.mount"])
        .stderr(std::process::Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => {
            actions.push("systemctl enable --now mnt-hugepages.mount".to_string());
        }
        _ => {
            warnings.push(
                "could not enable mnt-hugepages.mount; falling back to one-shot mount".to_string(),
            );
            // Fallback: one-shot mount so install can still proceed.
            let fallback = Command::new("mount")
                .args(["-t", "hugetlbfs", "nodev", "/mnt/hugepages"])
                .stderr(std::process::Stdio::null())
                .status();
            if matches!(fallback, Ok(ref s) if s.success()) {
                actions.push("mount -t hugetlbfs nodev /mnt/hugepages (fallback)".to_string());
            } else {
                warnings.push("could not mount hugetlbfs at /mnt/hugepages".to_string());
            }
        }
    }
}

/// Phase 4: Service setup for e2b (enable orchestrator).
fn e2b_service() -> Result<PhaseResult, SandboxInstallError> {
    match anolisa_platform::systemd::enable_unit("e2b-orchestrator.service") {
        Ok(()) => Ok(PhaseResult {
            phase: InstallPhase::ServiceSetup,
            status: PhaseStatus::Success,
            message: "enabled e2b-orchestrator.service".to_string(),
        }),
        Err(e) => Err(SandboxInstallError::ServiceFailed(format!(
            "failed to enable e2b-orchestrator.service: {e}"
        ))),
    }
}

/// Phase 5: Post-install verification for e2b.
fn e2b_verify(
    warnings: &mut Vec<String>,
    installed_version: &mut Option<String>,
) -> Result<PhaseResult, SandboxInstallError> {
    let warnings_before = warnings.len();

    // Check firecracker version (expect e2b fork v1.14.x)
    let fc_version = run_version_command("firecracker");
    let mut msg_parts = Vec::new();

    match &fc_version {
        Some(version) => {
            *installed_version = parse_firecracker_version(version);
            msg_parts.push(format!("firecracker {version}"));
        }
        None => {
            return Err(SandboxInstallError::VerifyFailed(
                "firecracker --version returned non-zero or not in PATH".to_string(),
            ));
        }
    }

    // gRPC health check on orchestrator :5008
    let health_ok = e2b_orchestrator_health_check();
    if health_ok {
        msg_parts.push("orchestrator :5008 healthy".to_string());
    } else {
        // Retry once after a brief wait (service may still be starting)
        std::thread::sleep(std::time::Duration::from_secs(5));
        if e2b_orchestrator_health_check() {
            msg_parts.push("orchestrator :5008 healthy (after retry)".to_string());
        } else {
            warnings.push(
                "e2b-orchestrator gRPC :5008 health check failed; service may still be starting"
                    .to_string(),
            );
            msg_parts.push("orchestrator :5008 unreachable (warning)".to_string());
        }
    }

    // Determine status based on whether THIS phase added warnings
    let status = if warnings.len() > warnings_before {
        PhaseStatus::Warning
    } else {
        PhaseStatus::Success
    };

    Ok(PhaseResult {
        phase: InstallPhase::PostVerify,
        status,
        message: msg_parts.join(", "),
    })
}

/// Check e2b-orchestrator gRPC health endpoint.
fn e2b_orchestrator_health_check() -> bool {
    // Try curl first (more commonly available)
    let curl_ok = Command::new("curl")
        .args(["-sf", "--max-time", "3", "http://127.0.0.1:5008/health"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if curl_ok {
        return true;
    }

    // Fallback: grpcurl
    Command::new("grpcurl")
        .args([
            "-plaintext",
            "127.0.0.1:5008",
            "grpc.health.v1.Health/Check",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Write a config file, creating parent directories if needed.
fn write_config_file(path: &Path, content: &str) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)
}

/// Default microVM asset locations.
///
/// `firecracker-kernel` ships vmlinux at `KERNEL_SRC` and `firecracker-rootfs`
/// ships base.ext4 at `ROOTFS_SRC`. We mirror them under the per-layout
/// `default_dir` (see [`fc_default_dir`]) via stable symlinks plus a runnable
/// `vm-config.json`, so the user can launch a microVM directly with:
///
/// ```sh
/// firecracker --config-file <state_dir>/firecracker/default/vm-config.json
/// ```
///
/// `FC_DEFAULT_DIR_DEFAULT` is the *FHS / system-mode* path used **only** in
/// dry-run text and documentation. The actual on-disk path is derived from
/// the per-install [`FsLayout::state_dir`] via [`fc_default_dir`] so it tracks
/// `--prefix` correctly.
const FC_DEFAULT_DIR_DEFAULT: &str = "/var/lib/anolisa/firecracker/default";
const FC_DEFAULT_KERNEL_SRC: &str = "/usr/share/firecracker/kernel/vmlinux.bin";
const FC_DEFAULT_ROOTFS_SRC: &str = "/usr/share/firecracker/rootfs/base.ext4";

/// Resolve the default microVM asset directory for the given layout.
///
/// In system mode with the default prefix this is
/// `/var/lib/anolisa/firecracker/default`. With a custom `--prefix` it is
/// rebased so a single host can stage multiple ANOLISA-managed FC instances.
fn fc_default_dir(layout: &FsLayout) -> std::path::PathBuf {
    layout.state_dir.join("firecracker").join("default")
}

/// Render the default vm-config.json with absolute asset paths embedded.
///
/// firecracker reads paths verbatim from the config (no relative-path
/// resolution against the config file's directory), so we must embed the
/// real on-disk paths — the same `default_dir` we just provisioned.
fn fc_default_vm_config_json(default_dir: &Path) -> String {
    format!(
        r#"{{
  "boot-source": {{
    "kernel_image_path": "{dir}/vmlinux",
    "boot_args": "console=ttyS0 reboot=k panic=1 pci=off"
  }},
  "drives": [
    {{
      "drive_id": "rootfs",
      "path_on_host": "{dir}/rootfs.ext4",
      "is_root_device": true,
      "is_read_only": false
    }}
  ],
  "machine-config": {{
    "vcpu_count": 1,
    "mem_size_mib": 512,
    "smt": false
  }}
}}
"#,
        dir = default_dir.display()
    )
}

/// Provision a default microVM config under [`fc_default_dir(layout)`].
///
/// Creates the directory, symlinks vmlinux + rootfs.ext4 from the shipped
/// `firecracker-kernel` / `firecracker-rootfs` payloads, and writes a runnable
/// `vm-config.json`. Each step is best-effort: failures degrade to warnings
/// without aborting the install (binaries are still usable manually).
fn firecracker_provision_default_assets(
    layout: &FsLayout,
    actions: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let default_dir = fc_default_dir(layout);
    if let Err(e) = fs::create_dir_all(&default_dir) {
        warnings.push(format!(
            "could not create default microVM dir {}: {e}",
            default_dir.display()
        ));
        return;
    }

    let kernel_src = Path::new(FC_DEFAULT_KERNEL_SRC);
    if !kernel_src.exists() {
        warnings.push(format!(
            "default kernel {} missing; install firecracker-kernel",
            kernel_src.display()
        ));
        return;
    }
    let rootfs_src = Path::new(FC_DEFAULT_ROOTFS_SRC);
    if !rootfs_src.exists() {
        warnings.push(format!(
            "default rootfs {} missing; install firecracker-rootfs",
            rootfs_src.display()
        ));
        return;
    }

    let vmlinux_link = default_dir.join("vmlinux");
    let rootfs_link = default_dir.join("rootfs.ext4");

    #[cfg(unix)]
    {
        // Refresh symlinks so that re-installs always point at the latest payloads.
        let _ = fs::remove_file(&vmlinux_link);
        let _ = fs::remove_file(&rootfs_link);
        if let Err(e) = std::os::unix::fs::symlink(kernel_src, &vmlinux_link) {
            warnings.push(format!(
                "could not symlink {} -> {}: {e}",
                vmlinux_link.display(),
                kernel_src.display()
            ));
            return;
        }
        if let Err(e) = std::os::unix::fs::symlink(rootfs_src, &rootfs_link) {
            warnings.push(format!(
                "could not symlink {} -> {}: {e}",
                rootfs_link.display(),
                rootfs_src.display()
            ));
            return;
        }
    }

    let config_path = default_dir.join("vm-config.json");
    let config_content = fc_default_vm_config_json(&default_dir);
    match fs::write(&config_path, &config_content) {
        Ok(()) => {
            actions.push(format!(
                "provisioned default microVM at {} (vmlinux, rootfs.ext4, vm-config.json)",
                default_dir.display()
            ));
        }
        Err(e) => {
            warnings.push(format!("could not write {}: {e}", config_path.display()));
        }
    }
}

// ===========================================================================
// gVisor backend (standalone / shim / docker / + substrate data-plane)
// ===========================================================================

/// Dispatch entry for gVisor install — routes to the correct pipeline based on
/// `--runtime` and `--control-panel` flags.
fn run_gvisor(
    request: &SandboxInstallRequest,
    env_facts: &EnvFacts,
    phases: &mut Vec<PhaseResult>,
    warnings: &mut Vec<String>,
    installed_version: &mut Option<String>,
) -> Result<(), SandboxInstallError> {
    // --- Phase 1: Pre-flight ---
    let preflight = gvisor_preflight(request, env_facts)?;
    phases.push(preflight);

    // --- Phase 2: Packages ---
    let packages = gvisor_packages(request, env_facts)?;
    phases.push(packages);

    // --- Phase 3: OS Primitives ---
    let os_config = gvisor_os_primitives(request, warnings)?;
    phases.push(os_config);

    // --- Phase 4: Service (gVisor has no persistent service) ---
    phases.push(PhaseResult {
        phase: InstallPhase::ServiceSetup,
        status: PhaseStatus::Skipped,
        message: "skipped: runsc is on-demand; containerd manages shim lifecycle".to_string(),
    });

    // --- Phase 5: Post-verify ---
    if !request.no_verify {
        let verify = gvisor_verify(request, warnings, installed_version)?;
        phases.push(verify);
    } else {
        phases.push(PhaseResult {
            phase: InstallPhase::PostVerify,
            status: PhaseStatus::Skipped,
            message: "skipped by --no-verify".to_string(),
        });
    }

    Ok(())
}

/// Phase 1: Pre-flight for gVisor.
fn gvisor_preflight(
    request: &SandboxInstallRequest,
    facts: &EnvFacts,
) -> Result<PhaseResult, SandboxInstallError> {
    // Check OS
    if facts.os != "linux" {
        return Err(SandboxInstallError::EnvNotSatisfied {
            reason: format!("gvisor requires Linux, got '{}'", facts.os),
            remediation: None,
        });
    }

    // Check arch (gVisor primarily targets x86_64; aarch64 is experimental)
    if facts.arch != "x86_64" && facts.arch != "aarch64" {
        return Err(SandboxInstallError::EnvNotSatisfied {
            reason: format!("gvisor requires x86_64 or aarch64, got '{}'", facts.arch),
            remediation: None,
        });
    }

    // Kernel >= 4.15 required
    if let Some(ref kernel) = facts.kernel
        && !kernel_version_at_least(kernel, 4, 15)
    {
        return Err(SandboxInstallError::EnvNotSatisfied {
            reason: format!("gvisor requires kernel >= 4.15, got '{kernel}'"),
            remediation: Some("upgrade your kernel to >= 4.15".to_string()),
        });
    }

    // ptrace_scope check (Yama LSM); gVisor needs ptrace_scope <= 2
    let ptrace_ok = check_ptrace_scope();
    if !ptrace_ok {
        return Err(SandboxInstallError::EnvNotSatisfied {
            reason: "kernel.yama.ptrace_scope > 2; gVisor systrap platform requires <= 2"
                .to_string(),
            remediation: Some(
                "echo 1 > /proc/sys/kernel/yama/ptrace_scope, or set kernel.yama.ptrace_scope=1 in sysctl"
                    .to_string(),
            ),
        });
    }

    // If shim mode, check containerd is active
    let mut extra_info = Vec::new();
    if request.runtime.as_deref() == Some("containerd")
        || request.control_panel.as_deref() == Some("substrate")
    {
        let ctd_active = is_service_active("containerd.service");
        if !ctd_active {
            return Err(SandboxInstallError::EnvNotSatisfied {
                reason: "containerd.service not active; shim mode requires a running containerd"
                    .to_string(),
                remediation: Some(
                    "install and start containerd first: anolisa osbase sandbox install container"
                        .to_string(),
                ),
            });
        }
        extra_info.push("containerd=active".to_string());
    }

    let kernel_str = facts.kernel.as_deref().unwrap_or("unknown");
    let msg = if extra_info.is_empty() {
        format!("kernel={kernel_str}, arch={}, ptrace_scope=ok", facts.arch)
    } else {
        format!(
            "kernel={kernel_str}, arch={}, ptrace_scope=ok, {}",
            facts.arch,
            extra_info.join(", ")
        )
    };

    Ok(PhaseResult {
        phase: InstallPhase::Preflight,
        status: PhaseStatus::Success,
        message: msg,
    })
}

/// Check Yama ptrace_scope <= 2.
fn check_ptrace_scope() -> bool {
    let path = Path::new("/proc/sys/kernel/yama/ptrace_scope");
    if !path.exists() {
        // No Yama LSM -> unrestricted, OK
        return true;
    }
    match fs::read_to_string(path) {
        Ok(content) => {
            let val: u32 = content.trim().parse().unwrap_or(0);
            val <= 2
        }
        Err(_) => true, // Cannot read -> assume OK
    }
}

/// Check whether a systemd service unit is currently active.
fn is_service_active(unit: &str) -> bool {
    Command::new("systemctl")
        .args(["is-active", "--quiet", unit])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Phase 2: Package installation for gVisor.
fn gvisor_packages(
    request: &SandboxInstallRequest,
    facts: &EnvFacts,
) -> Result<PhaseResult, SandboxInstallError> {
    if !dnf_command_exists() {
        return Err(SandboxInstallError::PackageFailed(
            "dnf binary not found in PATH; gvisor sandbox install requires dnf".to_string(),
        ));
    }
    let pkg_mgr = detect_package_manager(facts.pkg_base.as_deref()).map_err(|e| {
        SandboxInstallError::PackageFailed(format!("cannot detect package manager: {e}"))
    })?;

    let required_packages: &[&str] =
        match (request.runtime.as_deref(), request.control_panel.as_deref()) {
            (_, Some("substrate")) => GVISOR_SUBSTRATE_PACKAGES,
            (Some("containerd"), _) => GVISOR_SHIM_PACKAGES,
            (Some("docker"), _) => GVISOR_DOCKER_PACKAGES,
            _ => GVISOR_STANDALONE_PACKAGES,
        };

    let mut to_install = Vec::new();
    for pkg in required_packages {
        if !pkg_mgr.is_installed(pkg) {
            to_install.push(*pkg);
        }
    }

    // Pre-flight: probe each not-yet-installed package via `dnf repoquery` to
    // detect the case where the ANOLISA sandbox repo is missing/unconfigured.
    // We only block when every probe is conclusive AND at least one comes back
    // false — otherwise we let `dnf install` run and surface its own error
    // (avoids spurious failures on hosts where repoquery is restricted).
    if !to_install.is_empty() {
        let mut missing_in_repo: Vec<&str> = Vec::new();
        for pkg in &to_install {
            if let Some(false) = dnf_repoquery_available(pkg) {
                missing_in_repo.push(*pkg);
            }
        }
        if !missing_in_repo.is_empty() {
            return Err(SandboxInstallError::PackageFailed(
                gvisor_missing_rpm_error(&missing_in_repo, required_packages),
            ));
        }
    }

    if !to_install.is_empty() {
        pkg_mgr.install(&to_install).map_err(|e| {
            // Wrap the raw dnf error with the same actionable hint as the
            // pre-flight path so operators get a consistent remediation
            // regardless of which leg failed (e.g. repoquery succeeded but
            // install hit a checksum/signature issue).
            SandboxInstallError::PackageFailed(format!(
                "failed to install {}: {e}. If the failure is `No package \
                 named ... available`, the ANOLISA sandbox RPM repo (`{}`) \
                 is likely missing or unconfigured — see {}.",
                to_install.join(", "),
                ANOLISA_SANDBOX_REPO_ID,
                SANDBOX_RPM_PACKAGING_DOC,
            ))
        })?;
    }

    let msg = if to_install.is_empty() {
        format!("already installed: {}", required_packages.join(", "))
    } else {
        format!("installed: {}", to_install.join(", "))
    };

    Ok(PhaseResult {
        phase: InstallPhase::Packages,
        status: PhaseStatus::Success,
        message: msg,
    })
}

/// Phase 3: OS-level configuration for gVisor.
fn gvisor_os_primitives(
    request: &SandboxInstallRequest,
    warnings: &mut Vec<String>,
) -> Result<PhaseResult, SandboxInstallError> {
    let mut actions = Vec::new();
    let warnings_before = warnings.len();

    // 1. Write /etc/runsc/config.toml (platform=systrap)
    gvisor_write_runsc_config(request, &mut actions, warnings);

    // 2. If shim mode, register containerd runtime handler
    if request.runtime.as_deref() == Some("containerd")
        || request.control_panel.as_deref() == Some("substrate")
    {
        gvisor_register_containerd_handler(&mut actions, warnings);
    }

    // 3. If docker mode, register in daemon.json
    if request.runtime.as_deref() == Some("docker") {
        gvisor_register_docker_runtime(&mut actions, warnings);
    }

    // 4. If substrate, provision data-plane directories and config
    if request.control_panel.as_deref() == Some("substrate") {
        gvisor_provision_substrate_dataplane(&mut actions, warnings);
    }

    let msg = if actions.is_empty() {
        "no OS primitives configured".to_string()
    } else {
        actions.join("; ")
    };

    let had_warnings = warnings.len() > warnings_before;
    Ok(PhaseResult {
        phase: InstallPhase::OsPrimitives,
        status: if had_warnings {
            PhaseStatus::Warning
        } else {
            PhaseStatus::Success
        },
        message: msg,
    })
}

/// Write /etc/runsc/config.toml with platform=systrap and checkpoint/restore
/// flags enabled.
fn gvisor_write_runsc_config(
    request: &SandboxInstallRequest,
    actions: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let path = Path::new("/etc/runsc/config.toml");
    let checkpoint_line = if request.control_panel.as_deref() == Some("substrate") {
        "checkpoint = true\nrestore = true\n"
    } else {
        "# checkpoint/restore available but not force-enabled\n"
    };
    let content = format!(
        "# ANOLISA sandbox: gVisor runsc configuration\n\
         [runsc]\n\
         platform = \"systrap\"\n\
         network = \"sandbox\"\n\
         {checkpoint_line}"
    );
    match write_config_file(path, &content) {
        Ok(()) => actions.push(format!("wrote {}", path.display())),
        Err(e) => warnings.push(format!("could not write {}: {e}", path.display())),
    }
}

/// Register runsc as a containerd runtime handler.
///
/// containerd does NOT auto-import drop-in TOML fragments under
/// `/etc/containerd/`; the runtime entry must live in the main
/// `/etc/containerd/config.toml` (or be referenced via an explicit
/// `imports = [...]` line at the top of it). We therefore patch the main
/// config in place.
///
/// Behavior:
///   1. If the file is missing, generate the default with
///      `containerd config default` (the canonical way to bootstrap);
///      fall back to a minimal stub if `containerd` is not on PATH.
///   2. Idempotent: parse the existing file via `toml_edit` and only mutate
///      the `runsc` runtime sub-table when needed; if the desired
///      `runtime_type` / `options` are already set, skip the write.
///   3. AST-level merge (not string append): preserves user comments and
///      ordering, and crucially avoids producing duplicate plugin tables —
///      containerd 2.x parses TOML strictly and will refuse to start if the
///      same plugin table appears twice.
///   4. Restart containerd so the handler is picked up.
fn gvisor_register_containerd_handler(actions: &mut Vec<String>, warnings: &mut Vec<String>) {
    let cfg_path = Path::new("/etc/containerd/config.toml");

    // Step 1: ensure config.toml exists. Prefer `containerd config default`.
    if !cfg_path.exists() {
        let generated = Command::new("containerd")
            .args(["config", "default"])
            .output();
        let default_content = match generated {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
            _ => {
                // Minimal fallback so the merge below has a sane base.
                String::from(
                    "# generated by ANOLISA: minimal containerd config (containerd not on PATH)\nversion = 2\n\n[plugins.\"io.containerd.grpc.v1.cri\"]\n",
                )
            }
        };
        if let Err(e) = write_config_file(cfg_path, &default_content) {
            warnings.push(format!("could not create {}: {e}", cfg_path.display()));
            return;
        }
        actions.push(format!(
            "created {} (containerd default)",
            cfg_path.display()
        ));
    }

    // Step 2: read + parse via toml_edit (preserves comments & order).
    let existing = match fs::read_to_string(cfg_path) {
        Ok(s) => s,
        Err(e) => {
            warnings.push(format!("could not read {}: {e}", cfg_path.display()));
            return;
        }
    };
    let mut doc = match existing.parse::<toml_edit::DocumentMut>() {
        Ok(d) => d,
        Err(e) => {
            // Fail-fast: refusing to overwrite an unparseable config is safer
            // than silently truncating the operator's config.toml.
            warnings.push(format!(
                "refusing to patch {}: file is not valid TOML ({e}). Fix the \
                 file by hand or back it up and re-run; ANOLISA will not \
                 overwrite an unparseable containerd config.",
                cfg_path.display(),
            ));
            return;
        }
    };

    // Step 3: merge `runsc` runtime sub-table at the canonical path:
    //   plugins."io.containerd.grpc.v1.cri".containerd.runtimes.runsc
    // We descend through dotted-key tables so we don't clobber siblings
    // (other runtimes, snapshotter config, etc.).
    let changed = ensure_containerd_runsc_runtime(&mut doc);

    if !changed {
        actions.push(format!(
            "{} already registers io.containerd.runsc.v1",
            cfg_path.display()
        ));
    } else {
        let merged = doc.to_string();
        if let Err(e) = write_config_file(cfg_path, &merged) {
            warnings.push(format!("could not patch {}: {e}", cfg_path.display()));
            return;
        }
        actions.push(format!(
            "patched {} (registered io.containerd.runsc.v1 via toml_edit AST merge)",
            cfg_path.display()
        ));
    }

    // Step 4: restart containerd so it picks up the handler.
    let _ = Command::new("systemctl")
        .args(["restart", "containerd.service"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    actions.push("systemctl restart containerd.service".to_string());
}

/// Ensure the `plugins."io.containerd.grpc.v1.cri".containerd.runtimes.runsc`
/// sub-table contains the canonical runsc registration. Returns `true` if the
/// document was modified (caller must re-serialize and write back), `false`
/// if the desired state is already present.
///
/// We descend table-by-table rather than calling `doc["a.b.c"]` so dotted
/// path semantics are unambiguous regardless of how the user wrote their
/// existing config (inline tables, dotted keys, or nested `[[..]]` headers).
fn ensure_containerd_runsc_runtime(doc: &mut toml_edit::DocumentMut) -> bool {
    use toml_edit::{Item, Table, value};

    fn descend<'a>(parent: &'a mut Table, key: &str) -> &'a mut Table {
        let entry = parent
            .entry(key)
            .or_insert_with(|| Item::Table(Table::new()));
        if !entry.is_table() {
            // Replace non-table (None / Value / ArrayOfTables) with an empty
            // table so the merge can proceed without panicking.
            *entry = Item::Table(Table::new());
        }
        entry
            .as_table_mut()
            .expect("entry was just normalized to a table")
    }

    let root = doc.as_table_mut();
    let plugins = descend(root, "plugins");
    let cri = descend(plugins, "io.containerd.grpc.v1.cri");
    let containerd_tbl = descend(cri, "containerd");
    let runtimes = descend(containerd_tbl, "runtimes");
    let runsc = descend(runtimes, "runsc");

    let mut changed = false;

    // runtime_type = "io.containerd.runsc.v1"
    let want_type = "io.containerd.runsc.v1";
    let type_ok = runsc
        .get("runtime_type")
        .and_then(|i| i.as_str())
        .map(|s| s == want_type)
        .unwrap_or(false);
    if !type_ok {
        runsc["runtime_type"] = value(want_type);
        changed = true;
    }

    // [..runtimes.runsc.options]
    //   TypeUrl    = "io.containerd.runsc.v1.options"
    //   ConfigPath = "/etc/runsc/config.toml"
    let options = descend(runsc, "options");
    for (k, want) in [
        ("TypeUrl", "io.containerd.runsc.v1.options"),
        ("ConfigPath", "/etc/runsc/config.toml"),
    ] {
        let ok = options
            .get(k)
            .and_then(|i| i.as_str())
            .map(|s| s == want)
            .unwrap_or(false);
        if !ok {
            options[k] = value(want);
            changed = true;
        }
    }

    changed
}

/// Register runsc in Docker daemon.json.
///
/// Uses `serde_json::Value` to merge into the existing config so we:
///   * preserve unrelated keys (`log-driver`, `data-root`, ...) byte-for-byte
///     after pretty-printing,
///   * never produce duplicate keys (the previous string-concat path could),
///   * never silently overwrite an invalid daemon.json (we fail-fast instead
///     — a corrupted daemon.json takes the whole Docker daemon down, which
///     is much worse than refusing to install).
fn gvisor_register_docker_runtime(actions: &mut Vec<String>, warnings: &mut Vec<String>) {
    use serde_json::{Map, Value, json};

    let daemon_json = Path::new("/etc/docker/daemon.json");
    let want_path = "/usr/bin/runsc";

    // Step 1: read existing config (if any).
    let existing = if daemon_json.exists() {
        match fs::read_to_string(daemon_json) {
            Ok(s) => s,
            Err(e) => {
                warnings.push(format!("could not read {}: {e}", daemon_json.display()));
                return;
            }
        }
    } else {
        String::new()
    };

    // Step 2: parse (or start fresh). An empty / whitespace-only file is
    // treated as `{}`; anything else that fails to parse is surfaced as a
    // hard warning — we will NOT overwrite a non-empty unparseable file
    // because that risks taking down the user's Docker daemon.
    let mut root: Value = if existing.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        match serde_json::from_str(&existing) {
            Ok(v @ Value::Object(_)) => v,
            Ok(_) => {
                warnings.push(format!(
                    "refusing to patch {}: top-level value is not a JSON object. \
                     Fix the file by hand and re-run.",
                    daemon_json.display(),
                ));
                return;
            }
            Err(e) => {
                warnings.push(format!(
                    "refusing to patch {}: file is not valid JSON ({e}). \
                     ANOLISA will not overwrite an unparseable daemon.json \
                     because a broken file takes the whole Docker daemon \
                     down. Fix the file by hand or back it up and re-run.",
                    daemon_json.display(),
                ));
                return;
            }
        }
    };

    // Step 3: structured merge — only touch runtimes.runsc.path.
    let runtimes = root
        .as_object_mut()
        .expect("root verified as object above")
        .entry("runtimes".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !runtimes.is_object() {
        warnings.push(format!(
            "refusing to patch {}: existing `runtimes` field is not a JSON object",
            daemon_json.display(),
        ));
        return;
    }
    let runtimes_map = runtimes.as_object_mut().unwrap();

    // Idempotency: if runsc.path is already set to /usr/bin/runsc, skip write.
    if let Some(Value::Object(existing_runsc)) = runtimes_map.get("runsc") {
        if existing_runsc.get("path").and_then(Value::as_str) == Some(want_path) {
            actions.push(format!(
                "{} already registers runsc → {}",
                daemon_json.display(),
                want_path,
            ));
            return;
        }
    }
    runtimes_map.insert("runsc".to_string(), json!({ "path": want_path }));

    // Step 4: serialize (pretty so operators can still diff the file by eye).
    let serialized = match serde_json::to_string_pretty(&root) {
        Ok(s) => s + "\n",
        Err(e) => {
            warnings.push(format!("could not serialize merged daemon.json: {e}"));
            return;
        }
    };

    match write_config_file(daemon_json, &serialized) {
        Ok(()) => {
            actions.push(format!(
                "wrote {} (added runsc runtime via serde_json merge)",
                daemon_json.display()
            ));
            let _ = Command::new("systemctl")
                .args(["restart", "docker.service"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            actions.push("systemctl restart docker.service".to_string());
        }
        Err(e) => warnings.push(format!("could not write {}: {e}", daemon_json.display())),
    }
}

/// Provision Substrate data-plane directories, TLS placeholders, and
/// node-config.yaml. Pure local preparation — does NOT connect to cluster.
fn gvisor_provision_substrate_dataplane(actions: &mut Vec<String>, warnings: &mut Vec<String>) {
    // Create directory structure
    let dirs: &[&str] = &[
        "/var/lib/substrate/checkpoints",
        "/var/lib/substrate/state",
        "/etc/substrate/config",
        "/etc/substrate/tls",
    ];
    for dir in dirs {
        match fs::create_dir_all(dir) {
            Ok(()) => {}
            Err(e) => warnings.push(format!("could not create {dir}: {e}")),
        }
    }
    actions.push(
        "mkdir -p /var/lib/substrate/{checkpoints,state} /etc/substrate/{config,tls}".to_string(),
    );

    // TLS certificate placeholders
    let tls_placeholder = Path::new("/etc/substrate/tls/README");
    let tls_content = "# ANOLISA sandbox: TLS certificates placeholder\n# cert-manager will provision real certificates after kubeadm join.\n";
    let _ = write_config_file(tls_placeholder, tls_content);

    // node-config.yaml
    let node_config_path = Path::new("/etc/substrate/config/node-config.yaml");
    let node_config = "\
# ANOLISA sandbox: Substrate node configuration\n\
# This file is pre-provisioned; atelet reads it after kubeadm join.\n\
apiVersion: substrate.google.com/v1alpha1\n\
kind: NodeConfig\n\
metadata:\n\
  name: local-node\n\
spec:\n\
  runtime: gvisor\n\
  platform: systrap\n\
  checkpoint:\n\
    enabled: true\n\
    path: /var/lib/substrate/checkpoints\n\
  statePath: /var/lib/substrate/state\n\
";
    match write_config_file(node_config_path, node_config) {
        Ok(()) => actions.push(format!("wrote {}", node_config_path.display())),
        Err(e) => warnings.push(format!(
            "could not write {}: {e}",
            node_config_path.display()
        )),
    }
}

/// Phase 5: Post-install verification for gVisor.
fn gvisor_verify(
    request: &SandboxInstallRequest,
    warnings: &mut Vec<String>,
    installed_version: &mut Option<String>,
) -> Result<PhaseResult, SandboxInstallError> {
    let mut msg_parts = Vec::new();
    let warnings_before = warnings.len();

    // runsc --version
    let runsc_ver = run_version_command("runsc");
    match &runsc_ver {
        Some(version) => {
            *installed_version = Some(version.clone());
            msg_parts.push(format!("runsc {version}"));
        }
        None => {
            return Err(SandboxInstallError::VerifyFailed(
                "runsc --version returned non-zero or not in PATH".to_string(),
            ));
        }
    }

    // runsc do /bin/true — verify syscall interception
    let do_check = Command::new("runsc")
        .args(["do", "/bin/true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match do_check {
        Ok(s) if s.success() => msg_parts.push("runsc do /bin/true ok".to_string()),
        _ => {
            warnings
                .push("runsc do /bin/true failed; syscall interception may not work".to_string());
        }
    }

    // runsc help checkpoint (verify subcommand is registered).
    // NOTE: do NOT use `runsc checkpoint --help` here. gVisor uses the
    // google/subcommands framework whose subcommand SetFlags only registers
    // the command's own flags (image-path, leave-running, ...); --help is not
    // a registered flag, so stdlib flag parsing returns an error and the
    // process exits with subcommands.ExitUsageError (non-zero) — even though
    // the usage text is printed. Use `runsc help <cmd>` instead, which is the
    // framework's built-in introspection path and exits 0 on success.
    let ckpt_check = Command::new("runsc")
        .args(["help", "checkpoint"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match ckpt_check {
        Ok(s) if s.success() => msg_parts.push("checkpoint capability present".to_string()),
        _ => {
            warnings.push(
                "runsc help checkpoint failed; checkpoint/restore may not be available".to_string(),
            );
        }
    }

    // Shim-mode: verify the containerd runsc handler is registered.
    //
    // NOTE: do NOT use `ctr run --runtime io.containerd.runsc.v1 ...` here.
    // That used to be the check, but it pulls docker.io/library/busybox over
    // the network (often unreachable from CN ECS without a mirror) and also
    // requires the containerd daemon to be running. Failing either of those
    // unrelated preconditions is *not* evidence that the handler is
    // mis-registered, yet it produced a misleading DEGRADED warning.
    //
    // Instead, we check two static, network-free facts that together prove
    // the handler is wired in:
    //   1. the shim binary `containerd-shim-runsc-v1` exists in /usr/bin
    //      (installed by the containerd-shim-runsc-v1 RPM);
    //   2. /etc/containerd/config.toml contains the runtime_type line
    //      `io.containerd.runsc.v1` (written by our install path).
    if request.runtime.as_deref() == Some("containerd")
        || request.control_panel.as_deref() == Some("substrate")
    {
        let shim_path = Path::new("/usr/bin/containerd-shim-runsc-v1");
        let cfg_path = Path::new("/etc/containerd/config.toml");
        let shim_ok = shim_path.exists();
        let cfg_ok = std::fs::read_to_string(cfg_path)
            .map(|s| s.contains("io.containerd.runsc.v1"))
            .unwrap_or(false);
        match (shim_ok, cfg_ok) {
            (true, true) => msg_parts.push("runsc containerd handler registered".to_string()),
            (false, _) => warnings.push(
                "/usr/bin/containerd-shim-runsc-v1 missing; runsc shim not installed".to_string(),
            ),
            (true, false) => warnings.push(
                "/etc/containerd/config.toml missing io.containerd.runsc.v1 runtime entry"
                    .to_string(),
            ),
        }
    }

    // Substrate: verify binaries + directories
    if request.control_panel.as_deref() == Some("substrate") {
        for bin in &["/usr/local/bin/atelet", "/usr/local/bin/ateom-gvisor"] {
            if !Path::new(bin).exists() {
                warnings.push(format!("{bin} not found after install"));
            }
        }
        for dir in &["/var/lib/substrate", "/etc/substrate"] {
            if !Path::new(dir).is_dir() {
                warnings.push(format!("{dir} directory missing"));
            }
        }
        msg_parts.push("substrate data-plane verified".to_string());
    }

    let status = if warnings.len() > warnings_before {
        PhaseStatus::Warning
    } else {
        PhaseStatus::Success
    };

    Ok(PhaseResult {
        phase: InstallPhase::PostVerify,
        status,
        message: msg_parts.join(", "),
    })
}

// ===========================================================================
// Dry-run plan builder
// ===========================================================================

/// Build a dry-run plan describing what would happen.
pub fn build_dry_run_plan(request: &SandboxInstallRequest) -> SandboxInstallDryRun {
    let phases = match request.backend {
        SandboxBackendKind::Firecracker => firecracker_dry_run_phases(&request.variant),
        SandboxBackendKind::Gvisor => {
            gvisor_dry_run_phases(request.runtime.as_deref(), request.control_panel.as_deref())
        }
        _ => vec![DryRunPhase {
            phase: InstallPhase::Preflight,
            actions: vec![format!(
                "backend '{}' variant '{}' not yet implemented",
                request.backend, request.variant
            )],
        }],
    };

    SandboxInstallDryRun {
        backend: request.backend.to_string(),
        variant: request.variant.clone(),
        phases,
    }
}

fn firecracker_dry_run_phases(variant: &str) -> Vec<DryRunPhase> {
    match variant {
        "standard" | "default" => vec![
            DryRunPhase {
                phase: InstallPhase::Preflight,
                actions: vec![
                    "Check /dev/kvm exists and is accessible".to_string(),
                    "Check kernel >= 4.14".to_string(),
                    "Check arch is x86_64 or aarch64".to_string(),
                ],
            },
            DryRunPhase {
                phase: InstallPhase::Packages,
                actions: vec![format!(
                    "dnf install: {}",
                    FIRECRACKER_STANDARD_PACKAGES.join(", ")
                )],
            },
            DryRunPhase {
                phase: InstallPhase::OsPrimitives,
                actions: vec![
                    "Write /etc/modules-load.d/anolisa-sandbox-fc.conf (kvm_intel, kvm_amd)".to_string(),
                    "modprobe kvm_intel kvm_amd".to_string(),
                    format!(
                        "Provision {}/{{vmlinux,rootfs.ext4,vm-config.json}}",
                        FC_DEFAULT_DIR_DEFAULT
                    ),
                ],
            },
            DryRunPhase {
                phase: InstallPhase::ServiceSetup,
                actions: vec!["(none \u{2014} on-demand usage)".to_string()],
            },
            DryRunPhase {
                phase: InstallPhase::PostVerify,
                actions: vec!["firecracker --version".to_string(), "jailer --version".to_string()],
            },
        ],
        "e2b" => vec![
            DryRunPhase {
                phase: InstallPhase::Preflight,
                actions: vec![
                    "Check /dev/kvm exists and is accessible".to_string(),
                    "Check kernel >= 4.14".to_string(),
                    "Check arch is x86_64 or aarch64".to_string(),
                    "Check UFFD WP_ASYNC: grep /proc/kallsyms or kernel >= 6.7".to_string(),
                ],
            },
            DryRunPhase {
                phase: InstallPhase::Packages,
                actions: vec![format!(
                    "dnf install: {}",
                    FIRECRACKER_E2B_PACKAGES.join(", ")
                )],
            },
            DryRunPhase {
                phase: InstallPhase::OsPrimitives,
                actions: vec![
                    "Write /etc/sysctl.d/90-e2b.conf (vm.nr_hugepages=2048, ip_forward=1, ...)".to_string(),
                    "Write /usr/lib/sysusers.d/e2b.conf (e2b service account; provisions group before udev rule)".to_string(),
                    "Write /etc/udev/rules.d/90-e2b-userfaultfd.rules (MODE=0660, GROUP=e2b)".to_string(),
                    "Write /usr/lib/tmpfiles.d/e2b.conf (directories)".to_string(),
                    "modprobe: nbd(nbds_max=64), overlay, tun, erofs".to_string(),
                    "Write /etc/systemd/system/mnt-hugepages.mount + systemctl enable --now (persistent hugetlbfs)".to_string(),
                    format!(
                        "Provision {}/{{vmlinux,rootfs.ext4,vm-config.json}}",
                        FC_DEFAULT_DIR_DEFAULT
                    ),
                ],
            },
            DryRunPhase {
                phase: InstallPhase::ServiceSetup,
                actions: vec!["systemctl enable --now e2b-orchestrator.service".to_string()],
            },
            DryRunPhase {
                phase: InstallPhase::PostVerify,
                actions: vec![
                    "firecracker --version (expect v1.14.x e2b fork)".to_string(),
                    "e2b-orchestrator gRPC :5008 health check".to_string(),
                ],
            },
        ],
        _ => vec![DryRunPhase {
            phase: InstallPhase::Preflight,
            actions: vec![format!("variant '{variant}' not yet implemented")],
        }],
    }
}

fn gvisor_dry_run_phases(runtime: Option<&str>, control_panel: Option<&str>) -> Vec<DryRunPhase> {
    let mode_label = match (runtime, control_panel) {
        (_, Some("substrate")) => "shim + substrate data-plane",
        (Some("containerd"), _) => "shim (containerd)",
        (Some("docker"), _) => "docker",
        _ => "standalone",
    };

    let preflight_actions = {
        let mut a = vec![
            "Check OS = Linux".to_string(),
            "Check arch = x86_64 or aarch64".to_string(),
            "Check kernel >= 4.15".to_string(),
            "Check kernel.yama.ptrace_scope <= 2".to_string(),
        ];
        if runtime == Some("containerd") || control_panel == Some("substrate") {
            a.push("Check containerd.service active".to_string());
        }
        a
    };

    let pkg_list: &[&str] = match (runtime, control_panel) {
        (_, Some("substrate")) => GVISOR_SUBSTRATE_PACKAGES,
        (Some("containerd"), _) => GVISOR_SHIM_PACKAGES,
        (Some("docker"), _) => GVISOR_DOCKER_PACKAGES,
        _ => GVISOR_STANDALONE_PACKAGES,
    };

    let mut os_primitives_actions =
        vec!["Write /etc/runsc/config.toml (platform=systrap)".to_string()];
    if runtime == Some("containerd") || control_panel == Some("substrate") {
        os_primitives_actions.push(
            "Patch /etc/containerd/config.toml (register io.containerd.runsc.v1 runtime handler)"
                .to_string(),
        );
        os_primitives_actions.push("systemctl restart containerd.service".to_string());
    }
    if runtime == Some("docker") {
        os_primitives_actions
            .push("Write /etc/docker/daemon.json (register runsc OCI runtime)".to_string());
        os_primitives_actions.push("systemctl restart docker.service".to_string());
    }
    if control_panel == Some("substrate") {
        os_primitives_actions.push("mkdir -p /var/lib/substrate/{checkpoints,state}".to_string());
        os_primitives_actions.push("mkdir -p /etc/substrate/{config,tls}".to_string());
        os_primitives_actions.push("Write /etc/substrate/config/node-config.yaml".to_string());
        os_primitives_actions
            .push("Write /etc/substrate/tls/README (cert-manager placeholder)".to_string());
    }

    let mut verify_actions = vec![
        "runsc --version".to_string(),
        "runsc do /bin/true (syscall interception)".to_string(),
        "runsc help checkpoint (checkpoint capability)".to_string(),
    ];
    if runtime == Some("containerd") || control_panel == Some("substrate") {
        verify_actions.push(
            "Verify /usr/bin/containerd-shim-runsc-v1 + io.containerd.runsc.v1 in /etc/containerd/config.toml"
                .to_string(),
        );
    }
    if control_panel == Some("substrate") {
        verify_actions.push("Verify atelet + ateom-gvisor binaries in PATH".to_string());
        verify_actions.push("Verify /var/lib/substrate/ + /etc/substrate/ directories".to_string());
    }

    vec![
        DryRunPhase {
            phase: InstallPhase::Preflight,
            actions: preflight_actions,
        },
        DryRunPhase {
            phase: InstallPhase::Packages,
            actions: vec![format!(
                "dnf install: {} (mode: {mode_label})",
                pkg_list.join(", ")
            )],
        },
        DryRunPhase {
            phase: InstallPhase::OsPrimitives,
            actions: os_primitives_actions,
        },
        DryRunPhase {
            phase: InstallPhase::ServiceSetup,
            actions: vec!["(none \u{2014} runsc is on-demand)".to_string()],
        },
        DryRunPhase {
            phase: InstallPhase::PostVerify,
            actions: verify_actions,
        },
    ]
}

// ===========================================================================
// State integration helpers
// ===========================================================================

fn write_installed_state(
    layout: &FsLayout,
    request: &SandboxInstallRequest,
    outcome: &SandboxInstallOutcome,
    operation_id: &str,
) -> Result<(), StateError> {
    let state_path = layout.state_dir.join("installed.toml");

    let mut state = InstalledState::load(&state_path)?;

    let object_name = format!("sandbox-{}", request.backend);
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    // Use the operation_id passed from execute_sandbox_install so the central
    // log entry and `last_operation_id` in installed.toml refer to the same
    // operation (audit correlation).
    let op_id = operation_id.to_string();
    // Resolve installed firecracker version from Phase 5 verify output if
    // available; otherwise fall back to the anolisa CLI version (rather than
    // a hardcoded literal which silently goes stale).
    let installed_version = outcome
        .installed_version
        .clone()
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());

    // Upsert the installed object
    let obj = InstalledObject {
        kind: ObjectKind::Osbase,
        name: object_name.clone(),
        version: installed_version,
        status: ObjectStatus::Installed,
        manifest_digest: None,
        distribution_source: None,
        install_backend: None,
        installed_at: now.clone(),
        last_operation_id: Some(op_id.clone()),
        managed: true,
        adopted: false,
        subscription_scope: Default::default(),
        enabled_features: {
            let mut feats = vec![format!("variant={}", request.variant)];
            if let Some(ref rt) = request.runtime {
                feats.push(format!("runtime={rt}"));
            }
            if let Some(ref cp) = request.control_panel {
                feats.push(format!("control_panel={cp}"));
            }
            feats
        },
        component_refs: Vec::new(),
        files: Vec::new(),
        external_modified_files: Vec::new(),
        services: Vec::new(),
        health: Vec::new(),
    };

    state.upsert_object(obj);

    // Record operation
    let op = OperationRecord {
        id: op_id,
        command: format!(
            "osbase sandbox install {} --variant={}",
            request.backend, request.variant
        ),
        status: "succeeded".to_string(),
        started_at: now.clone(),
        finished_at: Some(now),
    };
    state.append_operation(op);

    state.save(&state_path)?;
    Ok(())
}

fn log_operation(
    layout: &FsLayout,
    operation_id: &str,
    command: &str,
    status: Option<LogStatus>,
    severity: Severity,
) -> Result<(), CentralLogError> {
    let log = CentralLog::open(layout.central_log.clone());
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let record = LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.to_string()),
        command: command.to_string(),
        source: "anolisa-cli".to_string(),
        component: Some("sandbox".to_string()),
        severity,
        message: format!(
            "sandbox install: {}",
            if status.is_some() {
                "completed"
            } else {
                "started"
            }
        ),
        actor: "cli".to_string(),
        install_mode: Some(match layout.mode {
            anolisa_platform::fs_layout::InstallMode::System => "system".to_string(),
            anolisa_platform::fs_layout::InstallMode::User => "user".to_string(),
        }),
        started_at: now.clone(),
        finished_at: status.map(|_| now),
        status,
        objects: vec!["sandbox".to_string()],
        backup_ids: Vec::new(),
        warnings: Vec::new(),
        details: serde_json::Value::Null,
    };
    log.append(&record)
}

fn generate_operation_id() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::SystemTime;

    let mut hasher = DefaultHasher::new();
    SystemTime::now().hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    format!("op-{:016x}", hasher.finish())
}

// ===========================================================================
// Utility helpers
// ===========================================================================

/// Parse kernel version string and check if it's >= major.minor.
fn kernel_version_at_least(kernel: &str, major: u32, minor: u32) -> bool {
    // Kernel string like "5.10.134-16.1.al8.x86_64"
    let version_part = kernel.split('-').next().unwrap_or(kernel);
    let parts: Vec<&str> = version_part.split('.').collect();
    if parts.len() < 2 {
        return false;
    }
    let k_major: u32 = match parts[0].parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    let k_minor: u32 = match parts[1].parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    (k_major, k_minor) >= (major, minor)
}

/// Run `<cmd> --version` and extract the version string.
fn run_version_command(cmd: &str) -> Option<String> {
    let output = Command::new(cmd).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Extract version: typically "Firecracker v1.13.1" or just the first line
    let first_line = stdout.lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        None
    } else {
        Some(first_line.to_string())
    }
}

/// Extract a semantic version (e.g. "1.14.4") from a firecracker --version
/// banner like "Firecracker v1.14.4". Falls back to a trimmed copy of the
/// input when no `vX.Y[.Z]` token is present so callers can still log the
/// raw banner. Returns `None` only for empty input.
fn parse_firecracker_version(banner: &str) -> Option<String> {
    let trimmed = banner.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Look for the first whitespace-delimited token that begins with 'v'
    // followed by a digit.
    for tok in trimmed.split_whitespace() {
        if let Some(rest) = tok.strip_prefix('v')
            && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
        {
            return Some(rest.to_string());
        }
    }
    Some(trimmed.to_string())
}

/// Check /dev/kvm permissions and emit warnings if suboptimal.
#[cfg(unix)]
fn check_kvm_permissions(warnings: &mut Vec<String>) {
    let kvm_path = Path::new("/dev/kvm");
    if !kvm_path.exists() {
        warnings.push("/dev/kvm does not exist after modprobe".to_string());
        return;
    }

    // Check if current user can access /dev/kvm
    match fs::metadata(kvm_path) {
        Ok(meta) => {
            use std::os::unix::fs::MetadataExt;
            let mode = meta.mode();
            // If not world-accessible and we're not root, check group
            if mode & 0o006 == 0 && !privilege::is_root() {
                let kvm_gid = meta.gid();
                let our_gid = nix::unistd::getegid().as_raw();
                if kvm_gid != our_gid {
                    warnings.push(format!(
                        "/dev/kvm has restrictive permissions (mode={:o}); consider adding your user to the kvm group",
                        mode & 0o777
                    ));
                }
            }
        }
        Err(e) => {
            warnings.push(format!("cannot stat /dev/kvm: {e}"));
        }
    }
}

#[cfg(not(unix))]
fn check_kvm_permissions(_warnings: &mut Vec<String>) {
    // No-op on non-unix platforms
}

/// Validate the request is for a supported backend/variant combination.
///
/// Implements the rev4 fail-fast checklist (gvisor-substrate-design-note §5.4.2):
/// rejects 7 illegal flag combinations up-front instead of letting them
/// surface as confusing errors deep in the pipeline.
pub fn validate_request(request: &SandboxInstallRequest) -> Result<(), SandboxInstallError> {
    match request.backend {
        SandboxBackendKind::Firecracker => {
            // fail-fast #7: firecracker bypasses L2 — refuse any --runtime.
            if request.runtime.is_some() {
                return Err(SandboxInstallError::Unsupported {
                    backend: "firecracker".to_string(),
                    variant: "firecracker does not use --runtime; it accesses KVM directly"
                        .to_string(),
                });
            }
            if request.control_panel.is_some() {
                return Err(SandboxInstallError::Unsupported {
                    backend: "firecracker".to_string(),
                    variant: "firecracker does not support --control-panel".to_string(),
                });
            }
            match request.variant.as_str() {
                "standard" | "default" | "e2b" => Ok(()),
                "kata-fc" => Err(SandboxInstallError::Unsupported {
                    backend: "firecracker".to_string(),
                    variant: format!("{} (not yet implemented)", request.variant),
                }),
                _ => Err(SandboxInstallError::Unsupported {
                    backend: "firecracker".to_string(),
                    variant: format!(
                        "{} (unknown variant; valid: standard, e2b, kata-fc)",
                        request.variant
                    ),
                }),
            }
        }
        SandboxBackendKind::Gvisor => {
            // fail-fast #1: gVisor has no fork; reject any --variant override
            // other than the default placeholder "default".
            //
            // The CLI fills `variant` with `default_variant()` when the user
            // omits --variant, so we accept that exact string. Any other
            // value (substrate / ax / e2b / runc / ...) means the user passed
            // --variant explicitly, which is illegal for gvisor.
            match request.variant.as_str() {
                "default" => {}
                "substrate" => {
                    return Err(SandboxInstallError::Unsupported {
                        backend: "gvisor".to_string(),
                        variant:
                            "gvisor does not support --variant; use --control-panel=substrate instead"
                                .to_string(),
                    });
                }
                "e2b" => {
                    return Err(SandboxInstallError::Unsupported {
                        backend: "gvisor".to_string(),
                        variant: "unknown variant for gvisor; e2b is a variant of firecracker"
                            .to_string(),
                    });
                }
                _ => {
                    return Err(SandboxInstallError::Unsupported {
                        backend: "gvisor".to_string(),
                        variant: format!(
                            "{} (gvisor does not support --variant; use --runtime=containerd|docker and/or --control-panel=substrate)",
                            request.variant
                        ),
                    });
                }
            }
            // fail-fast #3: rev4 only ships containerd / docker integration.
            if let Some(rt) = request.runtime.as_deref() {
                match rt {
                    "containerd" | "docker" => {}
                    _ => {
                        return Err(SandboxInstallError::Unsupported {
                            backend: "gvisor".to_string(),
                            variant: format!(
                                "unsupported runtime '{rt}'; rev4 supports containerd, docker"
                            ),
                        });
                    }
                }
            }
            // fail-fast #4: --control-panel=substrate strictly requires the
            // containerd shim — Substrate's data-plane (atelet) is wired
            // through containerd RuntimeClass, see design-note §5.3.4.
            if let Some(cp) = request.control_panel.as_deref() {
                if cp != "substrate" {
                    return Err(SandboxInstallError::Unsupported {
                        backend: "gvisor".to_string(),
                        variant: format!(
                            "unsupported --control-panel '{cp}'; rev4 only supports 'substrate'"
                        ),
                    });
                }
                if request.runtime.as_deref() != Some("containerd") {
                    return Err(SandboxInstallError::EnvNotSatisfied {
                        reason: "--control-panel=substrate requires --runtime=containerd"
                            .to_string(),
                        remediation: Some(
                            "add --runtime=containerd, or drop --control-panel=substrate"
                                .to_string(),
                        ),
                    });
                }
            }
            Ok(())
        }
        other => Err(SandboxInstallError::Unsupported {
            backend: other.to_string(),
            variant: format!("{} (not yet implemented)", request.variant),
        }),
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kernel_version_at_least() {
        assert!(kernel_version_at_least("5.10.134-16.1.al8.x86_64", 4, 14));
        assert!(kernel_version_at_least("4.14.0", 4, 14));
        assert!(kernel_version_at_least("4.15.0", 4, 14));
        assert!(!kernel_version_at_least("4.13.99", 4, 14));
        assert!(!kernel_version_at_least("3.10.0", 4, 14));
        assert!(kernel_version_at_least("6.1.0", 4, 14));
    }

    #[test]
    fn test_default_variant_resolution() {
        assert_eq!(
            SandboxBackendKind::Firecracker.default_variant(),
            "standard"
        );
        assert_eq!(SandboxBackendKind::Container.default_variant(), "runc");
    }

    #[test]
    fn test_validate_request_firecracker_standard() {
        let req = SandboxInstallRequest {
            backend: SandboxBackendKind::Firecracker,
            variant: "standard".to_string(),
            dry_run: false,
            force: false,
            no_verify: false,
            runtime: None,
            control_panel: None,
            json: false,
        };
        assert!(validate_request(&req).is_ok());
    }

    #[test]
    fn test_validate_request_unknown_variant() {
        let req = SandboxInstallRequest {
            backend: SandboxBackendKind::Firecracker,
            variant: "nonexistent".to_string(),
            dry_run: false,
            force: false,
            no_verify: false,
            runtime: None,
            control_panel: None,
            json: false,
        };
        assert!(validate_request(&req).is_err());
    }

    #[test]
    fn test_validate_request_unimplemented_backend() {
        let req = SandboxInstallRequest {
            backend: SandboxBackendKind::Kata,
            variant: "default".to_string(),
            dry_run: false,
            force: false,
            no_verify: false,
            runtime: None,
            control_panel: None,
            json: false,
        };
        assert!(validate_request(&req).is_err());
    }

    #[test]
    fn test_dry_run_plan_firecracker_standard() {
        let req = SandboxInstallRequest {
            backend: SandboxBackendKind::Firecracker,
            variant: "standard".to_string(),
            dry_run: true,
            force: false,
            no_verify: false,
            runtime: None,
            control_panel: None,
            json: false,
        };
        let plan = build_dry_run_plan(&req);
        assert_eq!(plan.phases.len(), 5);
        assert_eq!(plan.phases[0].phase, InstallPhase::Preflight);
        assert_eq!(plan.phases[4].phase, InstallPhase::PostVerify);
    }

    #[test]
    fn test_validate_request_firecracker_e2b() {
        let req = SandboxInstallRequest {
            backend: SandboxBackendKind::Firecracker,
            variant: "e2b".to_string(),
            dry_run: false,
            force: false,
            no_verify: false,
            runtime: None,
            control_panel: None,
            json: false,
        };
        assert!(validate_request(&req).is_ok());
    }

    #[test]
    fn test_dry_run_plan_firecracker_e2b() {
        let req = SandboxInstallRequest {
            backend: SandboxBackendKind::Firecracker,
            variant: "e2b".to_string(),
            dry_run: true,
            force: false,
            no_verify: false,
            runtime: None,
            control_panel: None,
            json: false,
        };
        let plan = build_dry_run_plan(&req);
        assert_eq!(plan.phases.len(), 5);
        assert_eq!(plan.phases[0].phase, InstallPhase::Preflight);
        assert_eq!(plan.phases[0].actions.len(), 4); // includes WP_ASYNC check
        assert_eq!(plan.phases[4].phase, InstallPhase::PostVerify);
        // Verify e2b-specific actions
        assert!(plan.phases[1].actions[0].contains("e2b-orchestrator"));
        assert!(plan.phases[3].actions[0].contains("e2b-orchestrator.service"));
    }

    #[test]
    fn test_wp_async_kernel_version_check() {
        // Kernel >= 6.7 should satisfy WP_ASYNC
        assert!(kernel_version_at_least("6.7.0", 6, 7));
        assert!(kernel_version_at_least("6.8.1-anck", 6, 7));
        assert!(kernel_version_at_least("7.0.0", 6, 7));
        // Kernel < 6.7 should not satisfy
        assert!(!kernel_version_at_least(
            "6.6.102-1.git.d4f518459.an23",
            6,
            7
        ));
        assert!(!kernel_version_at_least("5.10.134", 6, 7));
    }

    #[test]
    fn test_dry_run_plan_firecracker_standard_ships_default_assets() {
        let req = SandboxInstallRequest {
            backend: SandboxBackendKind::Firecracker,
            variant: "standard".to_string(),
            dry_run: true,
            force: false,
            no_verify: false,
            runtime: None,
            control_panel: None,
            json: false,
        };
        let plan = build_dry_run_plan(&req);
        let pkgs = &plan.phases[1].actions[0];
        assert!(
            pkgs.contains("firecracker-e2b-kernel"),
            "standard variant must install firecracker-e2b-kernel (variant-neutral data-only sub-package): {pkgs}"
        );
        assert!(
            pkgs.contains("firecracker-e2b-rootfs"),
            "standard variant must install firecracker-e2b-rootfs (variant-neutral data-only sub-package): {pkgs}"
        );
        let osc = &plan.phases[2].actions;
        assert!(
            osc.iter().any(|a| a.contains("vm-config.json")),
            "standard variant must provision default vm-config.json: {osc:?}"
        );
    }

    #[test]
    fn test_dry_run_plan_firecracker_e2b_ships_default_assets() {
        let req = SandboxInstallRequest {
            backend: SandboxBackendKind::Firecracker,
            variant: "e2b".to_string(),
            dry_run: true,
            force: false,
            no_verify: false,
            runtime: None,
            control_panel: None,
            json: false,
        };
        let plan = build_dry_run_plan(&req);
        let pkgs = &plan.phases[1].actions[0];
        assert!(pkgs.contains("firecracker-e2b-kernel"), "{pkgs}");
        assert!(pkgs.contains("firecracker-e2b-rootfs"), "{pkgs}");
        assert!(
            pkgs.contains("firecracker-e2b-busybox"),
            "e2b variant must install firecracker-e2b-busybox (guest rootfs init): {pkgs}"
        );
        assert!(
            pkgs.contains("firecracker-e2b-jailer"),
            "e2b variant must install firecracker-e2b-jailer (orchestrator sandbox wrapper): {pkgs}"
        );
        assert!(
            pkgs.contains("firecracker-e2b-tools"),
            "e2b variant must install firecracker-e2b-tools (snapshot-editor / cpu-template-helper): {pkgs}"
        );
        assert!(pkgs.contains("e2b-orchestrator"), "{pkgs}");
        let osc = &plan.phases[2].actions;
        assert!(
            osc.iter().any(|a| a.contains("vm-config.json")),
            "e2b variant must provision default vm-config.json: {osc:?}"
        );
    }

    // ----- gVisor tests -----

    #[test]
    fn test_validate_request_gvisor_standalone() {
        let req = SandboxInstallRequest {
            backend: SandboxBackendKind::Gvisor,
            variant: "default".to_string(),
            runtime: None,
            control_panel: None,
            dry_run: false,
            force: false,
            no_verify: false,
            json: false,
        };
        assert!(validate_request(&req).is_ok());
    }

    #[test]
    fn test_validate_request_gvisor_shim() {
        let req = SandboxInstallRequest {
            backend: SandboxBackendKind::Gvisor,
            variant: "default".to_string(),
            runtime: Some("containerd".to_string()),
            control_panel: None,
            dry_run: false,
            force: false,
            no_verify: false,
            json: false,
        };
        assert!(validate_request(&req).is_ok());
    }

    #[test]
    fn test_validate_request_gvisor_docker() {
        let req = SandboxInstallRequest {
            backend: SandboxBackendKind::Gvisor,
            variant: "default".to_string(),
            runtime: Some("docker".to_string()),
            control_panel: None,
            dry_run: false,
            force: false,
            no_verify: false,
            json: false,
        };
        assert!(validate_request(&req).is_ok());
    }

    #[test]
    fn test_validate_request_gvisor_substrate() {
        let req = SandboxInstallRequest {
            backend: SandboxBackendKind::Gvisor,
            variant: "default".to_string(),
            runtime: Some("containerd".to_string()),
            control_panel: Some("substrate".to_string()),
            dry_run: false,
            force: false,
            no_verify: false,
            json: false,
        };
        assert!(validate_request(&req).is_ok());
    }

    #[test]
    fn test_validate_gvisor_fail_fast_variant_substrate() {
        let req = SandboxInstallRequest {
            backend: SandboxBackendKind::Gvisor,
            variant: "substrate".to_string(),
            runtime: None,
            control_panel: None,
            dry_run: false,
            force: false,
            no_verify: false,
            json: false,
        };
        let err = validate_request(&req).unwrap_err();
        assert!(err.to_string().contains("--control-panel=substrate"));
    }

    #[test]
    fn test_validate_gvisor_fail_fast_variant_e2b() {
        let req = SandboxInstallRequest {
            backend: SandboxBackendKind::Gvisor,
            variant: "e2b".to_string(),
            runtime: None,
            control_panel: None,
            dry_run: false,
            force: false,
            no_verify: false,
            json: false,
        };
        let err = validate_request(&req).unwrap_err();
        assert!(err.to_string().contains("firecracker"));
    }

    #[test]
    fn test_validate_gvisor_fail_fast_unsupported_runtime() {
        let req = SandboxInstallRequest {
            backend: SandboxBackendKind::Gvisor,
            variant: "default".to_string(),
            runtime: Some("podman".to_string()),
            control_panel: None,
            dry_run: false,
            force: false,
            no_verify: false,
            json: false,
        };
        let err = validate_request(&req).unwrap_err();
        assert!(err.to_string().contains("podman"));
    }

    #[test]
    fn test_validate_gvisor_fail_fast_substrate_without_containerd() {
        let req = SandboxInstallRequest {
            backend: SandboxBackendKind::Gvisor,
            variant: "default".to_string(),
            runtime: Some("docker".to_string()),
            control_panel: Some("substrate".to_string()),
            dry_run: false,
            force: false,
            no_verify: false,
            json: false,
        };
        let err = validate_request(&req).unwrap_err();
        assert!(err.to_string().contains("--runtime=containerd"));
    }

    #[test]
    fn test_validate_firecracker_fail_fast_runtime() {
        let req = SandboxInstallRequest {
            backend: SandboxBackendKind::Firecracker,
            variant: "standard".to_string(),
            runtime: Some("containerd".to_string()),
            control_panel: None,
            dry_run: false,
            force: false,
            no_verify: false,
            json: false,
        };
        let err = validate_request(&req).unwrap_err();
        assert!(err.to_string().contains("KVM directly"));
    }

    #[test]
    fn test_dry_run_plan_gvisor_standalone() {
        let req = SandboxInstallRequest {
            backend: SandboxBackendKind::Gvisor,
            variant: "default".to_string(),
            runtime: None,
            control_panel: None,
            dry_run: true,
            force: false,
            no_verify: false,
            json: false,
        };
        let plan = build_dry_run_plan(&req);
        assert_eq!(plan.phases.len(), 5);
        assert_eq!(plan.phases[0].phase, InstallPhase::Preflight);
        assert_eq!(plan.phases[4].phase, InstallPhase::PostVerify);
        // Verify pkg list
        assert!(plan.phases[1].actions[0].contains("gvisor-runsc"));
        assert!(plan.phases[1].actions[0].contains("standalone"));
    }

    #[test]
    fn test_dry_run_plan_gvisor_substrate() {
        let req = SandboxInstallRequest {
            backend: SandboxBackendKind::Gvisor,
            variant: "default".to_string(),
            runtime: Some("containerd".to_string()),
            control_panel: Some("substrate".to_string()),
            dry_run: true,
            force: false,
            no_verify: false,
            json: false,
        };
        let plan = build_dry_run_plan(&req);
        assert_eq!(plan.phases.len(), 5);
        // Pre-flight should check containerd
        assert!(
            plan.phases[0]
                .actions
                .iter()
                .any(|a| a.contains("containerd"))
        );
        // Packages should include substrate
        assert!(plan.phases[1].actions[0].contains("atelet"));
        assert!(plan.phases[1].actions[0].contains("ateom-gvisor"));
        // OS primitives should include substrate dirs
        assert!(
            plan.phases[2]
                .actions
                .iter()
                .any(|a| a.contains("/var/lib/substrate"))
        );
        assert!(
            plan.phases[2]
                .actions
                .iter()
                .any(|a| a.contains("node-config.yaml"))
        );
        // Verify should check substrate
        assert!(plan.phases[4].actions.iter().any(|a| a.contains("atelet")));
    }

    // -- RPM packaging error message tests ---------------------------------
    //
    // These guard the human-readable error returned when the ANOLISA sandbox
    // RPMs are missing from configured repos. The format is part of the
    // operator-facing contract — changes that drop the repo ID, the doc path,
    // or the list of missing packages will silently degrade the diagnostic.

    #[test]
    fn test_gvisor_missing_rpm_error_lists_missing_and_required() {
        let missing = ["atelet", "ateom-gvisor"];
        let required = GVISOR_SUBSTRATE_PACKAGES;
        let msg = gvisor_missing_rpm_error(&missing, required);
        // Missing packages must appear verbatim.
        assert!(msg.contains("atelet"), "missing pkg name dropped: {msg}");
        assert!(
            msg.contains("ateom-gvisor"),
            "missing pkg name dropped: {msg}"
        );
        // Required set must be enumerated so operators see the full mode.
        assert!(
            msg.contains("gvisor-runsc"),
            "required set truncated: {msg}"
        );
        assert!(
            msg.contains("containerd-shim-runsc-v1"),
            "required set truncated: {msg}"
        );
    }

    #[test]
    fn test_gvisor_missing_rpm_error_points_to_repo_and_doc() {
        let missing = ["gvisor-runsc"];
        let msg = gvisor_missing_rpm_error(&missing, GVISOR_STANDALONE_PACKAGES);
        // The repo ID and doc path are the actionable hooks; both must appear.
        assert!(
            msg.contains(ANOLISA_SANDBOX_REPO_ID),
            "repo id missing: {msg}"
        );
        assert!(
            msg.contains(SANDBOX_RPM_PACKAGING_DOC),
            "doc path missing: {msg}"
        );
        // Confirm the message explicitly says these aren't upstream packages
        // — prevents operators from chasing distro repos.
        assert!(
            msg.contains("NOT shipped by upstream"),
            "upstream-not-available phrase missing: {msg}"
        );
    }

    #[test]
    fn test_anolisa_sandbox_repo_id_matches_manifest() {
        // sandbox-gvisor.toml declares `[dependencies.repository] id =
        // "anolisa-sandbox"`. If you rename either side without the other,
        // operators will see a repo ID in error messages that doesn't exist
        // in the manifest — catch the drift here.
        assert_eq!(ANOLISA_SANDBOX_REPO_ID, "anolisa-sandbox");
    }

    #[test]
    fn test_packaging_doc_path_is_sandbox_relative() {
        // The doc path is rendered into error messages verbatim; keep it
        // workspace-relative (no leading slash, no file:// prefix).
        assert!(
            SANDBOX_RPM_PACKAGING_DOC.starts_with("ANOLISA-design/"),
            "unexpected doc path: {SANDBOX_RPM_PACKAGING_DOC}"
        );
        assert!(
            SANDBOX_RPM_PACKAGING_DOC.ends_with("sandbox-rpm-packaging.md"),
            "unexpected doc path: {SANDBOX_RPM_PACKAGING_DOC}"
        );
    }

    // -- containerd config.toml AST merge ---------------------------------
    //
    // These guard the toml_edit-based merge in
    // `ensure_containerd_runsc_runtime`. The previous implementation did a
    // plain string append, which broke under containerd 2.x's strict TOML
    // parser as soon as the user already had a `runtimes` / `runtimes.runsc`
    // table. The tests below pin the merged-document invariants we rely on.

    fn render(doc: &toml_edit::DocumentMut) -> String {
        doc.to_string()
    }

    #[test]
    fn test_containerd_merge_writes_canonical_keys_into_empty_doc() {
        let mut doc = "".parse::<toml_edit::DocumentMut>().unwrap();
        let changed = ensure_containerd_runsc_runtime(&mut doc);
        assert!(changed, "empty doc must be marked as changed");
        let out = render(&doc);
        assert!(
            out.contains("io.containerd.runsc.v1"),
            "runtime_type missing: {out}"
        );
        assert!(
            out.contains("io.containerd.runsc.v1.options"),
            "TypeUrl missing: {out}"
        );
        assert!(
            out.contains("/etc/runsc/config.toml"),
            "ConfigPath missing: {out}"
        );
    }

    #[test]
    fn test_containerd_merge_is_idempotent() {
        let initial = r#"
[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.runsc]
  runtime_type = "io.containerd.runsc.v1"
  [plugins."io.containerd.grpc.v1.cri".containerd.runtimes.runsc.options]
    TypeUrl = "io.containerd.runsc.v1.options"
    ConfigPath = "/etc/runsc/config.toml"
"#;
        let mut doc = initial.parse::<toml_edit::DocumentMut>().unwrap();
        let changed = ensure_containerd_runsc_runtime(&mut doc);
        assert!(!changed, "already-correct doc must not be marked changed");
    }

    #[test]
    fn test_containerd_merge_preserves_sibling_runtime() {
        // Operator already has runc registered; we must not clobber it and
        // must NOT produce a duplicate `runtimes.runsc` table.
        let initial = r#"version = 2

[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.runc]
  runtime_type = "io.containerd.runc.v2"
"#;
        let mut doc = initial.parse::<toml_edit::DocumentMut>().unwrap();
        assert!(ensure_containerd_runsc_runtime(&mut doc));
        let out = render(&doc);

        // Sibling preserved.
        assert!(
            out.contains("io.containerd.runc.v2"),
            "runc clobbered: {out}"
        );
        // runsc registered.
        assert!(
            out.contains("io.containerd.runsc.v1"),
            "runsc missing: {out}"
        );

        // No duplicate `runtimes.runsc` table header (the regression we are
        // protecting against under containerd 2.x).
        let dup_count = out.matches("runtimes.runsc]").count();
        assert!(dup_count <= 1, "duplicate runsc table headers: {out}");
    }

    #[test]
    fn test_containerd_merge_updates_stale_runtime_type() {
        // User has a stale entry pointing at the v0 shim; merge must rewrite
        // it to the v1 shim without producing a second table.
        let initial = r#"
[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.runsc]
  runtime_type = "io.containerd.runsc.v0"
"#;
        let mut doc = initial.parse::<toml_edit::DocumentMut>().unwrap();
        assert!(ensure_containerd_runsc_runtime(&mut doc));
        let out = render(&doc);
        assert!(
            out.contains("io.containerd.runsc.v1"),
            "v1 shim not written: {out}"
        );
        assert!(
            !out.contains("io.containerd.runsc.v0"),
            "stale v0 shim left: {out}"
        );
    }
}
