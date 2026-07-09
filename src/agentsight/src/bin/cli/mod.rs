//! CLI subcommand modules for agentsight binary
//!
//! This module provides subcommand implementations:
//! - `token`: Query token consumption data
//! - `trace`: Trace agent activity via eBPF
//! - `audit`: Query audit events
//! - `discover`: Discover running AI agents
//! - `interruption`: Query and manage session interruption events

pub mod audit;
#[cfg(feature = "server")]
pub mod dashboard;
pub mod discover;
pub mod interruption;
pub mod metrics;
#[cfg(feature = "server")]
pub mod serve;
pub mod skill_metrics;
pub mod summary;
pub mod token;
pub mod trace;

/// Default configuration file path (shared by trace / serve / dashboard).
#[cfg(feature = "server")]
pub const DEFAULT_CONFIG_PATH: &str = "/etc/agentsight/config.json";

/// Load `ServerAuthConfig` from the agentsight config file.
///
/// Falls back to defaults if the file cannot be read or parsed.
#[cfg(feature = "server")]
pub fn load_server_auth_config(config_path: &str) -> agentsight::config::ServerAuthConfig {
    use agentsight::config::{AgentsightConfig, ensure_default_agents_config};

    let path = std::path::Path::new(config_path);
    let mut config = AgentsightConfig::new();

    // Ensure the config file exists (generate default if missing)
    if let Err(e) = ensure_default_agents_config(path) {
        log::warn!("Failed to ensure default config at {config_path:?}: {e}, using defaults");
        return config.server_auth;
    }

    if let Err(e) = config.load_from_file(path) {
        log::warn!("Failed to load config from {config_path:?}: {e}, using defaults");
    }

    config.server_auth
}

/// Parse period string into TimePeriod
pub fn parse_period(s: &str) -> agentsight::TimePeriod {
    match s {
        "today" => agentsight::TimePeriod::Today,
        "yesterday" => agentsight::TimePeriod::Yesterday,
        "week" => agentsight::TimePeriod::Week,
        "last_week" => agentsight::TimePeriod::LastWeek,
        "month" => agentsight::TimePeriod::Month,
        "last_month" => agentsight::TimePeriod::LastMonth,
        _ => agentsight::TimePeriod::Today,
    }
}

/// Calculate nanosecond timestamp for N hours ago
pub fn hours_ago_ns(hours: u64) -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    now.saturating_sub(hours * 3600 * 1_000_000_000)
}
