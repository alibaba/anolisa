use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use anyhow::Context;

// ==================== Default Constants ====================

/// Default LRU cache capacity for HTTP connections
pub const DEFAULT_CONNECTION_CAPACITY: usize = 24;

/// Default poll timeout for ring buffer polling (milliseconds)
pub const DEFAULT_POLL_TIMEOUT_MS: u64 = 100;

/// Default minimum duration threshold for HTTP requests (microseconds)
pub const DEFAULT_MIN_DUR_US: u64 = 10_000;

/// Default maximum body length for audit analyzer
pub const DEFAULT_MAX_BODY_LEN: usize = 64 * 1024;

/// Default maximum headers for HTTP parser
pub const DEFAULT_MAX_HEADERS: usize = 64;

/// Default database filename (shared for all data types)
pub const DEFAULT_DB_NAME: &str = "agentsight.db";

/// Default audit table name
pub const DEFAULT_AUDIT_TABLE: &str = "audit_events";

/// Default token table name
pub const DEFAULT_TOKEN_TABLE: &str = "token_records";

/// Default HTTP table name
pub const DEFAULT_HTTP_TABLE: &str = "http_records";

/// Default data retention period in days (0 = no limit)
pub const DEFAULT_RETENTION_DAYS: u64 = 30;

/// Default purge check interval (every N inserts)
pub const DEFAULT_PURGE_INTERVAL: u64 = 1000;

pub const HF_ENDPOINT: &str = "https://hf-mirror.com";

/// Get the HF_HOME path, expanding `~` to the user's home directory.
/// 
/// Uses `$HOME` on Unix and `$USERPROFILE` on Windows as fallback.
/// Returns `./.agentsight/tokenizers` if home directory cannot be determined.
pub fn hf_home() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".agentsight/tokenizers")
}

// ==================== Global Verbose State ====================

static VERBOSE: AtomicBool = AtomicBool::new(false);

pub fn set_verbose(v: bool) {
    init_logging(v, None);
}

/// Initialize logging with optional file output.
///
/// * `verbose` — true = debug level, false = warn level (unless `RUST_LOG` is set)
/// * `log_path` — if `Some`, log output is appended to this file; otherwise stderr
pub fn init_logging(verbose: bool, log_path: Option<&str>) {
    VERBOSE.store(verbose, Ordering::SeqCst);

    let level = if verbose {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Warn
    };

    let mut builder = env_logger::Builder::new();

    // Respect RUST_LOG if set, otherwise use verbose-based level.
    if let Ok(rust_log) = std::env::var("RUST_LOG") {
        builder.parse_filters(&rust_log);
    } else {
        builder.filter_level(level);
    }

    // Direct output to file if log_path is provided.
    if let Some(path) = log_path {
        use std::fs::OpenOptions;
        match OpenOptions::new().create(true).append(true).open(path) {
            Ok(file) => {
                builder.target(env_logger::Target::Pipe(Box::new(file)));
            }
            Err(e) => {
                eprintln!("agentsight: failed to open log file {:?}: {}", path, e);
            }
        }
    }

    // init() may fail if called twice; that's fine.
    let _ = builder.try_init();
}

pub fn verbose() -> bool {
    VERBOSE.load(Ordering::SeqCst)
}

// ==================== FFI Rule Configuration ====================

/// Cmdline rule for process matching (allowlist / denylist)
#[derive(Debug, Clone)]
pub struct CmdlineRule {
    /// Glob patterns matched against cmdline args position-by-position
    pub patterns: Vec<String>,
    /// Agent name for allow=1 rules (None for deny rules)
    pub agent_name: Option<String>,
    /// true = allowlist (attach), false = denylist (don't attach)
    pub allow: bool,
}

/// Domain rule for DNS-based SSL attachment filtering
#[derive(Debug, Clone)]
pub struct DomainRule {
    /// Glob pattern for domain matching
    pub pattern: String,
}

// ==================== Agent Discovery Configuration ====================

/// Default agents configuration JSON (embedded in binary).
///
/// Uses the same format as FFI's `agentsight_config_load_config()`:
/// `cmdline.allow` entries with `rule` and `agent_name`.
const DEFAULT_AGENTS_JSON: &str = include_str!("../agentsight.json");


/// Internal JSON structures for parsing the config file (same format as FFI).
#[derive(serde::Deserialize)]
struct JsonFullConfig {
    #[serde(default)]
    verbose: Option<i32>,
    #[serde(default)]
    log_path: Option<String>,
    #[serde(default)]
    cmdline: Option<JsonCmdline>,
    #[serde(default)]
    domain: Option<Vec<JsonDomainGroup>>,
}

#[derive(serde::Deserialize)]
struct JsonCmdline {
    #[serde(default)]
    allow: Option<Vec<JsonCmdlineEntry>>,
    #[serde(default)]
    deny: Option<Vec<JsonCmdlineEntry>>,
}

#[derive(serde::Deserialize)]
struct JsonCmdlineEntry {
    rule: Vec<String>,
    #[serde(default)]
    agent_name: Option<String>,
}

#[derive(serde::Deserialize)]
struct JsonDomainGroup {
    rule: Vec<String>,
}

/// Extract cmdline and domain rules from a parsed JsonFullConfig.
fn extract_rules(parsed: JsonFullConfig) -> (Vec<CmdlineRule>, Vec<DomainRule>) {
    let mut cmdline_rules = Vec::new();
    let mut domain_rules = Vec::new();

    if let Some(cmdline) = parsed.cmdline {
        if let Some(allow_list) = cmdline.allow {
            for entry in allow_list {
                if !entry.rule.is_empty() {
                    cmdline_rules.push(CmdlineRule {
                        patterns: entry.rule,
                        agent_name: entry.agent_name,
                        allow: true,
                    });
                }
            }
        }
        if let Some(deny_list) = cmdline.deny {
            for entry in deny_list {
                if !entry.rule.is_empty() {
                    cmdline_rules.push(CmdlineRule {
                        patterns: entry.rule,
                        agent_name: None,
                        allow: false,
                    });
                }
            }
        }
    }

    if let Some(domain_groups) = parsed.domain {
        for group in domain_groups {
            for pat in group.rule {
                if !pat.is_empty() {
                    domain_rules.push(DomainRule { pattern: pat });
                }
            }
        }
    }

    (cmdline_rules, domain_rules)
}

/// Parse a JSON config string into cmdline rules and domain rules.
///
/// This is the shared parser for both the config file and FFI's `load_config()`.
pub fn parse_json_rules(json: &str) -> Result<(Vec<CmdlineRule>, Vec<DomainRule>), String> {
    let parsed: JsonFullConfig = serde_json::from_str(json)
        .map_err(|e| format!("JSON parse error: {}", e))?;
    Ok(extract_rules(parsed))
}


/// Ensure the agents configuration file exists at the given path.
///
/// If the file does not exist, creates it with the embedded default configuration.
pub fn ensure_default_agents_config(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        return Ok(());
    }
    // Create parent directory if needed
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {:?}", parent))?;
    }
    std::fs::write(path, DEFAULT_AGENTS_JSON)
        .with_context(|| format!("Failed to write default agents config to {:?}", path))?;
    log::info!("Generated default agents config at {:?}", path);
    Ok(())
}

/// Load default cmdline rules (embedded), without touching the filesystem.
pub fn default_cmdline_rules() -> Vec<CmdlineRule> {
    let (rules, _) = parse_json_rules(DEFAULT_AGENTS_JSON)
        .expect("embedded DEFAULT_AGENTS_JSON is valid");
    rules
}

// ==================== Chrome Trace Export ====================

/// Check if chrome trace export is enabled (set once at startup)
pub fn chrome_trace() -> bool {
    static CHROME_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CHROME_TRACE.get_or_init(|| std::env::var("AGENTSIGHT_CHROME_TRACE").is_ok())
}

// ==================== AgentsightConfig ====================

/// Unified configuration for AgentSight
///
/// This struct contains all configuration parameters for the AgentSight system,
/// including storage, probing, parsing, aggregation, and analysis settings.
#[derive(Debug, Clone)]
pub struct AgentsightConfig {
    // --- Storage Configuration ---
    /// Base directory for database files
    pub storage_base_path: PathBuf,
    /// Database filename (shared for all data types)
    pub db_name: String,
    /// Audit table name
    pub audit_table: String,
    /// Token table name
    pub token_table: String,
    /// HTTP table name
    pub http_table: String,

    // --- Retention Configuration ---
    /// Data retention period in days (0 = no limit, records older than this are purged)
    pub retention_days: u64,
    /// Purge check interval (run purge every N inserts, 0 = never auto-purge)
    pub purge_interval: u64,

    // --- Probe Configuration ---
    /// Optional UID filter for process tracing
    pub target_uid: Option<u32>,
    /// Poll timeout for ring buffer polling (milliseconds)
    pub poll_timeout_ms: u64,
    /// Enable file watch probe (monitors .jsonl file opens from traced processes)
    pub enable_filewatch: bool,

    // --- HTTP/Aggregation Configuration ---
    /// LRU cache capacity for HTTP connections
    pub connection_capacity: usize,
    /// Minimum duration threshold for HTTP requests (microseconds)
    pub min_duration_us: u64,

    // --- Parser Configuration ---
    /// Maximum number of HTTP headers to parse
    pub max_headers: usize,

    // --- Analyzer Configuration ---
    /// Maximum body length for audit analysis
    pub max_body_len: usize,

    // --- Logging Configuration ---
    /// Enable verbose logging
    pub verbose: bool,
    /// Log file path (None = stderr)
    pub log_path: Option<String>,

    // --- Tokenizer Configuration ---
    /// Path to tokenizer file for accurate token counting (e.g., "/path/to/tokenizer.json")
    pub tokenizer_path: Option<PathBuf>,
    /// URL to download tokenizer from (e.g., "https://modelscope.cn/.../tokenizer.json")
    pub tokenizer_url: Option<String>,

    // --- FFI Rule Configuration ---
    /// User-defined cmdline rules for process allowlist/denylist
    pub cmdline_rules: Vec<CmdlineRule>,
    /// User-defined domain rules for DNS-based SSL attachment
    pub domain_rules: Vec<DomainRule>,

    // --- Config File Path ---
    /// Path to JSON configuration file
    pub config_path: Option<PathBuf>,
}

impl Default for AgentsightConfig {
    fn default() -> Self {
        Self {
            // Storage defaults
            storage_base_path: default_base_path(),
            db_name: DEFAULT_DB_NAME.to_string(),
            audit_table: DEFAULT_AUDIT_TABLE.to_string(),
            token_table: DEFAULT_TOKEN_TABLE.to_string(),
            http_table: DEFAULT_HTTP_TABLE.to_string(),
            retention_days: DEFAULT_RETENTION_DAYS,
            purge_interval: DEFAULT_PURGE_INTERVAL,

            // Probe defaults
            target_uid: None,
            poll_timeout_ms: DEFAULT_POLL_TIMEOUT_MS,
            enable_filewatch: false,

            // HTTP/Aggregation defaults
            connection_capacity: DEFAULT_CONNECTION_CAPACITY,
            min_duration_us: DEFAULT_MIN_DUR_US,

            // Parser defaults
            max_headers: DEFAULT_MAX_HEADERS,

            // Analyzer defaults
            max_body_len: DEFAULT_MAX_BODY_LEN,

            // Logging defaults
            verbose: false,
            log_path: None,

            // Tokenizer defaults (read from env vars)
            tokenizer_path: std::env::var("AGENTSIGHT_TOKENIZER_PATH").ok().map(PathBuf::from),
            tokenizer_url: Some("https://www.modelscope.cn/models/Qwen/Qwen3.5-27B/resolve/master/tokenizer.json".to_owned()),

            // FFI Rule defaults
            cmdline_rules: Vec::new(),
            domain_rules: Vec::new(),

            // Config file path default
            config_path: None,
        }
    }
}

impl AgentsightConfig {
    /// Create a new configuration with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new configuration with custom storage base path
    pub fn with_storage_path(base_path: PathBuf) -> Self {
        Self {
            storage_base_path: base_path,
            ..Default::default()
        }
    }

    /// Get the full path to the database
    pub fn db_path(&self) -> PathBuf {
        self.storage_base_path.join(&self.db_name)
    }

    /// Get the audit table name
    pub fn audit_table_name(&self) -> &str {
        &self.audit_table
    }

    /// Get the token table name
    pub fn token_table_name(&self) -> &str {
        &self.token_table
    }

    /// Set verbose mode
    pub fn set_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// Set storage base path
    pub fn set_storage_path(mut self, path: PathBuf) -> Self {
        self.storage_base_path = path;
        self
    }

    /// Set target UID
    pub fn set_target_uid(mut self, uid: Option<u32>) -> Self {
        self.target_uid = uid;
        self
    }

    /// Set enable_filewatch
    pub fn set_enable_filewatch(mut self, enable: bool) -> Self {
        self.enable_filewatch = enable;
        self
    }

    /// Set connection capacity
    pub fn set_connection_capacity(mut self, capacity: usize) -> Self {
        self.connection_capacity = capacity;
        self
    }

    /// Apply verbose setting to the global state
    pub fn apply_verbose(&self) {
        init_logging(self.verbose, self.log_path.as_deref());
    }

    /// Load configuration from a JSON string, appending rules to existing ones.
    ///
    /// Parses `verbose`, `log_path`, `cmdline` and `domain` fields.
    pub fn load_from_json(&mut self, json: &str) -> Result<(), String> {
        let mut parsed: JsonFullConfig = serde_json::from_str(json)
            .map_err(|e| format!("JSON parse error: {}", e))?;

        if let Some(v) = parsed.verbose {
            self.verbose = v != 0;
        }
        if let Some(p) = parsed.log_path.take() {
            self.log_path = Some(p);
        }

        let (cmdline_rules, domain_rules) = extract_rules(parsed);
        self.cmdline_rules.extend(cmdline_rules);
        self.domain_rules.extend(domain_rules);
        Ok(())
    }

    /// Set tokenizer path
    pub fn set_tokenizer_path(mut self, path: Option<PathBuf>) -> Self {
        self.tokenizer_path = path;
        self
    }

    /// Set tokenizer URL
    pub fn set_tokenizer_url(mut self, url: Option<String>) -> Self {
        self.tokenizer_url = url;
        self
    }

    /// Add a cmdline rule
    pub fn add_cmdline_rule(mut self, rule: CmdlineRule) -> Self {
        self.cmdline_rules.push(rule);
        self
    }

    /// Add a domain rule
    pub fn add_domain_rule(mut self, rule: DomainRule) -> Self {
        self.domain_rules.push(rule);
        self
    }

    /// Set config file path
    pub fn set_config_path(mut self, path: PathBuf) -> Self {
        self.config_path = Some(path);
        self
    }

    /// Load configuration from a JSON file, appending rules to existing ones.
    ///
    /// Reads the file and delegates to `load_from_json`. All fields supported by
    /// `load_from_json` (verbose, log_path, cmdline, domain) are loaded.
    pub fn load_from_file(&mut self, path: &Path) -> anyhow::Result<()> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config from {:?}", path))?;
        self.load_from_json(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse config from {:?}: {}", path, e))
    }

    /// Resolve the effective config file path.
    ///
    /// # Panics
    /// Panics if `config_path` was not set via `set_config_path` (CLI `--config`).
    pub fn resolve_config_path(&self) -> PathBuf {
        assert!(self.config_path.is_some(), "config_path must be set via --config");
        self.config_path.clone().unwrap()
    }
}

/// Get the default base path for storage
///
/// Returns `$HOME/.agentsight` or `/tmp/.agentsight` if HOME is not set
pub fn default_base_path() -> PathBuf {
    let home = "/var/log/sysak/";
    PathBuf::from(home).join(".agentsight")
}

/// Convert BPF ktime (nanoseconds since boot) to Unix timestamp (nanoseconds since epoch)
///
/// BPF's bpf_ktime_get_ns() returns nanoseconds since system boot.
/// This function converts it to a proper Unix timestamp.
///
/// # How it works
/// 1. Reads system uptime from /proc/uptime
/// 2. Calculates boot_time = current_unix_time - uptime
/// 3. Returns boot_time + ktime
///
/// # Performance
/// Boot time is calculated once and cached, so subsequent calls are O(1).
pub fn ktime_to_unix_ns(ktime_ns: u64) -> u64 {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    static BOOT_TIME_NS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

    let boot_time_ns = *BOOT_TIME_NS.get_or_init(|| {
        let now_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;

        // Read /proc/uptime to get system uptime in seconds
        let uptime_ns = match fs::read_to_string("/proc/uptime") {
            Ok(content) => {
                // Format: "123456.67 456.78" (uptime, idle_time)
                let uptime_secs: f64 = content
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                (uptime_secs * 1_000_000_000.0) as u64
            }
            Err(_) => return 0,
        };

        // boot_time = current_unix_time - uptime
        now_unix.saturating_sub(uptime_ns)
    });

    boot_time_ns.saturating_add(ktime_ns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_constants() {
        assert_eq!(DEFAULT_CONNECTION_CAPACITY, 24);
        assert_eq!(DEFAULT_POLL_TIMEOUT_MS, 100);
        assert_eq!(DEFAULT_MIN_DUR_US, 10_000);
        assert_eq!(DEFAULT_MAX_BODY_LEN, 64 * 1024);
        assert_eq!(DEFAULT_MAX_HEADERS, 64);
        assert_eq!(DEFAULT_DB_NAME, "agentsight.db");
        assert_eq!(DEFAULT_AUDIT_TABLE, "audit_events");
        assert_eq!(DEFAULT_TOKEN_TABLE, "token_records");
        assert_eq!(DEFAULT_HTTP_TABLE, "http_records");
        assert_eq!(DEFAULT_RETENTION_DAYS, 30);
        assert_eq!(DEFAULT_PURGE_INTERVAL, 1000);
    }

    #[test]
    fn test_hf_home() {
        let path = hf_home();
        assert!(path.to_str().unwrap().contains(".agentsight/tokenizers"));
    }

    #[test]
    fn test_default_base_path() {
        let path = default_base_path();
        assert_eq!(path, PathBuf::from("/var/log/sysak/.agentsight"));
    }

    #[test]
    fn test_ktime_to_unix_ns_nonzero() {
        // ktime_to_unix_ns should return a value > ktime_ns (boot time offset)
        let result = ktime_to_unix_ns(1_000_000);
        assert!(result >= 1_000_000);
    }

    #[test]
    fn test_ktime_to_unix_ns_zero() {
        let result = ktime_to_unix_ns(0);
        // Should return the boot time itself
        assert!(result > 0);
    }

    #[test]
    fn test_config_new_defaults() {
        let config = AgentsightConfig::new();
        assert_eq!(config.db_name, "agentsight.db");
        assert_eq!(config.connection_capacity, 24);
        assert_eq!(config.poll_timeout_ms, 100);
        assert_eq!(config.min_duration_us, 10_000);
        assert_eq!(config.max_headers, 64);
        assert_eq!(config.max_body_len, 64 * 1024);
        assert!(!config.verbose);
        assert!(config.log_path.is_none());
        assert!(config.target_uid.is_none());
        assert!(!config.enable_filewatch);
        assert_eq!(config.retention_days, 30);
        assert_eq!(config.purge_interval, 1000);
    }

    #[test]
    fn test_config_with_storage_path() {
        let config = AgentsightConfig::with_storage_path(PathBuf::from("/tmp/test"));
        assert_eq!(config.storage_base_path, PathBuf::from("/tmp/test"));
        assert_eq!(config.db_name, "agentsight.db");
    }

    #[test]
    fn test_config_db_path() {
        let config = AgentsightConfig::with_storage_path(PathBuf::from("/tmp/mydata"));
        assert_eq!(config.db_path(), PathBuf::from("/tmp/mydata/agentsight.db"));
    }

    #[test]
    fn test_config_table_names() {
        let config = AgentsightConfig::new();
        assert_eq!(config.audit_table_name(), "audit_events");
        assert_eq!(config.token_table_name(), "token_records");
    }

    #[test]
    fn test_config_builder_methods() {
        let config = AgentsightConfig::new()
            .set_verbose(true)
            .set_storage_path(PathBuf::from("/custom"))
            .set_target_uid(Some(1000))
            .set_enable_filewatch(true)
            .set_connection_capacity(48);
        assert!(config.verbose);
        assert_eq!(config.storage_base_path, PathBuf::from("/custom"));
        assert_eq!(config.target_uid, Some(1000));
        assert!(config.enable_filewatch);
        assert_eq!(config.connection_capacity, 48);
    }

    #[test]
    fn test_set_tokenizer_path() {
        let config = AgentsightConfig::new()
            .set_tokenizer_path(Some(PathBuf::from("/path/to/tokenizer.json")));
        assert_eq!(config.tokenizer_path, Some(PathBuf::from("/path/to/tokenizer.json")));
    }

    #[test]
    fn test_set_tokenizer_url() {
        let config = AgentsightConfig::new()
            .set_tokenizer_url(Some("https://example.com/tok.json".into()));
        assert_eq!(config.tokenizer_url, Some("https://example.com/tok.json".to_string()));
    }

    #[test]
    fn test_verbose_default_false() {
        // verbose() reads from global static; default should be false
        // Note: other tests might have set it, so just check it doesn't panic
        let _ = verbose();
    }

    #[test]
    fn test_add_cmdline_rule() {
        let rule = CmdlineRule {
            patterns: vec!["node".to_string(), "*claude*".to_string()],
            agent_name: Some("Claude Code".to_string()),
            allow: true,
        };
        let config = AgentsightConfig::new().add_cmdline_rule(rule);
        assert_eq!(config.cmdline_rules.len(), 1);
        assert_eq!(config.cmdline_rules[0].patterns, vec!["node", "*claude*"]);
        assert_eq!(config.cmdline_rules[0].agent_name, Some("Claude Code".to_string()));
        assert!(config.cmdline_rules[0].allow);
    }

    #[test]
    fn test_add_cmdline_rule_deny() {
        let rule = CmdlineRule {
            patterns: vec!["node".to_string(), "*webpack*".to_string()],
            agent_name: None,
            allow: false,
        };
        let config = AgentsightConfig::new().add_cmdline_rule(rule);
        assert_eq!(config.cmdline_rules.len(), 1);
        assert!(!config.cmdline_rules[0].allow);
        assert!(config.cmdline_rules[0].agent_name.is_none());
    }

    #[test]
    fn test_add_domain_rule() {
        let rule = DomainRule { pattern: "*.openai.com".to_string() };
        let config = AgentsightConfig::new().add_domain_rule(rule);
        assert_eq!(config.domain_rules.len(), 1);
        assert_eq!(config.domain_rules[0].pattern, "*.openai.com");
    }

    #[test]
    fn test_add_multiple_rules() {
        let config = AgentsightConfig::new()
            .add_cmdline_rule(CmdlineRule {
                patterns: vec!["node".to_string()],
                agent_name: Some("Agent1".to_string()),
                allow: true,
            })
            .add_cmdline_rule(CmdlineRule {
                patterns: vec!["python3".to_string()],
                agent_name: Some("Agent2".to_string()),
                allow: true,
            })
            .add_domain_rule(DomainRule { pattern: "*.openai.com".to_string() })
            .add_domain_rule(DomainRule { pattern: "*.anthropic.com".to_string() });
        assert_eq!(config.cmdline_rules.len(), 2);
        assert_eq!(config.domain_rules.len(), 2);
    }

    #[test]
    fn test_default_cmdline_rules() {
        let rules = default_cmdline_rules();
        assert!(!rules.is_empty());
        // All should be allow rules
        assert!(rules.iter().all(|r| r.allow));
        // Should contain Hermes, Cosh, OpenClaw agent names
        let names: Vec<&str> = rules.iter()
            .filter_map(|r| r.agent_name.as_deref())
            .collect();
        assert!(names.contains(&"Hermes"));
        assert!(names.contains(&"Cosh"));
        assert!(names.contains(&"OpenClaw"));
    }

    #[test]
    fn test_default_agents_json_valid() {
        // Verify the embedded JSON is valid and parses correctly
        let (cmdline_rules, domain_rules) = parse_json_rules(DEFAULT_AGENTS_JSON).unwrap();
        assert!(!cmdline_rules.is_empty());
        assert!(domain_rules.is_empty()); // no domain rules in default config
    }

    #[test]
    fn test_parse_json_rules_cmdline() {
        let json = r#"{
            "cmdline": {
                "allow": [{"rule": ["node", "*claude*"], "agent_name": "Claude Code"}],
                "deny": [{"rule": ["node", "*webpack*"]}]
            }
        }"#;
        let (cmdline_rules, domain_rules) = parse_json_rules(json).unwrap();
        assert_eq!(cmdline_rules.len(), 2);
        assert!(cmdline_rules[0].allow);
        assert_eq!(cmdline_rules[0].agent_name, Some("Claude Code".to_string()));
        assert!(!cmdline_rules[1].allow);
        assert!(cmdline_rules[1].agent_name.is_none());
        assert!(domain_rules.is_empty());
    }

    #[test]
    fn test_parse_json_rules_domain() {
        let json = r#"{"domain": [{"rule": ["*.openai.com", "*.anthropic.com"]}]}"#;
        let (cmdline_rules, domain_rules) = parse_json_rules(json).unwrap();
        assert!(cmdline_rules.is_empty());
        assert_eq!(domain_rules.len(), 2);
    }

    #[test]
    fn test_parse_json_rules_invalid() {
        let json = r#"{ invalid json }"#;
        assert!(parse_json_rules(json).is_err());
    }

    #[test]
    fn test_parse_json_rules_empty_rule_skipped() {
        let json = r#"{"cmdline":{"allow":[{"rule":[],"agent_name":"Skipped"},{"rule":["node"],"agent_name":"Kept"}]}}"#;
        let (cmdline_rules, _) = parse_json_rules(json).unwrap();
        assert_eq!(cmdline_rules.len(), 1);
        assert_eq!(cmdline_rules[0].agent_name, Some("Kept".to_string()));
    }
}
