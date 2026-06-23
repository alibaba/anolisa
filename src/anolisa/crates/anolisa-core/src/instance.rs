//! Instance metadata probing for telemetry integration
//!
//! Collects machine identity and hardware specs at register time,
//! producing an `InstanceSnapshot` that is written to
//! `/var/log/anolisa/sls/ops/instance.jsonl`.

use crate::metadata::MetadataClient;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

// ── Instance info ────────────────────────────────────────────────────

/// Instance metadata collected at register time (design doc §4.3)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceInfo {
    pub id: String,
    /// "ecs" | "swas" | "eds" | "unknown"
    pub source: String,
    /// instance-type from metadata API (e.g. "ecs.c7.xlarge")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_type: Option<String>,
    pub region: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vcpu_count: Option<u32>,
    /// image-id from metadata API
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_id: Option<String>,
    /// Distribution ID from /etc/os-release (e.g. "alinux", "ubuntu")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distro_name: Option<String>,
    /// Distribution version from /etc/os-release (e.g. "3", "20.04")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distro_version: Option<String>,
}

// ── Instance snapshot ────────────────────────────────────────────────

/// A single flat JSONL record written to instance.jsonl (design doc §4.3).
/// Fields are dot-prefixed to match the downstream telemetry schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceSnapshot {
    #[serde(rename = "instance.id")]
    pub instance_id: String,
    #[serde(rename = "instance.owner_account_id")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_owner_account_id: Option<String>,
    #[serde(rename = "instance.source")]
    pub instance_source: String,
    #[serde(rename = "instance.type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_type: Option<String>,
    /// vCPU count as string to match downstream SLS JSONL schema; empty when unavailable.
    #[serde(rename = "instance.vcpu_count")]
    pub instance_vcpu_count: String,
    #[serde(rename = "instance.region-id")]
    pub instance_region_id: String,
    #[serde(rename = "instance.image-id")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_image_id: Option<String>,
    #[serde(rename = "distro.name")]
    pub distro_name: String,
    #[serde(rename = "distro.version")]
    pub distro_version: String,
}

impl InstanceSnapshot {
    /// Build a flat snapshot from probed instance metadata.
    pub fn from_instance_info(info: &InstanceInfo) -> Self {
        Self {
            instance_id: info.id.clone(),
            instance_owner_account_id: info.owner_account_id.clone(),
            instance_source: info.source.clone(),
            instance_type: info.instance_type.clone(),
            instance_vcpu_count: info.vcpu_count.map(|n| n.to_string()).unwrap_or_default(),
            instance_region_id: info.region.clone(),
            instance_image_id: info.image_id.clone(),
            distro_name: info.distro_name.clone().unwrap_or_default(),
            distro_version: info.distro_version.clone().unwrap_or_default(),
        }
    }

    /// Write this snapshot as a single JSONL line to `path`.
    ///
    /// - If the file exists and is non-empty: the line is appended.
    /// - If the file is missing or empty: the line is written to a new file.
    pub fn write_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut line =
            serde_json::to_string(&self).map_err(|e| std::io::Error::other(e.to_string()))?;
        line.push('\n');

        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        file.write_all(line.as_bytes())?;
        file.flush()?;

        Ok(())
    }
}

// ── Instance prober ──────────────────────────────────────────────────

/// Configurable paths for instance probing (production defaults + test injection)
pub struct InstanceProber {
    client: MetadataClient,
    machine_id_path: PathBuf,
    release_path: PathBuf,
    os_release_path: PathBuf,
    instance_id_cache_path: PathBuf,
    cpu_present_path: PathBuf,
    image_id_path: PathBuf,
}

impl InstanceProber {
    /// Construct with custom paths (for unit tests only)
    pub fn with_paths(
        metadata_url_base: &str,
        machine_id_path: PathBuf,
        release_path: PathBuf,
        os_release_path: PathBuf,
        instance_id_cache_path: PathBuf,
        cpu_present_path: PathBuf,
        image_id_path: PathBuf,
    ) -> Self {
        Self {
            client: MetadataClient::new(metadata_url_base),
            machine_id_path,
            release_path,
            os_release_path,
            instance_id_cache_path,
            cpu_present_path,
            image_id_path,
        }
    }

    /// Construct with a pre-built `MetadataClient` (avoids re-parsing the URL).
    pub fn with_client(
        client: MetadataClient,
        machine_id_path: PathBuf,
        release_path: PathBuf,
        os_release_path: PathBuf,
        instance_id_cache_path: PathBuf,
        cpu_present_path: PathBuf,
        image_id_path: PathBuf,
    ) -> Self {
        Self {
            client,
            machine_id_path,
            release_path,
            os_release_path,
            instance_id_cache_path,
            cpu_present_path,
            image_id_path,
        }
    }

    /// Run all probes and return an `InstanceInfo` with best-effort values.
    pub fn probe(&self, region: &str) -> InstanceInfo {
        let (distro_name, distro_version) = self.probe_distro();

        InstanceInfo {
            id: self.probe_instance_id(),
            source: self.probe_product_type(),
            instance_type: self.probe_instance_type(),
            region: region.to_string(),
            owner_account_id: self.probe_owner_account_id(),
            vcpu_count: self.probe_vcpu_count(),
            image_id: self.probe_image_id(),
            distro_name,
            distro_version,
        }
    }

    // ── Instance ID ──────────────────────────────────────────────────

    fn probe_instance_id(&self) -> String {
        // 1. Metadata API, then cloud-init datasource
        if let Some(id) = self.client.query("instance-id") {
            self.write_cached_id(&id);
            return id;
        }
        // 2. Cached ID from a previous successful probe (ensures ID stability
        //    across re-registrations even if metadata API becomes unreachable)
        if let Some(id) = self.read_cached_id() {
            return id;
        }
        // 3. /etc/machine-id
        if let Ok(content) = fs::read_to_string(&self.machine_id_path) {
            let id = content.trim().to_string();
            if !id.is_empty() {
                self.write_cached_id(&id);
                return id;
            }
        }
        // 4. Generate UUID and cache
        self.get_or_generate_cached_id()
    }

    fn read_cached_id(&self) -> Option<String> {
        let content = fs::read_to_string(&self.instance_id_cache_path).ok()?;
        let id = content.trim().to_string();
        if id.is_empty() { None } else { Some(id) }
    }

    fn write_cached_id(&self, id: &str) {
        if let Some(parent) = self.instance_id_cache_path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                eprintln!(
                    "[anolisa] warn: cannot create instance-id cache dir {}: {e}",
                    parent.display()
                );
            }
        }
        if let Err(e) = fs::write(&self.instance_id_cache_path, id) {
            eprintln!(
                "[anolisa] warn: cannot write instance-id cache {}: {e}",
                self.instance_id_cache_path.display()
            );
        }
    }

    fn get_or_generate_cached_id(&self) -> String {
        if let Some(id) = self.read_cached_id() {
            return id;
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.write_cached_id(&id);
        id
    }

    // ── Product type ─────────────────────────────────────────────────

    fn probe_product_type(&self) -> String {
        // 1. /etc/anolisa-release PRODUCT_TYPE field
        if let Ok(content) = fs::read_to_string(&self.release_path) {
            if let Some(pt) = crate::register::find_product_type_in_release(&content) {
                return pt.to_string();
            }
        }

        // 2. EDS detection: desktop-id starts with "ecd"
        if let Some(desktop_id) = self.client.query("desktop-id") {
            if desktop_id.starts_with("ecd") {
                return "eds".to_string();
            }
        }

        // 3. ECS detection: instance-type starts with "ecs"
        if let Some(instance_type) = self.client.query("instance/instance-type") {
            if instance_type.starts_with("ecs") {
                return "ecs".to_string();
            }
        }

        "unknown".to_string()
    }

    // ── Instance type ────────────────────────────────────────────────

    fn probe_instance_type(&self) -> Option<String> {
        self.client.query_metadata("instance/instance-type")
    }

    // ── Owner account ID ─────────────────────────────────────────────

    fn probe_owner_account_id(&self) -> Option<String> {
        self.client.query_metadata("owner-account-id")
    }

    // ── vCPU count ──────────────────────────────────────────────────

    fn probe_vcpu_count(&self) -> Option<u32> {
        // Try /sys/devices/system/cpu/present
        if let Ok(content) = fs::read_to_string(&self.cpu_present_path)
            && let Some(count) = parse_cpu_present(&content)
        {
            return Some(count);
        }
        // Fallback: nproc
        let output = Command::new("nproc").output().ok()?;
        if output.status.success() {
            let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
            return s.parse().ok();
        }
        None
    }

    // ── Image ID ─────────────────────────────────────────────────────

    fn probe_image_id(&self) -> Option<String> {
        // 1. /etc/image-id (image_id="...")
        if let Ok(content) = fs::read_to_string(&self.image_id_path) {
            if let Some(id) = parse_image_id(&content) {
                return Some(id);
            }
        }
        // 2. Metadata API fallback
        self.client.query_metadata("image-id")
    }

    // ── Distribution ─────────────────────────────────────────────────

    fn probe_distro(&self) -> (Option<String>, Option<String>) {
        let content = match fs::read_to_string(&self.os_release_path) {
            Ok(c) => c,
            Err(_) => return (None, None),
        };

        let mut id: Option<String> = None;
        let mut version: Option<String> = None;

        for line in content.lines() {
            if id.is_none() {
                if let Some(val) = line.strip_prefix("ID=") {
                    id = Some(unquote(val));
                }
            }
            if version.is_none() {
                if let Some(val) = line.strip_prefix("VERSION_ID=") {
                    version = Some(unquote(val));
                }
            }
            if id.is_some() && version.is_some() {
                break;
            }
        }

        (id, version)
    }
}

// ── Parsing helpers ──────────────────────────────────────────────────

/// Parse `/sys/devices/system/cpu/present` format (e.g. "0-3" → 4, "0" → 1)
fn parse_cpu_present(content: &str) -> Option<u32> {
    let s = content.trim();
    if s.contains('-') {
        let parts: Vec<&str> = s.splitn(2, '-').collect();
        if parts.len() == 2 {
            let lo: u32 = parts[0].parse().ok()?;
            let hi: u32 = parts[1].parse().ok()?;
            return Some(hi - lo + 1);
        }
    }
    // Single CPU: "0"
    s.parse::<u32>().ok().map(|v| v + 1)
}

/// Parse `image_id="..."` from `/etc/image-id` content.
fn parse_image_id(content: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("image_id=") {
            let id = unquote(val);
            if !id.is_empty() {
                return Some(id);
            }
        }
    }
    None
}

/// Unquote a shell-style value (strip surrounding double or single quotes)
fn unquote(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

// ── Unit tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn prober(dir: &TempDir) -> InstanceProber {
        InstanceProber::with_paths(
            "http://127.0.0.1:19999/no-such-endpoint",
            dir.path().join("machine-id"),
            dir.path().join("anolisa-release"),
            dir.path().join("os-release"),
            dir.path().join("instance-id.cache"),
            dir.path().join("cpu-present"),
            dir.path().join("image-id"),
        )
    }

    #[test]
    fn test_probe_product_type_from_release_file() {
        let dir = TempDir::new().unwrap();
        let p = prober(&dir);
        fs::write(&p.release_path, "PRODUCT_TYPE=ecs\n").unwrap();
        assert_eq!(p.probe_product_type(), "ecs");
    }

    #[test]
    fn test_probe_product_type_unknown() {
        crate::metadata::with_cloud_init_disabled(|| {
            let dir = TempDir::new().unwrap();
            let p = prober(&dir);
            assert_eq!(p.probe_product_type(), "unknown");
        });
    }

    #[test]
    fn test_probe_product_type_from_instance_type() {
        crate::metadata::with_cloud_init_responses(
            &[("ds.meta_data.instance.instance-type", "ecs.c7.xlarge")],
            || {
                let dir = TempDir::new().unwrap();
                let p = prober(&dir);
                assert_eq!(p.probe_product_type(), "ecs");
            },
        );
    }

    #[test]
    fn test_probe_product_type_from_desktop_id() {
        crate::metadata::with_cloud_init_responses(
            &[("ds.meta_data.desktop-id", "ecd-abc123")],
            || {
                let dir = TempDir::new().unwrap();
                let p = prober(&dir);
                assert_eq!(p.probe_product_type(), "eds");
            },
        );
    }

    #[test]
    fn test_probe_instance_id_from_machine_id() {
        crate::metadata::with_cloud_init_disabled(|| {
            let dir = TempDir::new().unwrap();
            let p = prober(&dir);
            fs::write(&p.machine_id_path, "abc123def456\n").unwrap();
            assert_eq!(p.probe_instance_id(), "abc123def456");
        });
    }

    #[test]
    fn test_probe_instance_id_generated_and_cached() {
        crate::metadata::with_cloud_init_disabled(|| {
            let dir = TempDir::new().unwrap();
            let p = prober(&dir);
            let id1 = p.probe_instance_id();
            assert!(!id1.is_empty());

            // Second call should return the same cached ID
            let id2 = p.probe_instance_id();
            assert_eq!(id1, id2);
        });
    }

    #[test]
    fn test_probe_instance_id_prefers_cache_over_machine_id() {
        crate::metadata::with_cloud_init_disabled(|| {
            let dir = TempDir::new().unwrap();
            let p = prober(&dir);

            fs::write(&p.instance_id_cache_path, "i-cached123\n").unwrap();
            fs::write(&p.machine_id_path, "different-machine-id\n").unwrap();

            // Should return cached ID, not machine-id
            assert_eq!(p.probe_instance_id(), "i-cached123");
        });
    }

    #[test]
    fn test_probe_instance_id_caches_machine_id() {
        crate::metadata::with_cloud_init_disabled(|| {
            let dir = TempDir::new().unwrap();
            let p = prober(&dir);

            fs::write(&p.machine_id_path, "abc123def456\n").unwrap();
            assert_eq!(p.probe_instance_id(), "abc123def456");

            // Cache file should now contain the machine-id
            let cached = fs::read_to_string(&p.instance_id_cache_path).unwrap();
            assert_eq!(cached.trim(), "abc123def456");

            // Remove machine-id; second call should return cached value
            fs::remove_file(&p.machine_id_path).unwrap();
            assert_eq!(p.probe_instance_id(), "abc123def456");
        });
    }

    #[test]
    fn test_probe_image_id_from_etc_image_id() {
        let dir = TempDir::new().unwrap();
        let p = prober(&dir);
        fs::write(
            &p.image_id_path,
            "image_name=\"Alibaba Cloud Linux 4 LTS 64 bit\"\nimage_id=\"aliyun_4_x64_20G_agentic_alibase_20260612.vhd\"\nrelease_date=\"20260612200340\"\n",
        )
        .unwrap();
        assert_eq!(
            p.probe_image_id(),
            Some("aliyun_4_x64_20G_agentic_alibase_20260612.vhd".to_string())
        );
    }

    #[test]
    fn test_parse_image_id() {
        assert_eq!(
            parse_image_id(r#"image_id="aliyun_4_x64_20G_agentic_alibase_20260612.vhd""#),
            Some("aliyun_4_x64_20G_agentic_alibase_20260612.vhd".to_string())
        );
        assert_eq!(
            parse_image_id("image_id='quoted'\n"),
            Some("quoted".to_string())
        );
        assert_eq!(parse_image_id("image_name=\"foo\"\n"), None);
        assert_eq!(parse_image_id("image_id=\"\"\n"), None);
        assert_eq!(parse_image_id("\n"), None);
    }

    #[test]
    fn test_probe_vcpu_count_from_present() {
        let dir = TempDir::new().unwrap();
        let p = prober(&dir);
        fs::write(&p.cpu_present_path, "0-3\n").unwrap();
        assert_eq!(p.probe_vcpu_count(), Some(4));
    }

    #[test]
    fn test_probe_vcpu_count_single() {
        let dir = TempDir::new().unwrap();
        let p = prober(&dir);
        fs::write(&p.cpu_present_path, "0\n").unwrap();
        assert_eq!(p.probe_vcpu_count(), Some(1));
    }

    #[test]
    fn test_probe_distro() {
        let dir = TempDir::new().unwrap();
        let p = prober(&dir);
        fs::write(
            &p.os_release_path,
            "NAME=\"Alibaba Cloud Linux\"\nID=\"alinux\"\nVERSION_ID=\"3\"\n",
        )
        .unwrap();
        let (name, version) = p.probe_distro();
        assert_eq!(name, Some("alinux".to_string()));
        assert_eq!(version, Some("3".to_string()));
    }

    #[test]
    fn test_probe_distro_unquoted() {
        let dir = TempDir::new().unwrap();
        let p = prober(&dir);
        fs::write(
            &p.os_release_path,
            "NAME=Ubuntu\nID=ubuntu\nVERSION_ID=20.04\n",
        )
        .unwrap();
        let (name, version) = p.probe_distro();
        assert_eq!(name, Some("ubuntu".to_string()));
        assert_eq!(version, Some("20.04".to_string()));
    }

    #[test]
    fn test_snapshot_write_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("instance.jsonl");
        let snapshot = InstanceSnapshot {
            instance_id: "i-test".to_string(),
            instance_owner_account_id: Some("1644215368948677".to_string()),
            instance_source: "ecs".to_string(),
            instance_type: Some("ecs.g7.xlarge".to_string()),
            instance_vcpu_count: "4".to_string(),
            instance_region_id: "cn-hangzhou".to_string(),
            instance_image_id: Some("img-test".to_string()),
            distro_name: "alinux".to_string(),
            distro_version: "3".to_string(),
        };
        snapshot.write_to(&path).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"instance.id\":\"i-test\""));
        assert!(content.contains("\"instance.owner_account_id\":\"1644215368948677\""));
        assert!(content.contains("\"instance.source\":\"ecs\""));
        assert!(content.contains("\"instance.type\":\"ecs.g7.xlarge\""));
        assert!(content.contains("\"instance.vcpu_count\":\"4\""));
        assert!(content.contains("\"instance.region-id\":\"cn-hangzhou\""));
        assert!(content.contains("\"instance.image-id\":\"img-test\""));
        assert!(content.contains("\"distro.name\":\"alinux\""));
        assert!(content.contains("\"distro.version\":\"3\""));
    }

    #[test]
    fn test_snapshot_append_on_existing_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("instance.jsonl");

        // First write
        let snapshot = InstanceSnapshot {
            instance_id: "i-test".to_string(),
            instance_owner_account_id: Some("1644215368948677".to_string()),
            instance_source: "ecs".to_string(),
            instance_type: Some("ecs.g7.xlarge".to_string()),
            instance_vcpu_count: "4".to_string(),
            instance_region_id: "cn-hangzhou".to_string(),
            instance_image_id: Some("img-test".to_string()),
            distro_name: "alinux".to_string(),
            distro_version: "3".to_string(),
        };
        snapshot.write_to(&path).unwrap();

        // Second write appends another line
        snapshot.write_to(&path).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"instance.id\":\"i-test\""));
        assert!(lines[1].contains("\"instance.id\":\"i-test\""));
    }

    #[test]
    fn test_parse_cpu_present_range() {
        assert_eq!(parse_cpu_present("0-3"), Some(4));
        assert_eq!(parse_cpu_present("0-7"), Some(8));
        assert_eq!(parse_cpu_present("0"), Some(1));
        assert_eq!(parse_cpu_present("2-5"), Some(4));
    }
}
