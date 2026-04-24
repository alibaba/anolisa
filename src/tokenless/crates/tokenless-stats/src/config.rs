//! Configuration for tokenless.
//!
//! Stored at `~/.tokenless/config.json`. Controls global feature flags.
//! Environment variable `TOKENLESS_STATS_ENABLED` overrides file config.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Global tokenless configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenlessConfig {
    /// Whether to record compression stats (default: true)
    #[serde(default = "default_true")]
    pub stats_enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for TokenlessConfig {
    fn default() -> Self {
        Self {
            stats_enabled: true,
        }
    }
}

impl TokenlessConfig {
    fn config_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_default()
            .join(".tokenless/config.json")
    }

    /// Whether a config file exists on disk.
    pub fn config_file_exists() -> bool {
        Self::config_path().exists()
    }

    /// Load config with an explicit env override value and optional custom path.
    /// Priority: env_override > config.json file > default(true)
    pub fn load_with_env_and_path(env_val: Option<&str>, path: Option<&PathBuf>) -> Self {
        if let Some(val) = env_val {
            let enabled =
                val == "1" || val.eq_ignore_ascii_case("true") || val.eq_ignore_ascii_case("yes");
            return Self {
                stats_enabled: enabled,
            };
        }

        // Fall back to file config
        let default_path = Self::config_path();
        let config_path = path.unwrap_or(&default_path);
        std::fs::read_to_string(config_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Load config with an explicit env override value.
    /// Priority: env_override > config.json file > default(true)
    pub fn load_with_env(env_val: Option<&str>) -> Self {
        Self::load_with_env_and_path(env_val, None)
    }

    /// Load config: env var overrides file config, file config overrides defaults.
    /// Priority: TOKENLESS_STATS_ENABLED env > config.json file > default(true)
    pub fn load() -> Self {
        let env_val = std::env::var("TOKENLESS_STATS_ENABLED").ok();
        Self::load_with_env(env_val.as_deref())
    }

    /// Save config to disk.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)
    }

    /// Returns true if stats recording is enabled (env override or file config).
    pub fn is_stats_enabled(&self) -> bool {
        self.stats_enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = TokenlessConfig::default();
        assert!(config.is_stats_enabled());
    }

    #[test]
    fn test_load_missing_file() {
        let tmp_dir = std::env::temp_dir().join("tokenless_test_missing");
        let _ = std::fs::remove_dir_all(&tmp_dir);
        let path = tmp_dir.join("config.json");
        let config = TokenlessConfig::load_with_env_and_path(None, Some(&path));
        assert!(config.is_stats_enabled());
    }

    #[test]
    fn test_load_invalid_json() {
        let tmp_dir = std::env::temp_dir().join("tokenless_test_invalid_json");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let path = tmp_dir.join("config.json");
        let _ = std::fs::write(&path, "not json");
        let config = TokenlessConfig::load_with_env_and_path(None, Some(&path));
        assert!(config.is_stats_enabled());
    }

    #[test]
    fn test_env_override_enabled() {
        let config = TokenlessConfig::load_with_env(Some("1"));
        assert!(config.is_stats_enabled());
    }

    #[test]
    fn test_env_override_disabled() {
        let config = TokenlessConfig::load_with_env(Some("0"));
        assert!(!config.is_stats_enabled());
    }

    #[test]
    fn test_env_override_true_string() {
        let config = TokenlessConfig::load_with_env(Some("true"));
        assert!(config.is_stats_enabled());
    }

    #[test]
    fn test_env_override_overrides_file() {
        let tmp_dir = std::env::temp_dir().join("tokenless_test_override");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let path = tmp_dir.join("config.json");
        // Write file config with stats_enabled=false
        let _ = std::fs::write(&path, "{\"stats_enabled\":false}");
        // Env override to enable
        let config = TokenlessConfig::load_with_env_and_path(Some("1"), Some(&path));
        assert!(config.is_stats_enabled());
    }
}
