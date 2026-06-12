//! Audit subsystem: PEP, PDP, log.
//!
//! See `docs/audit-design.md` for the design. Public surface:
//!
//! - [`check`] — full PEP→PDP→log cycle; returns a `Decision` and writes
//!   a redacted `LogEntry` to the on-disk audit log.
//! - [`evaluate::evaluate`] — pure PDP, no I/O. Takes an `Action` and a
//!   `LoadedPolicy`, returns a `Decision`.
//! - [`policy::LoadedPolicy`] — policy + source + version metadata.
//!   `LoadedPolicy::load()` resolves the active policy from env / user file
//!   / `/etc` / built-in.
//! - [`builtin`] — three embedded presets (permissive / balanced / strict).
//! - [`action::parse_action_string`] — raw shell string → `Action`.
//!   Rejects shell metacharacters; callers map errors to `Outcome::Deny`.
//! - [`log::read_entries`] / [`log::audit_log_path`] — log file access.
//! - [`redact::redact_action`] — sensitive-field scrubber (used internally
//!   on log write; exported for completeness).

pub mod action;
pub mod builtin;
pub mod evaluate;
pub mod glob;
pub mod log;
pub mod policy;
pub mod redact;

pub use action::{parse_action_string, ParseError};
pub use builtin::BuiltinPreset;
pub use evaluate::evaluate;
pub use policy::{LoadedPolicy, PolicySource};

use chrono::Utc;
use cosh_types::audit::{Action, Decision, LogEntry, LogSource};
use cosh_types::error::CoshError;

/// Call-site identity captured for each audit log entry.
#[derive(Debug, Clone)]
pub struct CallerInfo {
    pub session_id: String,
    pub user: String,
    pub uid: u32,
    pub euid: u32,
    pub sudo_user: Option<String>,
    pub pid: u32,
}

impl CallerInfo {
    /// Inspect environment and process identity to build a `CallerInfo`.
    /// Best-effort: missing pieces fall back to `"unknown"` / `0` rather
    /// than failing the audit call.
    pub fn detect() -> Self {
        let user = std::env::var("USER")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("LOGNAME").ok().filter(|s| !s.is_empty()))
            .unwrap_or_else(|| "unknown".to_string());
        let session_id = std::env::var("COSH_SESSION_ID")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                format!("p{}-t{}", std::process::id(), Utc::now().timestamp())
            });
        let sudo_user = std::env::var("SUDO_USER").ok().filter(|s| !s.is_empty());
        let uid = nix::unistd::Uid::current().as_raw();
        let euid = nix::unistd::Uid::effective().as_raw();
        Self {
            session_id,
            user,
            uid,
            euid,
            sudo_user,
            pid: std::process::id(),
        }
    }
}

/// Run a full PEP→PDP→log cycle. The action is evaluated under `loaded`,
/// then a redacted copy is appended to the audit log. On log-write failure
/// the produced Decision is embedded in `error.details["decision"]` so the
/// caller can surface the verdict alongside the 402 error.
pub fn check(
    action: Action,
    source: LogSource,
    loaded: &LoadedPolicy,
) -> Result<Decision, CoshError> {
    let decision = evaluate(&action, loaded);
    if let Err(mut e) = record_to_log(action, &decision, source) {
        if let Ok(v) = serde_json::to_value(&decision) {
            e = e.with_details(serde_json::json!({ "decision": v }));
        }
        return Err(e);
    }
    Ok(decision)
}

/// Record a pre-decided `Decision` against an `Action` without re-running
/// the PDP. Used when a PEP has reached a verdict outside the regular
/// evaluate path — e.g. the CLI synthesizing `Outcome::Deny` for a raw
/// action string that failed to parse (audit-design.md §4 step "parse
/// failed").
pub fn record_decision(
    action: Action,
    decision: &Decision,
    source: LogSource,
) -> Result<(), CoshError> {
    record_to_log(action, decision, source)
}

/// Variant of `check` that produces a `Decision` without touching the log.
/// Provided so the TUI's existing per-tool `is_safe` callback (a synchronous
/// classification step that runs before the user sees an action, not as
/// part of the audit-traceable execution path) can reuse the PDP without
/// generating one log entry per keystroke. The execute path must call
/// `check` proper to ensure the decision is recorded.
pub fn classify(action: &Action, loaded: &LoadedPolicy) -> Decision {
    evaluate(action, loaded)
}

fn record_to_log(
    mut action: Action,
    decision: &Decision,
    source: LogSource,
) -> Result<(), CoshError> {
    let redacted = redact::redact_action(&mut action);
    let caller = CallerInfo::detect();
    let entry = LogEntry {
        timestamp: Utc::now(),
        session_id: caller.session_id,
        user: caller.user,
        uid: caller.uid,
        euid: caller.euid,
        sudo_user: caller.sudo_user,
        pid: caller.pid,
        action,
        decision: decision.clone(),
        source,
        redacted,
    };
    log::write_entry(&entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosh_types::audit::{ActionSubsystem, Outcome};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// Tests that mutate the `COSH_AUDIT_LOG` env var must hold this lock —
    /// `std::env::set_var` is process-global, so parallel test threads would
    /// otherwise race and overwrite each other's log paths.
    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner())
    }

    struct LogEnvGuard {
        _dir: tempfile::TempDir,
        _lock: MutexGuard<'static, ()>,
    }

    fn temp_log_env() -> LogEnvGuard {
        let lock = env_lock();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("COSH_AUDIT_LOG", dir.path().join("audit.log"));
        LogEnvGuard { _dir: dir, _lock: lock }
    }

    fn pkg_install() -> Action {
        Action {
            subsystem: ActionSubsystem::Pkg,
            operation: "install".to_string(),
            target: Some("nginx".to_string()),
            args: vec![],
            raw: Some("pkg install nginx".to_string()),
        }
    }

    #[test]
    fn check_balanced_pkg_install_is_require_approval_and_writes_log() {
        let _guard = temp_log_env();
        let loaded = builtin::balanced();
        let decision = check(pkg_install(), LogSource::Cli, &loaded).unwrap();
        assert_eq!(decision.outcome, Outcome::RequireApproval);
        // verify the log file got an entry
        let path = log::audit_log_path();
        let entries = log::read_entries(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action.operation, "install");
        assert_eq!(entries[0].decision.outcome, Outcome::RequireApproval);
    }

    #[test]
    fn caller_info_detect_does_not_panic() {
        // Just smoke: CallerInfo::detect() must be infallible.
        let info = CallerInfo::detect();
        assert!(!info.session_id.is_empty());
        assert!(!info.user.is_empty());
    }

    #[test]
    fn redact_path_marks_entry_redacted() {
        let _guard = temp_log_env();
        let loaded = builtin::balanced();
        let action = Action {
            subsystem: ActionSubsystem::Pkg,
            operation: "install".to_string(),
            target: Some("nginx".to_string()),
            args: vec![("password".to_string(), "hunter2".to_string())],
            raw: None,
        };
        check(action, LogSource::Cli, &loaded).unwrap();
        let entries = log::read_entries(&log::audit_log_path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].redacted, "expected redacted=true on log entry");
        assert_eq!(entries[0].action.args[0].1, "<redacted>");
    }
}
