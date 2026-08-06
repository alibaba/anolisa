// SPDX-License-Identifier: Apache-2.0
//! Daemon configuration (`/etc/anolisa/blaze/config.toml`).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{BlazeError, ConfigErrorSource, Result};
use crate::policy::parse_duration;

/// Top-level daemon configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DaemonConfig {
    #[serde(default)]
    pub daemon: DaemonSection,
    #[serde(default)]
    pub listen: ListenSection,
    /// Backend name → binary path mapping (e.g. `firecracker = "/usr/bin/firecracker"`).
    #[serde(default)]
    pub backends: HashMap<String, PathBuf>,
    #[serde(default)]
    pub policy: PolicySection,
    #[serde(default)]
    pub storage: StorageSection,
    #[serde(default)]
    pub pool: PoolSection,
    #[serde(default)]
    pub template: TemplateSection,
    #[serde(default)]
    pub metrics: MetricsSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonSection {
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default = "default_state_dir")]
    pub state_dir: PathBuf,
    #[serde(default = "default_socket")]
    pub socket: PathBuf,
}

impl Default for DaemonSection {
    fn default() -> Self {
        Self {
            log_level: default_log_level(),
            state_dir: default_state_dir(),
            socket: default_socket(),
        }
    }
}

/// Remote API listener configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListenSection {
    /// TCP address for remote HTTP API (e.g. "0.0.0.0:14159").
    /// Empty string or absent means remote API is disabled.
    #[serde(default)]
    pub http_addr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicySection {
    #[serde(default = "default_policy_dir")]
    pub dir: PathBuf,
    #[serde(default = "default_on_load_error")]
    pub on_load_error: PolicyLoadErrorMode,
}

impl Default for PolicySection {
    fn default() -> Self {
        Self {
            dir: default_policy_dir(),
            on_load_error: default_on_load_error(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PolicyLoadErrorMode {
    Fail,
    Warn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolSection {
    #[serde(default = "default_pool_warm_ttl")]
    pub default_warm_ttl: String,
    #[serde(default = "default_pool_gc_interval")]
    pub gc_interval: String,
}

impl Default for PoolSection {
    fn default() -> Self {
        Self {
            default_warm_ttl: default_pool_warm_ttl(),
            gc_interval: default_pool_gc_interval(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateSection {
    #[serde(default = "default_template_dir")]
    pub dir: PathBuf,
    #[serde(default = "default_template_gc_interval")]
    pub gc_interval: String,
    #[serde(default = "default_template_idle_ttl")]
    pub idle_ttl: String,
}

impl Default for TemplateSection {
    fn default() -> Self {
        Self {
            dir: default_template_dir(),
            gc_interval: default_template_gc_interval(),
            idle_ttl: default_template_idle_ttl(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSection {
    #[serde(default = "default_prometheus_socket")]
    pub prometheus_socket: PathBuf,
}

impl Default for MetricsSection {
    fn default() -> Self {
        Self {
            prometheus_socket: default_prometheus_socket(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSection {
    /// Primary directory for vmlinux, rootfs base images, memfile bases.
    /// All runtime image files are looked up here by default.
    #[serde(default = "default_images_dir")]
    pub images_dir: PathBuf,

    /// Provider-owned runtime slots. This must not be the image directory.
    #[serde(default = "default_instances_dir")]
    pub instances_dir: PathBuf,

    /// Storage provider backend name (e.g. "file", "btrfs", "zfs").
    #[serde(default = "default_storage_provider")]
    pub provider: String,

    /// Warm pool target size (0 = no pool).
    /// NOTE: Reserved for future use. Not yet wired into runtime.
    #[serde(default)]
    pub pool_size: usize,

    /// Whether to pre-start VMs in pool slots.
    /// NOTE: Reserved for future use. Not yet wired into runtime.
    #[serde(default)]
    pub prefork: bool,

    /// Interval for persisting already-written provider-owned artifacts.
    ///
    /// The literal `disabled` turns off periodic synchronization.
    #[serde(default = "default_sync_interval", alias = "flush_interval")]
    pub sync_interval: String,

    /// Maximum time the scheduler waits for one artifact synchronization attempt.
    #[serde(default = "default_sync_timeout")]
    pub sync_timeout: String,

    /// Logical size of file-provider root filesystem slots.
    #[serde(default = "default_rootfs_size")]
    pub rootfs_size: u64,

    /// Logical size of file-provider guest memory slots.
    #[serde(default = "default_mem_size")]
    pub mem_size: u64,
}

impl Default for StorageSection {
    fn default() -> Self {
        Self {
            images_dir: default_images_dir(),
            instances_dir: default_instances_dir(),
            provider: default_storage_provider(),
            pool_size: 0,
            prefork: false,
            sync_interval: default_sync_interval(),
            sync_timeout: default_sync_timeout(),
            rootfs_size: default_rootfs_size(),
            mem_size: default_mem_size(),
        }
    }
}

/// Parsed periodic storage-artifact synchronization policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageSyncSchedule {
    /// Do not run periodic storage-artifact synchronization.
    Disabled,
    /// Run one sweep after every configured interval.
    Every(Duration),
}

impl StorageSection {
    /// Parse the periodic storage-artifact synchronization setting.
    pub fn sync_schedule(&self) -> Result<StorageSyncSchedule> {
        if self.sync_interval == "disabled" {
            return Ok(StorageSyncSchedule::Disabled);
        }
        let duration = parse_duration(&self.sync_interval)
            .ok_or_else(|| invalid_storage_duration("sync_interval", &self.sync_interval, true))?;
        validate_storage_clock_duration("sync_interval", &self.sync_interval, duration)?;
        Ok(StorageSyncSchedule::Every(duration))
    }

    /// Parse the maximum duration of one artifact synchronization attempt.
    pub fn sync_timeout_duration(&self) -> Result<Duration> {
        let duration = parse_duration(&self.sync_timeout)
            .ok_or_else(|| invalid_storage_duration("sync_timeout", &self.sync_timeout, false))?;
        validate_storage_clock_duration("sync_timeout", &self.sync_timeout, duration)?;
        Ok(duration)
    }
}

impl DaemonConfig {
    /// Load and parse a daemon configuration file at `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)?;
        let cfg: DaemonConfig = toml::from_str(&raw)?;
        cfg.validate()?;
        tracing::info!(path = %path.display(), "loaded blaze daemon config");
        Ok(cfg)
    }

    /// Validate cross-field invariants that serde cannot express.
    pub fn validate(&self) -> Result<()> {
        validate_storage_paths(&self.storage.images_dir, &self.storage.instances_dir)?;
        self.storage.sync_schedule()?;
        self.storage.sync_timeout_duration()?;
        Ok(())
    }
}

fn invalid_storage_duration(name: &str, value: &str, allow_disabled: bool) -> BlazeError {
    let expected = if allow_disabled {
        "a positive duration or \"disabled\""
    } else {
        "a positive duration"
    };
    BlazeError::ConfigError {
        source: ConfigErrorSource::InvalidValue(format!(
            "storage.{name} ({value:?}) must be {expected}"
        )),
    }
}

fn validate_storage_clock_duration(name: &str, value: &str, duration: Duration) -> Result<()> {
    if std::time::Instant::now().checked_add(duration).is_none() {
        return Err(BlazeError::ConfigError {
            source: ConfigErrorSource::InvalidValue(format!(
                "storage.{name} ({value:?}) exceeds the monotonic clock range"
            )),
        });
    }
    Ok(())
}

/// Reject storage roots whose ownership domains overlap.
pub fn validate_storage_paths(images_dir: &Path, instances_dir: &Path) -> Result<()> {
    if images_dir == instances_dir
        || images_dir.starts_with(instances_dir)
        || instances_dir.starts_with(images_dir)
    {
        return Err(BlazeError::ConfigError {
            source: ConfigErrorSource::InvalidValue(format!(
                "storage.images_dir ({}) and storage.instances_dir ({}) must be disjoint",
                images_dir.display(),
                instances_dir.display()
            )),
        });
    }
    Ok(())
}

// ----- defaults -----

fn default_log_level() -> String {
    "info".to_string()
}
fn default_state_dir() -> PathBuf {
    PathBuf::from("/var/lib/blaze")
}
fn default_socket() -> PathBuf {
    PathBuf::from("/run/blaze/api.sock")
}
fn default_policy_dir() -> PathBuf {
    PathBuf::from("/etc/anolisa/blaze/policies")
}
fn default_on_load_error() -> PolicyLoadErrorMode {
    PolicyLoadErrorMode::Fail
}
fn default_pool_warm_ttl() -> String {
    "30m".to_string()
}
fn default_pool_gc_interval() -> String {
    "5m".to_string()
}
fn default_template_dir() -> PathBuf {
    PathBuf::from("/var/lib/blaze/templates")
}
fn default_template_gc_interval() -> String {
    "10m".to_string()
}
fn default_template_idle_ttl() -> String {
    "1h".to_string()
}
fn default_prometheus_socket() -> PathBuf {
    PathBuf::from("/run/blaze/metrics.sock")
}
fn default_images_dir() -> PathBuf {
    PathBuf::from("/var/lib/blaze/images")
}
fn default_instances_dir() -> PathBuf {
    PathBuf::from("/var/lib/blaze/instances")
}
fn default_storage_provider() -> String {
    "file".to_string()
}
fn default_sync_interval() -> String {
    "disabled".to_string()
}
fn default_sync_timeout() -> String {
    "30s".to_string()
}
fn default_rootfs_size() -> u64 {
    8 * 1024 * 1024 * 1024
}
fn default_mem_size() -> u64 {
    4 * 1024 * 1024 * 1024
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip() {
        let cfg: DaemonConfig = toml::from_str("").expect("empty parses to defaults");
        assert_eq!(cfg.daemon.log_level, "info");
        assert_eq!(cfg.policy.on_load_error, PolicyLoadErrorMode::Fail);
        assert!(cfg.backends.is_empty());
        assert_ne!(cfg.storage.images_dir, cfg.storage.instances_dir);
        assert_eq!(
            cfg.storage.sync_schedule().expect("sync schedule"),
            StorageSyncSchedule::Disabled
        );
    }

    #[test]
    fn parses_full_example() {
        let toml_str = r#"
            [daemon]
            log_level = "debug"
            state_dir = "/tmp/blaze"
            socket = "/tmp/blaze/api.sock"

            [backends]
            firecracker = "/usr/bin/firecracker"
            linux-sandbox = "/usr/bin/anolisa-linux-sandbox"

            [policy]
            dir = "/etc/anolisa/blaze/policies"
            on_load_error = "warn"
        "#;
        let cfg: DaemonConfig = toml::from_str(toml_str).expect("parses");
        assert_eq!(cfg.daemon.log_level, "debug");
        assert_eq!(cfg.policy.on_load_error, PolicyLoadErrorMode::Warn);
        assert_eq!(cfg.backends.len(), 2);
    }

    #[test]
    fn rejects_equal_or_nested_storage_roots() {
        for (images, instances) in [
            ("/var/lib/blaze/data", "/var/lib/blaze/data"),
            ("/var/lib/blaze/data", "/var/lib/blaze/data/instances"),
            ("/var/lib/blaze/images/base", "/var/lib/blaze/images"),
        ] {
            let mut cfg = DaemonConfig::default();
            cfg.storage.images_dir = PathBuf::from(images);
            cfg.storage.instances_dir = PathBuf::from(instances);
            let error = cfg.validate().expect_err("overlapping paths");
            assert!(error.to_string().contains("must be disjoint"));
        }
    }

    #[test]
    fn storage_sync_schedule_accepts_disabled_or_positive_duration() {
        let mut cfg = DaemonConfig::default();
        cfg.storage.sync_interval = "disabled".into();
        cfg.validate().expect("disabled schedule");
        assert_eq!(
            cfg.storage.sync_schedule().expect("schedule"),
            StorageSyncSchedule::Disabled
        );

        cfg.storage.sync_interval = "15s".into();
        cfg.validate().expect("positive schedule");
        assert_eq!(
            cfg.storage.sync_schedule().expect("schedule"),
            StorageSyncSchedule::Every(Duration::from_secs(15))
        );
    }

    #[test]
    fn storage_sync_interval_accepts_legacy_input_alias() {
        let cfg: DaemonConfig = toml::from_str(
            r#"
                [storage]
                flush_interval = "15s"
            "#,
        )
        .expect("legacy interval key parses");

        assert_eq!(cfg.storage.sync_interval, "15s");
        assert_eq!(
            cfg.storage.sync_schedule().expect("schedule"),
            StorageSyncSchedule::Every(Duration::from_secs(15))
        );
    }

    #[test]
    fn storage_sync_interval_rejects_duplicate_aliases() {
        let error = toml::from_str::<DaemonConfig>(
            r#"
                [storage]
                flush_interval = "15s"
                sync_interval = "30s"
            "#,
        )
        .expect_err("legacy and canonical interval keys must not coexist");

        assert!(error.to_string().contains("duplicate field"), "{error}");
        assert!(error.to_string().contains("sync_interval"), "{error}");
    }

    #[test]
    fn storage_sync_schedule_rejects_invalid_values() {
        for interval in ["0s", "not-a-duration"] {
            let mut cfg = DaemonConfig::default();
            cfg.storage.sync_interval = interval.into();
            let error = cfg.validate().expect_err("invalid sync interval");
            assert!(
                error.to_string().contains("storage.sync_interval"),
                "{error}"
            );
        }

        for timeout in ["0s", "disabled", "not-a-duration"] {
            let mut cfg = DaemonConfig::default();
            cfg.storage.sync_timeout = timeout.into();
            let error = cfg.validate().expect_err("invalid sync timeout");
            assert!(
                error.to_string().contains("storage.sync_timeout"),
                "{error}"
            );
        }

        for (field, value) in [
            ("sync_interval", "18446744073709551615s"),
            ("sync_timeout", "18446744073709551615s"),
        ] {
            let mut cfg = DaemonConfig::default();
            if field == "sync_interval" {
                cfg.storage.sync_interval = value.into();
            } else {
                cfg.storage.sync_timeout = value.into();
            }
            let error = cfg.validate().expect_err("clock range overflow");
            assert!(error.to_string().contains(field), "{error}");
            assert!(
                error.to_string().contains("monotonic clock range"),
                "{error}"
            );
        }
    }
}
