use anyhow::Context;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};

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

/// Initialize or reconfigure logging with optional file output.
///
/// Safe to call on every `AgentSight::new` in the same process: the output
/// destination and filter are updated in place.
///
/// * `verbose` — true = debug level, false = warn level (unless `RUST_LOG` is set)
/// * `log_path` — if `Some`, log output is appended to this file; otherwise stderr
pub fn init_logging(verbose: bool, log_path: Option<&str>) {
    VERBOSE.store(verbose, Ordering::SeqCst);
    crate::logging::init(verbose, log_path);
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

/// HTTPS rule for DNS-based SSL attachment filtering
#[derive(Debug, Clone)]
pub struct HttpsRule {
    /// Glob pattern for domain matching
    pub pattern: String,
}

/// HTTP target entry — can be an IP/port endpoint or a domain name.
/// Code auto-detects: entries parseable as TcpTarget are treated as endpoints;
/// everything else is treated as a domain (resolved via DNS at startup + runtime).
#[derive(Debug, Clone)]
pub enum HttpTarget {
    Endpoint(TcpTarget),
    Domain(String),
}

// ==================== Agent Discovery Configuration ====================

/// Default agents configuration JSON (embedded in binary).
///
/// Uses the same format as FFI's `agentsight_config_load_config()`:
/// `cmdline.allow` entries with `rule` and `agent_name`.
const DEFAULT_AGENTS_JSON: &str = include_str!("../agentsight.json");

// ==================== TCP Target Configuration ====================

/// A single TCP traffic capture target.
///
/// Filters captured plain-HTTP traffic by destination IP and/or port.
/// `ip = None` means any destination IP; `port = None` means any port.
///
/// String format (used in JSON config and CLI):
///   `":8080"`          → port-only (any IP, port 8080)
///   `"*:8080"`         → port-only (alias of `:8080`)
///   `"10.0.0.1"`       → IP-only   (IP 10.0.0.1, any port)
///   `"10.0.0.1:*"`     → IP-only   (alias of `10.0.0.1`)
///   `"10.0.0.1:8080"`  → exact     (IP 10.0.0.1, port 8080)
///   `"*"` / `"*:*"` / `":*"` → full wildcard (any IP, any port — captures **all** TCP traffic)
#[derive(Debug, Clone, PartialEq)]
pub struct TcpTarget {
    pub ip: Option<Ipv4Addr>,
    pub port: Option<u16>,
}

impl FromStr for TcpTarget {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty TcpTarget string".to_string());
        }

        // Full wildcard shortcuts: "*", "*:*", ":*"
        if s == "*" || s == "*:*" || s == ":*" {
            return Ok(TcpTarget {
                ip: None,
                port: None,
            });
        }

        // Helper: parse `"*"` as wildcard, otherwise as IPv4.
        let parse_ip = |t: &str| -> Result<Option<Ipv4Addr>, String> {
            if t == "*" {
                Ok(None)
            } else {
                t.parse::<Ipv4Addr>()
                    .map(Some)
                    .map_err(|_| format!("invalid IP address '{t}'"))
            }
        };
        // Helper: parse `"*"` as wildcard, otherwise as u16 port.
        let parse_port = |t: &str| -> Result<Option<u16>, String> {
            if t == "*" {
                Ok(None)
            } else {
                t.parse::<u16>()
                    .map(Some)
                    .map_err(|_| format!("invalid port '{t}'"))
            }
        };

        if s.starts_with(':') {
            // ":port" — port-only
            let port = parse_port(s.strip_prefix(':').unwrap())?;
            Ok(TcpTarget { ip: None, port })
        } else if s.contains(':') {
            // "ip:port" (either side may be `*`)
            let (ip_str, port_str) = s.rsplit_once(':').unwrap();

            let ip = parse_ip(ip_str)?;
            let port = parse_port(port_str)?;
            Ok(TcpTarget { ip, port })
        } else {
            // "ip" — IP-only (no `*` here — already handled above)
            let ip = parse_ip(s)?;
            Ok(TcpTarget { ip, port: None })
        }
    }
}

/// Internal JSON structures for parsing the config file (same format as FFI).
#[derive(serde::Deserialize)]
struct JsonFullConfig {
    #[serde(default, rename = "traceEnabled")]
    trace_enabled: Option<bool>,
    #[serde(default)]
    verbose: Option<i32>,
    #[serde(default)]
    log_path: Option<String>,
    #[serde(default)]
    cmdline: Option<JsonCmdline>,
    #[serde(default)]
    https: Option<Vec<JsonDomainGroup>>,
    #[serde(default)]
    http: Option<Vec<JsonHttpGroup>>,
    #[serde(default)]
    encryption: Option<JsonEncryption>,
    #[serde(default)]
    runtime: Option<JsonRuntime>,
    #[serde(default)]
    deadloop: Option<JsonDeadloop>,
    #[serde(default)]
    cgroup_filter_enabled: Option<bool>,
    #[serde(default)]
    cgroup_ids: Option<Vec<u64>>,
}

/// DeadLoop 检测配置区段
#[derive(serde::Deserialize, Clone, Debug)]
struct JsonDeadloop {
    /// 是否启用自动 kill（默认 false，仅记录日志）
    #[serde(default)]
    enabled: Option<bool>,
    /// 触发自动 kill 的循环检测次数阈值（默认 3）
    #[serde(default)]
    kill_after_count: Option<usize>,
}

/// Runtime 动态配置区段（支持热加载，无需重启）
#[derive(serde::Deserialize, Clone, Debug)]
pub struct JsonRuntime {
    /// SLS Logtail 输出文件路径。非空时激活 SLS 上传。
    #[serde(default)]
    pub sls_logtail_path: Option<String>,
}

/// 加密配置：可选公钥（PEM 字符串）或公钥文件路径
#[derive(serde::Deserialize)]
struct JsonEncryption {
    #[serde(default)]
    public_key: Option<String>,
    #[serde(default)]
    public_key_path: Option<String>,
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

#[derive(serde::Deserialize)]
struct JsonHttpGroup {
    rule: Vec<String>,
}

/// Extract cmdline, https, and http rules from a parsed JsonFullConfig.
fn extract_rules(parsed: &JsonFullConfig) -> (Vec<CmdlineRule>, Vec<HttpsRule>, Vec<HttpTarget>) {
    let mut cmdline_rules = Vec::new();
    let mut https_rules = Vec::new();
    let mut http_targets = Vec::new();

    if let Some(ref cmdline) = parsed.cmdline {
        if let Some(ref allow_list) = cmdline.allow {
            for entry in allow_list {
                if !entry.rule.is_empty() {
                    cmdline_rules.push(CmdlineRule {
                        patterns: entry.rule.clone(),
                        agent_name: entry.agent_name.clone(),
                        allow: true,
                    });
                }
            }
        }
        if let Some(ref deny_list) = cmdline.deny {
            for entry in deny_list {
                if !entry.rule.is_empty() {
                    cmdline_rules.push(CmdlineRule {
                        patterns: entry.rule.clone(),
                        agent_name: None,
                        allow: false,
                    });
                }
            }
        }
    }

    if let Some(ref https_groups) = parsed.https {
        for group in https_groups {
            for pat in &group.rule {
                if !pat.is_empty() {
                    https_rules.push(HttpsRule {
                        pattern: pat.clone(),
                    });
                }
            }
        }
    }

    if let Some(ref http_groups) = parsed.http {
        for group in http_groups {
            for entry in &group.rule {
                if entry.is_empty() {
                    continue;
                }
                match entry.parse::<TcpTarget>() {
                    Ok(t) => http_targets.push(HttpTarget::Endpoint(t)),
                    Err(_) => http_targets.push(HttpTarget::Domain(entry.clone())),
                }
            }
        }
    }

    (cmdline_rules, https_rules, http_targets)
}

/// Parse a JSON config string into cmdline rules, https rules, and http targets.
///
/// This is the shared parser for both the config file and FFI's `load_config()`.
pub fn parse_json_rules(
    json: &str,
) -> Result<(Vec<CmdlineRule>, Vec<HttpsRule>, Vec<HttpTarget>), String> {
    let parsed: JsonFullConfig =
        serde_json::from_str(json).map_err(|e| format!("JSON parse error: {e}"))?;
    Ok(extract_rules(&parsed))
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
            .with_context(|| format!("Failed to create directory {parent:?}"))?;
    }
    std::fs::write(path, DEFAULT_AGENTS_JSON)
        .with_context(|| format!("Failed to write default agents config to {path:?}"))?;
    log::info!("Generated default agents config at {path:?}");
    Ok(())
}

/// Load default cmdline rules (embedded), without touching the filesystem.
pub fn default_cmdline_rules() -> Vec<CmdlineRule> {
    let (rules, _, _) =
        parse_json_rules(DEFAULT_AGENTS_JSON).expect("embedded DEFAULT_AGENTS_JSON is valid");
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

    // --- Trace Control ---
    /// Controls whether SLS-uploaded `LLMCall` records carry conversation
    /// content fields (`gen_ai.input.messages`, `gen_ai.output.messages`,
    /// `gen_ai.system_instructions`).
    ///
    /// * `false` (default): only token / model / provider / timing metadata
    ///   is uploaded; conversation bodies are dropped at the SLS layer.
    /// * `true`: full conversation content is uploaded.
    ///
    /// This flag does **not** stop the agent itself; eBPF probes, local
    /// SQLite persistence and token metering keep running regardless.
    /// Local SQLite always stores full content; this only controls SLS.
    ///
    /// JSON field name: `traceEnabled`.
    pub trace_enabled: bool,

    // --- Probe Configuration ---
    /// Optional UID filter for process tracing
    pub target_uid: Option<u32>,
    /// Poll timeout for ring buffer polling (milliseconds)
    pub poll_timeout_ms: u64,
    /// Enable file watch probe (monitors .jsonl file opens from traced processes)
    pub enable_filewatch: bool,
    /// Enable cgroup-level event filtering. When true, proctrace /
    /// filewatch / filewrite only emit events from cgroup ids
    /// registered via `Probes::add_traced_cgroup`. procmon is unaffected
    /// (full audit coverage). Default: false (no cgroup filtering).
    pub cgroup_filter_enabled: bool,
    /// Cgroup inode IDs seeded into `cgroup_filter` at startup.
    /// Only effective when `cgroup_filter_enabled` is true.
    /// Obtain via `stat -c %i /sys/fs/cgroup/<path>` (v2) or
    /// `stat -c %i /sys/fs/cgroup/memory/<path>` (v1).
    pub cgroup_ids: Vec<u64>,
    /// TCP capture targets for plain HTTP capture (empty = disabled).
    /// Each entry specifies destination IP, port, or both.
    pub tcp_targets: Vec<TcpTarget>,

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
    /// User-defined HTTPS rules for DNS-based SSL attachment
    pub https_rules: Vec<HttpsRule>,
    /// User-defined HTTP targets (IP/port endpoints + domains for tcpsniff)
    pub http_targets: Vec<HttpTarget>,

    // --- Config File Path ---
    /// Path to JSON configuration file
    pub config_path: Option<PathBuf>,

    // --- Encryption Configuration ---
    /// RSA 公钥（PEM 字符串）。从 agentsight.json `encryption.public_key`
    /// 或 `encryption.public_key_path` 加载。若为 None，则不加密敏感消息字段。
    pub encryption_public_key: Option<String>,

    // --- Runtime Dynamic Configuration ---
    /// SLS Logtail 输出文件路径（来自 `runtime.sls_logtail_path`）。
    /// 非空时激活 SLS 上传。支持运行期热加载。
    pub sls_logtail_path: Option<String>,

    // --- DeadLoop Auto-Kill Configuration ---
    /// 是否启用 DeadLoop 自动 kill 止血（默认 false）
    pub deadloop_kill_enabled: bool,
    /// 触发 kill 的循环次数阈值（检测到 N 次后 kill）
    pub deadloop_kill_after_count: usize,
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

            // Trace control defaults
            // Default = false (privacy-safe): SLS uploads carry only token /
            // model / timing metadata. Conversation content is opt-in via
            // explicit `"traceEnabled": true` in the config file.
            trace_enabled: false,

            // Probe defaults
            target_uid: None,
            poll_timeout_ms: DEFAULT_POLL_TIMEOUT_MS,
            enable_filewatch: false,
            cgroup_filter_enabled: false,
            cgroup_ids: Vec::new(),
            tcp_targets: Vec::new(),

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
            tokenizer_path: std::env::var("AGENTSIGHT_TOKENIZER_PATH")
                .ok()
                .map(PathBuf::from),
            tokenizer_url: Some(
                "https://www.modelscope.cn/models/Qwen/Qwen3.5-27B/resolve/master/tokenizer.json"
                    .to_owned(),
            ),

            // FFI Rule defaults
            cmdline_rules: Vec::new(),
            https_rules: Vec::new(),
            http_targets: Vec::new(),

            // Config file path default
            config_path: None,

            // Encryption defaults (loaded from config file)
            encryption_public_key: None,

            // Runtime dynamic configuration defaults
            sls_logtail_path: None,

            // DeadLoop auto-kill defaults (disabled by default)
            deadloop_kill_enabled: false,
            deadloop_kill_after_count: 3,
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

    /// Enable or disable cgroup-level event filtering.
    ///
    /// When enabled, only events from cgroup ids registered via
    /// `Probes::add_traced_cgroup` are emitted by the filtered probes.
    /// procmon keeps full audit coverage regardless. Must be set before
    /// probes are created (the value is baked into the BPF rodata at load).
    pub fn set_cgroup_filter_enabled(mut self, enabled: bool) -> Self {
        self.cgroup_filter_enabled = enabled;
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
    /// Parses `verbose`, `log_path`, `cmdline`, `https` and `http` fields.
    pub fn load_from_json(&mut self, json: &str) -> Result<(), String> {
        let mut parsed: JsonFullConfig =
            serde_json::from_str(json).map_err(|e| format!("JSON parse error: {e}"))?;

        if let Some(t) = parsed.trace_enabled {
            self.trace_enabled = t;
        }
        if let Some(v) = parsed.verbose {
            self.verbose = v != 0;
        }
        if let Some(p) = parsed.log_path.take() {
            self.log_path = Some(p);
        }

        // 加载加密公钥：优先 public_key（内联 PEM），其次 public_key_path（文件路径）
        if let Some(enc) = parsed.encryption.take() {
            if let Some(pem) = enc.public_key {
                let trimmed = pem.trim();
                if !trimmed.is_empty() {
                    self.encryption_public_key = Some(trimmed.to_string());
                }
            } else if let Some(path) = enc.public_key_path {
                let trimmed = path.trim();
                if !trimmed.is_empty() {
                    match std::fs::read_to_string(trimmed) {
                        Ok(content) => {
                            self.encryption_public_key = Some(content);
                        }
                        Err(e) => {
                            log::warn!(
                                "Failed to read encryption public_key_path {trimmed:?}: {e}, encryption disabled"
                            );
                        }
                    }
                }
            }
        }

        // 解析 runtime 动态配置
        if let Some(ref rt) = parsed.runtime {
            if let Some(ref path) = rt.sls_logtail_path {
                let trimmed = path.trim();
                if !trimmed.is_empty() {
                    self.sls_logtail_path = Some(trimmed.to_string());
                }
            }
        }

        // 解析 deadloop 自动 kill 配置
        if let Some(ref dl) = parsed.deadloop {
            if let Some(enabled) = dl.enabled {
                self.deadloop_kill_enabled = enabled;
            }
            if let Some(count) = dl.kill_after_count {
                self.deadloop_kill_after_count = count;
            }
        }

        // Parse cgroup filter settings
        if let Some(v) = parsed.cgroup_filter_enabled {
            self.cgroup_filter_enabled = v;
        }
        if let Some(ids) = parsed.cgroup_ids.take() {
            self.cgroup_ids = ids;
        }

        let (cmdline_rules, https_rules, http_targets) = extract_rules(&parsed);
        self.cmdline_rules.extend(cmdline_rules);
        self.https_rules.extend(https_rules);
        self.http_targets.extend(http_targets);
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

    /// Add an HTTPS rule (domain glob pattern for SSL attachment)
    pub fn add_https_rule(mut self, rule: HttpsRule) -> Self {
        self.https_rules.push(rule);
        self
    }

    /// Add an HTTP target (IP/port endpoint or domain for tcpsniff)
    pub fn add_http_target(mut self, target: HttpTarget) -> Self {
        self.http_targets.push(target);
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
            .with_context(|| format!("Failed to read config from {path:?}"))?;
        self.load_from_json(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse config from {path:?}: {e}"))
    }

    /// Resolve the effective config file path.
    ///
    /// # Panics
    /// Panics if `config_path` was not set via `set_config_path` (CLI `--config`).
    pub fn resolve_config_path(&self) -> PathBuf {
        assert!(
            self.config_path.is_some(),
            "config_path must be set via --config"
        );
        self.config_path.clone().unwrap()
    }
}

/// Parse `runtime.sls_logtail_path` from a JSON config string (tri-state).
///
/// Returns:
/// * `None`               — field is absent or JSON parse failed (no signal).
/// * `Some(None)`         — field is present but empty / whitespace-only
///                          → **deactivation** signal (pause SLS uploads).
/// * `Some(Some(path))`   — field is present and non-empty (after trim)
///                          → **activation / re-activation** signal.
///
/// Used by the config watcher to react to runtime hot-reload changes
/// (delete `/etc/anolisa/enable_token_collector` → empty string → pause;
/// re-create the trigger with a non-empty `SLS_LOG_PATH` → resume).
pub fn parse_runtime_sls_path(json: &str) -> Option<Option<String>> {
    #[derive(serde::Deserialize)]
    struct Partial {
        #[serde(default)]
        runtime: Option<JsonRuntime>,
    }
    let parsed: Partial = serde_json::from_str(json).ok()?;
    let rt = parsed.runtime?;
    let path = rt.sls_logtail_path?;
    let trimmed = path.trim();
    if trimmed.is_empty() {
        Some(None)
    } else {
        Some(Some(trimmed.to_string()))
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
    fn test_tcp_target_parse_exact() {
        let t: TcpTarget = "10.0.0.1:8080".parse().unwrap();
        assert_eq!(t.ip, Some("10.0.0.1".parse().unwrap()));
        assert_eq!(t.port, Some(8080));
    }

    #[test]
    fn test_tcp_target_parse_port_only() {
        let t: TcpTarget = ":8080".parse().unwrap();
        assert_eq!(t.ip, None);
        assert_eq!(t.port, Some(8080));

        // "*:8080" is an alias of ":8080"
        let t2: TcpTarget = "*:8080".parse().unwrap();
        assert_eq!(t2, t);
    }

    #[test]
    fn test_tcp_target_parse_ip_only() {
        let t: TcpTarget = "10.0.0.1".parse().unwrap();
        assert_eq!(t.ip, Some("10.0.0.1".parse().unwrap()));
        assert_eq!(t.port, None);

        // "10.0.0.1:*" is an alias of "10.0.0.1"
        let t2: TcpTarget = "10.0.0.1:*".parse().unwrap();
        assert_eq!(t2, t);
    }

    #[test]
    fn test_tcp_target_parse_full_wildcard() {
        for s in ["*", "*:*", ":*"] {
            let t: TcpTarget = s.parse().unwrap();
            assert_eq!(t.ip, None, "{s}");
            assert_eq!(t.port, None, "{s}");
        }
    }

    #[test]
    fn test_tcp_target_parse_invalid() {
        assert!("".parse::<TcpTarget>().is_err());
        assert!("not-an-ip".parse::<TcpTarget>().is_err());
        assert!("10.0.0.1:bad".parse::<TcpTarget>().is_err());
        assert!("bad:8080".parse::<TcpTarget>().is_err());
    }

    #[test]
    fn test_tcp_target_parse_via_http_targets() {
        let json = r#"{"http": [{"rule": ["*", "*:8080", "10.0.0.1:*", "10.0.0.1:9090", "some.host.com"]}]}"#;
        let (_, _, http_targets) = parse_json_rules(json).unwrap();
        assert_eq!(http_targets.len(), 5);
        // 0: full wildcard endpoint
        match &http_targets[0] {
            HttpTarget::Endpoint(t) => {
                assert_eq!(t.ip, None);
                assert_eq!(t.port, None);
            }
            _ => panic!("expected Endpoint"),
        }
        // 4: domain (unparseable as TcpTarget)
        matches!(http_targets[4], HttpTarget::Domain(_));
    }

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
        assert!(!config.cgroup_filter_enabled);
        assert_eq!(config.retention_days, 30);
        assert_eq!(config.purge_interval, 1000);
    }

    /// `traceEnabled` is **off** by default (privacy-safe). Conversation
    /// content fields are dropped from SLS uploads unless the config file
    /// explicitly sets `"traceEnabled": true`. This test locks that default
    /// so it cannot silently regress.
    #[test]
    fn test_trace_enabled_default_is_false() {
        let config = AgentsightConfig::new();
        assert!(
            !config.trace_enabled,
            "AgentsightConfig::new() must default trace_enabled=false to keep \
             SLS uploads free of conversation content unless explicitly opted in"
        );
    }

    /// Loading a config that omits `traceEnabled` must NOT flip the default
    /// (the field is `Option<bool>` and only overrides when `Some`).
    #[test]
    fn test_load_from_json_missing_trace_enabled_keeps_default_false() {
        let mut config = AgentsightConfig::new();
        config.load_from_json("{}").unwrap();
        assert!(!config.trace_enabled);
    }

    /// Explicit `"traceEnabled": true` opts into uploading conversation content.
    #[test]
    fn test_load_from_json_explicit_trace_enabled_true() {
        let mut config = AgentsightConfig::new();
        config.load_from_json(r#"{"traceEnabled": true}"#).unwrap();
        assert!(config.trace_enabled);
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
        assert_eq!(
            config.tokenizer_path,
            Some(PathBuf::from("/path/to/tokenizer.json"))
        );
    }

    #[test]
    fn test_set_tokenizer_url() {
        let config =
            AgentsightConfig::new().set_tokenizer_url(Some("https://example.com/tok.json".into()));
        assert_eq!(
            config.tokenizer_url,
            Some("https://example.com/tok.json".to_string())
        );
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
        assert_eq!(
            config.cmdline_rules[0].agent_name,
            Some("Claude Code".to_string())
        );
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
    fn test_add_https_rule() {
        let rule = HttpsRule {
            pattern: "*.openai.com".to_string(),
        };
        let config = AgentsightConfig::new().add_https_rule(rule);
        assert_eq!(config.https_rules.len(), 1);
        assert_eq!(config.https_rules[0].pattern, "*.openai.com");
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
            .add_https_rule(HttpsRule {
                pattern: "*.openai.com".to_string(),
            })
            .add_https_rule(HttpsRule {
                pattern: "*.anthropic.com".to_string(),
            });
        assert_eq!(config.cmdline_rules.len(), 2);
        assert_eq!(config.https_rules.len(), 2);
    }

    #[test]
    fn test_default_cmdline_rules() {
        let rules = default_cmdline_rules();
        assert!(!rules.is_empty());
        // All should be allow rules
        assert!(rules.iter().all(|r| r.allow));
        // Should contain Hermes, Cosh, OpenClaw agent names
        let names: Vec<&str> = rules
            .iter()
            .filter_map(|r| r.agent_name.as_deref())
            .collect();
        assert!(names.contains(&"Hermes"));
        assert!(names.contains(&"Cosh"));
        assert!(names.contains(&"OpenClaw"));
    }

    #[test]
    fn test_default_agents_json_valid() {
        let (cmdline_rules, https_rules, http_targets) =
            parse_json_rules(DEFAULT_AGENTS_JSON).unwrap();
        assert!(!cmdline_rules.is_empty());
        // https rules: dashscope.aliyuncs.com configured by default
        assert_eq!(https_rules.len(), 1);
        assert!(http_targets.is_empty());
    }

    #[test]
    fn test_parse_json_rules_cmdline() {
        let json = r#"{
            "cmdline": {
                "allow": [{"rule": ["node", "*claude*"], "agent_name": "Claude Code"}],
                "deny": [{"rule": ["node", "*webpack*"]}]
            }
        }"#;
        let (cmdline_rules, https_rules, http_targets) = parse_json_rules(json).unwrap();
        assert_eq!(cmdline_rules.len(), 2);
        assert!(cmdline_rules[0].allow);
        assert_eq!(cmdline_rules[0].agent_name, Some("Claude Code".to_string()));
        assert!(!cmdline_rules[1].allow);
        assert!(cmdline_rules[1].agent_name.is_none());
        assert!(https_rules.is_empty());
        assert!(http_targets.is_empty());
    }

    #[test]
    fn test_parse_json_rules_https() {
        let json = r#"{"https": [{"rule": ["*.openai.com", "*.anthropic.com"]}]}"#;
        let (cmdline_rules, https_rules, http_targets) = parse_json_rules(json).unwrap();
        assert!(cmdline_rules.is_empty());
        assert_eq!(https_rules.len(), 2);
        assert_eq!(https_rules[0].pattern, "*.openai.com");
        assert_eq!(https_rules[1].pattern, "*.anthropic.com");
        assert!(http_targets.is_empty());
    }

    #[test]
    fn test_parse_json_rules_invalid() {
        let json = r#"{ invalid json }"#;
        assert!(parse_json_rules(json).is_err());
    }

    #[test]
    fn test_parse_json_rules_empty_rule_skipped() {
        let json = r#"{"cmdline":{"allow":[{"rule":[],"agent_name":"Skipped"},{"rule":["node"],"agent_name":"Kept"}]}}"#;
        let (cmdline_rules, _, _) = parse_json_rules(json).unwrap();
        assert_eq!(cmdline_rules.len(), 1);
        assert_eq!(cmdline_rules[0].agent_name, Some("Kept".to_string()));
    }

    #[test]
    fn test_parse_runtime_sls_path_present() {
        let json = r#"{"runtime": {"sls_logtail_path": "/var/log/sls/agentsight.log"}}"#;
        assert_eq!(
            parse_runtime_sls_path(json),
            Some(Some("/var/log/sls/agentsight.log".to_string()))
        );
    }

    #[test]
    fn test_parse_runtime_sls_path_empty() {
        let json = r#"{"runtime": {"sls_logtail_path": ""}}"#;
        // Empty string is now a deactivation signal (Some(None)), not absence (None).
        assert_eq!(parse_runtime_sls_path(json), Some(None));
    }

    #[test]
    fn test_parse_runtime_sls_path_whitespace_only() {
        let json = r#"{"runtime": {"sls_logtail_path": "   "}}"#;
        // Whitespace-only is treated as empty → deactivation signal.
        assert_eq!(parse_runtime_sls_path(json), Some(None));
    }

    #[test]
    fn test_parse_runtime_sls_path_missing_section() {
        let json = r#"{"cmdline": {"allow": []}}"#;
        // Field absent → no signal at all.
        assert_eq!(parse_runtime_sls_path(json), None);
    }

    #[test]
    fn test_parse_runtime_sls_path_invalid_json() {
        assert_eq!(parse_runtime_sls_path("not json"), None);
    }

    #[test]
    fn test_load_from_json_runtime_sls_path() {
        let json = r#"{"runtime": {"sls_logtail_path": "/tmp/sls.log"}}"#;
        let mut config = AgentsightConfig::new();
        config.load_from_json(json).unwrap();
        assert_eq!(config.sls_logtail_path, Some("/tmp/sls.log".to_string()));
    }

    #[test]
    fn test_load_from_json_runtime_sls_path_empty_is_none() {
        let json = r#"{"runtime": {"sls_logtail_path": ""}}"#;
        let mut config = AgentsightConfig::new();
        config.load_from_json(json).unwrap();
        assert_eq!(config.sls_logtail_path, None);
    }

    // ─── DeadLoop config tests ───────────────────────────────────────────────

    #[test]
    fn test_deadloop_config_defaults() {
        let config = AgentsightConfig::new();
        assert!(!config.deadloop_kill_enabled);
        assert_eq!(config.deadloop_kill_after_count, 3);
    }

    #[test]
    fn test_load_from_json_deadloop_enabled() {
        let json = r#"{"deadloop": {"enabled": true, "kill_after_count": 5}}"#;
        let mut config = AgentsightConfig::new();
        config.load_from_json(json).unwrap();
        assert!(config.deadloop_kill_enabled);
        assert_eq!(config.deadloop_kill_after_count, 5);
    }

    #[test]
    fn test_load_from_json_deadloop_disabled_explicit() {
        let json = r#"{"deadloop": {"enabled": false}}"#;
        let mut config = AgentsightConfig::new();
        config.load_from_json(json).unwrap();
        assert!(!config.deadloop_kill_enabled);
        assert_eq!(config.deadloop_kill_after_count, 3); // keeps default
    }

    #[test]
    fn test_load_from_json_deadloop_partial_only_count() {
        let json = r#"{"deadloop": {"kill_after_count": 10}}"#;
        let mut config = AgentsightConfig::new();
        config.load_from_json(json).unwrap();
        assert!(!config.deadloop_kill_enabled); // keeps default
        assert_eq!(config.deadloop_kill_after_count, 10);
    }

    #[test]
    fn test_load_from_json_no_deadloop_section() {
        let json = r#"{"cmdline": {"allow": []}}"#;
        let mut config = AgentsightConfig::new();
        config.load_from_json(json).unwrap();
        assert!(!config.deadloop_kill_enabled);
        assert_eq!(config.deadloop_kill_after_count, 3);
    }
}
