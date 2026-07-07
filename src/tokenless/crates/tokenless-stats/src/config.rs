//! Configuration for tokenless.
//!
//! Stored at `~/.tokenless/config.json`. Controls global feature flags.
//! Environment variables `TOKENLESS_STATS_ENABLED` and
//! `TOKENLESS_SLS_ENABLED` override file config independently.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Global tokenless configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenlessConfig {
    /// Whether to record compression stats (default: true)
    #[serde(default = "default_true")]
    pub stats_enabled: bool,
    /// Whether SLS integration is enabled (default: true). When enabled,
    /// each compression is also appended as a JSONL record for SLS ingestion.
    #[serde(default = "default_true")]
    pub sls_enabled: bool,
    /// Whether compression is actually applied (default: true).
    /// When false, tokenless runs in dry-run mode: it computes and records
    /// the predicted savings but emits the original (uncompressed) text,
    /// enabling A/B comparison of the same task with/without compression.
    #[serde(default = "default_true")]
    pub compression_enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for TokenlessConfig {
    fn default() -> Self {
        Self {
            stats_enabled: true,
            sls_enabled: true,
            compression_enabled: true,
        }
    }
}

/// Parse a boolean env value: "1", "true", "yes" (case-insensitive) → true.
/// All other non-empty values — including "0", "false", "no" — return false.
/// (Empty strings are filtered to `None` by callers before reaching this
/// function, so they never reach here.)
fn parse_env_bool(val: &str) -> bool {
    val == "1" || val.eq_ignore_ascii_case("true") || val.eq_ignore_ascii_case("yes")
}

impl TokenlessConfig {
    fn config_path() -> PathBuf {
        // Resolve home via the shared passwd-rooted helper so an attacker
        // cannot redirect the config path by setting $HOME before invoking
        // any tokenless binary. When no trusted home is available, return
        // a path under /dev/null so the open/create call fails loudly
        // (ENOENT or ENOTDIR) rather than silently landing in the CWD
        // (which PathBuf::from("").join(...) would produce).
        let home = crate::home::get_home_dir();
        if home.is_empty() {
            return PathBuf::from("/dev/null/.tokenless/config.json");
        }
        PathBuf::from(home).join(".tokenless/config.json")
    }

    /// Whether a config file exists on disk.
    pub fn config_file_exists() -> bool {
        Self::config_path().exists()
    }

    /// Load config with explicit env overrides for all toggles and optional custom path.
    /// Priority (per toggle): env > config.json file > default
    /// Empty env var values are normalized to None (treated as unset).
    /// When stats and sls envs are both set, skips the config file read entirely
    /// (compression still defaults to true unless its own env is set).
    pub fn load_with_envs_and_path(
        stats_env: Option<&str>,
        sls_env: Option<&str>,
        compression_env: Option<&str>,
        path: Option<&PathBuf>,
    ) -> Self {
        // Normalize empty strings to None — an empty env var means "unset".
        let stats_env = stats_env.filter(|v| !v.is_empty());
        let sls_env = sls_env.filter(|v| !v.is_empty());
        let compression_env = compression_env.filter(|v| !v.is_empty());

        // When both stats and sls env vars are set, skip the file read entirely.
        // This avoids unnecessary I/O when the config file is on a slow
        // or unavailable filesystem (e.g. broken NFS mount).
        if let (Some(stats_val), Some(sls_val)) = (stats_env, sls_env) {
            return Self {
                stats_enabled: parse_env_bool(stats_val),
                sls_enabled: parse_env_bool(sls_val),
                compression_enabled: compression_env.map(parse_env_bool).unwrap_or(true),
            };
        }

        let default_path = Self::config_path();
        let config_path = path.unwrap_or(&default_path);
        let base = std::fs::read_to_string(config_path)
            .ok()
            .and_then(|s| serde_json::from_str::<TokenlessConfig>(&s).ok())
            .unwrap_or_default();

        let stats_enabled = if let Some(val) = stats_env {
            parse_env_bool(val)
        } else {
            base.stats_enabled
        };

        let sls_enabled = if let Some(val) = sls_env {
            parse_env_bool(val)
        } else {
            base.sls_enabled
        };

        let compression_enabled = if let Some(val) = compression_env {
            parse_env_bool(val)
        } else {
            base.compression_enabled
        };

        Self {
            stats_enabled,
            sls_enabled,
            compression_enabled,
        }
    }

    /// Load config with explicit env overrides for stats and sls toggles.
    pub fn load_with_envs(stats_env: Option<&str>, sls_env: Option<&str>) -> Self {
        Self::load_with_envs_and_path(stats_env, sls_env, None, None)
    }

    /// Load config with an explicit env override value and optional custom path.
    /// Backward-compatible wrapper: only overrides stats_enabled.
    pub fn load_with_env_and_path(env_val: Option<&str>, path: Option<&PathBuf>) -> Self {
        Self::load_with_envs_and_path(env_val, None, None, path)
    }

    /// Load config with an explicit env override value.
    /// Backward-compatible wrapper: only overrides stats_enabled.
    pub fn load_with_env(env_val: Option<&str>) -> Self {
        Self::load_with_envs(env_val, None)
    }

    /// Load config: env vars override file config, file config overrides defaults.
    /// Priority: env > config.json file > default (per toggle)
    /// Empty env var values are treated as unset (fall through to file config).
    pub fn load() -> Self {
        let stats_env = std::env::var("TOKENLESS_STATS_ENABLED")
            .ok()
            .filter(|v| !v.is_empty());
        let sls_env = std::env::var("TOKENLESS_SLS_ENABLED")
            .ok()
            .filter(|v| !v.is_empty());
        let compression_env = std::env::var("TOKENLESS_COMPRESSION_ENABLED")
            .ok()
            .filter(|v| !v.is_empty());
        Self::load_with_envs_and_path(
            stats_env.as_deref(),
            sls_env.as_deref(),
            compression_env.as_deref(),
            None,
        )
    }

    /// Save config to disk.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)?;
        // Restrict to owner-only — the config may contain per-user
        // settings that should not be readable by other local users.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).ok();
        }
        Ok(())
    }

    /// Returns true if stats recording is enabled (env override or file config).
    pub fn is_stats_enabled(&self) -> bool {
        self.stats_enabled
    }

    /// Returns true if SLS integration is enabled (env override or file config).
    pub fn is_sls_enabled(&self) -> bool {
        self.sls_enabled
    }

    /// Returns true if compression is applied (env override or file config).
    /// When false, tokenless runs in dry-run mode.
    pub fn is_compression_enabled(&self) -> bool {
        self.compression_enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("tests/config_tests.rs");
}
