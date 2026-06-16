//! Service-unit lifecycle executor.
//!
//! Alpha contract: ANOLISA needs to start owned service units after a
//! component installs, stop them before disable / uninstall, and surface
//! systemctl status on `anolisa restart`. This module wraps the minimum
//! amount of `systemctl` we need without a dbus dependency.
//!
//! The trait surface is small on purpose. Any platform that does not
//! match the alpha factory rules — non-Linux, install_mode == "user",
//! container runtime detected — gets a [`NotSupportedServiceManager`]
//! whose ops succeed with `state: NotSupported, supported: false,
//! changed: false`. Callers treat that as a quiet skip rather than a
//! warning, so the lifecycle on macOS / inside CI containers / under
//! user-mode installs stays a no-op for services.
//!
//! `FakeServiceManager` is the executor used by `service.rs`'s own unit
//! tests and by integration tests that need to assert which ops the
//! orchestrators dispatched.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

use anolisa_env::EnvFacts;

/// One operation issued against a service manager. Used both to drive
/// systemctl and to record what a [`FakeServiceManager`] saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceOp {
    /// Query service state.
    Probe,
    /// Start a stopped service.
    Start,
    /// Stop a running service.
    Stop,
    /// Restart a service.
    Restart,
    /// Enable service startup.
    Enable,
    /// Disable service startup.
    Disable,
}

impl ServiceOp {
    /// Wire label, matched by `systemctl <op> <unit>` and audit logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Probe => "probe",
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Enable => "enable",
            Self::Disable => "disable",
        }
    }
}

/// Coarse runtime state of a service unit. Mirrors the subset of
/// `systemctl is-active` answers callers can act on, plus
/// `NotSupported` for hosts where ANOLISA refuses to drive systemctl.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    /// Unit is active.
    Active,
    /// Unit is inactive.
    Inactive,
    /// Unit is failed.
    Failed,
    /// Unit is starting.
    Activating,
    /// Unit is stopping.
    Deactivating,
    /// Unit is not installed or not known to the manager.
    NotInstalled,
    /// Host/mode deliberately does not support this service manager.
    NotSupported,
    /// Manager returned a state outside the modeled vocabulary.
    Unknown,
}

impl ServiceState {
    /// Stable lowercase wire label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
            Self::Failed => "failed",
            Self::Activating => "activating",
            Self::Deactivating => "deactivating",
            Self::NotInstalled => "not_installed",
            Self::NotSupported => "not_supported",
            Self::Unknown => "unknown",
        }
    }
}

/// What [`ServiceManager`] returns from a single op.
#[derive(Debug, Clone)]
pub struct ServiceOutcome {
    /// Backend label, e.g. `"systemd"`, `"not-supported"`, or a custom
    /// fake-manager name.
    pub manager: String,
    /// Service unit name (e.g. `"agentsight.service"`).
    pub unit: String,
    /// Op the caller requested.
    pub op: ServiceOp,
    /// State of the unit after the op.
    pub state: ServiceState,
    /// `false` when the backend is a deliberate skip (e.g. user mode).
    pub supported: bool,
    /// `true` when the op caused a state change. Always `false` for
    /// probe and for unsupported backends.
    pub changed: bool,
    /// Human-readable description, used for logs / warnings.
    pub message: String,
}

/// Failure surface for a single service op. Errors are non-fatal at
/// the lifecycle layer — orchestrators surface them as warnings.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// `systemctl` could not be spawned.
    #[error("spawning systemctl failed: {source}")]
    Spawn {
        /// Underlying spawn error.
        #[source]
        source: std::io::Error,
    },
    /// Native manager command returned a non-zero exit code.
    #[error("systemctl {op} {unit} exited with status {code}: {stderr}")]
    NonZeroExit {
        /// Operation attempted.
        op: String,
        /// Unit targeted by the operation.
        unit: String,
        /// Process exit code.
        code: i32,
        /// Captured stderr.
        stderr: String,
    },
}

/// Backend trait that every service-manager impl satisfies.
pub trait ServiceManager: Send + Sync {
    /// Wire label written into `ServiceRef.manager` and log records.
    fn manager(&self) -> &str;
    /// `false` when this backend is a deliberate skip — orchestrators
    /// short-circuit so they don't emit per-unit warnings.
    fn supported(&self) -> bool;
    /// Reason text when `supported() == false`. `None` for active backends.
    fn unsupported_reason(&self) -> Option<&str> {
        None
    }

    /// Probe the service without attempting mutation.
    fn probe_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError>;
    /// Start the service.
    fn start_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError>;
    /// Stop the service.
    fn stop_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError>;
    /// Restart the service.
    fn restart_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError>;
    /// Enable startup for the service.
    fn enable_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError>;
    /// Disable startup for the service.
    fn disable_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError>;
}

/// Pick a backend for the current host + install mode. Linux + system
/// mode + no container → real `systemctl` driver; everything else →
/// quiet skip.
pub fn for_install_mode(install_mode: &str, env: &EnvFacts) -> Box<dyn ServiceManager> {
    if env.os != "linux" {
        return Box::new(NotSupportedServiceManager::new(format!(
            "service manager unsupported on os '{}'",
            env.os,
        )));
    }
    if install_mode != "system" {
        return Box::new(NotSupportedServiceManager::new(
            "service manager unsupported in install_mode='user' (alpha)".to_string(),
        ));
    }
    if let Some(rt) = env.container.as_deref() {
        return Box::new(NotSupportedServiceManager::new(format!(
            "container runtime '{rt}' detected — refusing to drive systemctl from inside a container",
        )));
    }
    Box::new(SystemdServiceManager::new())
}

/// Quiet-skip backend. Every op succeeds with `state: NotSupported`.
pub struct NotSupportedServiceManager {
    reason: String,
}

impl NotSupportedServiceManager {
    /// Build a quiet-skip backend with a stable reason string.
    pub fn new(reason: String) -> Self {
        Self { reason }
    }
    fn outcome(&self, unit: &str, op: ServiceOp) -> ServiceOutcome {
        ServiceOutcome {
            manager: "not-supported".to_string(),
            unit: unit.to_string(),
            op,
            state: ServiceState::NotSupported,
            supported: false,
            changed: false,
            message: self.reason.clone(),
        }
    }
}

impl ServiceManager for NotSupportedServiceManager {
    fn manager(&self) -> &str {
        "not-supported"
    }
    fn supported(&self) -> bool {
        false
    }
    fn unsupported_reason(&self) -> Option<&str> {
        Some(&self.reason)
    }
    fn probe_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError> {
        Ok(self.outcome(unit, ServiceOp::Probe))
    }
    fn start_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError> {
        Ok(self.outcome(unit, ServiceOp::Start))
    }
    fn stop_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError> {
        Ok(self.outcome(unit, ServiceOp::Stop))
    }
    fn restart_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError> {
        Ok(self.outcome(unit, ServiceOp::Restart))
    }
    fn enable_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError> {
        Ok(self.outcome(unit, ServiceOp::Enable))
    }
    fn disable_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError> {
        Ok(self.outcome(unit, ServiceOp::Disable))
    }
}

/// Real `systemctl` backend. Resolves the binary off `PATH`, and
/// returns spawn errors as `ServiceError::Spawn` so the caller can
/// downgrade them to warnings.
pub struct SystemdServiceManager {
    binary: PathBuf,
}

impl SystemdServiceManager {
    /// Build a manager that invokes `systemctl` from `PATH`.
    pub fn new() -> Self {
        Self {
            binary: PathBuf::from("systemctl"),
        }
    }

    fn probe_state(&self, unit: &str) -> Result<ServiceState, ServiceError> {
        let mut cmd = Command::new(&self.binary);
        cmd.arg("is-active").arg(unit);
        let output = cmd
            .output()
            .map_err(|source| ServiceError::Spawn { source })?;
        // `is-active` exits 3 for inactive/failed units — read stdout
        // regardless of exit code.
        let stdout = String::from_utf8_lossy(&output.stdout)
            .trim()
            .to_lowercase();
        Ok(match stdout.as_str() {
            "active" => ServiceState::Active,
            "reloading" | "activating" => ServiceState::Activating,
            "deactivating" => ServiceState::Deactivating,
            "inactive" => ServiceState::Inactive,
            "failed" => ServiceState::Failed,
            "unknown" | "" => ServiceState::NotInstalled,
            _ => ServiceState::Unknown,
        })
    }

    fn run_op(&self, op: ServiceOp, unit: &str) -> Result<ServiceOutcome, ServiceError> {
        let prior = self.probe_state(unit)?;
        if matches!(op, ServiceOp::Probe) {
            return Ok(ServiceOutcome {
                manager: "systemd".to_string(),
                unit: unit.to_string(),
                op,
                state: prior,
                supported: true,
                changed: false,
                message: format!("systemctl is-active reported {}", prior.as_str()),
            });
        }
        let mut cmd = Command::new(&self.binary);
        cmd.arg(op.as_str()).arg(unit);
        let output = cmd
            .output()
            .map_err(|source| ServiceError::Spawn { source })?;
        if !output.status.success() {
            let code = output.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(ServiceError::NonZeroExit {
                op: op.as_str().to_string(),
                unit: unit.to_string(),
                code,
                stderr,
            });
        }
        let post = self.probe_state(unit)?;
        let changed = match op {
            ServiceOp::Start => prior != ServiceState::Active && post == ServiceState::Active,
            ServiceOp::Stop => prior == ServiceState::Active && post != ServiceState::Active,
            ServiceOp::Restart => true,
            ServiceOp::Enable | ServiceOp::Disable => true,
            ServiceOp::Probe => false,
        };
        Ok(ServiceOutcome {
            manager: "systemd".to_string(),
            unit: unit.to_string(),
            op,
            state: post,
            supported: true,
            changed,
            message: format!(
                "systemctl {} {} ok (state={})",
                op.as_str(),
                unit,
                post.as_str()
            ),
        })
    }
}

impl Default for SystemdServiceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceManager for SystemdServiceManager {
    fn manager(&self) -> &str {
        "systemd"
    }
    fn supported(&self) -> bool {
        true
    }
    fn probe_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError> {
        self.run_op(ServiceOp::Probe, unit)
    }
    fn start_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError> {
        self.run_op(ServiceOp::Start, unit)
    }
    fn stop_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError> {
        self.run_op(ServiceOp::Stop, unit)
    }
    fn restart_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError> {
        self.run_op(ServiceOp::Restart, unit)
    }
    fn enable_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError> {
        self.run_op(ServiceOp::Enable, unit)
    }
    fn disable_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError> {
        self.run_op(ServiceOp::Disable, unit)
    }
}

/// Test executor. Records every (op, unit) call and lets tests assert
/// which units the orchestrators tried to drive without actually
/// invoking systemctl.
pub struct FakeServiceManager {
    manager_name: String,
    supported: bool,
    state: Mutex<ServiceState>,
    calls: Mutex<Vec<(ServiceOp, String)>>,
    fail_ops: Mutex<HashSet<(ServiceOp, String)>>,
}

impl FakeServiceManager {
    /// Build a supported fake manager with an initially inactive unit
    /// state and no injected failures.
    pub fn new() -> Self {
        Self {
            manager_name: "fake".to_string(),
            supported: true,
            state: Mutex::new(ServiceState::Inactive),
            calls: Mutex::new(Vec::new()),
            fail_ops: Mutex::new(HashSet::new()),
        }
    }
    /// Snapshot of every call recorded so far, in dispatch order.
    pub fn calls(&self) -> Vec<(ServiceOp, String)> {
        self.calls.lock().expect("poisoned").clone()
    }
    /// Override the unit's reported state before the next op. Useful
    /// for testing reactivation / probe semantics.
    pub fn set_state(&self, state: ServiceState) {
        *self.state.lock().expect("poisoned") = state;
    }
    /// Cause a specific (op, unit) pair to return `NonZeroExit` so
    /// tests can assert orchestrator warning paths.
    pub fn fail(&self, op: ServiceOp, unit: &str) {
        self.fail_ops
            .lock()
            .expect("poisoned")
            .insert((op, unit.to_string()));
    }
    fn record(&self, op: ServiceOp, unit: &str) -> Result<ServiceOutcome, ServiceError> {
        self.calls
            .lock()
            .expect("poisoned")
            .push((op, unit.to_string()));
        if self
            .fail_ops
            .lock()
            .expect("poisoned")
            .contains(&(op, unit.to_string()))
        {
            return Err(ServiceError::NonZeroExit {
                op: op.as_str().to_string(),
                unit: unit.to_string(),
                code: 1,
                stderr: "fake forced failure".to_string(),
            });
        }
        let prior = *self.state.lock().expect("poisoned");
        let next = match op {
            ServiceOp::Start | ServiceOp::Restart => ServiceState::Active,
            ServiceOp::Stop => ServiceState::Inactive,
            _ => prior,
        };
        *self.state.lock().expect("poisoned") = next;
        Ok(ServiceOutcome {
            manager: self.manager_name.clone(),
            unit: unit.to_string(),
            op,
            state: next,
            supported: self.supported,
            changed: prior != next,
            message: format!("fake {} {} ok", op.as_str(), unit),
        })
    }
}

impl Default for FakeServiceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceManager for FakeServiceManager {
    fn manager(&self) -> &str {
        &self.manager_name
    }
    fn supported(&self) -> bool {
        self.supported
    }
    fn probe_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError> {
        self.record(ServiceOp::Probe, unit)
    }
    fn start_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError> {
        self.record(ServiceOp::Start, unit)
    }
    fn stop_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError> {
        self.record(ServiceOp::Stop, unit)
    }
    fn restart_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError> {
        self.record(ServiceOp::Restart, unit)
    }
    fn enable_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError> {
        self.record(ServiceOp::Enable, unit)
    }
    fn disable_service(&self, unit: &str) -> Result<ServiceOutcome, ServiceError> {
        self.record(ServiceOp::Disable, unit)
    }
}

/// Append a [`crate::central_log::LogKind::Component`] central-log record describing one
/// service op (start / stop / restart / probe / enable / disable). Pairs
/// each `manager.{start,stop}_service` call with an audit line so
/// `anolisa logs --op-id <id>` can reconstruct what happened to the
/// owned units, not just the surrounding verb. Logs are best-effort:
/// failures are swallowed because the parent verb has already committed.
// Service audit records intentionally spell out operation, actor, mode,
// and unit fields so lifecycle call sites show the full audit context.
#[allow(clippy::too_many_arguments)]
pub fn record_service_op(
    log: Option<&crate::central_log::CentralLog>,
    op: ServiceOp,
    component: &str,
    unit: &str,
    operation_id: &str,
    actor: &str,
    install_mode: &str,
    error: Option<&str>,
) {
    let Some(log) = log else {
        return;
    };
    use crate::central_log::{LogKind, LogRecord, Severity};
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let (severity, message) = match error {
        None => (
            Severity::Info,
            format!("service {} ok for {}/{}", op.as_str(), component, unit),
        ),
        Some(err) => (
            Severity::Warn,
            format!(
                "service {} skipped for {}/{}: {}",
                op.as_str(),
                component,
                unit,
                err
            ),
        ),
    };
    let _ = log.append(&LogRecord {
        kind: LogKind::Component,
        operation_id: Some(operation_id.to_string()),
        command: format!("service:{}", op.as_str()),
        source: "anolisa-core".to_string(),
        component: Some(component.to_string()),
        severity,
        message,
        actor: actor.to_string(),
        install_mode: Some(install_mode.to_string()),
        started_at: now.clone(),
        finished_at: Some(now),
        status: None,
        objects: vec![component.to_string()],
        backup_ids: Vec::new(),
        warnings: Vec::new(),
        details: serde_json::json!({"unit": unit}),
    });
}

/// Append a [`crate::central_log::LogKind::Component`] record describing a service op that
/// was *skipped* because the resolved [`ServiceManager`] reports
/// `supported() == false` (typical on macOS dev machines, `--install-mode
/// user`, or an unsupported container runtime). Without this audit line
/// the verb appears to have silently never touched any units, which
/// makes it hard to tell "no services declared" from "services declared
/// but no manager available".
///
/// Severity is `Info` because this is the expected, advertised behavior
/// of the unsupported manager — operators reading logs should see it as
/// a documented skip rather than a fault. The `details` payload carries
/// `supported = false` plus the manager-supplied `unsupported_reason`
/// (when available) so machine consumers can disambiguate platforms.
// Unsupported-service audit lines need the same context plus manager details;
// keeping them explicit avoids a throwaway builder with weaker invariants.
#[allow(clippy::too_many_arguments)]
pub fn record_service_op_unsupported(
    log: Option<&crate::central_log::CentralLog>,
    op: ServiceOp,
    component: &str,
    unit: &str,
    operation_id: &str,
    actor: &str,
    install_mode: &str,
    manager_name: &str,
    unsupported_reason: Option<&str>,
) {
    let Some(log) = log else {
        return;
    };
    use crate::central_log::{LogKind, LogRecord, Severity};
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let reason_str = unsupported_reason.unwrap_or("manager not supported on this platform");
    let message = format!(
        "service {} skipped for {}/{}: {} ({})",
        op.as_str(),
        component,
        unit,
        manager_name,
        reason_str,
    );
    let _ = log.append(&LogRecord {
        kind: LogKind::Component,
        operation_id: Some(operation_id.to_string()),
        command: format!("service:{}", op.as_str()),
        source: "anolisa-core".to_string(),
        component: Some(component.to_string()),
        severity: Severity::Info,
        message,
        actor: actor.to_string(),
        install_mode: Some(install_mode.to_string()),
        started_at: now.clone(),
        finished_at: Some(now),
        status: None,
        objects: vec![component.to_string()],
        backup_ids: Vec::new(),
        warnings: Vec::new(),
        details: serde_json::json!({
            "unit": unit,
            "supported": false,
            "manager": manager_name,
            "reason": reason_str,
        }),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fake_env(os: &str, container: Option<&str>) -> EnvFacts {
        EnvFacts {
            os: os.to_string(),
            arch: "x86_64".to_string(),
            libc: None,
            kernel: None,
            pkg_base: None,
            os_id: None,
            os_version: None,
            btf: None,
            cap_bpf: None,
            container: container.map(|s| s.to_string()),
            user: "tester".to_string(),
            uid: 1000,
            home: PathBuf::from("/home/tester"),
        }
    }

    #[test]
    fn fake_manager_records_calls_in_order() {
        let m = FakeServiceManager::new();
        m.start_service("a.service").unwrap();
        m.stop_service("a.service").unwrap();
        m.probe_service("b.service").unwrap();
        let calls = m.calls();
        assert_eq!(
            calls,
            vec![
                (ServiceOp::Start, "a.service".to_string()),
                (ServiceOp::Stop, "a.service".to_string()),
                (ServiceOp::Probe, "b.service".to_string()),
            ]
        );
    }

    #[test]
    fn fake_manager_tracks_state_transitions() {
        let m = FakeServiceManager::new();
        let started = m.start_service("a.service").unwrap();
        assert_eq!(started.state, ServiceState::Active);
        assert!(started.changed);
        // Second start is a no-op transition (already active).
        let again = m.start_service("a.service").unwrap();
        assert_eq!(again.state, ServiceState::Active);
        assert!(!again.changed);
        let stopped = m.stop_service("a.service").unwrap();
        assert_eq!(stopped.state, ServiceState::Inactive);
        assert!(stopped.changed);
    }

    #[test]
    fn fake_manager_can_force_failure_per_op() {
        let m = FakeServiceManager::new();
        m.fail(ServiceOp::Start, "a.service");
        let err = m.start_service("a.service").unwrap_err();
        match err {
            ServiceError::NonZeroExit { op, unit, code, .. } => {
                assert_eq!(op, "start");
                assert_eq!(unit, "a.service");
                assert_eq!(code, 1);
            }
            other => panic!("expected NonZeroExit, got {other:?}"),
        }
        // Other ops on the same unit still succeed.
        assert!(m.stop_service("a.service").is_ok());
    }

    #[test]
    fn not_supported_manager_short_circuits_every_op() {
        let m = NotSupportedServiceManager::new("test reason".to_string());
        for outcome in [
            m.probe_service("x.service").unwrap(),
            m.start_service("x.service").unwrap(),
            m.stop_service("x.service").unwrap(),
            m.restart_service("x.service").unwrap(),
            m.enable_service("x.service").unwrap(),
            m.disable_service("x.service").unwrap(),
        ] {
            assert_eq!(outcome.state, ServiceState::NotSupported);
            assert!(!outcome.supported);
            assert!(!outcome.changed);
            assert_eq!(outcome.manager, "not-supported");
            assert_eq!(outcome.message, "test reason");
        }
    }

    #[test]
    fn factory_returns_systemd_only_for_linux_system_no_container() {
        let m = for_install_mode("system", &fake_env("linux", None));
        assert_eq!(m.manager(), "systemd");
        assert!(m.supported());
    }

    #[test]
    fn factory_skips_user_install_mode_on_linux() {
        let m = for_install_mode("user", &fake_env("linux", None));
        assert!(!m.supported());
        assert_eq!(m.manager(), "not-supported");
        assert!(m.unsupported_reason().unwrap().contains("install_mode"));
    }

    #[test]
    fn factory_skips_non_linux_hosts() {
        let m = for_install_mode("system", &fake_env("darwin", None));
        assert!(!m.supported());
        assert!(m.unsupported_reason().unwrap().contains("darwin"));
    }

    #[test]
    fn factory_skips_inside_containers() {
        let m = for_install_mode("system", &fake_env("linux", Some("docker")));
        assert!(!m.supported());
        assert!(m.unsupported_reason().unwrap().contains("docker"));
    }

    /// `record_service_op` must emit one `LogKind::Component` line per
    /// call with `command: "service:<op>"`, the component + unit
    /// captured, install_mode stamped, and the parent operation_id
    /// threaded through. Severity is Info on success and Warn on a
    /// reported error so audit consumers can grep failed teardowns
    /// without having to parse the message text.
    #[test]
    fn record_service_op_writes_kind_component_lines_with_correct_severity() {
        use crate::central_log::CentralLog;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("central.log");
        let log = CentralLog::open(path.clone());

        record_service_op(
            Some(&log),
            ServiceOp::Start,
            "agentsight",
            "agentsight.service",
            "op-test-001",
            "tester",
            "system",
            None,
        );
        record_service_op(
            Some(&log),
            ServiceOp::Stop,
            "agentsight",
            "agentsight.service",
            "op-test-001",
            "tester",
            "system",
            Some("systemctl exited 1"),
        );

        let content = std::fs::read_to_string(&path).expect("read log");
        let lines: Vec<serde_json::Value> = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("parse line"))
            .collect();
        assert_eq!(lines.len(), 2, "expected one record per op");

        let ok = &lines[0];
        assert_eq!(ok.get("kind").and_then(|v| v.as_str()), Some("component"));
        assert_eq!(
            ok.get("command").and_then(|v| v.as_str()),
            Some("service:start"),
        );
        assert_eq!(
            ok.get("severity").and_then(|v| v.as_str()),
            Some("info"),
            "successful service ops must be Info",
        );
        assert_eq!(
            ok.get("component").and_then(|v| v.as_str()),
            Some("agentsight"),
        );
        assert_eq!(
            ok.get("install_mode").and_then(|v| v.as_str()),
            Some("system"),
        );
        assert_eq!(
            ok.get("operation_id").and_then(|v| v.as_str()),
            Some("op-test-001"),
        );
        assert_eq!(
            ok.get("details")
                .and_then(|v| v.get("unit"))
                .and_then(|v| v.as_str()),
            Some("agentsight.service"),
        );

        let err = &lines[1];
        assert_eq!(
            err.get("command").and_then(|v| v.as_str()),
            Some("service:stop"),
        );
        assert_eq!(
            err.get("severity").and_then(|v| v.as_str()),
            Some("warn"),
            "service ops that errored must be Warn so audit pipelines can grep",
        );
        let msg = err.get("message").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            msg.contains("systemctl exited 1") && msg.contains("agentsight.service"),
            "warn message must carry the underlying error and unit: {msg}",
        );
    }

    /// `record_service_op` is a no-op when no log handle is provided.
    /// Pins the contract so callers that want to skip auditing (e.g.
    /// in-tree service-start fallbacks for hosts where the central log
    /// is read-only) don't accidentally pay an empty-string write.
    #[test]
    fn record_service_op_with_no_log_handle_is_a_noop() {
        record_service_op(
            None,
            ServiceOp::Start,
            "agentsight",
            "agentsight.service",
            "op-test-002",
            "tester",
            "system",
            None,
        );
        // No assertion needed — the contract is "do not panic, do not
        // touch disk". A future regression that introduces an unwrap on
        // the optional log would surface here.
    }

    /// When the resolved `ServiceManager` is the not-supported stub
    /// (macOS dev, `--install-mode user`, container), the verb still
    /// must leave an audit trail so operators can tell "no services
    /// declared" from "services declared but no manager available".
    /// Pins the wire contract: kind=component, command=service:<op>,
    /// severity=Info (this is documented behaviour, not a fault),
    /// details.supported=false, details.reason carries the manager's
    /// own unsupported reason verbatim.
    #[test]
    fn record_service_op_unsupported_writes_supported_false_details() {
        use crate::central_log::CentralLog;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("central.log");
        let log = CentralLog::open(path.clone());

        record_service_op_unsupported(
            Some(&log),
            ServiceOp::Start,
            "agentsight",
            "agentsight.service",
            "op-test-003",
            "tester",
            "user",
            "not-supported",
            Some("install_mode=user is not supported by systemd manager"),
        );

        let content = std::fs::read_to_string(&path).expect("read log");
        let lines: Vec<serde_json::Value> = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("parse line"))
            .collect();
        assert_eq!(lines.len(), 1);
        let rec = &lines[0];
        assert_eq!(rec.get("kind").and_then(|v| v.as_str()), Some("component"));
        assert_eq!(
            rec.get("command").and_then(|v| v.as_str()),
            Some("service:start"),
        );
        assert_eq!(
            rec.get("severity").and_then(|v| v.as_str()),
            Some("info"),
            "unsupported skip is documented behaviour, not a fault — must be Info",
        );
        assert_eq!(
            rec.get("install_mode").and_then(|v| v.as_str()),
            Some("user"),
        );
        let details = rec.get("details").expect("details present");
        assert_eq!(
            details.get("supported").and_then(|v| v.as_bool()),
            Some(false)
        );
        assert_eq!(
            details.get("manager").and_then(|v| v.as_str()),
            Some("not-supported"),
        );
        assert!(
            details
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .contains("install_mode=user"),
        );
    }

    /// `record_service_op_unsupported` is a no-op without a log handle.
    /// Mirrors `record_service_op_with_no_log_handle_is_a_noop` so both
    /// helpers share the "do not panic on `None`" contract.
    #[test]
    fn record_service_op_unsupported_with_no_log_handle_is_a_noop() {
        record_service_op_unsupported(
            None,
            ServiceOp::Stop,
            "agentsight",
            "agentsight.service",
            "op-test-004",
            "tester",
            "system",
            "not-supported",
            Some("nope"),
        );
    }
}
