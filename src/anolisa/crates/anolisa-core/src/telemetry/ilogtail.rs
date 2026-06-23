//! ilogtail installation, configuration, and region detection.
//!
//! This module handles:
//! - [`RegionProbe`]: detect region-id via ECS metadata API / cloud-init
//! - [`IlogtailInstaller`]: download, install, configure SLS accounts and
//!   user_defined_id tags (both user-side and ops-side)
//!
//! It also supports the `ilogtaild-aliyun-security` variant: when that init
//! script is present and reports "ilogtail is running", configuration is
//! written to `/opt/aliyun-security/ilogtail-config/` instead of
//! `/etc/ilogtail/`.

use crate::metadata::MetadataClient;
use crate::telemetry::{TelemetryConfig, TelemetryError, validate_sls_account_id};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

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
    client: MetadataClient,
}

impl RegionProbe {
    pub fn new(metadata_url: &str) -> Self {
        Self {
            client: MetadataClient::from_key_url(metadata_url),
        }
    }

    /// Detect region-id and infer network environment to decide internal vs public network.
    pub fn probe(&self) -> Result<RegionInfo, TelemetryError> {
        // Unified probe: metadata API first, then cloud-init datasource.
        if let Some(region) = self.client.query("region-id") {
            return Ok(RegionInfo {
                region_id: region,
                use_internal: true,
            });
        }
        // Self-hosted: fallback to cn-hangzhou, use public network
        Ok(RegionInfo {
            region_id: "cn-hangzhou".to_string(),
            use_internal: false,
        })
    }
}

// ── IlogtailInstaller ─────────────────────────────────────────────────

/// Identifies which ilogtail daemon layout is active on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IlogtailVariant {
    /// Standard `/etc/init.d/ilogtaild` + `/etc/ilogtail/` layout.
    Standard,
    /// Alibaba Cloud security hardening variant:
    /// `/etc/init.d/ilogtaild-aliyun-security` + `/opt/aliyun-security/ilogtail-config/`.
    AliyunSecurity,
}

/// ilogtail installation and configuration
pub struct IlogtailInstaller<'a> {
    config: &'a TelemetryConfig,
    /// Cached variant detection so configure/remove operations use the same
    /// paths even when [`Self::ensure_installed`] was not called.
    variant: std::cell::Cell<Option<IlogtailVariant>>,
}

impl<'a> IlogtailInstaller<'a> {
    pub fn new(config: &'a TelemetryConfig) -> Self {
        Self {
            config,
            variant: std::cell::Cell::new(None),
        }
    }

    /// Detect the active ilogtail variant and cache the result.
    fn variant(&self) -> Result<IlogtailVariant, TelemetryError> {
        if let Some(v) = self.variant.get() {
            return Ok(v);
        }
        let v = if self.is_aliyun_security_running()? {
            IlogtailVariant::AliyunSecurity
        } else {
            IlogtailVariant::Standard
        };
        self.variant.set(Some(v));
        Ok(v)
    }

    /// Return the users directory for the active variant.
    fn users_dir(&self) -> Result<PathBuf, TelemetryError> {
        Ok(match self.variant()? {
            IlogtailVariant::Standard => self.config.ilogtail_users_dir.clone(),
            IlogtailVariant::AliyunSecurity => self.config.aliyun_security_users_dir.clone(),
        })
    }

    /// Return the user_defined_id file path for the active variant.
    fn user_defined_id_path(&self) -> Result<PathBuf, TelemetryError> {
        Ok(match self.variant()? {
            IlogtailVariant::Standard => self.config.user_defined_id_path.clone(),
            IlogtailVariant::AliyunSecurity => {
                self.config.aliyun_security_user_defined_id_path.clone()
            }
        })
    }

    /// Check if ilogtail is already installed and running; install if not.
    ///
    /// The `ilogtaild-aliyun-security` variant is preferred: if it is already
    /// installed and running, no additional ilogtail packages are installed.
    pub fn ensure_installed(&self, region_info: &RegionInfo) -> Result<(), TelemetryError> {
        match self.variant()? {
            IlogtailVariant::AliyunSecurity => Ok(()),
            IlogtailVariant::Standard => {
                if self.is_running()? {
                    return Ok(());
                }
                self.install(region_info)
            }
        }
    }

    /// Check whether the aliyun-security init script exists and reports running.
    fn is_aliyun_security_running(&self) -> Result<bool, TelemetryError> {
        let init_script = &self.config.aliyun_security_init;
        if !init_script.exists() {
            return Ok(false);
        }
        let output = run_init_status(init_script, "ilogtaild-aliyun-security")?;
        Ok(String::from_utf8_lossy(&output.stdout).contains("ilogtail is running"))
    }

    fn is_running(&self) -> Result<bool, TelemetryError> {
        let init_script = &self.config.ilogtaild_init;
        if !init_script.exists() {
            return Ok(false);
        }
        let output = run_init_status(init_script, "ilogtaild")?;
        Ok(String::from_utf8_lossy(&output.stdout).contains("ilogtail is running"))
    }

    /// Download and execute the official logtail installation script.
    fn install(&self, region_info: &RegionInfo) -> Result<(), TelemetryError> {
        let tmp_file = tempfile::Builder::new()
            .prefix("logtail-")
            .suffix(".sh")
            .tempfile()
            .map_err(TelemetryError::Io)?;
        let tmp_script = tmp_file.path().to_string_lossy().to_string();
        let _tmp_guard = tmp_file;
        let region_id = &region_info.region_id;

        if !region_id.chars().all(|c| c.is_alphanumeric() || c == '-') {
            return Err(TelemetryError::Command(format!(
                "invalid region-id: {region_id:?}"
            )));
        }

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
            .map_err(|e| TelemetryError::Command(format!("curl failed: {e}")))?;

        if !dl.success() {
            let _ = fs::remove_file(&tmp_script);
            return Err(TelemetryError::InstallFailed {
                code: dl.code().unwrap_or(-1),
                stderr: format!("curl download failed via {network} network: {url}"),
            });
        }

        Command::new("chmod")
            .args(["755", &tmp_script])
            .status()
            .map_err(|e| TelemetryError::Command(format!("chmod failed: {e}")))?;

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
            install.map_err(|e| TelemetryError::Command(format!("logtail install failed: {e}")))?;

        if !install.status.success() {
            let stdout = String::from_utf8_lossy(&install.stdout);
            let stderr = String::from_utf8_lossy(&install.stderr);
            return Err(TelemetryError::InstallFailed {
                code: install.status.code().unwrap_or(-1),
                stderr: format!("stdout: {stdout}\nstderr: {stderr}"),
            });
        }

        Ok(())
    }

    /// Configure SLS account file under the active variant's users directory.
    pub fn configure_account(&self) -> Result<(), TelemetryError> {
        validate_sls_account_id(&self.config.sls_account_id)?;
        let users_dir = self.users_dir()?;
        fs::create_dir_all(&users_dir)?;

        let account_file = users_dir.join(&self.config.sls_account_id);
        if !account_file.exists() {
            fs::write(&account_file, "")?;
        }
        Ok(())
    }

    /// Configure ops-side SLS account file under the active variant's users directory.
    pub fn configure_ops_account(&self) -> Result<(), TelemetryError> {
        validate_sls_account_id(&self.config.ops_sls_account_id)?;
        let users_dir = self.users_dir()?;
        fs::create_dir_all(&users_dir)?;

        let account_file = users_dir.join(&self.config.ops_sls_account_id);
        if !account_file.exists() {
            fs::write(&account_file, "")?;
        }
        Ok(())
    }

    /// Configure user_defined_id: append missing tags to the file.
    pub fn configure_user_defined_ids(&self) -> Result<(), TelemetryError> {
        Self::append_user_defined_ids(&self.user_defined_id_path()?, &self.config.user_defined_ids)
    }

    /// Configure ops-side user_defined_id tags.
    pub fn configure_ops_user_defined_ids(&self) -> Result<(), TelemetryError> {
        Self::append_user_defined_ids(
            &self.user_defined_id_path()?,
            &self.config.ops_user_defined_ids,
        )
    }

    /// Configure instance-id in the active variant's user_defined_id file.
    ///
    /// Writes a line of the form `instance-id=<id>`. If a line starting with
    /// `instance-id=` already exists, it is replaced (so re-registration is
    /// idempotent and instance-id changes are reflected). Empty `instance_id`
    /// values are rejected.
    pub fn configure_instance_id(&self, instance_id: &str) -> Result<(), TelemetryError> {
        if instance_id.is_empty() {
            return Err(TelemetryError::Command(
                "instance_id is empty; cannot write to user_defined_id".to_string(),
            ));
        }

        let path = self.user_defined_id_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let new_line = format!("instance-id={instance_id}");

        let existing = if path.exists() {
            fs::read_to_string(&path)?
        } else {
            String::new()
        };

        let mut content = existing
            .lines()
            .filter(|l| !l.trim().starts_with("instance-id="))
            .map(|l| format!("{l}\n"))
            .collect::<String>();

        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&new_line);
        content.push('\n');

        fs::write(&path, content)?;
        Ok(())
    }

    /// Shared helper: append missing tags to user_defined_id file.
    fn append_user_defined_ids(path: &Path, ids: &[String]) -> Result<(), TelemetryError> {
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
        for id in ids {
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

    /// Remove both user-side and ops-side SLS account files from the active
    /// variant's users directory.
    pub fn remove_account_files(&self) -> Result<(), TelemetryError> {
        let users_dir = self.users_dir()?;

        // 1. Remove user-side SLS account file
        if !self.config.sls_account_id.is_empty() {
            validate_sls_account_id(&self.config.sls_account_id)?;
            let account_file = users_dir.join(&self.config.sls_account_id);
            if account_file.exists() {
                fs::remove_file(&account_file)?;
            }
        }

        // 2. Remove ops-side SLS account file
        if !self.config.ops_sls_account_id.is_empty() {
            validate_sls_account_id(&self.config.ops_sls_account_id)?;
            let ops_account_file = users_dir.join(&self.config.ops_sls_account_id);
            if ops_account_file.exists() {
                fs::remove_file(&ops_account_file)?;
            }
        }

        Ok(())
    }

    /// Clean up the active variant's user_defined_id file by removing all tags
    /// owned by us.
    ///
    /// Removes both the configured user/op tags and any `instance-id=...` line
    /// written by [`Self::configure_instance_id`]. If the file becomes empty
    /// after removal, it is deleted entirely.
    pub fn remove_user_defined_ids(&self) -> Result<(), TelemetryError> {
        let path = self.user_defined_id_path()?;
        if !path.exists() {
            return Ok(());
        }

        // Collect all tags we own (user-side + ops-side)
        let all_our_tags: Vec<&str> = self
            .config
            .user_defined_ids
            .iter()
            .chain(self.config.ops_user_defined_ids.iter())
            .map(|s| s.as_str())
            .collect();

        let existing = fs::read_to_string(&path)?;
        let filtered: String = existing
            .lines()
            .filter(|l| {
                let line = l.trim();
                !all_our_tags.contains(&line) && !line.starts_with("instance-id=")
            })
            .map(|l| format!("{l}\n"))
            .collect();

        if filtered.trim().is_empty() {
            fs::remove_file(&path)?;
        } else {
            fs::write(&path, &filtered)?;
        }

        Ok(())
    }
}

fn run_init_status(init_script: &Path, name: &str) -> Result<Output, TelemetryError> {
    let mut command = Command::new(init_script);
    command
        .arg("status")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = crate::process::spawn_retry_etxtbsy(&mut command)
        .map_err(|e| TelemetryError::Command(format!("failed to check {name} status: {e}")))?;
    child
        .wait_with_output()
        .map_err(|e| TelemetryError::Command(format!("failed to check {name} status: {e}")))
}

// ── Unit tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::test_config;
    use tempfile::TempDir;

    /// Write an executable mock init script at `path` that prints `output`.
    #[cfg(unix)]
    fn write_mock_init_script(path: &Path, output: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{}'\n",
                output.replace('\\', "\\\\").replace('\'', "'\"'\"'")
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perm = fs::metadata(path).unwrap().permissions();
        perm.set_mode(0o755);
        fs::set_permissions(path, perm).unwrap();
    }

    // ── RegionProbe ──────────────────────────────────────────────────

    #[test]
    fn test_region_fallback_when_both_unavailable() {
        crate::metadata::with_cloud_init_disabled(|| {
            let probe = RegionProbe::new("http://127.0.0.1:19999/nope");
            let info = probe.probe().unwrap();
            assert_eq!(info.region_id, "cn-hangzhou");
            assert!(!info.use_internal);
        });
    }

    // ── IlogtailInstaller: configure ─────────────────────────────────

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
    fn test_configure_ops_account_creates_file() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        let installer = IlogtailInstaller::new(&cfg);
        installer.configure_ops_account().unwrap();

        let ops_account_file = cfg.ilogtail_users_dir.join(&cfg.ops_sls_account_id);
        assert!(ops_account_file.exists());
    }

    #[test]
    fn test_configure_account_missing_id() {
        let dir = TempDir::new().unwrap();
        let mut cfg = test_config(&dir);
        cfg.sls_account_id = String::new();
        let installer = IlogtailInstaller::new(&cfg);
        assert!(matches!(
            installer.configure_account(),
            Err(TelemetryError::MissingAccountId)
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

        installer.configure_user_defined_ids().unwrap();
        installer.configure_user_defined_ids().unwrap();

        let content = fs::read_to_string(&cfg.user_defined_id_path).unwrap();
        assert_eq!(content.matches("tag_a").count(), 1);
        assert_eq!(content.matches("tag_b").count(), 1);
    }

    #[test]
    fn test_configure_ops_user_defined_ids_appends() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        let installer = IlogtailInstaller::new(&cfg);
        installer.configure_ops_user_defined_ids().unwrap();

        let content = fs::read_to_string(&cfg.user_defined_id_path).unwrap();
        assert!(content.contains("anolisa-livetrace"));
    }

    // ── IlogtailInstaller: aliyun-security variant ───────────────────

    #[test]
    #[cfg(unix)]
    fn test_configure_account_uses_aliyun_security_path_when_running() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        write_mock_init_script(&cfg.aliyun_security_init, "ilogtail is running");

        let installer = IlogtailInstaller::new(&cfg);
        installer.configure_account().unwrap();

        assert!(
            cfg.aliyun_security_users_dir
                .join(&cfg.sls_account_id)
                .exists()
        );
        assert!(!cfg.ilogtail_users_dir.join(&cfg.sls_account_id).exists());
    }

    #[test]
    #[cfg(unix)]
    fn test_configure_user_defined_ids_uses_aliyun_security_path_when_running() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        write_mock_init_script(&cfg.aliyun_security_init, "ilogtail is running");

        let installer = IlogtailInstaller::new(&cfg);
        installer.configure_user_defined_ids().unwrap();

        let content = fs::read_to_string(&cfg.aliyun_security_user_defined_id_path).unwrap();
        assert!(content.contains("tag_a"));
        assert!(content.contains("tag_b"));
        assert!(!cfg.user_defined_id_path.exists());
    }

    #[test]
    #[cfg(unix)]
    fn test_ensure_installed_prefers_aliyun_security_and_skips_standard_install() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        write_mock_init_script(&cfg.aliyun_security_init, "ilogtail is running");

        let installer = IlogtailInstaller::new(&cfg);
        let region = RegionInfo {
            region_id: "cn-hangzhou".into(),
            use_internal: false,
        };
        installer.ensure_installed(&region).unwrap();

        // No standard install should have been attempted (no network call).
        // Configuration should target the aliyun-security path.
        installer.configure_account().unwrap();
        assert!(
            cfg.aliyun_security_users_dir
                .join(&cfg.sls_account_id)
                .exists()
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_ensure_installed_retries_when_init_script_is_temporarily_busy() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        write_mock_init_script(&cfg.aliyun_security_init, "ilogtail is running");

        let writer = fs::OpenOptions::new()
            .write(true)
            .open(&cfg.aliyun_security_init)
            .unwrap();
        let cfg_for_thread = cfg.clone();
        let handle = std::thread::spawn(move || {
            let installer = IlogtailInstaller::new(&cfg_for_thread);
            let region = RegionInfo {
                region_id: "cn-hangzhou".into(),
                use_internal: false,
            };
            installer.ensure_installed(&region)
        });

        std::thread::sleep(std::time::Duration::from_millis(25));
        drop(writer);

        handle.join().unwrap().unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn test_remove_account_files_uses_aliyun_security_path_when_running() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        write_mock_init_script(&cfg.aliyun_security_init, "ilogtail is running");

        fs::create_dir_all(&cfg.aliyun_security_users_dir).unwrap();
        let account_file = cfg.aliyun_security_users_dir.join(&cfg.sls_account_id);
        let ops_account_file = cfg.aliyun_security_users_dir.join(&cfg.ops_sls_account_id);
        fs::write(&account_file, "").unwrap();
        fs::write(&ops_account_file, "").unwrap();

        let installer = IlogtailInstaller::new(&cfg);
        installer.remove_account_files().unwrap();

        assert!(!account_file.exists());
        assert!(!ops_account_file.exists());
    }

    // ── IlogtailInstaller: remove ────────────────────────────────────

    #[test]
    fn test_remove_account_files_and_user_defined_ids() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);

        // Write tags and account files first
        fs::write(
            &cfg.user_defined_id_path,
            "tag_a\ntag_b\nanolisa-livetrace\nother_tag\n",
        )
        .unwrap();
        fs::create_dir_all(&cfg.ilogtail_users_dir).unwrap();
        let account_file = cfg.ilogtail_users_dir.join(&cfg.sls_account_id);
        let ops_account_file = cfg.ilogtail_users_dir.join(&cfg.ops_sls_account_id);
        fs::write(&account_file, "").unwrap();
        fs::write(&ops_account_file, "").unwrap();

        let installer = IlogtailInstaller::new(&cfg);
        installer.remove_account_files().unwrap();
        installer.remove_user_defined_ids().unwrap();

        // All our tags should be removed from user_defined_id
        let content = fs::read_to_string(&cfg.user_defined_id_path).unwrap();
        assert!(!content.contains("tag_a"));
        assert!(!content.contains("tag_b"));
        assert!(!content.contains("anolisa-livetrace"));
        assert!(content.contains("other_tag"));

        // Both SLS account files should be deleted
        assert!(!account_file.exists());
        assert!(!ops_account_file.exists());
    }

    #[test]
    fn test_remove_user_defined_ids_deletes_empty_file() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);

        // Write only our tags; file should be deleted after removal
        fs::write(
            &cfg.user_defined_id_path,
            "tag_a\ntag_b\nanolisa-livetrace\n",
        )
        .unwrap();

        let installer = IlogtailInstaller::new(&cfg);
        installer.remove_user_defined_ids().unwrap();

        assert!(!cfg.user_defined_id_path.exists());
    }

    #[test]
    fn test_remove_user_defined_ids_noop_when_file_absent() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        let installer = IlogtailInstaller::new(&cfg);
        assert!(installer.remove_user_defined_ids().is_ok());
    }

    // ── IlogtailInstaller: instance-id ───────────────────────────────

    #[test]
    fn test_configure_instance_id_creates_file() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        let installer = IlogtailInstaller::new(&cfg);
        installer.configure_instance_id("i-abc123").unwrap();

        let content = fs::read_to_string(&cfg.user_defined_id_path).unwrap();
        assert!(content.contains("instance-id=i-abc123"));
    }

    #[test]
    fn test_configure_instance_id_rejects_empty() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        let installer = IlogtailInstaller::new(&cfg);
        assert!(installer.configure_instance_id("").is_err());
    }

    #[test]
    fn test_configure_instance_id_idempotent() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        let installer = IlogtailInstaller::new(&cfg);

        installer.configure_instance_id("i-abc123").unwrap();
        installer.configure_instance_id("i-abc123").unwrap();

        let content = fs::read_to_string(&cfg.user_defined_id_path).unwrap();
        assert_eq!(content.matches("instance-id=i-abc123").count(), 1);
    }

    #[test]
    fn test_configure_instance_id_updates_on_change() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        let installer = IlogtailInstaller::new(&cfg);

        installer.configure_instance_id("i-old").unwrap();
        installer.configure_instance_id("i-new").unwrap();

        let content = fs::read_to_string(&cfg.user_defined_id_path).unwrap();
        assert!(!content.contains("instance-id=i-old"));
        assert!(content.contains("instance-id=i-new"));
    }

    #[test]
    fn test_remove_user_defined_ids_cleans_instance_id() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        fs::write(
            &cfg.user_defined_id_path,
            "tag_a\ninstance-id=i-abc123\nother_tag\n",
        )
        .unwrap();

        let installer = IlogtailInstaller::new(&cfg);
        installer.remove_user_defined_ids().unwrap();

        let content = fs::read_to_string(&cfg.user_defined_id_path).unwrap();
        assert!(!content.contains("instance-id=i-abc123"));
        assert!(content.contains("other_tag"));
    }

    #[test]
    fn test_configure_instance_id_preserves_multi_section_file() {
        let dir = TempDir::new().unwrap();
        let cfg = test_config(&dir);
        let installer = IlogtailInstaller::new(&cfg);

        fs::write(
            &cfg.user_defined_id_path,
            "# Section 1\ntag_a\n\n# Section 2\ntag_b\n",
        )
        .unwrap();

        installer.configure_instance_id("i-abc123").unwrap();

        let content = fs::read_to_string(&cfg.user_defined_id_path).unwrap();
        assert!(content.contains("# Section 1"));
        assert!(content.contains("tag_a"));
        assert!(content.contains("# Section 2"));
        assert!(content.contains("tag_b"));
        assert!(content.contains("instance-id=i-abc123"));
        assert!(content.ends_with('\n'));
    }
}
