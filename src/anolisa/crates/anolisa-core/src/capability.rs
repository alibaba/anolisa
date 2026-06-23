//! Linux file-capability (`setcap`) executor.
//!
//! Some components are only usable after an install-time `setcap` —
//! agentsight needs `cap_bpf,cap_perfmon` on its binary so eBPF loads for
//! non-root users. File capabilities are xattrs on the target filesystem,
//! applied after the file lands; packaging (tar) cannot carry them
//! reliably, so ANOLISA applies them as an install-time operation.
//!
//! The trait surface is one operation — [`CapabilityManager::apply`] —
//! deliberately small so it reads like the [`crate::service`] executor.
//! Hosts outside the alpha factory rules (non-Linux, `install_mode ==
//! "user"`, container) get a [`NotSupportedCapabilityManager`] whose
//! `apply` is a quiet skip (`supported: false`): setcap is meaningless
//! without root + a system-mode layout, so those installs are no-ops for
//! capabilities.
//!
//! [`FakeCapabilityManager`] is the executor used by this module's unit
//! tests and by integration tests asserting which binaries were targeted.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use anolisa_env::EnvFacts;

/// What [`CapabilityManager`] returns from a single `apply`.
#[derive(Debug, Clone)]
pub struct CapabilityOutcome {
    /// Backend label, e.g. `"setcap"`, `"not-supported"`, or a fake name.
    pub manager: String,
    /// Binary the capabilities were applied to.
    pub path: PathBuf,
    /// Capability names requested (e.g. `["cap_bpf", "cap_perfmon"]`).
    pub caps: Vec<String>,
    /// `false` when the backend is a deliberate skip (user mode, container).
    pub supported: bool,
    /// `true` when capabilities were actually written. Always `false` for
    /// unsupported backends.
    pub changed: bool,
    /// Human-readable description, used for logs / warnings.
    pub message: String,
}

/// Failure surface for a single `apply`. Non-fatal by default — the
/// orchestrator decides warn-vs-abort from the spec's `optional` flag.
#[derive(Debug, thiserror::Error)]
pub enum CapabilityError {
    /// `setcap` could not be spawned (missing binary, no permission).
    #[error("spawning setcap failed: {source}")]
    Spawn {
        /// Underlying spawn error.
        #[source]
        source: std::io::Error,
    },
    /// `setcap` ran but returned a non-zero exit code (no xattr support,
    /// not root, malformed capability string).
    #[error("setcap '{caps}' {path} exited with status {code}: {stderr}")]
    NonZeroExit {
        /// Capability string passed to setcap.
        caps: String,
        /// Target path.
        path: String,
        /// Process exit code.
        code: i32,
        /// Captured stderr.
        stderr: String,
    },
}

/// Backend trait every capability-manager impl satisfies.
pub trait CapabilityManager: Send + Sync {
    /// Wire label written into log records.
    fn manager(&self) -> &str;
    /// `false` when this backend is a deliberate skip — orchestrators
    /// short-circuit so they don't emit per-binary warnings.
    fn supported(&self) -> bool;
    /// Reason text when [`supported`](Self::supported) is false.
    fn unsupported_reason(&self) -> Option<&str> {
        None
    }
    /// Apply `caps` to `path` (`setcap "cap_bpf,cap_perfmon+ep" <path>`).
    fn apply(&self, path: &Path, caps: &[String]) -> Result<CapabilityOutcome, CapabilityError>;
}

/// `setcap` capability string for `caps`: lowercased, comma-joined, with
/// the `+ep` flag so the binary carries the caps in its effective and
/// permitted sets for any user who runs it.
fn setcap_arg(caps: &[String]) -> String {
    format!("{}+ep", caps.join(",").to_lowercase())
}

/// Real backend: shells out to `setcap`, mirroring how
/// [`crate::service::SystemdServiceManager`] drives `systemctl`.
pub struct SetcapManager {
    binary: PathBuf,
}

impl SetcapManager {
    /// Build a manager that invokes the `setcap` binary from `PATH`.
    pub fn new() -> Self {
        Self {
            binary: PathBuf::from("setcap"),
        }
    }
}

impl Default for SetcapManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CapabilityManager for SetcapManager {
    fn manager(&self) -> &str {
        "setcap"
    }
    fn supported(&self) -> bool {
        true
    }
    fn apply(&self, path: &Path, caps: &[String]) -> Result<CapabilityOutcome, CapabilityError> {
        let arg = setcap_arg(caps);
        let output = Command::new(&self.binary)
            .arg(&arg)
            .arg(path)
            .output()
            .map_err(|source| CapabilityError::Spawn { source })?;
        if !output.status.success() {
            let code = output.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(CapabilityError::NonZeroExit {
                caps: arg,
                path: path.display().to_string(),
                code,
                stderr,
            });
        }
        Ok(CapabilityOutcome {
            manager: "setcap".to_string(),
            path: path.to_path_buf(),
            caps: caps.to_vec(),
            supported: true,
            changed: true,
            message: format!("setcap {arg} {} ok", path.display()),
        })
    }
}

/// Quiet-skip backend for hosts where setcap is not actionable
/// (non-Linux, user mode, container).
pub struct NotSupportedCapabilityManager {
    reason: String,
}

impl NotSupportedCapabilityManager {
    /// Build a skip backend carrying `reason` for the audit log.
    pub fn new(reason: String) -> Self {
        Self { reason }
    }
}

impl CapabilityManager for NotSupportedCapabilityManager {
    fn manager(&self) -> &str {
        "not-supported"
    }
    fn supported(&self) -> bool {
        false
    }
    fn unsupported_reason(&self) -> Option<&str> {
        Some(&self.reason)
    }
    fn apply(&self, path: &Path, caps: &[String]) -> Result<CapabilityOutcome, CapabilityError> {
        Ok(CapabilityOutcome {
            manager: "not-supported".to_string(),
            path: path.to_path_buf(),
            caps: caps.to_vec(),
            supported: false,
            changed: false,
            message: self.reason.clone(),
        })
    }
}

/// Pick a [`CapabilityManager`] for the host + install mode.
///
/// Returns a real [`SetcapManager`] only on Linux, in `system` mode, and
/// outside a container. Root is intentionally *not* gated here: a non-root
/// host still gets the real manager so an `optional = false` capability
/// whose `setcap` fails aborts the install — a quiet skip would silently
/// swallow that failure. user-mode / non-Linux / container hosts get a
/// [`NotSupportedCapabilityManager`].
pub fn for_install_mode(install_mode: &str, env: &EnvFacts) -> Box<dyn CapabilityManager> {
    if env.os != "linux" {
        return Box::new(NotSupportedCapabilityManager::new(format!(
            "capability assignment unsupported on os '{}'",
            env.os,
        )));
    }
    if install_mode != "system" {
        return Box::new(NotSupportedCapabilityManager::new(
            "capability assignment unsupported in install_mode='user' (setcap needs system mode + root)"
                .to_string(),
        ));
    }
    if let Some(rt) = env.container.as_deref() {
        return Box::new(NotSupportedCapabilityManager::new(format!(
            "container runtime '{rt}' detected — refusing to setcap inside a container",
        )));
    }
    Box::new(SetcapManager::new())
}

/// In-memory [`CapabilityManager`] for tests: records every `apply` and
/// can be told to fail specific paths.
pub struct FakeCapabilityManager {
    manager_name: String,
    supported: bool,
    calls: Mutex<Vec<(PathBuf, Vec<String>)>>,
    fail_paths: Mutex<HashSet<PathBuf>>,
}

impl FakeCapabilityManager {
    /// Supported fake with no injected failures.
    pub fn new() -> Self {
        Self {
            manager_name: "fake".to_string(),
            supported: true,
            calls: Mutex::new(Vec::new()),
            fail_paths: Mutex::new(HashSet::new()),
        }
    }
    /// Snapshot of every `apply` recorded so far, in dispatch order.
    pub fn calls(&self) -> Vec<(PathBuf, Vec<String>)> {
        self.calls.lock().expect("poisoned").clone()
    }
    /// Cause `apply` on `path` to return `NonZeroExit` so tests can assert
    /// the orchestrator's warn / abort paths.
    pub fn fail(&self, path: &Path) {
        self.fail_paths
            .lock()
            .expect("poisoned")
            .insert(path.to_path_buf());
    }
}

impl Default for FakeCapabilityManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CapabilityManager for FakeCapabilityManager {
    fn manager(&self) -> &str {
        &self.manager_name
    }
    fn supported(&self) -> bool {
        self.supported
    }
    fn apply(&self, path: &Path, caps: &[String]) -> Result<CapabilityOutcome, CapabilityError> {
        self.calls
            .lock()
            .expect("poisoned")
            .push((path.to_path_buf(), caps.to_vec()));
        if self.fail_paths.lock().expect("poisoned").contains(path) {
            return Err(CapabilityError::NonZeroExit {
                caps: caps.join(","),
                path: path.display().to_string(),
                code: 1,
                stderr: "fake forced failure".to_string(),
            });
        }
        Ok(CapabilityOutcome {
            manager: self.manager_name.clone(),
            path: path.to_path_buf(),
            caps: caps.to_vec(),
            supported: self.supported,
            changed: true,
            message: format!("fake setcap {} ok", path.display()),
        })
    }
}

/// One resolved capability assignment to apply. `path` is already
/// layout-expanded and boundary-validated by the caller, so the executor
/// never touches manifest templates or the filesystem layout.
#[derive(Debug, Clone)]
pub struct CapabilityRequest {
    /// Binary to receive the capabilities (absolute, owned-root-checked).
    pub path: PathBuf,
    /// Capability names to grant.
    pub caps: Vec<String>,
    /// `true` → failure degrades to a warning; `false` → failure aborts
    /// the install and the caller rolls back.
    pub optional: bool,
}

/// Aggregate result of [`apply_capabilities`].
#[derive(Debug, Default)]
pub struct CapabilityRunOutcome {
    /// Count of capabilities successfully applied.
    pub applied: usize,
    /// Per-request warnings from tolerated (`optional`) failures.
    pub warnings: Vec<String>,
    /// `Some(reason)` when a required capability failed — the caller must
    /// roll back the install. Set at the first required failure; no further
    /// requests are attempted.
    pub aborted: Option<String>,
}

/// Apply each request against `manager`, deciding warn-vs-abort from each
/// request's `optional` flag and recording one central-log line per request.
///
/// - `manager.supported() == false`: every request is a quiet skip (one
///   `Info` audit line each), no warnings, never aborts.
/// - apply Ok: `Info` audit line, `applied += 1`.
/// - apply Err && `optional`: warning pushed, `Warn` audit line, continue.
/// - apply Err && !`optional`: `aborted` set, `Warn` audit line, **stop** —
///   later requests are not attempted so the caller's rollback is clean.
#[allow(clippy::too_many_arguments)]
pub fn apply_capabilities(
    manager: &dyn CapabilityManager,
    requests: &[CapabilityRequest],
    log: Option<&crate::central_log::CentralLog>,
    component: &str,
    operation_id: &str,
    actor: &str,
    install_mode: &str,
) -> CapabilityRunOutcome {
    let mut outcome = CapabilityRunOutcome::default();
    if !manager.supported() {
        for req in requests {
            record_capability_op_unsupported(
                log,
                &req.path,
                &req.caps,
                component,
                operation_id,
                actor,
                install_mode,
                manager.manager(),
                manager.unsupported_reason(),
            );
        }
        return outcome;
    }
    for req in requests {
        match manager.apply(&req.path, &req.caps) {
            Ok(_) => {
                record_capability_op(
                    log,
                    &req.path,
                    &req.caps,
                    component,
                    operation_id,
                    actor,
                    install_mode,
                    None,
                );
                outcome.applied += 1;
            }
            Err(err) => {
                let msg = err.to_string();
                record_capability_op(
                    log,
                    &req.path,
                    &req.caps,
                    component,
                    operation_id,
                    actor,
                    install_mode,
                    Some(&msg),
                );
                if req.optional {
                    outcome.warnings.push(format!(
                        "optional capability for {} failed: {msg}",
                        req.path.display()
                    ));
                } else {
                    outcome.aborted = Some(format!(
                        "required capability for {} failed: {msg}",
                        req.path.display()
                    ));
                    return outcome;
                }
            }
        }
    }
    outcome
}

/// Append a [`crate::central_log::LogKind::Component`] record for one
/// capability apply. `Info` on success, `Warn` when `error` is set (a
/// tolerated `optional` failure). Best-effort: log errors are swallowed
/// because the parent verb has already committed the file write.
// Capability audit records spell out path/caps/actor/mode so install call
// sites show the full audit context, mirroring `record_service_op`.
#[allow(clippy::too_many_arguments)]
pub fn record_capability_op(
    log: Option<&crate::central_log::CentralLog>,
    path: &Path,
    caps: &[String],
    component: &str,
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
            format!("capability apply ok for {}: {}", component, path.display()),
        ),
        Some(err) => (
            Severity::Warn,
            format!(
                "capability apply skipped for {}: {} ({err})",
                component,
                path.display()
            ),
        ),
    };
    let _ = log.append(&LogRecord {
        kind: LogKind::Component,
        operation_id: Some(operation_id.to_string()),
        command: "capability:apply".to_string(),
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
        details: serde_json::json!({"path": path.display().to_string(), "caps": caps}),
    });
}

/// Append a [`crate::central_log::LogKind::Component`] record for a
/// capability that was *skipped* because the resolved manager reports
/// `supported() == false` (non-Linux, user mode, container). `Info`
/// severity — this is the documented behavior, not a fault — and the
/// `details` carry `supported = false` plus the skip reason.
#[allow(clippy::too_many_arguments)]
pub fn record_capability_op_unsupported(
    log: Option<&crate::central_log::CentralLog>,
    path: &Path,
    caps: &[String],
    component: &str,
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
    let reason_str =
        unsupported_reason.unwrap_or("capability manager not supported on this platform");
    let message = format!(
        "capability apply skipped for {}: {} {} ({})",
        component,
        path.display(),
        manager_name,
        reason_str,
    );
    let _ = log.append(&LogRecord {
        kind: LogKind::Component,
        operation_id: Some(operation_id.to_string()),
        command: "capability:apply".to_string(),
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
            "path": path.display().to_string(),
            "caps": caps,
            "supported": false,
            "manager": manager_name,
            "reason": reason_str,
        }),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_manager_records_apply_calls_in_order() {
        let m = FakeCapabilityManager::new();
        m.apply(Path::new("/a"), &["cap_bpf".to_string()]).unwrap();
        m.apply(Path::new("/b"), &["cap_net_admin".to_string()])
            .unwrap();
        assert_eq!(
            m.calls(),
            vec![
                (PathBuf::from("/a"), vec!["cap_bpf".to_string()]),
                (PathBuf::from("/b"), vec!["cap_net_admin".to_string()]),
            ]
        );
    }

    #[test]
    fn fake_manager_can_force_failure_per_path() {
        let m = FakeCapabilityManager::new();
        m.fail(Path::new("/a"));
        let err = m
            .apply(Path::new("/a"), &["cap_bpf".to_string()])
            .unwrap_err();
        match err {
            CapabilityError::NonZeroExit { path, code, .. } => {
                assert_eq!(path, "/a");
                assert_eq!(code, 1);
            }
            other => panic!("expected NonZeroExit, got {other:?}"),
        }
        // A different path still succeeds.
        assert!(m.apply(Path::new("/b"), &["cap_bpf".to_string()]).is_ok());
    }

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
    fn factory_returns_setcap_for_linux_system_no_container() {
        let m = for_install_mode("system", &fake_env("linux", None));
        assert!(m.supported());
        assert_eq!(m.manager(), "setcap");
    }

    #[test]
    fn factory_skips_user_mode() {
        let m = for_install_mode("user", &fake_env("linux", None));
        assert!(!m.supported());
        assert!(m.unsupported_reason().unwrap().contains("user"));
    }

    #[test]
    fn factory_skips_non_linux() {
        let m = for_install_mode("system", &fake_env("darwin", None));
        assert!(!m.supported());
    }

    #[test]
    fn factory_skips_in_container() {
        let m = for_install_mode("system", &fake_env("linux", Some("docker")));
        assert!(!m.supported());
        let reason = m.unsupported_reason().unwrap();
        assert!(reason.contains("container") || reason.contains("docker"));
    }

    #[test]
    fn factory_allows_non_root() {
        // Root is deliberately NOT gated: a non-root host still gets the real
        // SetcapManager so optional=false failures can abort instead of being
        // silently skipped (a NotSupported skip would be wrong for "no root").
        let m = for_install_mode("system", &fake_env("linux", None));
        assert!(m.supported());
        assert_eq!(m.manager(), "setcap");
    }

    #[test]
    fn not_supported_manager_apply_is_quiet_skip() {
        let m = NotSupportedCapabilityManager::new("nope".to_string());
        let out = m.apply(Path::new("/a"), &["cap_bpf".to_string()]).unwrap();
        assert!(!out.supported);
        assert!(!out.changed);
        assert_eq!(out.manager, "not-supported");
    }

    #[test]
    fn setcap_arg_joins_caps_with_ep_flag() {
        assert_eq!(
            setcap_arg(&["CAP_BPF".to_string(), "CAP_PERFMON".to_string()]),
            "cap_bpf,cap_perfmon+ep"
        );
        assert_eq!(setcap_arg(&["cap_bpf".to_string()]), "cap_bpf+ep");
    }

    fn req(path: &str, optional: bool) -> CapabilityRequest {
        CapabilityRequest {
            path: PathBuf::from(path),
            caps: vec!["cap_bpf".to_string()],
            optional,
        }
    }

    #[test]
    fn apply_capabilities_applies_all_and_counts() {
        let m = FakeCapabilityManager::new();
        let reqs = vec![req("/a", false), req("/b", false)];
        let out = apply_capabilities(&m, &reqs, None, "comp", "op1", "cli", "system");
        assert_eq!(out.applied, 2);
        assert!(out.warnings.is_empty());
        assert!(out.aborted.is_none());
    }

    #[test]
    fn apply_capabilities_unsupported_manager_skips_silently() {
        let m = NotSupportedCapabilityManager::new("user mode".to_string());
        let reqs = vec![req("/a", false)];
        let out = apply_capabilities(&m, &reqs, None, "comp", "op1", "cli", "user");
        assert_eq!(out.applied, 0);
        assert!(out.warnings.is_empty());
        assert!(out.aborted.is_none());
    }

    #[test]
    fn apply_capabilities_optional_failure_warns_and_continues() {
        let m = FakeCapabilityManager::new();
        m.fail(Path::new("/a"));
        let reqs = vec![req("/a", true), req("/b", false)];
        let out = apply_capabilities(&m, &reqs, None, "comp", "op1", "cli", "system");
        assert_eq!(out.applied, 1);
        assert_eq!(out.warnings.len(), 1);
        assert!(out.warnings[0].contains("/a"));
        assert!(out.aborted.is_none());
    }

    #[test]
    fn apply_capabilities_required_failure_aborts_and_stops() {
        let m = FakeCapabilityManager::new();
        m.fail(Path::new("/a"));
        let reqs = vec![req("/a", false), req("/b", false)];
        let out = apply_capabilities(&m, &reqs, None, "comp", "op1", "cli", "system");
        let reason = out.aborted.expect("must abort on required failure");
        assert!(reason.contains("/a"));
        // Stopped at /a — /b was never attempted (rollback relies on this).
        let calls = m.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, PathBuf::from("/a"));
    }

    /// One `LogKind::Component` line per apply with `command:
    /// "capability:apply"`, the component + install_mode + operation_id
    /// stamped, and `details.path` / `details.caps` carried. Severity is
    /// Info on success and Warn when an error is reported (a tolerated
    /// optional failure) so audit pipelines can grep failed grants.
    #[test]
    fn record_capability_op_writes_kind_component_lines_with_correct_severity() {
        use crate::central_log::CentralLog;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("central.log");
        let log = CentralLog::open(path.clone());

        record_capability_op(
            Some(&log),
            Path::new("/opt/agentsight/bin/agentsight"),
            &["cap_bpf".to_string(), "cap_perfmon".to_string()],
            "agentsight",
            "op-cap-001",
            "tester",
            "system",
            None,
        );
        record_capability_op(
            Some(&log),
            Path::new("/opt/agentsight/bin/agentsight"),
            &["cap_bpf".to_string()],
            "agentsight",
            "op-cap-001",
            "tester",
            "system",
            Some("setcap exited 1"),
        );

        let content = std::fs::read_to_string(&path).expect("read log");
        let lines: Vec<serde_json::Value> = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("parse line"))
            .collect();
        assert_eq!(lines.len(), 2, "expected one record per apply");

        let ok = &lines[0];
        assert_eq!(ok.get("kind").and_then(|v| v.as_str()), Some("component"));
        assert_eq!(
            ok.get("command").and_then(|v| v.as_str()),
            Some("capability:apply"),
        );
        assert_eq!(
            ok.get("severity").and_then(|v| v.as_str()),
            Some("info"),
            "successful capability applies must be Info",
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
            Some("op-cap-001"),
        );
        assert_eq!(
            ok.get("details")
                .and_then(|v| v.get("path"))
                .and_then(|v| v.as_str()),
            Some("/opt/agentsight/bin/agentsight"),
        );
        assert_eq!(
            ok.get("details")
                .and_then(|v| v.get("caps"))
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(2),
        );

        let err = &lines[1];
        assert_eq!(
            err.get("severity").and_then(|v| v.as_str()),
            Some("warn"),
            "capability applies that errored must be Warn so audit pipelines can grep",
        );
        let msg = err.get("message").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            msg.contains("setcap exited 1") && msg.contains("/opt/agentsight/bin/agentsight"),
            "warn message must carry the underlying error and path: {msg}",
        );
    }

    /// `record_capability_op` is a no-op without a log handle — do not
    /// panic, do not touch disk. A future regression that unwraps the
    /// optional log would surface here.
    #[test]
    fn record_capability_op_with_no_log_handle_is_a_noop() {
        record_capability_op(
            None,
            Path::new("/opt/agentsight/bin/agentsight"),
            &["cap_bpf".to_string()],
            "agentsight",
            "op-cap-002",
            "tester",
            "system",
            None,
        );
    }

    /// When the resolved manager is the not-supported stub (non-Linux,
    /// user mode, container), the verb still leaves an audit trail so
    /// operators can tell "no caps declared" from "caps declared but no
    /// manager available". Pins the wire contract: kind=component,
    /// command=capability:apply, severity=Info (documented behaviour, not
    /// a fault), details.supported=false, details.reason verbatim.
    #[test]
    fn record_capability_op_unsupported_writes_supported_false_details() {
        use crate::central_log::CentralLog;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("central.log");
        let log = CentralLog::open(path.clone());

        record_capability_op_unsupported(
            Some(&log),
            Path::new("/opt/agentsight/bin/agentsight"),
            &["cap_bpf".to_string()],
            "agentsight",
            "op-cap-003",
            "tester",
            "user",
            "not-supported",
            Some("install_mode=user is not supported by setcap manager"),
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
            Some("capability:apply"),
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

    /// `record_capability_op_unsupported` is a no-op without a log handle.
    #[test]
    fn record_capability_op_unsupported_with_no_log_handle_is_a_noop() {
        record_capability_op_unsupported(
            None,
            Path::new("/opt/agentsight/bin/agentsight"),
            &["cap_bpf".to_string()],
            "agentsight",
            "op-cap-004",
            "tester",
            "system",
            "not-supported",
            Some("nope"),
        );
    }
}
