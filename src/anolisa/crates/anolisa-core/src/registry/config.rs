//! Registry configuration: layered resolution of the distribution index URL
//! and its cache policy.
//!
//! Resolution order is bundled default < `/etc/anolisa/config.toml` <
//! `ANOLISA_REGISTRY_URL`. The core stays prefix-agnostic: the caller supplies
//! both the config path and the env override, so layering is fully testable
//! without touching the real filesystem or process environment.

use std::io;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use super::error::RegistryError;

/// Built-in distribution index URL.
///
/// The live public mirror on Aliyun OSS. Since remote fetching is now
/// default-on (every `enable` resolves the index from here unless overridden),
/// this value is load-bearing: a deployment retargets it via the
/// `[registry] url` config key or `ANOLISA_REGISTRY_URL`, and a cold-cache
/// offline run degrades to the bundled local index rather than hard-failing.
const DEFAULT_INDEX_URL: &str =
    "https://anolisa.oss-cn-hangzhou.aliyuncs.com/anolisa-releases/anolisa/v1/index.toml";

/// Default cache freshness window (1 hour). Index older than this triggers a
/// refetch on the next [`RegistryClient`](super::RegistryClient) access.
const DEFAULT_CACHE_TTL_SECS: u64 = 3600;

/// Resolved registry settings after layering config file + env override.
#[derive(Debug, Clone)]
pub struct RegistryConfig {
    /// Absolute URL of the distribution `index.toml`.
    pub index_url: String,
    /// Cache freshness window; index older than this triggers a refetch.
    pub cache_ttl: Duration,
    /// Serve a stale cached index (with a warning) when the network is down.
    pub offline_fallback: bool,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self::bundled_default()
    }
}

impl RegistryConfig {
    /// Built-in defaults pointing at the public mirror.
    pub fn bundled_default() -> Self {
        Self {
            index_url: DEFAULT_INDEX_URL.to_string(),
            cache_ttl: Duration::from_secs(DEFAULT_CACHE_TTL_SECS),
            offline_fallback: true,
        }
    }

    /// Resolve settings by layering, in increasing priority: bundled default,
    /// the `[registry]` table of `config_path`, then `env_url`.
    ///
    /// A missing config file is **not** an error — defaults stand. `env_url`
    /// (typically `ANOLISA_REGISTRY_URL`) overrides only the index URL, and an
    /// empty/whitespace value is ignored so an unset-but-exported variable
    /// does not blank the URL.
    ///
    /// # Errors
    /// Returns [`RegistryError::Config`] when the file exists but cannot be
    /// read or its `[registry]` table is malformed; the error carries the path.
    pub fn load(config_path: &Path, env_url: Option<&str>) -> Result<Self, RegistryError> {
        let mut config = Self::bundled_default();

        // Layer 2: config file. Absence is fine; any other read/parse failure
        // surfaces as Config so the user learns which file is broken.
        match std::fs::read_to_string(config_path) {
            Ok(content) => {
                let parsed: ConfigFileRaw =
                    toml::from_str(&content).map_err(|e| RegistryError::Config {
                        path: config_path.to_path_buf(),
                        reason: e.to_string(),
                    })?;
                if let Some(reg) = parsed.registry {
                    config.apply_file_table(reg);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(RegistryError::Config {
                    path: config_path.to_path_buf(),
                    reason: format!("cannot read: {e}"),
                });
            }
        }

        // Layer 3: env override (highest priority), URL only.
        if let Some(url) = env_url {
            let trimmed = url.trim();
            if !trimmed.is_empty() {
                config.index_url = trimmed.to_string();
            }
        }

        Ok(config)
    }

    /// Like [`load`](Self::load), but returns `None` when **no layer opts in**
    /// to the remote registry — i.e. `env_url` is empty/absent AND the config
    /// file has no `[registry]` table.
    ///
    /// The CLI uses this to keep remote fetching strictly opt-in for the MVP:
    /// without explicit configuration it falls back to the bundled local
    /// index, so default installs never block on a network timeout against
    /// the placeholder mirror URL.
    ///
    /// # Errors
    /// Same as [`load`](Self::load): [`RegistryError::Config`] on an
    /// unreadable or malformed config file.
    pub fn load_if_configured(
        config_path: &Path,
        env_url: Option<&str>,
    ) -> Result<Option<Self>, RegistryError> {
        let env_opted = env_url.is_some_and(|u| !u.trim().is_empty());

        let file_table = match std::fs::read_to_string(config_path) {
            Ok(content) => {
                let parsed: ConfigFileRaw =
                    toml::from_str(&content).map_err(|e| RegistryError::Config {
                        path: config_path.to_path_buf(),
                        reason: e.to_string(),
                    })?;
                parsed.registry
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(RegistryError::Config {
                    path: config_path.to_path_buf(),
                    reason: format!("cannot read: {e}"),
                });
            }
        };

        if !env_opted && file_table.is_none() {
            return Ok(None);
        }

        let mut config = Self::bundled_default();
        if let Some(table) = file_table {
            config.apply_file_table(table);
        }
        if env_opted && let Some(url) = env_url {
            config.index_url = url.trim().to_string();
        }
        Ok(Some(config))
    }

    /// Overlay the non-`None` fields of a parsed `[registry]` table.
    fn apply_file_table(&mut self, table: RegistryTableRaw) {
        if let Some(url) = table.url {
            self.index_url = url;
        }
        if let Some(secs) = table.cache_ttl_secs {
            self.cache_ttl = Duration::from_secs(secs);
        }
        if let Some(offline) = table.offline_fallback {
            self.offline_fallback = offline;
        }
    }
}

/// Top-level config file shape. Only the `[registry]` table is consumed;
/// unknown tables (other ANOLISA config sections) are ignored by serde.
#[derive(Debug, Default, Deserialize)]
struct ConfigFileRaw {
    registry: Option<RegistryTableRaw>,
}

/// Tolerant view of the `[registry]` table — every field optional so partial
/// tables layer cleanly over the bundled defaults.
#[derive(Debug, Default, Deserialize)]
struct RegistryTableRaw {
    url: Option<String>,
    cache_ttl_secs: Option<u64>,
    offline_fallback: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn write_config(dir: &TempDir, body: &str) -> PathBuf {
        let path = dir.path().join("config.toml");
        let mut f = std::fs::File::create(&path).expect("create config");
        f.write_all(body.as_bytes()).expect("write config");
        path
    }

    #[test]
    fn bundled_default_has_expected_values() {
        let c = RegistryConfig::bundled_default();
        assert_eq!(c.index_url, DEFAULT_INDEX_URL);
        assert_eq!(c.cache_ttl, Duration::from_secs(3600));
        assert!(c.offline_fallback);
    }

    #[test]
    fn missing_file_no_env_yields_defaults() {
        let path = Path::new("/nonexistent/anolisa/config.toml");
        let c = RegistryConfig::load(path, None).expect("missing file is not an error");
        assert_eq!(c.index_url, DEFAULT_INDEX_URL);
        assert!(c.offline_fallback);
    }

    #[test]
    fn config_file_overrides_all_fields() {
        let dir = TempDir::new().unwrap();
        let path = write_config(
            &dir,
            r#"
            [registry]
            url = "https://example.test/v1/index.toml"
            cache_ttl_secs = 120
            offline_fallback = false
            "#,
        );
        let c = RegistryConfig::load(&path, None).expect("valid config");
        assert_eq!(c.index_url, "https://example.test/v1/index.toml");
        assert_eq!(c.cache_ttl, Duration::from_secs(120));
        assert!(!c.offline_fallback);
    }

    #[test]
    fn partial_table_layers_over_defaults() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "[registry]\ncache_ttl_secs = 7\n");
        let c = RegistryConfig::load(&path, None).expect("valid config");
        // url/offline keep defaults; only ttl changed.
        assert_eq!(c.index_url, DEFAULT_INDEX_URL);
        assert_eq!(c.cache_ttl, Duration::from_secs(7));
        assert!(c.offline_fallback);
    }

    #[test]
    fn env_overrides_file_url() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "[registry]\nurl = \"https://file.test/index.toml\"\n");
        let c =
            RegistryConfig::load(&path, Some("https://env.test/index.toml")).expect("valid config");
        assert_eq!(c.index_url, "https://env.test/index.toml");
    }

    #[test]
    fn empty_env_is_ignored() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "[registry]\nurl = \"https://file.test/index.toml\"\n");
        let c = RegistryConfig::load(&path, Some("   ")).expect("valid config");
        assert_eq!(c.index_url, "https://file.test/index.toml");
    }

    #[test]
    fn unrelated_tables_are_ignored() {
        let dir = TempDir::new().unwrap();
        let path = write_config(
            &dir,
            "[telemetry]\nenabled = true\n\n[registry]\nurl = \"https://r.test/i.toml\"\n",
        );
        let c = RegistryConfig::load(&path, None).expect("valid config");
        assert_eq!(c.index_url, "https://r.test/i.toml");
    }

    #[test]
    fn load_if_configured_none_when_nothing_opts_in() {
        let path = Path::new("/nonexistent/anolisa/config.toml");
        assert!(
            RegistryConfig::load_if_configured(path, None)
                .expect("missing everything is not an error")
                .is_none()
        );
        // Empty env value must not count as opting in.
        assert!(
            RegistryConfig::load_if_configured(path, Some("  "))
                .expect("blank env is not an error")
                .is_none()
        );
    }

    #[test]
    fn load_if_configured_env_opts_in() {
        let path = Path::new("/nonexistent/anolisa/config.toml");
        let c = RegistryConfig::load_if_configured(path, Some("http://r.test/i.toml"))
            .expect("valid")
            .expect("env opts in");
        assert_eq!(c.index_url, "http://r.test/i.toml");
    }

    #[test]
    fn load_if_configured_registry_table_opts_in() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "[registry]\nurl = \"https://file.test/i.toml\"\n");
        let c = RegistryConfig::load_if_configured(&path, None)
            .expect("valid")
            .expect("table opts in");
        assert_eq!(c.index_url, "https://file.test/i.toml");
    }

    #[test]
    fn load_if_configured_file_without_registry_table_stays_none() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "[telemetry]\nenabled = true\n");
        assert!(
            RegistryConfig::load_if_configured(&path, None)
                .expect("valid file, no registry table")
                .is_none()
        );
    }

    #[test]
    fn malformed_registry_table_errors_with_path() {
        let dir = TempDir::new().unwrap();
        // cache_ttl_secs expects an integer; a string is a type error.
        let path = write_config(&dir, "[registry]\ncache_ttl_secs = \"soon\"\n");
        let err = RegistryConfig::load(&path, None).expect_err("type mismatch must error");
        match err {
            RegistryError::Config { path: p, .. } => assert_eq!(p, path),
            other => panic!("expected Config, got {other:?}"),
        }
    }
}
