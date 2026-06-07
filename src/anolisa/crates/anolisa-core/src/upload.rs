//! ilogtail upload enablement module
//!
//! Responsibilities:
//! 1. Detect region-id (instance metadata → cloud-init file → failure)
//! 2. Install / check ilogtail
//! 3. Configure SLS account file and user_defined_id
//! 4. Provide start() / stop() for register / unregister calls

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

/// agentsight SLS log enablement marker file
const SLS_LOG_MARKER: &str = "/etc/anolisa/enable_sls_log";

/// Default SLS account ID
const DEFAULT_SLS_ACCOUNT_ID_B64: &str = "MTgwODA3ODk1MDc3MDI2NA==";

// ── Configuration ────────────────────────────────────────────────────

/// Configurable parameters for ilogtail upload
#[derive(Debug, Clone)]
pub struct UploadConfig {
    /// SLS primary account ID (written to /etc/ilogtail/users/<id>)
    pub sls_account_id: String,
    /// user_defined_id tag list (written to /etc/ilogtail/user_defined_id)
    pub user_defined_ids: Vec<String>,
    /// ilogtaild init script path
    pub ilogtaild_init: PathBuf,
    /// ilogtail user files directory
    pub ilogtail_users_dir: PathBuf,
    /// user_defined_id file path
    pub user_defined_id_path: PathBuf,
    /// SLS log enablement marker file path
    pub sls_log_marker: PathBuf,
    /// Instance metadata URL (ECS internal network)
    pub metadata_url: String,
}

impl Default for UploadConfig {
    fn default() -> Self {
        // Decode the embedded base64 default; panic at startup if constant is corrupted
        let default_id = BASE64
            .decode(DEFAULT_SLS_ACCOUNT_ID_B64)
            .map(|b| String::from_utf8(b).expect("DEFAULT_SLS_ACCOUNT_ID_B64 is not valid UTF-8"))
            .expect("DEFAULT_SLS_ACCOUNT_ID_B64 is not valid base64");

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
            sls_log_marker: PathBuf::from(SLS_LOG_MARKER),
            metadata_url: "http://100.100.100.200/latest/meta-data/region-id".into(),
        }
    }
}

// ── Error types ───────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum UploadError {
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
}

/// Validate that an SLS account ID contains only ASCII digits.
/// This prevents path traversal attacks when the ID is used as a filename
/// component under `/etc/ilogtail/users/<id>`.
pub fn validate_sls_account_id(id: &str) -> Result<(), UploadError> {
    if id.is_empty() {
        return Err(UploadError::MissingAccountId);
    }
    if !id.chars().all(|c| c.is_ascii_digit()) {
        return Err(UploadError::InvalidAccountId(format!(
            "expected digits only, got {id:?}"
        )));
    }
    Ok(())
}

// ── RegionProbe ───────────────────────────────────────────────────────

/// Detection result: region-id + whether to use internal network
#[derive(Debug, Clone)]
pub struct RegionInfo {
    pub region_id: String,
    /// true  = use Alibaba Cloud internal network URL (instance metadata API reachable)
    /// false = use public network URL (self-hosted / external network)
    pub use_internal: bool,
}

/// region-id probe
///
/// Priority:
/// 1. ECS instance metadata API (`http://100.100.100.200/latest/meta-data/region-id`)
///    → on success, `use_internal = true` (confirmed on Alibaba Cloud internal network)
/// 2. `cloud-init query ds` (generic, supports ECS / EDS / Wuying etc.)
///    → on success, `use_internal = true` (cloud-init available, likely Alibaba Cloud)
/// 3. fallback `cn-hangzhou`
///    → `use_internal = false` (detection failed, use public network)
pub struct RegionProbe {
    metadata_url: String,
}

impl RegionProbe {
    pub fn new(metadata_url: &str) -> Self {
        Self {
            metadata_url: metadata_url.to_string(),
        }
    }

    /// Detect region-id and infer network environment to decide internal vs public network.
    pub fn probe(&self) -> Result<RegionInfo, UploadError> {
        // 1. Instance metadata API (ECS / SWAS, direct internal access, fastest)
        if let Some(region) = self.query_metadata() {
            return Ok(RegionInfo {
                region_id: region,
                use_internal: true,
            });
        }
        // 2. cloud-init query ds (generic, supports EDS / Wuying / self-hosted)
        if let Some(region) = self.query_cloud_init() {
            return Ok(RegionInfo {
                region_id: region,
                use_internal: true,
            });
        }
        // 3. Self-hosted: fallback to cn-hangzhou, use public network
        Ok(RegionInfo {
            region_id: "cn-hangzhou".to_string(),
            use_internal: false,
        })
    }

    /// Request instance metadata API via curl, 2-second timeout
    fn query_metadata(&self) -> Option<String> {
        let output = Command::new("curl")
            .args([
                "-sf", // -s: silent, -f: fail on HTTP error
                "--max-time",
                "2", // 2s timeout to avoid long blocks on non-ECS environments
                &self.metadata_url,
            ])
            .output()
            .ok()?;

        if output.status.success() {
            let region = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !region.is_empty() {
                return Some(region);
            }
        }
        None
    }

    /// Get region-id via `cloud-init query ds`
    ///
    /// Output example:
    /// ```json
    /// {
    ///   "meta_data": {
    ///     "region-id": "cn-hangzhou",
    ///     ...
    ///   }
    /// }
    /// ```
    fn query_cloud_init(&self) -> Option<String> {
        let output = Command::new("cloud-init")
            .args(["query", "ds"])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        // Parse with serde_json, extract .meta_data["region-id"]
        let json: serde_json::Value = serde_json::from_str(&stdout).ok()?;
        let region = json
            .get("meta_data")?
            .get("region-id")?
            .as_str()?
            .trim()
            .to_string();

        if region.is_empty() {
            None
        } else {
            Some(region)
        }
    }
}

// ── IlogtailInstaller ─────────────────────────────────────────────────

/// ilogtail installation and configuration
pub struct IlogtailInstaller<'a> {
    config: &'a UploadConfig,
}

impl<'a> IlogtailInstaller<'a> {
    pub fn new(config: &'a UploadConfig) -> Self {
        Self { config }
    }

    /// Check if ilogtail is already installed and running; install if not
    pub fn ensure_installed(&self, region_info: &RegionInfo) -> Result<(), UploadError> {
        if self.is_running()? {
            return Ok(());
        }
        self.install(region_info)
    }

    fn is_running(&self) -> Result<bool, UploadError> {
        let init_script = &self.config.ilogtaild_init;
        if !init_script.exists() {
            return Ok(false);
        }
        let output = Command::new(init_script)
            .arg("status")
            .output()
            .map_err(|e| UploadError::Command(format!("failed to check ilogtaild status: {e}")))?;
        Ok(String::from_utf8_lossy(&output.stdout).contains("ilogtail is running"))
    }

    /// Download and execute the official logtail installation script.
    ///
    /// Based on `RegionInfo::use_internal`, directly selects internal or public URL,
    /// no need for internal-first-then-fallback to avoid unnecessary timeout waits.
    fn install(&self, region_info: &RegionInfo) -> Result<(), UploadError> {
        // Use tempfile to create a secure temporary file, preventing symlink attacks
        // on predictable paths (e.g., /tmp/logtail.<pid>.sh).
        let tmp_file = tempfile::Builder::new()
            .prefix("logtail-")
            .suffix(".sh")
            .tempfile()
            .map_err(UploadError::Io)?;
        let tmp_script = tmp_file.path().to_string_lossy().to_string();
        // Keep the file open (prevents reuse of the name) until we're done
        let _tmp_guard = tmp_file;
        let region_id = &region_info.region_id;

        // Select URL directly based on detection result
        let (url, network) = if region_info.use_internal {
            (
                format!(
                    "https://logtail-release-{region_id}.oss-{region_id}-internal.aliyuncs.com/linux64/logtail.sh"
                ),
                "internal",
            )
        } else {
            (
                format!(
                    "https://logtail-release-{region_id}.oss-{region_id}.aliyuncs.com/linux64/logtail.sh"
                ),
                "public",
            )
        };

        // Download (curl --connect-timeout 5 --max-time 10)
        let dl = Command::new("curl")
            .args([
                "-fsSL",
                "--connect-timeout",
                "5",
                "--max-time",
                "10",
                "-o",
                &tmp_script,
                &url,
            ])
            .status()
            .map_err(|e| UploadError::Command(format!("curl failed: {e}")))?;

        if !dl.success() {
            let _ = fs::remove_file(&tmp_script);
            return Err(UploadError::InstallFailed {
                code: dl.code().unwrap_or(-1),
                stderr: format!("curl download failed via {network} network: {url}"),
            });
        }

        // chmod +x
        Command::new("chmod")
            .args(["755", &tmp_script])
            .status()
            .map_err(|e| UploadError::Command(format!("chmod failed: {e}")))?;

        // Execute installation script
        // Public network: region-id needs "-internet" suffix
        let install = if region_info.use_internal {
            Command::new("sh")
                .args([&tmp_script, "install", region_id.as_str()])
                .output()
        } else {
            let public_region = format!("{region_id}-internet");
            Command::new("sh")
                .args([tmp_script.as_str(), "install", &public_region])
                .output()
        };
        let _ = fs::remove_file(&tmp_script);
        let install =
            install.map_err(|e| UploadError::Command(format!("logtail install failed: {e}")))?;

        if !install.status.success() {
            let stdout = String::from_utf8_lossy(&install.stdout);
            let stderr = String::from_utf8_lossy(&install.stderr);
            return Err(UploadError::InstallFailed {
                code: install.status.code().unwrap_or(-1),
                stderr: format!("stdout: {stdout}\nstderr: {stderr}"),
            });
        }

        Ok(())
    }

    /// Configure SLS account file: `/etc/ilogtail/users/<account_id>`
    pub fn configure_account(&self) -> Result<(), UploadError> {
        validate_sls_account_id(&self.config.sls_account_id)?;
        let users_dir = &self.config.ilogtail_users_dir;
        fs::create_dir_all(users_dir)?;

        let account_file = users_dir.join(&self.config.sls_account_id);
        if !account_file.exists() {
            fs::write(&account_file, "")?;
        }
        Ok(())
    }

    /// Configure user_defined_id: append missing tags to the file
    pub fn configure_user_defined_ids(&self) -> Result<(), UploadError> {
        let path = &self.config.user_defined_id_path;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let existing = if path.exists() {
            fs::read_to_string(path)?
        } else {
            String::new()
        };

        let mut appended = false;
        let mut content = existing.clone();
        for id in &self.config.user_defined_ids {
            if !existing.lines().any(|l| l.trim() == id.as_str()) {
                if !content.ends_with('\n') && !content.is_empty() {
                    content.push('\n');
                }
                content.push_str(id);
                content.push('\n');
                appended = true;
            }
        }

        if appended {
            fs::write(path, &content)?;
        }
        Ok(())
    }
}

// ── UploadStarter ─────────────────────────────────────────────────────

/// Unified entry point for upload enablement, called by register / unregister
pub struct UploadStarter {
    config: UploadConfig,
}

impl UploadStarter {
    pub fn new(config: UploadConfig) -> Self {
        Self { config }
    }

    /// Enable upload: install ilogtail → configure account → configure user_defined_id → enable agentsight data write
    ///
    /// Called after `anolisa subscription register` successfully writes register.json.
    pub fn start(&self) -> Result<(), UploadError> {
        validate_sls_account_id(&self.config.sls_account_id)?;

        // 1. Detect region-id and infer network environment
        let probe = RegionProbe::new(&self.config.metadata_url);
        let region_info = probe.probe()?;

        // 2. Install / confirm ilogtail is running (selects URL based on use_internal)
        let installer = IlogtailInstaller::new(&self.config);
        installer.ensure_installed(&region_info)?;

        // 3. Configure SLS account file
        installer.configure_account()?;

        // 4. Configure user_defined_id
        installer.configure_user_defined_ids()?;

        // 5. Enable agentsight logging
        let marker = &self.config.sls_log_marker;
        if let Some(parent) = marker.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(marker, "")?;

        Ok(())
    }

    /// Stop upload: remove user_defined_id tags and SLS account file (keep ilogtail installed)
    ///
    /// Called after `anolisa subscription unregister` successfully writes register.json.
    /// Note: does not uninstall ilogtail itself, only revokes upload configuration.
    pub fn stop(&self) -> Result<(), UploadError> {
        // 0. Remove agentsight SLS log marker file
        let marker = &self.config.sls_log_marker;
        if marker.exists() {
            fs::remove_file(marker)?;
        }

        // 1. Remove SLS account file (validate ID to prevent path traversal)
        if !self.config.sls_account_id.is_empty() {
            validate_sls_account_id(&self.config.sls_account_id)?;
            let account_file = self
                .config
                .ilogtail_users_dir
                .join(&self.config.sls_account_id);
            if account_file.exists() {
                fs::remove_file(&account_file)?;
            }
        }

        // 2. Clean up user_defined_id file
        let path = &self.config.user_defined_id_path;
        if !path.exists() {
            return Ok(());
        }

        let existing = fs::read_to_string(path)?;
        let filtered: String = existing
            .lines()
            .filter(|l| {
                let line = l.trim();
                !self.config.user_defined_ids.iter().any(|id| id == line)
            })
            .map(|l| format!("{l}\n"))
            .collect();

        if filtered.trim().is_empty() {
            // After filtering, only empty lines remain — delete the file
            fs::remove_file(path)?;
        } else {
            fs::write(path, &filtered)?;
        }

        Ok(())
    }
}

// ── Unit tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_config(dir: &TempDir) -> UploadConfig {
        UploadConfig {
            sls_account_id: "123456789".into(),
            user_defined_ids: vec!["tag_a".into(), "tag_b".into()],
            ilogtaild_init: dir.path().join("ilogtaild"),
            ilogtail_users_dir: dir.path().join("users"),
            user_defined_id_path: dir.path().join("user_defined_id"),
            sls_log_marker: dir.path().join("enable_sls_log"),
            metadata_url: "http://127.0.0.1:19999/no-such-endpoint".into(),
        }
    }

    // ── RegionProbe ──────────────────────────────────────────────────

    #[test]
    fn test_region_fallback_when_both_unavailable() {
        // Metadata URL unreachable, cloud-init unavailable
        let probe = RegionProbe::new("http://127.0.0.1:19999/nope");
        // On CI / macOS both paths fail, fallback to cn-hangzhou via public network
        let info = probe.probe().unwrap();
        assert_eq!(info.region_id, "cn-hangzhou");
        assert!(!info.use_internal);
    }

    // ── IlogtailInstaller ────────────────────────────────────────────

    #[test]
    fn test_configure_account_creates_file() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        let installer = IlogtailInstaller::new(&cfg);
        installer.configure_account().unwrap();

        let account_file = cfg.ilogtail_users_dir.join(&cfg.sls_account_id);
        assert!(account_file.exists());
    }

    #[test]
    fn test_configure_account_missing_id() {
        let dir = TempDir::new().unwrap();
        let mut cfg = test_config(&dir);
        cfg.sls_account_id = String::new();
        let installer = IlogtailInstaller::new(&cfg);
        assert!(matches!(
            installer.configure_account(),
            Err(UploadError::MissingAccountId)
        ));
    }

    #[test]
    fn test_configure_user_defined_ids_appends() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        let installer = IlogtailInstaller::new(&cfg);
        installer.configure_user_defined_ids().unwrap();

        let content = fs::read_to_string(&cfg.user_defined_id_path).unwrap();
        assert!(content.contains("tag_a"));
        assert!(content.contains("tag_b"));
    }

    #[test]
    fn test_configure_user_defined_ids_idempotent() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        let installer = IlogtailInstaller::new(&cfg);

        // Write twice — should not duplicate entries
        installer.configure_user_defined_ids().unwrap();
        installer.configure_user_defined_ids().unwrap();

        let content = fs::read_to_string(&cfg.user_defined_id_path).unwrap();
        assert_eq!(content.matches("tag_a").count(), 1);
        assert_eq!(content.matches("tag_b").count(), 1);
    }

    // ── UploadStarter::stop ──────────────────────────────────────────

    #[test]
    fn test_stop_removes_tags_and_account_file() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);

        // Write tags and account file first
        fs::write(&cfg.user_defined_id_path, "tag_a\ntag_b\nother_tag\n").unwrap();
        fs::create_dir_all(&cfg.ilogtail_users_dir).unwrap();
        let account_file = cfg.ilogtail_users_dir.join(&cfg.sls_account_id);
        fs::write(&account_file, "").unwrap();

        let starter = UploadStarter::new(cfg.clone());
        starter.stop().unwrap();

        // Module's tags should be removed from user_defined_id
        let content = fs::read_to_string(&cfg.user_defined_id_path).unwrap();
        assert!(!content.contains("tag_a"));
        assert!(!content.contains("tag_b"));
        assert!(content.contains("other_tag"));

        // SLS account file should be deleted
        assert!(!account_file.exists());
    }

    #[test]
    fn test_stop_deletes_empty_user_defined_id() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);

        // Write only this module's tags; file should be deleted after stop
        fs::write(&cfg.user_defined_id_path, "tag_a\ntag_b\n").unwrap();

        let starter = UploadStarter::new(cfg.clone());
        starter.stop().unwrap();

        // After filtering, only empty lines remain — file should be deleted
        assert!(!cfg.user_defined_id_path.exists());
    }

    #[test]
    fn test_stop_noop_when_file_absent() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        let starter = UploadStarter::new(cfg);
        // File does not exist — should not error
        assert!(starter.stop().is_ok());
    }

    #[test]
    fn test_start_fails_without_account_id() {
        let dir = TempDir::new().unwrap();
        let mut cfg = test_config(&dir);
        cfg.sls_account_id = String::new();
        let starter = UploadStarter::new(cfg);
        assert!(matches!(
            starter.start(),
            Err(UploadError::MissingAccountId)
        ));
    }
}
