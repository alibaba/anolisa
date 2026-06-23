//! TOML-based security configuration.
//!
//! Loaded from `skillfs-security.toml` via `--config <PATH>`. CLI flags
//! override values from the config file.

use std::fmt;
use std::path::Path;

use serde::Deserialize;

use super::activation::ActivationMode;
use super::activation_reload::ReloadMode;
use super::install::{
    StagingConfig, UnactivatedVisibility, validate_post_publish_patterns, validate_staging_patterns,
};
use super::refresh::FailedResolveBehavior;

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SecurityConfig {
    pub decision: Option<DecisionConfig>,
    pub policy: Option<PolicyConfig>,
    pub audit: Option<AuditSection>,
    pub events: Option<EventsSection>,
    pub trusted_writer: Option<TrustedWriterSection>,
    pub activation: Option<ActivationSection>,
    pub notify: Option<NotifySection>,
    pub activation_events: Option<ActivationEventsSection>,
    pub ledger: Option<LedgerSection>,
    pub install: Option<InstallSection>,
    pub control_socket: Option<ControlSocketSection>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionConfig {
    pub command: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyConfig {
    /// Risk response when the decision provider flags findings.
    /// Accepted values: `"fallback"`, `"warn"`, `"hide"`.
    /// Parsed and validated here but consumed by the external decision
    /// provider — SkillFS does not act on this field directly.
    pub on_risk: Option<String>,
    /// Behavior when resolve fails. `"hide"` (default) replaces the
    /// mapping with Hidden; `"keep_previous"` leaves the existing
    /// mapping unchanged. Consumed by [`RefreshController`].
    pub on_failure: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditSection {
    pub log_path: Option<String>,
    pub queue_capacity: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventsSection {
    pub log_path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedWriterSection {
    pub process_name: Option<String>,
    pub exe: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationSection {
    /// `"off"` (default) or `"file"`.
    pub mode: Option<String>,
    /// `"off"` (default) or `"poll"`.
    pub reload: Option<String>,
    /// Poll interval in milliseconds (default 250).
    pub reload_interval_ms: Option<u64>,
    /// Total poll timeout in milliseconds (default 5000).
    pub reload_timeout_ms: Option<u64>,
    /// A5: periodic watcher interval in milliseconds (default 30000).
    pub watcher_interval_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotifySection {
    /// `"off"` (default) or `"unix-socket"`.
    pub mode: Option<String>,
    pub socket_path: Option<String>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationEventsSection {
    pub log_path: Option<String>,
}

/// A6/B1: Ledger backing root configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerSection {
    /// Private source-side work path for external daemons.
    /// When set, all daemon-facing operations (notify skillDir,
    /// activation bootstrap, activation reload, startup reconcile,
    /// activation watcher) use this path instead of the source.
    pub backing_root: Option<String>,
}

/// I2/I4: Installer staging configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallSection {
    pub staging_patterns: Option<Vec<String>>,
    pub unactivated_visibility: Option<String>,
    pub quiet_timeout_ms: Option<u64>,
    pub post_publish_grace_ms: Option<u64>,
    pub post_publish_write_patterns: Option<Vec<String>>,
}

/// Trusted peer control socket configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlSocketSection {
    pub path: Option<String>,
    pub trusted_peer_exe: Option<String>,
    pub trusted_peer_uid: Option<u32>,
    pub trusted_peer_gid: Option<u32>,
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(toml::de::Error),
    InvalidValue {
        field: &'static str,
        value: String,
        allowed: &'static str,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "config I/O error: {e}"),
            ConfigError::Parse(e) => write!(f, "config parse error: {e}"),
            ConfigError::InvalidValue {
                field,
                value,
                allowed,
            } => write!(
                f,
                "config: invalid value '{value}' for {field}; allowed: {allowed}"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

impl SecurityConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path).map_err(ConfigError::Io)?;
        let cfg: Self = toml::from_str(&content).map_err(ConfigError::Parse)?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if let Some(value) = self.policy.as_ref().and_then(|p| p.on_failure.as_deref()) {
            match value {
                "hide" | "keep_previous" => {}
                other => {
                    return Err(ConfigError::InvalidValue {
                        field: "policy.on_failure",
                        value: other.to_string(),
                        allowed: "hide, keep_previous",
                    });
                }
            }
        }
        if let Some(value) = self.policy.as_ref().and_then(|p| p.on_risk.as_deref()) {
            match value {
                "fallback" | "warn" | "hide" => {}
                other => {
                    return Err(ConfigError::InvalidValue {
                        field: "policy.on_risk",
                        value: other.to_string(),
                        allowed: "fallback, warn, hide",
                    });
                }
            }
        }
        if let Some(value) = self.activation.as_ref().and_then(|a| a.mode.as_deref()) {
            if ActivationMode::parse(value).is_none() {
                return Err(ConfigError::InvalidValue {
                    field: "activation.mode",
                    value: value.to_string(),
                    allowed: "off, file",
                });
            }
        }
        if let Some(value) = self.activation.as_ref().and_then(|a| a.reload.as_deref()) {
            if ReloadMode::parse(value).is_none() {
                return Err(ConfigError::InvalidValue {
                    field: "activation.reload",
                    value: value.to_string(),
                    allowed: "off, poll",
                });
            }
        }
        if let Some(t) = self.activation.as_ref().and_then(|a| a.reload_interval_ms) {
            if t == 0 || t > 60_000 {
                return Err(ConfigError::InvalidValue {
                    field: "activation.reload_interval_ms",
                    value: t.to_string(),
                    allowed: "1..60000",
                });
            }
        }
        if let Some(t) = self.activation.as_ref().and_then(|a| a.reload_timeout_ms) {
            if t == 0 || t > 300_000 {
                return Err(ConfigError::InvalidValue {
                    field: "activation.reload_timeout_ms",
                    value: t.to_string(),
                    allowed: "1..300000",
                });
            }
        }
        if let Some(ref act) = self.activation {
            if let (Some(interval), Some(timeout)) = (act.reload_interval_ms, act.reload_timeout_ms)
            {
                if interval > timeout {
                    return Err(ConfigError::InvalidValue {
                        field: "activation.reload_interval_ms",
                        value: interval.to_string(),
                        allowed: "must be <= activation.reload_timeout_ms",
                    });
                }
            }
        }
        if let Some(t) = self.activation.as_ref().and_then(|a| a.watcher_interval_ms) {
            if !(1000..=300_000).contains(&t) {
                return Err(ConfigError::InvalidValue {
                    field: "activation.watcher_interval_ms",
                    value: t.to_string(),
                    allowed: "1000..300000",
                });
            }
        }
        if let Some(ref notify) = self.notify {
            let mode = notify.mode.as_deref().unwrap_or("off");
            match mode {
                "off" | "unix-socket" => {}
                other => {
                    return Err(ConfigError::InvalidValue {
                        field: "notify.mode",
                        value: other.to_string(),
                        allowed: "off, unix-socket",
                    });
                }
            }
            if mode == "unix-socket" {
                let has_path = notify
                    .socket_path
                    .as_deref()
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false);
                if !has_path {
                    return Err(ConfigError::InvalidValue {
                        field: "notify.socket_path",
                        value: String::new(),
                        allowed: "non-empty path when notify.mode = unix-socket",
                    });
                }
            }
            if let Some(t) = notify.timeout_ms {
                if t == 0 || t > 300_000 {
                    return Err(ConfigError::InvalidValue {
                        field: "notify.timeout_ms",
                        value: t.to_string(),
                        allowed: "1..300000",
                    });
                }
            }
        }
        if let Some(ref install) = self.install {
            if let Some(ref vis) = install.unactivated_visibility {
                if UnactivatedVisibility::parse(vis).is_none() {
                    return Err(ConfigError::InvalidValue {
                        field: "install.unactivated_visibility",
                        value: vis.to_string(),
                        allowed: "hidden",
                    });
                }
            }
            if let Some(ref patterns) = install.staging_patterns {
                validate_staging_patterns(patterns)?;
            }
            if let Some(t) = install.quiet_timeout_ms {
                if !(100..=300_000).contains(&t) {
                    return Err(ConfigError::InvalidValue {
                        field: "install.quiet_timeout_ms",
                        value: t.to_string(),
                        allowed: "100..300000",
                    });
                }
            }
            // I4: post-publish grace validation — both or neither.
            let has_grace = install.post_publish_grace_ms.is_some();
            let has_pp_patterns = install
                .post_publish_write_patterns
                .as_ref()
                .map(|p| !p.is_empty())
                .unwrap_or(false);
            let has_pp_patterns_field = install.post_publish_write_patterns.is_some();
            if has_grace && !has_pp_patterns {
                return Err(ConfigError::InvalidValue {
                    field: "install.post_publish_write_patterns",
                    value: String::new(),
                    allowed: "non-empty pattern list when post_publish_grace_ms is set",
                });
            }
            if has_pp_patterns && !has_grace {
                return Err(ConfigError::InvalidValue {
                    field: "install.post_publish_grace_ms",
                    value: String::new(),
                    allowed: "must be set when post_publish_write_patterns is configured",
                });
            }
            if has_pp_patterns_field && !has_pp_patterns && !has_grace {
                // Empty patterns list without grace: no-op, allowed.
            }
            if let Some(t) = install.post_publish_grace_ms {
                if !(100..=300_000).contains(&t) {
                    return Err(ConfigError::InvalidValue {
                        field: "install.post_publish_grace_ms",
                        value: t.to_string(),
                        allowed: "100..300000",
                    });
                }
            }
            if let Some(ref patterns) = install.post_publish_write_patterns {
                if !patterns.is_empty() {
                    validate_post_publish_patterns(patterns)?;
                }
            }
        }
        if let Some(ref cs) = self.control_socket {
            let has_path = cs
                .path
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            let has_exe = cs
                .trusted_peer_exe
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if has_path && !has_exe {
                return Err(ConfigError::InvalidValue {
                    field: "control_socket.trusted_peer_exe",
                    value: String::new(),
                    allowed: "non-empty path when control_socket.path is set",
                });
            }
            if !has_path && has_exe {
                return Err(ConfigError::InvalidValue {
                    field: "control_socket.path",
                    value: String::new(),
                    allowed: "non-empty path when control_socket.trusted_peer_exe is set",
                });
            }
        }
        Ok(())
    }

    pub fn failed_resolve_behavior(&self) -> FailedResolveBehavior {
        match self.policy.as_ref().and_then(|p| p.on_failure.as_deref()) {
            Some("keep_previous") => FailedResolveBehavior::KeepPreviousMapping,
            _ => FailedResolveBehavior::HideOnFailure,
        }
    }

    pub fn decision_command(&self) -> Option<&str> {
        self.decision
            .as_ref()
            .and_then(|d| d.command.as_deref())
            .filter(|s| !s.trim().is_empty())
    }

    pub fn events_log_path(&self) -> Option<&str> {
        self.events
            .as_ref()
            .and_then(|e| e.log_path.as_deref())
            .filter(|s| !s.trim().is_empty())
    }

    pub fn audit_log_path(&self) -> Option<&str> {
        self.audit
            .as_ref()
            .and_then(|a| a.log_path.as_deref())
            .filter(|s| !s.trim().is_empty())
    }

    pub fn audit_queue_capacity(&self) -> Option<usize> {
        self.audit.as_ref().and_then(|a| a.queue_capacity)
    }

    pub fn trusted_writer_name(&self) -> Option<&str> {
        self.trusted_writer
            .as_ref()
            .and_then(|t| t.process_name.as_deref())
            .filter(|s| !s.trim().is_empty())
    }

    pub fn trusted_writer_exe(&self) -> Option<&str> {
        self.trusted_writer
            .as_ref()
            .and_then(|t| t.exe.as_deref())
            .filter(|s| !s.trim().is_empty())
    }

    pub fn notify_mode(&self) -> &str {
        self.notify
            .as_ref()
            .and_then(|n| n.mode.as_deref())
            .unwrap_or("off")
    }

    /// Returns the socket path only when `notify.mode = "unix-socket"` AND a
    /// non-empty `socket_path` is present. `mode = "off"` always returns
    /// `None` even when `socket_path` is set.
    pub fn notify_socket_path(&self) -> Option<&str> {
        if self.notify_mode() != "unix-socket" {
            return None;
        }
        self.notify
            .as_ref()
            .and_then(|n| n.socket_path.as_deref())
            .filter(|s| !s.trim().is_empty())
    }

    pub fn notify_timeout_ms(&self) -> Option<u64> {
        self.notify.as_ref().and_then(|n| n.timeout_ms)
    }

    pub fn activation_events_log_path(&self) -> Option<&str> {
        self.activation_events
            .as_ref()
            .and_then(|a| a.log_path.as_deref())
            .filter(|s| !s.trim().is_empty())
    }

    pub fn activation_mode(&self) -> ActivationMode {
        self.activation
            .as_ref()
            .and_then(|a| a.mode.as_deref())
            .and_then(ActivationMode::parse)
            .unwrap_or_default()
    }

    pub fn reload_mode(&self) -> ReloadMode {
        self.activation
            .as_ref()
            .and_then(|a| a.reload.as_deref())
            .and_then(ReloadMode::parse)
            .unwrap_or_default()
    }

    pub fn reload_interval_ms(&self) -> Option<u64> {
        self.activation.as_ref().and_then(|a| a.reload_interval_ms)
    }

    pub fn reload_timeout_ms(&self) -> Option<u64> {
        self.activation.as_ref().and_then(|a| a.reload_timeout_ms)
    }

    pub fn watcher_interval_ms(&self) -> Option<u64> {
        self.activation.as_ref().and_then(|a| a.watcher_interval_ms)
    }

    /// A6/B1: Returns the configured ledger backing root path.
    /// Only meaningful with `--security --activation-mode file`.
    /// Returns `None` when the `[ledger]` section is absent or
    /// `backing_root` is empty/whitespace.
    pub fn ledger_backing_root(&self) -> Option<&str> {
        self.ledger
            .as_ref()
            .and_then(|l| l.backing_root.as_deref())
            .filter(|s| !s.trim().is_empty())
    }

    /// Returns the control socket path when configured.
    pub fn control_socket_path(&self) -> Option<&str> {
        self.control_socket
            .as_ref()
            .and_then(|c| c.path.as_deref())
            .filter(|s| !s.trim().is_empty())
    }

    /// Returns the trusted peer executable path when configured.
    pub fn control_socket_trusted_peer_exe(&self) -> Option<&str> {
        self.control_socket
            .as_ref()
            .and_then(|c| c.trusted_peer_exe.as_deref())
            .filter(|s| !s.trim().is_empty())
    }

    /// Returns the optional trusted peer uid constraint.
    pub fn control_socket_trusted_peer_uid(&self) -> Option<u32> {
        self.control_socket
            .as_ref()
            .and_then(|c| c.trusted_peer_uid)
    }

    /// Returns the optional trusted peer gid constraint.
    pub fn control_socket_trusted_peer_gid(&self) -> Option<u32> {
        self.control_socket
            .as_ref()
            .and_then(|c| c.trusted_peer_gid)
    }

    /// I2: Build a `StagingConfig` from the `[install]` section.
    /// Returns `None` when the section is absent or has no patterns.
    pub fn staging_config(&self) -> Option<StagingConfig> {
        let install = self.install.as_ref()?;
        let raw_patterns = install.staging_patterns.as_ref()?;
        if raw_patterns.is_empty() {
            return None;
        }
        let patterns = validate_staging_patterns(raw_patterns).ok()?;
        Some(StagingConfig {
            patterns,
            unactivated_visibility: install
                .unactivated_visibility
                .as_deref()
                .and_then(UnactivatedVisibility::parse)
                .unwrap_or_default(),
        })
    }

    /// Returns the quiet timeout in milliseconds when configured.
    pub fn quiet_timeout_ms(&self) -> Option<u64> {
        self.install.as_ref().and_then(|i| i.quiet_timeout_ms)
    }

    /// I4: Returns the post-publish grace window in milliseconds.
    pub fn post_publish_grace_ms(&self) -> Option<u64> {
        self.install.as_ref().and_then(|i| i.post_publish_grace_ms)
    }

    /// I4: Returns the post-publish write patterns when configured
    /// and non-empty.
    pub fn post_publish_write_patterns(&self) -> Option<&[String]> {
        self.install
            .as_ref()
            .and_then(|i| i.post_publish_write_patterns.as_deref())
            .filter(|p| !p.is_empty())
    }

    /// Validate that the backing root is configured and accessible when
    /// required.
    pub fn validate_backing_root_accessible(&self, in_place: bool) -> Result<(), ConfigError> {
        let needs_backing = in_place
            && (self.activation_mode() != ActivationMode::Off || self.notify_mode() != "off");
        if !needs_backing {
            return Ok(());
        }
        let Some(backing) = self.ledger_backing_root() else {
            return Err(ConfigError::InvalidValue {
                field: "ledger.backing_root",
                value: String::new(),
                allowed: "must be configured when in-place mode has activation or notify enabled",
            });
        };
        let path = std::path::Path::new(backing);
        if !path.exists() {
            return Err(ConfigError::InvalidValue {
                field: "ledger.backing_root",
                value: backing.to_string(),
                allowed: "path must exist when in-place mode has activation or notify enabled",
            });
        }
        if !path.is_dir() {
            return Err(ConfigError::InvalidValue {
                field: "ledger.backing_root",
                value: backing.to_string(),
                allowed: "path must be a directory",
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_parses_to_defaults() {
        let cfg: SecurityConfig = toml::from_str("").unwrap();
        assert!(cfg.decision.is_none());
        assert!(cfg.policy.is_none());
        assert!(cfg.audit.is_none());
        assert!(cfg.events.is_none());
        assert!(cfg.trusted_writer.is_none());
        assert!(cfg.activation.is_none());
        assert!(cfg.notify.is_none());
        assert!(cfg.activation_events.is_none());
        assert!(cfg.ledger.is_none());
        assert!(cfg.install.is_none());
        assert_eq!(cfg.activation_mode(), ActivationMode::Off);
        assert_eq!(cfg.notify_mode(), "off");
        assert!(cfg.activation_events_log_path().is_none());
        assert!(cfg.staging_config().is_none());
    }

    #[test]
    fn partial_config_parses() {
        let toml = r#"
[decision]
command = "agent-sec-cli skill-ledger"

[policy]
on_failure = "keep_previous"
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.decision_command(), Some("agent-sec-cli skill-ledger"));
        assert_eq!(
            cfg.failed_resolve_behavior(),
            FailedResolveBehavior::KeepPreviousMapping
        );
    }

    #[test]
    fn full_config_parses() {
        let toml = r#"
[decision]
command = "agent-sec-cli skill-ledger"

[policy]
on_risk = "fallback"
on_failure = "hide"

[audit]
log_path = "/var/log/skillfs-audit.jsonl"
queue_capacity = 1024

[events]
log_path = "/var/log/skillfs-events.jsonl"

[trusted_writer]
process_name = "agent-sec-daemon"
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.decision_command(), Some("agent-sec-cli skill-ledger"));
        assert_eq!(
            cfg.failed_resolve_behavior(),
            FailedResolveBehavior::HideOnFailure
        );
        assert_eq!(cfg.audit_log_path(), Some("/var/log/skillfs-audit.jsonl"));
        assert_eq!(cfg.audit_queue_capacity(), Some(1024));
        assert_eq!(cfg.events_log_path(), Some("/var/log/skillfs-events.jsonl"));
        assert_eq!(cfg.trusted_writer_name(), Some("agent-sec-daemon"));
    }

    #[test]
    fn on_failure_hide_maps_to_hide_on_failure() {
        let toml = r#"
[policy]
on_failure = "hide"
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            cfg.failed_resolve_behavior(),
            FailedResolveBehavior::HideOnFailure
        );
    }

    #[test]
    fn on_failure_keep_previous_maps_correctly() {
        let toml = r#"
[policy]
on_failure = "keep_previous"
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            cfg.failed_resolve_behavior(),
            FailedResolveBehavior::KeepPreviousMapping
        );
    }

    #[test]
    fn missing_on_failure_defaults_to_hide() {
        let toml = r#"
[policy]
on_risk = "warn"
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            cfg.failed_resolve_behavior(),
            FailedResolveBehavior::HideOnFailure
        );
    }

    #[test]
    fn unknown_field_rejected() {
        let toml = r#"
[decision]
command = "foo"
unknown_field = "bar"
"#;
        let result: Result<SecurityConfig, _> = toml::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn empty_string_values_treated_as_absent() {
        let toml = r#"
[decision]
command = "  "

[trusted_writer]
process_name = ""
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert!(cfg.decision_command().is_none());
        assert!(cfg.trusted_writer_name().is_none());
    }

    #[test]
    fn load_nonexistent_file_returns_io_error() {
        let result = SecurityConfig::load(Path::new("/nonexistent/skillfs-security.toml"));
        assert!(matches!(result, Err(ConfigError::Io(_))));
    }

    #[test]
    fn invalid_on_failure_value_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "[policy]\non_failure = \"keep-prev\"\n").unwrap();
        let result = SecurityConfig::load(&path);
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("keep-prev"),
            "error should mention the bad value: {err}"
        );
    }

    #[test]
    fn invalid_on_risk_value_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "[policy]\non_risk = \"yolo\"\n").unwrap();
        let result = SecurityConfig::load(&path);
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));
    }

    #[test]
    fn valid_on_failure_hide_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ok.toml");
        std::fs::write(&path, "[policy]\non_failure = \"hide\"\n").unwrap();
        let cfg = SecurityConfig::load(&path).unwrap();
        assert_eq!(
            cfg.failed_resolve_behavior(),
            FailedResolveBehavior::HideOnFailure
        );
    }

    #[test]
    fn activation_mode_file_parses() {
        let toml = r#"
[activation]
mode = "file"
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.activation_mode(), ActivationMode::File);
    }

    #[test]
    fn activation_mode_off_parses() {
        let toml = r#"
[activation]
mode = "off"
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.activation_mode(), ActivationMode::Off);
    }

    #[test]
    fn invalid_activation_mode_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "[activation]\nmode = \"auto\"\n").unwrap();
        let result = SecurityConfig::load(&path);
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));
    }

    #[test]
    fn missing_activation_mode_defaults_to_off() {
        let toml = r#"
[activation]
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.activation_mode(), ActivationMode::Off);
    }

    #[test]
    fn full_config_with_activation_parses() {
        let toml = r#"
[decision]
command = "agent-sec-cli skill-ledger"

[activation]
mode = "file"
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.decision_command(), Some("agent-sec-cli skill-ledger"));
        assert_eq!(cfg.activation_mode(), ActivationMode::File);
    }

    #[test]
    fn notify_section_unix_socket_parses() {
        let toml = r#"
[notify]
mode = "unix-socket"
socket_path = "/run/user/1000/agent-sec-core/daemon.sock"
timeout_ms = 3000
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.notify_mode(), "unix-socket");
        assert_eq!(
            cfg.notify_socket_path(),
            Some("/run/user/1000/agent-sec-core/daemon.sock")
        );
        assert_eq!(cfg.notify_timeout_ms(), Some(3000));
    }

    #[test]
    fn notify_section_off_parses() {
        let toml = r#"
[notify]
mode = "off"
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.notify_mode(), "off");
        assert!(cfg.notify_socket_path().is_none());
    }

    #[test]
    fn missing_notify_defaults_to_off() {
        let cfg: SecurityConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.notify_mode(), "off");
        assert!(cfg.notify_socket_path().is_none());
        assert!(cfg.notify_timeout_ms().is_none());
    }

    #[test]
    fn invalid_notify_mode_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "[notify]\nmode = \"websocket\"\n").unwrap();
        let result = SecurityConfig::load(&path);
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));
    }

    #[test]
    fn invalid_notify_timeout_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "[notify]\ntimeout_ms = 0\n").unwrap();
        let result = SecurityConfig::load(&path);
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));

        let path2 = dir.path().join("bad2.toml");
        std::fs::write(&path2, "[notify]\ntimeout_ms = 500000\n").unwrap();
        let result2 = SecurityConfig::load(&path2);
        assert!(matches!(result2, Err(ConfigError::InvalidValue { .. })));
    }

    #[test]
    fn full_config_with_notify_parses() {
        let toml = r#"
[activation]
mode = "file"

[notify]
mode = "unix-socket"
socket_path = "/tmp/daemon.sock"
timeout_ms = 5000
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.activation_mode(), ActivationMode::File);
        assert_eq!(cfg.notify_mode(), "unix-socket");
        assert_eq!(cfg.notify_socket_path(), Some("/tmp/daemon.sock"));
        assert_eq!(cfg.notify_timeout_ms(), Some(5000));
    }

    #[test]
    fn unix_socket_mode_without_socket_path_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "[notify]\nmode = \"unix-socket\"\n").unwrap();
        let result = SecurityConfig::load(&path);
        assert!(
            matches!(
                result,
                Err(ConfigError::InvalidValue {
                    field: "notify.socket_path",
                    ..
                })
            ),
            "unix-socket without socket_path must fail: {result:?}"
        );
    }

    #[test]
    fn unix_socket_mode_with_empty_socket_path_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(
            &path,
            "[notify]\nmode = \"unix-socket\"\nsocket_path = \"  \"\n",
        )
        .unwrap();
        let result = SecurityConfig::load(&path);
        assert!(
            matches!(
                result,
                Err(ConfigError::InvalidValue {
                    field: "notify.socket_path",
                    ..
                })
            ),
            "unix-socket with whitespace-only socket_path must fail: {result:?}"
        );
    }

    #[test]
    fn activation_events_log_path_parses() {
        let toml = r#"
[activation_events]
log_path = "/var/log/skillfs-activation-events.jsonl"
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            cfg.activation_events_log_path(),
            Some("/var/log/skillfs-activation-events.jsonl")
        );
    }

    #[test]
    fn activation_events_empty_path_treated_as_absent() {
        let toml = r#"
[activation_events]
log_path = "  "
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert!(cfg.activation_events_log_path().is_none());
    }

    #[test]
    fn full_config_with_activation_events_parses() {
        let toml = r#"
[activation]
mode = "file"

[notify]
mode = "unix-socket"
socket_path = "/tmp/daemon.sock"
timeout_ms = 5000

[activation_events]
log_path = "/tmp/activation-events.jsonl"
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.activation_mode(), ActivationMode::File);
        assert_eq!(cfg.notify_mode(), "unix-socket");
        assert_eq!(
            cfg.activation_events_log_path(),
            Some("/tmp/activation-events.jsonl")
        );
    }

    #[test]
    fn off_mode_with_socket_path_does_not_enable_notify() {
        let toml = r#"
[notify]
mode = "off"
socket_path = "/tmp/daemon.sock"
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.notify_mode(), "off");
        assert!(
            cfg.notify_socket_path().is_none(),
            "mode=off must suppress socket_path"
        );
    }

    #[test]
    fn reload_mode_poll_parses() {
        let toml = r#"
[activation]
mode = "file"
reload = "poll"
reload_interval_ms = 100
reload_timeout_ms = 3000
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.reload_mode(), super::ReloadMode::Poll);
        assert_eq!(cfg.reload_interval_ms(), Some(100));
        assert_eq!(cfg.reload_timeout_ms(), Some(3000));
    }

    #[test]
    fn reload_mode_off_parses() {
        let toml = r#"
[activation]
mode = "file"
reload = "off"
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.reload_mode(), super::ReloadMode::Off);
    }

    #[test]
    fn missing_reload_defaults_to_off() {
        let toml = r#"
[activation]
mode = "file"
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.reload_mode(), super::ReloadMode::Off);
        assert!(cfg.reload_interval_ms().is_none());
        assert!(cfg.reload_timeout_ms().is_none());
    }

    #[test]
    fn invalid_reload_mode_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "[activation]\nreload = \"auto\"\n").unwrap();
        let result = SecurityConfig::load(&path);
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));
    }

    #[test]
    fn invalid_reload_interval_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "[activation]\nreload_interval_ms = 0\n").unwrap();
        let result = SecurityConfig::load(&path);
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));

        let path2 = dir.path().join("bad2.toml");
        std::fs::write(&path2, "[activation]\nreload_interval_ms = 100000\n").unwrap();
        let result2 = SecurityConfig::load(&path2);
        assert!(matches!(result2, Err(ConfigError::InvalidValue { .. })));
    }

    #[test]
    fn invalid_reload_timeout_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "[activation]\nreload_timeout_ms = 0\n").unwrap();
        let result = SecurityConfig::load(&path);
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));

        let path2 = dir.path().join("bad2.toml");
        std::fs::write(&path2, "[activation]\nreload_timeout_ms = 500000\n").unwrap();
        let result2 = SecurityConfig::load(&path2);
        assert!(matches!(result2, Err(ConfigError::InvalidValue { .. })));
    }

    #[test]
    fn full_config_with_reload_parses() {
        let toml = r#"
[activation]
mode = "file"
reload = "poll"
reload_interval_ms = 200
reload_timeout_ms = 4000

[notify]
mode = "unix-socket"
socket_path = "/tmp/daemon.sock"
timeout_ms = 5000
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.activation_mode(), super::ActivationMode::File);
        assert_eq!(cfg.reload_mode(), super::ReloadMode::Poll);
        assert_eq!(cfg.reload_interval_ms(), Some(200));
        assert_eq!(cfg.reload_timeout_ms(), Some(4000));
        assert_eq!(cfg.notify_mode(), "unix-socket");
    }

    #[test]
    fn reload_interval_greater_than_timeout_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(
            &path,
            "[activation]\nreload_interval_ms = 5000\nreload_timeout_ms = 1000\n",
        )
        .unwrap();
        let result = SecurityConfig::load(&path);
        assert!(
            matches!(
                result,
                Err(ConfigError::InvalidValue {
                    field: "activation.reload_interval_ms",
                    ..
                })
            ),
            "interval > timeout must be rejected: {result:?}"
        );
    }

    #[test]
    fn reload_interval_equal_to_timeout_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ok.toml");
        std::fs::write(
            &path,
            "[activation]\nreload_interval_ms = 1000\nreload_timeout_ms = 1000\n",
        )
        .unwrap();
        let cfg = SecurityConfig::load(&path).unwrap();
        assert_eq!(cfg.reload_interval_ms(), Some(1000));
        assert_eq!(cfg.reload_timeout_ms(), Some(1000));
    }

    // -----------------------------------------------------------------------
    // A6/B1: Ledger backing root config tests
    // -----------------------------------------------------------------------

    #[test]
    fn ledger_backing_root_parses() {
        let toml = r#"
[ledger]
backing_root = "/run/skillfs-ledger/source"
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            cfg.ledger_backing_root(),
            Some("/run/skillfs-ledger/source")
        );
    }

    #[test]
    fn ledger_section_absent_returns_none() {
        let cfg: SecurityConfig = toml::from_str("").unwrap();
        assert!(cfg.ledger_backing_root().is_none());
    }

    #[test]
    fn ledger_backing_root_empty_treated_as_absent() {
        let toml = r#"
[ledger]
backing_root = "  "
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert!(cfg.ledger_backing_root().is_none());
    }

    #[test]
    fn ledger_section_with_unknown_field_rejected() {
        let toml = r#"
[ledger]
backing_root = "/tmp/x"
unknown = true
"#;
        let result: Result<SecurityConfig, _> = toml::from_str(toml);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // I2: Install section config tests
    // -----------------------------------------------------------------------

    #[test]
    fn install_section_parses() {
        let toml = r#"
[install]
staging_patterns = [".openclaw-install-stage-*"]
unactivated_visibility = "hidden"
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        let install = cfg.install.as_ref().unwrap();
        assert_eq!(
            install.staging_patterns.as_ref().unwrap(),
            &vec![".openclaw-install-stage-*".to_string()]
        );
        assert_eq!(install.unactivated_visibility.as_deref(), Some("hidden"));
    }

    #[test]
    fn install_section_staging_config_builds() {
        let toml = r#"
[install]
staging_patterns = [".openclaw-install-stage-*", ".pip-staging"]
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        let staging = cfg.staging_config().expect("staging config");
        assert_eq!(staging.patterns.len(), 2);
    }

    #[test]
    fn install_section_absent_returns_none_staging() {
        let cfg: SecurityConfig = toml::from_str("").unwrap();
        assert!(cfg.staging_config().is_none());
    }

    #[test]
    fn install_section_empty_patterns_returns_none_staging() {
        let toml = r#"
[install]
staging_patterns = []
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert!(cfg.staging_config().is_none());
    }

    #[test]
    fn install_invalid_unactivated_visibility_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "[install]\nunactivated_visibility = \"visible\"\n").unwrap();
        let result = SecurityConfig::load(&path);
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));
    }

    #[test]
    fn install_quiet_timeout_ms_valid_value_accepted() {
        let toml = r#"
[install]
staging_patterns = [".openclaw-install-stage-*"]
quiet_timeout_ms = 2000
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.quiet_timeout_ms(), Some(2000));
    }

    #[test]
    fn install_quiet_timeout_ms_boundary_values() {
        let dir = tempfile::tempdir().unwrap();

        let path = dir.path().join("low.toml");
        std::fs::write(&path, "[install]\nquiet_timeout_ms = 100\n").unwrap();
        let cfg = SecurityConfig::load(&path).unwrap();
        assert_eq!(cfg.quiet_timeout_ms(), Some(100));

        let path = dir.path().join("high.toml");
        std::fs::write(&path, "[install]\nquiet_timeout_ms = 300000\n").unwrap();
        let cfg = SecurityConfig::load(&path).unwrap();
        assert_eq!(cfg.quiet_timeout_ms(), Some(300_000));
    }

    #[test]
    fn install_quiet_timeout_ms_zero_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "[install]\nquiet_timeout_ms = 0\n").unwrap();
        let result = SecurityConfig::load(&path);
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));
    }

    #[test]
    fn install_quiet_timeout_ms_too_large_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "[install]\nquiet_timeout_ms = 500000\n").unwrap();
        let result = SecurityConfig::load(&path);
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));
    }

    #[test]
    fn install_quiet_timeout_ms_below_minimum_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "[install]\nquiet_timeout_ms = 50\n").unwrap();
        let result = SecurityConfig::load(&path);
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));
    }

    #[test]
    fn install_quiet_timeout_ms_absent_returns_none() {
        let toml = r#"
[install]
staging_patterns = [".openclaw-install-stage-*"]
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert!(cfg.quiet_timeout_ms().is_none());
    }

    #[test]
    fn install_removed_complete_on_rename_rejected_as_unknown() {
        let toml = r#"
[install]
staging_patterns = [".openclaw-install-stage-*"]
complete_on_rename_to_skill = true
"#;
        let result: Result<SecurityConfig, _> = toml::from_str(toml);
        assert!(
            result.is_err(),
            "complete_on_rename_to_skill is no longer a valid field"
        );
    }

    #[test]
    fn install_invalid_staging_pattern_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "[install]\nstaging_patterns = [\"foo/bar\"]\n").unwrap();
        let result = SecurityConfig::load(&path);
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));
    }

    #[test]
    fn install_sensitive_staging_pattern_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "[install]\nstaging_patterns = [\".skill-meta\"]\n").unwrap();
        let result = SecurityConfig::load(&path);
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));
    }

    #[test]
    fn install_section_unknown_field_rejected() {
        let toml = r#"
[install]
staging_patterns = [".openclaw-install-stage-*"]
unknown = true
"#;
        let result: Result<SecurityConfig, _> = toml::from_str(toml);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Trusted peer control socket config tests
    // -----------------------------------------------------------------------

    #[test]
    fn control_socket_full_config_parses() {
        let toml = r#"
[control_socket]
path = "/run/skillfs/skillfs.sock"
trusted_peer_exe = "/usr/local/bin/agent-sec-cli"
trusted_peer_uid = 1000
trusted_peer_gid = 1000
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.control_socket_path(), Some("/run/skillfs/skillfs.sock"));
        assert_eq!(
            cfg.control_socket_trusted_peer_exe(),
            Some("/usr/local/bin/agent-sec-cli")
        );
        assert_eq!(cfg.control_socket_trusted_peer_uid(), Some(1000));
        assert_eq!(cfg.control_socket_trusted_peer_gid(), Some(1000));
    }

    #[test]
    fn control_socket_absent_returns_none() {
        let cfg: SecurityConfig = toml::from_str("").unwrap();
        assert!(cfg.control_socket_path().is_none());
        assert!(cfg.control_socket_trusted_peer_exe().is_none());
        assert!(cfg.control_socket_trusted_peer_uid().is_none());
        assert!(cfg.control_socket_trusted_peer_gid().is_none());
    }

    #[test]
    fn control_socket_empty_path_treated_as_absent() {
        let toml = r#"
[control_socket]
path = "  "
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert!(cfg.control_socket_path().is_none());
    }

    #[test]
    fn control_socket_path_without_exe_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(
            &path,
            r#"
[control_socket]
path = "/run/skillfs/skillfs.sock"
"#,
        )
        .unwrap();
        let result = SecurityConfig::load(&path);
        assert!(
            matches!(
                result,
                Err(ConfigError::InvalidValue {
                    field: "control_socket.trusted_peer_exe",
                    ..
                })
            ),
            "path without trusted_peer_exe must fail: {result:?}"
        );
    }

    #[test]
    fn control_socket_exe_without_path_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(
            &path,
            r#"
[control_socket]
trusted_peer_exe = "/usr/local/bin/agent-sec-cli"
"#,
        )
        .unwrap();
        let result = SecurityConfig::load(&path);
        assert!(
            matches!(
                result,
                Err(ConfigError::InvalidValue {
                    field: "control_socket.path",
                    ..
                })
            ),
            "trusted_peer_exe without path must fail: {result:?}"
        );
    }

    #[test]
    fn control_socket_optional_uid_gid_accepted() {
        let toml = r#"
[control_socket]
path = "/run/skillfs/skillfs.sock"
trusted_peer_exe = "/usr/local/bin/agent-sec-cli"
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert!(cfg.control_socket_trusted_peer_uid().is_none());
        assert!(cfg.control_socket_trusted_peer_gid().is_none());
    }

    #[test]
    fn control_socket_unknown_field_rejected() {
        let toml = r#"
[control_socket]
path = "/run/skillfs/skillfs.sock"
trusted_peer_exe = "/usr/local/bin/agent-sec-cli"
unknown = true
"#;
        let result: Result<SecurityConfig, _> = toml::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn install_defaults_preserved_when_absent() {
        let toml = r#"
[activation]
mode = "file"
"#;
        let cfg: SecurityConfig = toml::from_str(toml).unwrap();
        assert!(cfg.install.is_none());
        assert!(cfg.staging_config().is_none());
        assert_eq!(cfg.activation_mode(), ActivationMode::File);
    }

    #[test]
    fn full_config_with_install_section_parses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("full.toml");
        std::fs::write(
            &path,
            r#"
[activation]
mode = "file"

[notify]
mode = "unix-socket"
socket_path = "/tmp/daemon.sock"

[install]
staging_patterns = [".openclaw-install-stage-*"]
unactivated_visibility = "hidden"
"#,
        )
        .unwrap();
        let cfg = SecurityConfig::load(&path).unwrap();
        assert_eq!(cfg.activation_mode(), ActivationMode::File);
        assert!(cfg.staging_config().is_some());
    }

    // -----------------------------------------------------------------------
    // I4: Post-publish grace config tests
    // -----------------------------------------------------------------------

    #[test]
    fn post_publish_both_configured_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ok.toml");
        std::fs::write(
            &path,
            r#"
[install]
post_publish_grace_ms = 5000
post_publish_write_patterns = [".openclaw/**"]
"#,
        )
        .unwrap();
        let cfg = SecurityConfig::load(&path).unwrap();
        assert_eq!(cfg.post_publish_grace_ms(), Some(5000));
        assert_eq!(
            cfg.post_publish_write_patterns(),
            Some([".openclaw/**".to_string()].as_slice())
        );
    }

    #[test]
    fn post_publish_grace_without_patterns_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "[install]\npost_publish_grace_ms = 5000\n").unwrap();
        let result = SecurityConfig::load(&path);
        assert!(
            matches!(
                result,
                Err(ConfigError::InvalidValue {
                    field: "install.post_publish_write_patterns",
                    ..
                })
            ),
            "grace without patterns must fail: {result:?}"
        );
    }

    #[test]
    fn post_publish_patterns_without_grace_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(
            &path,
            "[install]\npost_publish_write_patterns = [\".openclaw/**\"]\n",
        )
        .unwrap();
        let result = SecurityConfig::load(&path);
        assert!(
            matches!(
                result,
                Err(ConfigError::InvalidValue {
                    field: "install.post_publish_grace_ms",
                    ..
                })
            ),
            "patterns without grace must fail: {result:?}"
        );
    }

    #[test]
    fn post_publish_empty_patterns_with_grace_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(
            &path,
            "[install]\npost_publish_grace_ms = 5000\npost_publish_write_patterns = []\n",
        )
        .unwrap();
        let result = SecurityConfig::load(&path);
        assert!(
            matches!(result, Err(ConfigError::InvalidValue { .. })),
            "empty patterns with grace must fail: {result:?}"
        );
    }

    #[test]
    fn post_publish_grace_ms_boundary_values() {
        let dir = tempfile::tempdir().unwrap();

        let path = dir.path().join("low.toml");
        std::fs::write(
            &path,
            "[install]\npost_publish_grace_ms = 100\npost_publish_write_patterns = [\".openclaw/**\"]\n",
        )
        .unwrap();
        let cfg = SecurityConfig::load(&path).unwrap();
        assert_eq!(cfg.post_publish_grace_ms(), Some(100));

        let path = dir.path().join("high.toml");
        std::fs::write(
            &path,
            "[install]\npost_publish_grace_ms = 300000\npost_publish_write_patterns = [\".openclaw/**\"]\n",
        )
        .unwrap();
        let cfg = SecurityConfig::load(&path).unwrap();
        assert_eq!(cfg.post_publish_grace_ms(), Some(300_000));
    }

    #[test]
    fn post_publish_grace_ms_too_low_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(
            &path,
            "[install]\npost_publish_grace_ms = 50\npost_publish_write_patterns = [\".openclaw/**\"]\n",
        )
        .unwrap();
        let result = SecurityConfig::load(&path);
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));
    }

    #[test]
    fn post_publish_grace_ms_too_high_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(
            &path,
            "[install]\npost_publish_grace_ms = 500000\npost_publish_write_patterns = [\".openclaw/**\"]\n",
        )
        .unwrap();
        let result = SecurityConfig::load(&path);
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));
    }

    #[test]
    fn post_publish_invalid_pattern_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(
            &path,
            "[install]\npost_publish_grace_ms = 5000\npost_publish_write_patterns = [\".skill-meta/**\"]\n",
        )
        .unwrap();
        let result = SecurityConfig::load(&path);
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));
    }

    #[test]
    fn post_publish_absent_returns_none() {
        let cfg: SecurityConfig = toml::from_str("").unwrap();
        assert!(cfg.post_publish_grace_ms().is_none());
        assert!(cfg.post_publish_write_patterns().is_none());
    }

    #[test]
    fn post_publish_with_staging_patterns_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ok.toml");
        std::fs::write(
            &path,
            r#"
[install]
staging_patterns = [".openclaw-install-stage-*"]
post_publish_grace_ms = 5000
post_publish_write_patterns = [".openclaw/**"]
"#,
        )
        .unwrap();
        let cfg = SecurityConfig::load(&path).unwrap();
        assert!(cfg.staging_config().is_some());
        assert_eq!(cfg.post_publish_grace_ms(), Some(5000));
    }
}
