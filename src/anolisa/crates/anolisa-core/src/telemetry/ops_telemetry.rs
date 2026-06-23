//! Ops telemetry environment setup.
//!
//! Responsibilities:
//! - Create `/var/log/anolisa/sls/ops/` directory
//! - Pre-create component `.jsonl` files
//! - Enable / remove agentsight SLS log marker
//! - Write `instance.jsonl` snapshot

use crate::instance::{InstanceInfo, InstanceProber, InstanceSnapshot};
use crate::metadata::MetadataClient;
use crate::telemetry::{TelemetryConfig, TelemetryError};
use std::fs;

/// Component .jsonl files to pre-create in the ops directory
const OPS_LOG_FILES: &[&str] = &[
    "instance",
    "agentsight",
    "agent-sec-core",
    "cosh",
    "tokenless",
    "ws-ckpt",
    "skillfs",
];

/// Ops telemetry setup: directory, .jsonl files, instance snapshot, SLS marker.
pub struct OpsTelemetrySetup<'a> {
    config: &'a TelemetryConfig,
}

impl<'a> OpsTelemetrySetup<'a> {
    pub fn new(config: &'a TelemetryConfig) -> Self {
        Self { config }
    }

    /// Create `/var/log/anolisa/sls/ops/` with mode 755
    pub fn create_ops_dir(&self) -> Result<(), TelemetryError> {
        let ops_dir = &self.config.ops_dir;
        if !ops_dir.exists() {
            fs::create_dir_all(ops_dir)?;
        }

        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(ops_dir, fs::Permissions::from_mode(0o755))?;
        }

        Ok(())
    }

    /// Pre-create empty .jsonl files with mode 666 in ops directory
    pub fn create_ops_jsonl_files(&self) -> Result<(), TelemetryError> {
        let ops_dir = &self.config.ops_dir;
        for name in OPS_LOG_FILES {
            let file_path = ops_dir.join(format!("{name}.jsonl"));
            if !file_path.exists() {
                fs::write(&file_path, "")?;
            }

            #[cfg(target_os = "linux")]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&file_path, fs::Permissions::from_mode(0o666))?;
            }
        }
        Ok(())
    }

    /// Enable agentsight SLS log marker file
    pub fn enable_sls_log_marker(&self) -> Result<(), TelemetryError> {
        let marker = &self.config.sls_log_marker;
        if let Some(parent) = marker.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(marker, "")?;
        Ok(())
    }

    /// Remove agentsight SLS log marker file
    pub fn remove_sls_log_marker(&self) -> Result<(), TelemetryError> {
        let marker = &self.config.sls_log_marker;
        if marker.exists() {
            fs::remove_file(marker)?;
        }
        Ok(())
    }

    /// Write instance.jsonl snapshot via InstanceProber.
    ///
    /// Returns the probed [`InstanceInfo`] so callers can reuse the instance-id
    /// for other telemetry configuration (e.g. `user_defined_id`).
    pub fn write_instance_snapshot(&self, region: &str) -> Result<InstanceInfo, TelemetryError> {
        // `metadata_url` points at the region-id endpoint; `from_key_url`
        // strips the trailing key to obtain the `/latest/meta-data` base.
        let client = MetadataClient::from_key_url(&self.config.metadata_url);

        let prober = InstanceProber::with_client(
            client,
            self.config.machine_id_path.clone(),
            self.config.release_path.clone(),
            self.config.os_release_path.clone(),
            self.config.instance_id_cache_path.clone(),
            self.config.cpu_present_path.clone(),
            self.config.image_id_path.clone(),
        );

        let instance_info = prober.probe(region);

        let snapshot = InstanceSnapshot::from_instance_info(&instance_info);

        let instance_jsonl = self.config.ops_dir.join("instance.jsonl");
        snapshot.write_to(&instance_jsonl)?;

        Ok(instance_info)
    }
}

// ── Unit tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::test_config;
    use tempfile::TempDir;

    #[test]
    fn test_create_ops_dir() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        let ops = OpsTelemetrySetup::new(&cfg);
        ops.create_ops_dir().unwrap();

        let ops_dir = dir.path().join("ops");
        assert!(ops_dir.exists());
        assert!(ops_dir.is_dir());
    }

    #[test]
    fn test_create_ops_jsonl_files() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        let ops = OpsTelemetrySetup::new(&cfg);
        ops.create_ops_dir().unwrap();
        ops.create_ops_jsonl_files().unwrap();

        let ops_dir = dir.path().join("ops");
        for name in OPS_LOG_FILES {
            let file_path = ops_dir.join(format!("{name}.jsonl"));
            assert!(file_path.exists(), "expected {name}.jsonl to exist");
        }
    }

    #[test]
    fn test_ops_jsonl_files_idempotent() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        let ops = OpsTelemetrySetup::new(&cfg);
        ops.create_ops_dir().unwrap();
        ops.create_ops_jsonl_files().unwrap();
        ops.create_ops_jsonl_files().unwrap(); // second call should not fail

        let ops_dir = dir.path().join("ops");
        let instance_file = ops_dir.join("instance.jsonl");
        assert!(instance_file.exists());
    }

    #[test]
    fn test_enable_and_remove_sls_log_marker() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        let ops = OpsTelemetrySetup::new(&cfg);

        ops.enable_sls_log_marker().unwrap();
        assert!(cfg.sls_log_marker.exists());

        ops.remove_sls_log_marker().unwrap();
        assert!(!cfg.sls_log_marker.exists());
    }

    #[test]
    fn test_remove_sls_log_marker_noop_when_absent() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        let ops = OpsTelemetrySetup::new(&cfg);
        assert!(ops.remove_sls_log_marker().is_ok());
    }
}
