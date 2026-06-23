//! Telemetry enablement orchestration for SLS / ilogtail.
//!
//! Responsibilities:
//! 1. Hold shared configuration (`TelemetryConfig`) and errors (`TelemetryError`).
//! 2. Orchestrate [`ilogtail::IlogtailInstaller`] and [`ops_telemetry::OpsTelemetrySetup`]
//!    during register / unregister.
//!
//! Submodules:
//! - [`ilogtail`]: region detection and ilogtail installation/configuration
//! - [`ops_telemetry`]: ops directory, component .jsonl files, instance snapshot

pub mod ilogtail;
pub mod ops_telemetry;

pub use ilogtail::{IlogtailInstaller, RegionInfo, RegionProbe};
pub use ops_telemetry::OpsTelemetrySetup;

use std::path::PathBuf;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

/// agentsight SLS log enablement marker file
const SLS_LOG_MARKER: &str = "/etc/anolisa/enable_token_collector";

/// Default user-side SLS account ID (base64 encoded)
const DEFAULT_SLS_ACCOUNT_ID_B64: &str = "MTgwODA3ODk1MDc3MDI2NA==";

/// Default ops-side SLS account ID (base64 encoded)
const DEFAULT_OPS_SLS_ACCOUNT_ID_B64: &str = "MTY0NDIxNTM2ODk0ODY3Nw==";

// ── Configuration ────────────────────────────────────────────────────

/// Configurable parameters for telemetry enablement
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    /// SLS user-side account ID (written to `/etc/ilogtail/users/<id>`)
    pub sls_account_id: String,
    /// user_defined_id tag list for user-side (written to /etc/ilogtail/user_defined_id)
    pub user_defined_ids: Vec<String>,
    /// ilogtaild init script path
    pub ilogtaild_init: PathBuf,
    /// ilogtail user files directory
    pub ilogtail_users_dir: PathBuf,
    /// user_defined_id file path
    pub user_defined_id_path: PathBuf,
    /// aliyun-security ilogtaild init script path
    pub aliyun_security_init: PathBuf,
    /// aliyun-security ilogtail users directory
    pub aliyun_security_users_dir: PathBuf,
    /// aliyun-security user_defined_id file path
    pub aliyun_security_user_defined_id_path: PathBuf,
    /// SLS log enablement marker file path
    pub sls_log_marker: PathBuf,
    /// Instance metadata URL (ECS internal network)
    pub metadata_url: String,
    /// Ops-side SLS account ID (written to `/etc/ilogtail/users/<id>`)
    pub ops_sls_account_id: String,
    /// user_defined_id tags for ops-side
    pub ops_user_defined_ids: Vec<String>,
    /// Ops directory for component .jsonl files
    pub ops_dir: PathBuf,
    /// Instance ID cache path
    pub instance_id_cache_path: PathBuf,
    /// Path to `/etc/machine-id` (used as instance ID fallback)
    pub machine_id_path: PathBuf,
    /// Path to `/etc/anolisa-release` (used for product type detection)
    pub release_path: PathBuf,
    /// Path to `/etc/os-release` (used for distro detection)
    pub os_release_path: PathBuf,
    /// Path to `/sys/devices/system/cpu/present` (used for vCPU count)
    pub cpu_present_path: PathBuf,
    /// Path to `/etc/image-id` (used for image ID detection)
    pub image_id_path: PathBuf,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        // Decode the embedded base64 default; panic at startup if constant is corrupted
        let default_id = BASE64
            .decode(DEFAULT_SLS_ACCOUNT_ID_B64)
            .map(|b| String::from_utf8(b).expect("DEFAULT_SLS_ACCOUNT_ID_B64 is not valid UTF-8"))
            .expect("DEFAULT_SLS_ACCOUNT_ID_B64 is not valid base64");

        let default_ops_id = BASE64
            .decode(DEFAULT_OPS_SLS_ACCOUNT_ID_B64)
            .map(|b| {
                String::from_utf8(b).expect("DEFAULT_OPS_SLS_ACCOUNT_ID_B64 is not valid UTF-8")
            })
            .expect("DEFAULT_OPS_SLS_ACCOUNT_ID_B64 is not valid base64");

        Self {
            sls_account_id: default_id,
            user_defined_ids: vec![
                "sysom_unity_metrics".into(),
                "sysom_livetrace_oncpu".into(),
                "sysom_livetrace_meta".into(),
            ],
            ilogtaild_init: PathBuf::from("/etc/init.d/ilogtaild"),
            ilogtail_users_dir: PathBuf::from("/etc/ilogtail/users"),
            user_defined_id_path: PathBuf::from("/etc/ilogtail/user_defined_id"),
            aliyun_security_init: PathBuf::from("/etc/init.d/ilogtaild-aliyun-security"),
            aliyun_security_users_dir: PathBuf::from("/opt/aliyun-security/ilogtail-config/users"),
            aliyun_security_user_defined_id_path: PathBuf::from(
                "/opt/aliyun-security/ilogtail-config/user_defined_id",
            ),
            sls_log_marker: PathBuf::from(SLS_LOG_MARKER),
            metadata_url: "http://100.100.100.200/latest/meta-data/region-id".into(),
            ops_sls_account_id: default_ops_id,
            ops_user_defined_ids: vec!["anolisa-livetrace".into()],
            ops_dir: PathBuf::from("/var/log/anolisa/sls/ops"),
            instance_id_cache_path: PathBuf::from("/var/lib/anolisa/instance-id.cache"),
            machine_id_path: PathBuf::from("/etc/machine-id"),
            release_path: PathBuf::from("/etc/anolisa-release"),
            os_release_path: PathBuf::from("/etc/os-release"),
            cpu_present_path: PathBuf::from("/sys/devices/system/cpu/present"),
            image_id_path: PathBuf::from("/etc/image-id"),
        }
    }
}

// ── Error types ───────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    #[error("cannot detect region-id: {0}")]
    RegionNotFound(String),
    #[error("ilogtail installation failed (exit {code}): {stderr}")]
    InstallFailed { code: i32, stderr: String },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("command error: {0}")]
    Command(String),
    #[error("sls_account_id is not configured")]
    MissingAccountId,
    #[error("invalid sls_account_id: {0}")]
    InvalidAccountId(String),
    #[error(
        "telemetry step '{step}' failed: {source}. Partial configuration may remain; run 'sudo anolisa unregister --force' to clean up, then retry 'sudo anolisa register'."
    )]
    StepFailed {
        step: String,
        source: Box<TelemetryError>,
    },
}

/// Validate that an SLS account ID contains only ASCII digits.
/// This prevents path traversal attacks when the ID is used as a filename
/// component under `/etc/ilogtail/users/<id>`.
pub fn validate_sls_account_id(id: &str) -> Result<(), TelemetryError> {
    if id.is_empty() {
        return Err(TelemetryError::MissingAccountId);
    }
    if !id.chars().all(|c| c.is_ascii_digit()) {
        return Err(TelemetryError::InvalidAccountId(format!(
            "expected digits only, got {id:?}"
        )));
    }
    Ok(())
}

// ── TelemetryStarter ──────────────────────────────────────────────────

/// Unified entry point for telemetry enablement, called by register / unregister.
///
/// Orchestrates [`IlogtailInstaller`] (ilogtail install + SLS account config)
/// and [`OpsTelemetrySetup`] (ops directory + component .jsonl + instance snapshot).
pub struct TelemetryStarter {
    config: TelemetryConfig,
}

impl TelemetryStarter {
    pub fn new(config: TelemetryConfig) -> Self {
        Self { config }
    }

    /// Enable telemetry for both user-side and ops-side data links.
    ///
    /// Called after `anolisa register` successfully writes register.json.
    ///
    /// # Partial failure semantics
    ///
    /// Steps are designed to be idempotent, but any step may fail after earlier
    /// steps have already modified the system (e.g. ilogtail account files or
    /// ops directory). On failure a [`TelemetryError::StepFailed`] is returned
    /// that names the failing step and advises the user to run
    /// `sudo anolisa unregister --force` to clean up before retrying.
    pub fn start(&self) -> Result<(), TelemetryError> {
        validate_sls_account_id(&self.config.sls_account_id)?;

        // 1. Detect region-id and infer network environment
        let probe = RegionProbe::new(&self.config.metadata_url);
        let region_info = Self::run_step("detect region", probe.probe())?;

        // 2. Install / confirm ilogtail is running
        let installer = IlogtailInstaller::new(&self.config);
        Self::run_step("install ilogtail", installer.ensure_installed(&region_info))?;

        // 3. Configure user-side SLS account file
        Self::run_step("configure user account", installer.configure_account())?;

        // 4. Configure user-side user_defined_id
        Self::run_step(
            "configure user tags",
            installer.configure_user_defined_ids(),
        )?;

        // 5. Configure ops-side SLS account file
        Self::run_step("configure ops account", installer.configure_ops_account())?;

        // 6. Configure ops-side user_defined_id
        Self::run_step(
            "configure ops tags",
            installer.configure_ops_user_defined_ids(),
        )?;

        // 7-10. Ops telemetry setup
        let ops = OpsTelemetrySetup::new(&self.config);
        Self::run_step("create ops directory", ops.create_ops_dir())?;
        Self::run_step("create ops jsonl files", ops.create_ops_jsonl_files())?;
        Self::run_step("enable sls log marker", ops.enable_sls_log_marker())?;
        let instance_info = Self::run_step(
            "write instance snapshot",
            ops.write_instance_snapshot(&region_info.region_id),
        )?;

        // 11. Write instance-id to user_defined_id so ilogtail can identify this machine
        Self::run_step(
            "configure instance id",
            installer.configure_instance_id(&instance_info.id),
        )?;

        Ok(())
    }

    /// Wrap a single telemetry step result with step-name context.
    ///
    /// On success returns the inner value. On failure returns
    /// [`TelemetryError::StepFailed`] so callers know which operation left
    /// partial state behind.
    fn run_step<T>(step: &str, result: Result<T, TelemetryError>) -> Result<T, TelemetryError> {
        result.map_err(|e| TelemetryError::StepFailed {
            step: step.to_string(),
            source: Box::new(e),
        })
    }

    /// Stop telemetry: remove both user-side and ops-side configurations.
    ///
    /// Called after `anolisa unregister` successfully writes register.json.
    /// Note: does not uninstall ilogtail itself, only revokes upload configuration.
    /// Ops directory and .jsonl files are preserved (components still write locally).
    pub fn stop(&self) -> Result<(), TelemetryError> {
        // 0. Remove agentsight SLS log marker file
        let ops = OpsTelemetrySetup::new(&self.config);
        ops.remove_sls_log_marker()?;

        // 1-2. Remove both user-side and ops-side SLS account files
        let installer = IlogtailInstaller::new(&self.config);
        installer.remove_account_files()?;

        // 3. Clean up user_defined_id file (remove both user + ops tags)
        installer.remove_user_defined_ids()?;

        Ok(())
    }
}

// ── Test helpers ───────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) fn test_config(dir: &tempfile::TempDir) -> TelemetryConfig {
    TelemetryConfig {
        sls_account_id: "123456789".into(),
        user_defined_ids: vec!["tag_a".into(), "tag_b".into()],
        ilogtaild_init: dir.path().join("ilogtaild"),
        ilogtail_users_dir: dir.path().join("users"),
        user_defined_id_path: dir.path().join("user_defined_id"),
        aliyun_security_init: dir.path().join("ilogtaild-aliyun-security"),
        aliyun_security_users_dir: dir.path().join("aliyun-security/users"),
        aliyun_security_user_defined_id_path: dir.path().join("aliyun-security/user_defined_id"),
        sls_log_marker: dir.path().join("enable_sls_log"),
        metadata_url: "http://127.0.0.1:19999/no-such-endpoint".into(),
        ops_sls_account_id: "987654321".into(),
        ops_user_defined_ids: vec!["anolisa-livetrace".into()],
        ops_dir: dir.path().join("ops"),
        instance_id_cache_path: dir.path().join("instance-id.cache"),
        machine_id_path: dir.path().join("machine-id"),
        release_path: dir.path().join("anolisa-release"),
        os_release_path: dir.path().join("os-release"),
        cpu_present_path: dir.path().join("cpu-present"),
        image_id_path: dir.path().join("image-id"),
    }
}

// ── Unit tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_start_fails_without_account_id() {
        let dir = TempDir::new().unwrap();
        let mut cfg = test_config(&dir);
        cfg.sls_account_id = String::new();
        let starter = TelemetryStarter::new(cfg);
        assert!(matches!(
            starter.start(),
            Err(TelemetryError::MissingAccountId)
        ));
    }
}
