//! Tokenless CLI - LLM token optimization via schema and response compression.
mod env_check;
mod init;
mod mcp;

use clap::{Parser, Subcommand, ValueEnum};
use std::fs;
use std::io::{self, IsTerminal as _, Read};
use std::process;
use std::sync::Arc;
use tokenless_ccr::{SqliteStore, StashStore, extract_hash, is_valid_hash};
use tokenless_schema::{ResponseCompressor, SchemaCompressor};
use tokenless_stats::{
    CompressionMode, DiffSort, OperationType, StatsRecord, StatsRecorder, TokenlessConfig,
};
use tokenless_stats::{estimate_tokens, estimate_tokens_from_bytes};
use tokenless_stats::{
    format_compare, format_compare_json, format_diff_report, format_list, format_show,
    format_summary, record_report, session_report, tool_use_report,
};

#[derive(Parser)]
#[command(
    name = "tokenless",
    version,
    about = "LLM token optimization via schema and response compression"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Compress OpenAI Function Calling tool schemas
    CompressSchema {
        #[arg(short, long)]
        file: Option<String>,
        /// Compress a JSON array of schemas
        #[arg(long)]
        batch: bool,
        /// Agent ID for stats (e.g. "copilot-shell")
        #[arg(long)]
        agent_id: Option<String>,
        /// Session ID for grouping
        #[arg(long)]
        session_id: Option<String>,
        /// Tool use ID
        #[arg(long)]
        tool_use_id: Option<String>,
        /// Disable reversible stash. By default, truncated descriptions are
        /// stashed so they can be retrieved via `tokenless retrieve`; this
        /// flag makes truncation lossy (the pre-stash behavior).
        #[arg(long)]
        no_stash: bool,
        /// Override the stash database path. Defaults to
        /// $TOKENLESS_DATA_DIR/stash.db or ~/.tokenless/stash.db.
        /// Resolved under the trusted home directory; rejected if outside.
        #[arg(long)]
        stash_db: Option<String>,
    },
    /// Compress API responses
    CompressResponse {
        #[arg(short, long)]
        file: Option<String>,
        /// Agent ID for stats
        #[arg(long)]
        agent_id: Option<String>,
        /// Session ID for grouping
        #[arg(long)]
        session_id: Option<String>,
        /// Tool use ID
        #[arg(long)]
        tool_use_id: Option<String>,
        /// Max string length before truncation
        #[arg(long)]
        truncate_strings_at: Option<usize>,
        /// Max array length before truncation
        #[arg(long)]
        truncate_arrays_at: Option<usize>,
        /// Max nesting depth before truncation
        #[arg(long)]
        max_depth: Option<usize>,
        /// Disable reversible stash. By default, dropped array items are
        /// stashed so they can be retrieved via `tokenless retrieve`; this
        /// flag makes truncation lossy (the pre-stash behavior).
        #[arg(long)]
        no_stash: bool,
        /// Override the stash database path. Defaults to
        /// $TOKENLESS_DATA_DIR/stash.db or ~/.tokenless/stash.db.
        /// Resolved under the trusted home directory; rejected if outside.
        #[arg(long)]
        stash_db: Option<String>,
    },
    /// Retrieve a stashed payload by its hash key. Accepts a bare 24-hex hash
    /// or any text containing a `<<tokenless:HASH>>` marker (the marker is
    /// extracted automatically).
    Retrieve {
        /// The stash hash, or a line containing a `<<tokenless:HASH>>` marker.
        hash: String,
        /// Override the stash database path. Defaults to
        /// $TOKENLESS_DATA_DIR/stash.db or ~/.tokenless/stash.db.
        #[arg(long)]
        stash_db: Option<String>,
    },
    /// View and export statistics
    #[command(subcommand)]
    Stats(StatsCommands),
    /// Encode JSON to TOON format
    CompressToon {
        #[arg(short, long)]
        file: Option<String>,
        /// Agent ID for stats
        #[arg(long)]
        agent_id: Option<String>,
        /// Session ID for grouping
        #[arg(long)]
        session_id: Option<String>,
        /// Tool use ID
        #[arg(long)]
        tool_use_id: Option<String>,
    },
    /// Decode TOON format back to JSON
    DecompressToon {
        #[arg(short, long)]
        file: Option<String>,
    },
    /// Check tool environment readiness
    EnvCheck {
        /// Check a specific tool
        #[arg(long)]
        tool: Option<String>,
        /// Check all tools
        #[arg(long)]
        all: bool,
        /// Auto-fix missing dependencies
        #[arg(long)]
        fix: bool,
        /// Output full checklist
        #[arg(long)]
        checklist: bool,
        /// Output machine-readable JSON (for hook/plugin consumption)
        #[arg(long)]
        json: bool,
    },
    /// Detect installed agent frameworks and install tokenless adapters
    Init {
        /// Install a specific framework by name (e.g. claude-code, qoder)
        #[arg(long)]
        framework: Option<String>,
        /// Install all installable frameworks without prompting
        #[arg(long)]
        all: bool,
        /// List framework status without installing
        #[arg(long)]
        list: bool,
    },
    /// Start the tokenless MCP stdio server (exposes `tokenless_retrieve` so
    /// an MCP-connected agent can recover stashed payloads on demand).
    #[command(subcommand)]
    Mcp(McpCommands),
}

#[derive(Subcommand)]
enum McpCommands {
    /// Start the MCP stdio server.
    Serve,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DiffSortArg {
    Saved,
    Time,
}

const DEFAULT_DIFF_LIMIT: usize = 20;

impl From<DiffSortArg> for DiffSort {
    fn from(value: DiffSortArg) -> Self {
        match value {
            DiffSortArg::Saved => DiffSort::Saved,
            DiffSortArg::Time => DiffSort::Time,
        }
    }
}

#[derive(Subcommand)]
enum StatsCommands {
    /// Show summary statistics with breakdown by operation
    Summary {
        #[arg(long)]
        limit: Option<usize>,
        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
        /// Compare two runs by session: baseline (compression-off) vs tokenless (compression-on).
        /// Provide exactly two session IDs.
        #[arg(long, num_args = 2, value_names = ["BASELINE_SESSION", "TOKENLESS_SESSION"])]
        compare: Option<Vec<String>>,
    },
    /// List recent records
    List {
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Show before/after text content for a specific record
    Show {
        /// Record database ID
        id: i64,
    },
    /// Explain estimated token savings with a unified content diff
    Diff {
        /// Record database ID
        #[arg(
            value_name = "RECORD_ID",
            required_unless_present = "session",
            conflicts_with = "session"
        )]
        id: Option<i64>,
        /// Session ID to summarize or inspect
        #[arg(long, required_unless_present = "id")]
        session: Option<String>,
        /// Restrict a session diff to one tool call
        #[arg(long, requires = "session")]
        tool_use_id: Option<String>,
        /// Maximum number of chains in a session overview
        #[arg(
            short,
            long,
            default_value_t = DEFAULT_DIFF_LIMIT,
            value_parser = parse_positive_usize,
            conflicts_with_all = ["id", "tool_use_id"]
        )]
        limit: usize,
        /// Session overview ordering
        #[arg(long, value_enum, conflicts_with_all = ["id", "tool_use_id"])]
        sort: Option<DiffSortArg>,
        /// Unchanged lines around each content change
        #[arg(short = 'U', long, default_value_t = 3)]
        context: usize,
        /// Disable ANSI colors even when stdout is a terminal
        #[arg(long)]
        no_color: bool,
        /// Output machine-readable JSON with structured diff hunks
        #[arg(long)]
        json: bool,
    },
    /// Clear all statistics
    Clear {
        #[arg(long)]
        yes: bool,
    },
    /// Show stats recording status
    Status,
    /// Enable stats recording
    Enable,
    /// Disable stats recording
    Disable,
}

/// Maximum input size (64 MiB) to prevent OOM on accidental large-file stdin.
const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;

fn parse_positive_usize(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("invalid positive integer: {value}"))?;
    if parsed == 0 {
        return Err("value must be greater than zero".to_string());
    }
    Ok(parsed)
}

fn read_input(file: &Option<String>) -> Result<String, String> {
    // Cap stream reads at MAX_INPUT_BYTES + 1 via Read::take so a hostile
    // input cannot allocate gigabytes before the size check fires. The
    // post-read length comparison catches the truncated-at-limit case so
    // we still reject (rather than silently process a partial buffer).
    let limit = MAX_INPUT_BYTES as u64 + 1;
    let too_large = || {
        format!(
            "Input exceeds {} MiB limit",
            MAX_INPUT_BYTES / (1024 * 1024)
        )
    };
    match file {
        Some(path) => {
            let mut content = String::new();
            fs::File::open(path)
                .map_err(|e| format!("Failed to open file '{}': {}", path, e))?
                .take(limit)
                .read_to_string(&mut content)
                .map_err(|e| format!("Failed to read file '{}': {}", path, e))?;
            if content.len() > MAX_INPUT_BYTES {
                return Err(too_large());
            }
            Ok(content)
        }
        None => {
            use std::io::IsTerminal as _;
            if io::stdin().is_terminal() {
                return Err("No input provided. Use --file <path> or pipe via stdin: echo '{...}' | tokenless <command>".to_string());
            }
            let mut buf = String::new();
            io::stdin()
                .lock()
                .take(limit)
                .read_to_string(&mut buf)
                .map_err(|e| format!("Failed to read stdin: {}", e))?;
            if buf.len() > MAX_INPUT_BYTES {
                return Err(too_large());
            }
            if buf.trim().is_empty() {
                return Err("No input received on stdin".to_string());
            }
            Ok(buf)
        }
    }
}

/// Resolve the current user's home directory.
///
/// Re-exports `tokenless_stats::get_home_dir` so both the CLI binary and
/// shared stats/config code agree on a single passwd-rooted source of
/// truth (see `tokenless_stats::home`).
pub fn get_home_dir() -> String {
    tokenless_stats::get_home_dir()
}

/// Validate a custom tokenless data directory against the user's home.
///
/// Unlike database file overrides, the directory may not exist yet. The
/// nearest existing ancestor is canonicalized so a symlink cannot redirect
/// the eventual directory outside the trusted home. Validation and later
/// directory creation are separate filesystem operations, so callers must
/// not treat this check as protection from concurrent path replacement.
fn validate_data_dir_path(env_path: &str, home: &str) -> Result<String, String> {
    if home.is_empty() {
        return Err("no trusted home directory available".to_string());
    }

    let canonical_home = std::path::Path::new(home)
        .canonicalize()
        .map_err(|e| format!("home directory '{}' cannot be resolved: {}", home, e))?;
    let candidate = std::path::Path::new(env_path);
    if !candidate.is_absolute() {
        return Err(format!("path '{}' is not absolute", env_path));
    }
    if candidate
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(format!("path '{}' contains parent traversal", env_path));
    }
    if candidate.exists() && !candidate.is_dir() {
        return Err(format!("path '{}' is not a directory", env_path));
    }

    let mut existing_ancestor = candidate;
    while !existing_ancestor.exists() {
        existing_ancestor = existing_ancestor
            .parent()
            .ok_or_else(|| format!("path '{}' has no existing ancestor to validate", env_path))?;
    }
    let resolved_ancestor = existing_ancestor
        .canonicalize()
        .map_err(|e| format!("path '{}' cannot be resolved: {}", env_path, e))?;
    if !resolved_ancestor.starts_with(&canonical_home) {
        return Err(format!(
            "path '{}' is outside home directory '{}'",
            env_path, home
        ));
    }

    Ok(env_path.to_string())
}

/// Resolve the directory containing tokenless SQLite databases.
///
/// `TOKENLESS_DATA_DIR` affects `stats.db` and `stash.db` only. Invalid
/// overrides are ignored in favor of the existing `~/.tokenless` default.
/// Returns an error when no trusted home anchor exists.
fn get_data_dir(home: &str) -> Result<String, String> {
    if home.is_empty() {
        return Err("no trusted home directory available".to_string());
    }

    let data_dir = match std::env::var("TOKENLESS_DATA_DIR") {
        Ok(env_path) if !env_path.is_empty() => match validate_data_dir_path(&env_path, home) {
            Ok(path) => path,
            Err(reason) => {
                eprintln!("[tokenless] ignoring TOKENLESS_DATA_DIR: {}", reason);
                format!("{}/.tokenless", home)
            }
        },
        _ => format!("{}/.tokenless", home),
    };
    Ok(data_dir)
}

// Lazily snapshot path inputs for one command so stats and stash share the
// same passwd lookup and data-directory validation without global caching.
#[derive(Default)]
struct DatabasePathResolver {
    home: std::sync::OnceLock<String>,
    data_dir: std::sync::OnceLock<Result<String, String>>,
}

impl DatabasePathResolver {
    fn home(&self) -> &str {
        self.home.get_or_init(get_home_dir)
    }

    fn data_dir(&self) -> Result<&str, &str> {
        self.data_dir
            .get_or_init(|| get_data_dir(self.home()))
            .as_ref()
            .map(String::as_str)
            .map_err(String::as_str)
    }
}

/// Resolve the database path. When `TOKENLESS_STATS_DB` is set, the path
/// is validated to ensure it resides under the user's home directory;
/// otherwise `TOKENLESS_DATA_DIR` and then the default data directory are
/// used. This prevents an attacker from redirecting the database to a
/// system-critical location (e.g. `/etc/evil.db`).
fn get_db_path_with(paths: &DatabasePathResolver) -> String {
    let home = paths.home();
    // When no trusted home is available (empty string from passwd lookup
    // failure), return a path that will safely fail on open/create rather
    // than silently writing to / or CWD.
    if home.is_empty() {
        eprintln!("[tokenless] no home directory available — stats DB writes disabled");
        return "/dev/null/.tokenless/stats.db".to_string();
    }
    match std::env::var("TOKENLESS_STATS_DB") {
        Ok(env_path) if !env_path.is_empty() => match validate_db_path(&env_path, home) {
            Ok(path) => return path,
            Err(reason) => eprintln!("[tokenless] ignoring TOKENLESS_STATS_DB: {}", reason),
        },
        _ => {}
    }
    let data_dir = match paths.data_dir() {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("[tokenless] {} — stats DB writes disabled", reason);
            return "/dev/null/.tokenless/stats.db".to_string();
        }
    };
    std::path::Path::new(&data_dir)
        .join("stats.db")
        .to_string_lossy()
        .into_owned()
}

/// Validate a TOKENLESS_STATS_DB candidate against the user's home directory.
/// Returns the original path on success, or a human-readable rejection reason.
///
/// Extracted from `get_db_path` so unit tests can exercise the bypass paths
/// (ParentDir traversal, nonexistent parents, missing home anchor) without
/// mutating process-wide env vars.
fn validate_db_path(env_path: &str, home: &str) -> Result<String, String> {
    // Reject when we have no trusted home anchor:
    // Path::starts_with("") returns true for every path, which would
    // let an attacker point the database at any system location.
    if home.is_empty() {
        return Err("no trusted home directory available".to_string());
    }
    // Canonicalize the home anchor as well as the candidate path. Passwd
    // entries can name a directory that traverses a symlink (e.g. macOS
    // /Users/u where /Users is a symlink to /home, or distros that put
    // /home/u behind /export/home/u). If we compare a canonicalized
    // env_path against a raw home, the prefix check rejects legitimate
    // paths AND, conversely, a `home == "/"` slip-through (rejected at
    // the passwd layer in tokenless-stats::home but defended in depth
    // here) would match every absolute path under `starts_with`.
    let canonical_home = std::path::Path::new(home)
        .canonicalize()
        .map_err(|e| format!("home directory '{}' cannot be resolved: {}", home, e))?;
    let p = std::path::Path::new(env_path);
    // Accept only paths under the user's real home directory.
    // For not-yet-created DB files, the parent directory MUST itself
    // canonicalize — falling back to an unresolved parent would let
    // `~/x/../../etc/evil.db` slip past the starts_with(&home) check,
    // since Path::starts_with matches components literally and an
    // unresolved path still begins with the home prefix.
    let resolved = p
        .canonicalize()
        .or_else(|_| {
            p.parent()
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
                .and_then(|parent| parent.canonicalize())
        })
        .map_err(|e| format!("path '{}' cannot be resolved: {}", env_path, e))?;
    if resolved.starts_with(&canonical_home) {
        Ok(env_path.to_string())
    } else {
        Err(format!(
            "path '{}' is outside home directory '{}'",
            env_path, home
        ))
    }
}

fn ensure_db_dir(db_path: &str) -> Result<(), (String, i32)> {
    if let Some(parent) = std::path::Path::new(&db_path).parent() {
        fs::create_dir_all(parent)
            .map_err(|e| (format!("Failed to create database directory: {}", e), 1))?;
    }
    Ok(())
}

fn open_recorder() -> Result<StatsRecorder, (String, i32)> {
    open_recorder_with(&DatabasePathResolver::default())
}

fn open_recorder_with(paths: &DatabasePathResolver) -> Result<StatsRecorder, (String, i32)> {
    let db_path = get_db_path_with(paths);
    ensure_db_dir(&db_path)?;
    StatsRecorder::new(db_path).map_err(|e| (format!("Failed to open database: {}", e), 1))
}

/// Resolve the stash database path under the trusted home directory.
///
/// Mirrors `get_db_path`'s trust model (passwd-rooted home, validated env
/// override) so an attacker cannot redirect the stash to a system-critical
/// location by setting `TOKENLESS_STASH_DB` or passing `--stash-db`. Returns
/// `None` when no trusted home anchor exists or an override is rejected —
/// callers fail open (no stash, lossy truncation) rather than writing state
/// to an untrusted location.
fn get_stash_db_path_with(
    paths: &DatabasePathResolver,
    override_path: Option<&str>,
) -> Option<String> {
    let home = paths.home();
    if home.is_empty() {
        eprintln!("[tokenless] no home directory available — stash disabled");
        return None;
    }
    // An explicit --stash-db override is validated the same way as the
    // TOKENLESS_STASH_DB env var: on rejection we warn AND fall back to the
    // default under the trusted home (rather than silently disabling the
    // stash), so a typo doesn't quietly drop reversibility.
    if let Some(p) = override_path.filter(|s| !s.is_empty()) {
        match validate_db_path(p, home) {
            Ok(valid) => return Some(valid),
            Err(reason) => eprintln!("[tokenless] rejecting --stash-db {}: {}", p, reason),
        }
    }
    if let Ok(env_path) = std::env::var("TOKENLESS_STASH_DB")
        && !env_path.is_empty()
    {
        match validate_db_path(&env_path, home) {
            Ok(path) => return Some(path),
            Err(reason) => eprintln!("[tokenless] ignoring TOKENLESS_STASH_DB: {}", reason),
        }
    }
    let data_dir = match paths.data_dir() {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("[tokenless] {} — stash disabled", reason);
            return None;
        }
    };
    Some(
        std::path::Path::new(&data_dir)
            .join("stash.db")
            .to_string_lossy()
            .into_owned(),
    )
}

/// Open a stash store, returning the specific failure cause. Used by
/// user-initiated paths (`retrieve`) where a generic "unavailable" message
/// would hide the real reason (no home, path rejected, corrupt DB, …).
fn open_stash_store_or_err(override_path: Option<&str>) -> Result<Arc<dyn StashStore>, String> {
    open_stash_store_or_err_with(&DatabasePathResolver::default(), override_path)
}

fn open_stash_store_or_err_with(
    paths: &DatabasePathResolver,
    override_path: Option<&str>,
) -> Result<Arc<dyn StashStore>, String> {
    let path = get_stash_db_path_with(paths, override_path)
        .ok_or_else(|| "no trusted home directory available for stash db".to_string())?;
    if let Some(parent) = std::path::Path::new(&path).parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create stash directory {}: {}", parent.display(), e))?;
    }
    SqliteStore::new(&path)
        .map_err(|e| format!("cannot open stash db at {}: {}", path, e))
        .map(|s| Arc::new(s) as Arc<dyn StashStore>)
}

/// Open a stash store, failing open to `None` on any error. Compression
/// proceeds without stash (lossy truncation) when the home anchor is missing,
/// the parent directory cannot be created, or the database cannot be opened.
fn open_stash_store(override_path: Option<&str>) -> Option<Arc<dyn StashStore>> {
    open_stash_store_with(&DatabasePathResolver::default(), override_path)
}

fn open_stash_store_with(
    paths: &DatabasePathResolver,
    override_path: Option<&str>,
) -> Option<Arc<dyn StashStore>> {
    match open_stash_store_or_err_with(paths, override_path) {
        Ok(store) => Some(store),
        Err(e) => {
            eprintln!("[tokenless] stash disabled: {}", e);
            None
        }
    }
}

fn run() -> Result<(), (String, i32)> {
    let cli = Cli::parse();
    run_command(cli.command)
}

fn run_command(command: Commands) -> Result<(), (String, i32)> {
    match command {
        Commands::CompressSchema {
            file,
            batch,
            agent_id,
            session_id,
            tool_use_id,
            no_stash,
            stash_db,
        } => {
            let input = read_input(&file).map_err(|e| (e, 2))?;
            let value: serde_json::Value = serde_json::from_str(&input)
                .map_err(|e| (format!("JSON parse error: {}", e), 2))?;

            // Load config before deciding on the stash so we can skip it
            // entirely when compression is disabled (dry-run). Attaching the
            // stash in dry-run would write entries whose `<<tokenless:KEY>>`
            // markers never reach the LLM (the original input is emitted),
            // orphaning them.
            let config = TokenlessConfig::load();
            let database_paths = DatabasePathResolver::default();
            let compression_on = config.is_compression_enabled();
            let stash = if no_stash || !compression_on {
                None
            } else {
                open_stash_store_with(&database_paths, stash_db.as_deref())
            };
            let mut compressor = SchemaCompressor::new();
            if let Some(ref store) = stash {
                compressor = compressor.with_stash_store(store.clone());
            }

            let after_compact = if batch || value.is_array() {
                let arr = value
                    .as_array()
                    .ok_or_else(|| ("Expected a JSON array for --batch mode".to_string(), 1))?;
                let results: Vec<serde_json::Value> =
                    arr.iter().map(|item| compressor.compress(item)).collect();
                serde_json::to_string(&results).unwrap_or_default()
            } else {
                let result = compressor.compress(&value);
                serde_json::to_string(&result).unwrap_or_default()
            };

            let before_tokens = estimate_tokens(&input);
            let after_tokens = estimate_tokens(&after_compact);
            let output_text = if after_tokens >= before_tokens {
                eprintln!(
                    "tokenless: schema compression did not reduce size ({} -> {} est. tokens), outputting original",
                    before_tokens, after_tokens
                );
                // No-savings discard edge: if a stash was attached and a
                // description was truncated, those writes orphan (markers
                // live in `after_compact`, which is discarded). Truncation
                // almost always yields savings, so this is rare; orphaned
                // entries are TTL-cleaned.
                input.clone()
            } else {
                after_compact.clone()
            };

            let mode = resolve_mode(compression_on, before_tokens, after_tokens);
            let emit_text = if compression_on {
                output_text.clone()
            } else {
                input.clone()
            };
            println!("{}", emit_text);

            record_compression_stats(
                &config,
                &database_paths,
                OperationType::CompressSchema,
                agent_id,
                session_id,
                tool_use_id,
                input,
                output_text,
                mode,
                None,
                None,
                None,
            );
        }
        Commands::CompressResponse {
            file,
            agent_id,
            session_id,
            tool_use_id,
            truncate_strings_at,
            truncate_arrays_at,
            max_depth,
            no_stash,
            stash_db,
        } => {
            let input = read_input(&file).map_err(|e| (e, 2))?;
            let value: serde_json::Value = serde_json::from_str(&input)
                .map_err(|e| (format!("JSON parse error: {}", e), 2))?;

            let mut compressor = ResponseCompressor::new();
            if let Some(v) = truncate_strings_at {
                compressor = compressor.with_truncate_strings_at(v);
            }
            if let Some(v) = truncate_arrays_at {
                compressor = compressor.with_truncate_arrays_at(v);
            }
            if let Some(v) = max_depth {
                compressor = compressor.with_max_depth(v);
            }
            // Load config before deciding on the stash so we can skip it
            // entirely when compression is disabled (dry-run). Attaching the
            // stash in dry-run would write entries whose `<<tokenless:KEY>>`
            // markers never reach the LLM (the original input is emitted),
            // orphaning them.
            let config = TokenlessConfig::load();
            let database_paths = DatabasePathResolver::default();
            let compression_on = config.is_compression_enabled();
            let stash = if no_stash || !compression_on {
                None
            } else {
                open_stash_store_with(&database_paths, stash_db.as_deref())
            };
            if let Some(ref store) = stash {
                compressor = compressor.with_stash_store(store.clone());
            }
            let result = compressor.compress(&value);
            // Stash observability: capture write/error counts + live entry
            // count for stats. All three are None when no stash store is
            // attached (vs Some(0) when a stash is attached but nothing was
            // truncated) so stats queries can distinguish "no stash" runs
            // from "stash, zero writes" runs. Counts are read AFTER compress
            // so they reflect this call; stash_size reflects entries added.
            let stash_writes = stash.as_ref().map(|_| compressor.stash_writes());
            let stash_errors = stash.as_ref().map(|_| compressor.stash_errors());
            let stash_size = stash.as_ref().map(|s| s.len());
            // Surface persistent stash backend failures (disk full, locked DB,
            // I/O) so they aren't invisible — compression degrades to the
            // lossy marker per entry, but a non-zero count means the stash
            // path is broken and retrievals will miss.
            if matches!(stash_errors, Some(e) if e > 0) {
                eprintln!(
                    "[tokenless] stash: {} write(s) failed during compression; truncated entries are not retrievable (check stash db health)",
                    stash_errors.expect("checked Some above")
                );
            }
            let after_compact = serde_json::to_string(&result).unwrap_or_else(|_| String::new());

            let before_tokens = estimate_tokens(&input);
            let after_tokens = estimate_tokens(&after_compact);
            let output_text = if after_tokens >= before_tokens {
                eprintln!(
                    "tokenless: response compression did not reduce size ({} -> {} est. tokens), outputting original",
                    before_tokens, after_tokens
                );
                // No-savings discard edge: if a stash was attached and an
                // array was truncated, those writes orphan (markers live in
                // `after_compact`, which is discarded). Truncation almost
                // always yields savings, so this is rare; orphaned entries
                // are TTL-cleaned.
                input.clone()
            } else {
                after_compact.clone()
            };

            let mode = resolve_mode(compression_on, before_tokens, after_tokens);
            let emit_text = if compression_on {
                output_text.clone()
            } else {
                input.clone()
            };
            println!("{}", emit_text);

            record_compression_stats(
                &config,
                &database_paths,
                OperationType::CompressResponse,
                agent_id,
                session_id,
                tool_use_id,
                input,
                output_text,
                mode,
                stash_writes,
                stash_errors,
                stash_size,
            );
        }
        Commands::Retrieve { hash, stash_db } => {
            let store = match open_stash_store_or_err(stash_db.as_deref()) {
                Ok(s) => s,
                Err(e) => {
                    return Err((format!("stash unavailable: {}", e), 1));
                }
            };
            // Accept either a bare 24-hex hash or text containing a marker;
            // extract_hash validates the embedded hash. When no marker is
            // found, validate the bare hash format before the DB round-trip
            // so a mistaken non-hash argument (e.g. a file path) gets a clear
            // format error instead of a misleading "no stashed payload".
            let key = match extract_hash(&hash) {
                Some(h) => h.to_string(),
                None if is_valid_hash(&hash) => hash.to_string(),
                None => {
                    return Err((
                        format!(
                            "invalid stash hash: {:?} (expected 24 hex chars or a <<tokenless:HASH>> marker)",
                            hash
                        ),
                        1,
                    ));
                }
            };
            match store.retrieve(&key) {
                Ok(Some(payload)) => {
                    println!("{}", payload);
                }
                Ok(None) => {
                    return Err((format!("no stashed payload for hash: {}", key), 1));
                }
                Err(e) => {
                    return Err((format!("stash retrieve failed: {}", e), 1));
                }
            }
        }
        Commands::Stats(stats_cmd) => {
            let recorder = open_recorder()?;

            match stats_cmd {
                StatsCommands::Summary {
                    limit,
                    json,
                    compare,
                } => {
                    if let Some(sessions) = compare {
                        let baseline_sid = sessions[0].as_str();
                        let tokenless_sid = sessions[1].as_str();
                        let baseline = recorder
                            .records_by_session(baseline_sid, limit)
                            .map_err(|e| (format!("Failed to query baseline: {}", e), 1))?;
                        let tokenless = recorder
                            .records_by_session(tokenless_sid, limit)
                            .map_err(|e| (format!("Failed to query tokenless: {}", e), 1))?;
                        // Warn if a session's records do not match the expected mode,
                        // i.e. the baseline run was not recorded as dry-run.
                        warn_mode_mismatch("baseline", &baseline, CompressionMode::DryRun);
                        warn_mode_mismatch("tokenless", &tokenless, CompressionMode::Active);
                        if json {
                            println!("{}", format_compare_json(&baseline, &tokenless));
                        } else {
                            println!("{}", format_compare(&baseline, &tokenless));
                        }
                        return Ok(());
                    }
                    let records = recorder
                        .all_records(limit)
                        .map_err(|e| (format!("Failed to query records: {}", e), 1))?;
                    if json {
                        println!("{}", tokenless_stats::format_summary_json(&records, None));
                    } else {
                        println!(
                            "{}",
                            format_summary(&records, Some("Tokenless Statistics Summary"), None)
                        );
                    }
                }
                StatsCommands::List { limit } => {
                    let records = recorder
                        .all_records(Some(limit))
                        .map_err(|e| (format!("Failed to query records: {}", e), 1))?;
                    println!("{}", format_list(&records, limit));
                }
                StatsCommands::Show { id } => {
                    let record = recorder
                        .record_by_id(id)
                        .map_err(|e| (format!("Failed to query record: {}", e), 1))?
                        .ok_or_else(|| (format!("Record not found: {}", id), 1))?;
                    println!("{}", format_show(&record));
                }
                StatsCommands::Diff {
                    id,
                    session,
                    tool_use_id,
                    limit,
                    sort,
                    context,
                    no_color,
                    json,
                } => {
                    let report = match (id, session) {
                        (Some(id), None) => {
                            let record = recorder
                                .record_by_id(id)
                                .map_err(|e| (format!("Failed to query record: {}", e), 1))?
                                .ok_or_else(|| (format!("Record not found: {}", id), 1))?;
                            record_report(&record, context)
                        }
                        (None, Some(session_id)) => {
                            let records = recorder
                                .records_for_diff(&session_id, tool_use_id.as_deref())
                                .map_err(|e| (format!("Failed to query diff records: {}", e), 1))?;
                            if records.is_empty() {
                                let scope = tool_use_id.as_deref().map_or_else(
                                    || format!("session {session_id:?}"),
                                    |tool| format!("tool use {tool:?} in session {session_id:?}"),
                                );
                                return Err((format!("No records found for {}", scope), 1));
                            }
                            if let Some(tool_use_id) = tool_use_id {
                                tool_use_report(&records, &session_id, &tool_use_id, context)
                            } else {
                                session_report(
                                    &records,
                                    &session_id,
                                    limit,
                                    sort.map(DiffSort::from).unwrap_or_default(),
                                )
                            }
                        }
                        _ => {
                            return Err((
                                "Specify exactly one of RECORD_ID or --session".to_string(),
                                2,
                            ));
                        }
                    };
                    if json {
                        let output = serde_json::to_string_pretty(&report)
                            .map_err(|e| (format!("Failed to serialize diff report: {}", e), 1))?;
                        println!("{}", output);
                    } else {
                        let color = !no_color
                            && std::env::var_os("NO_COLOR").is_none()
                            && io::stdout().is_terminal();
                        println!("{}", format_diff_report(&report, color));
                    }
                }
                StatsCommands::Clear { yes } => {
                    if !yes {
                        print!("Are you sure you want to clear all statistics? [y/N] ");
                        use std::io::Write;
                        let _ = io::stdout().flush();
                        let mut input = String::new();
                        if io::stdin().read_line(&mut input).unwrap_or(0) == 0 {
                            println!("Cancelled.");
                            return Ok(());
                        }
                        if !input.trim().eq_ignore_ascii_case("y") {
                            println!("Cancelled.");
                            return Ok(());
                        }
                    }
                    recorder
                        .clear()
                        .map_err(|e| (format!("Failed to clear: {}", e), 1))?;
                    println!("Statistics cleared.");
                }
                StatsCommands::Status => {
                    let stats_env_set = std::env::var("TOKENLESS_STATS_ENABLED")
                        .ok()
                        .filter(|v| !v.is_empty());
                    let sls_env_set = std::env::var("TOKENLESS_SLS_ENABLED")
                        .ok()
                        .filter(|v| !v.is_empty());
                    let config = TokenlessConfig::load();
                    let file_exists = TokenlessConfig::config_file_exists();

                    let stats_state = if config.is_stats_enabled() {
                        "ENABLED"
                    } else {
                        "DISABLED"
                    };
                    let stats_source = if stats_env_set.is_some() {
                        "env override"
                    } else if file_exists {
                        "config file"
                    } else {
                        "default"
                    };
                    println!("Stats recording: {} (via {})", stats_state, stats_source);

                    let sls_state = if config.is_sls_enabled() {
                        "ENABLED"
                    } else {
                        "DISABLED"
                    };
                    let sls_source = if sls_env_set.is_some() {
                        "env override"
                    } else if file_exists {
                        "config file"
                    } else {
                        "default"
                    };
                    println!("SLS recording:   {} (via {})", sls_state, sls_source);
                }
                StatsCommands::Enable => {
                    let mut config = TokenlessConfig::load();
                    config.stats_enabled = true;
                    config
                        .save()
                        .map_err(|e| (format!("Failed to save config: {}", e), 1))?;
                    println!("Stats recording enabled.");
                }
                StatsCommands::Disable => {
                    let mut config = TokenlessConfig::load();
                    config.stats_enabled = false;
                    config
                        .save()
                        .map_err(|e| (format!("Failed to save config: {}", e), 1))?;
                    println!("Stats recording disabled.");
                }
            }
        }
        Commands::CompressToon {
            file,
            agent_id,
            session_id,
            tool_use_id,
        } => {
            let input = read_input(&file).map_err(|e| (e, 2))?;
            let value: serde_json::Value = serde_json::from_str(&input)
                .map_err(|e| (format!("JSON parse error: {}", e), 2))?;
            let output = toon_format::encode_default(&value)
                .map_err(|e| (format!("toon encode failed: {}", e), 2))?;
            let output = output.trim_end().to_string();

            // If no token savings, output original instead of TOON result
            let before_tokens = estimate_tokens_from_bytes(input.len());
            let after_tokens = estimate_tokens_from_bytes(output.len());
            let no_savings = output.is_empty() || after_tokens >= before_tokens;
            if no_savings {
                eprintln!(
                    "tokenless: TOON encoding did not reduce size ({} -> {} est. tokens), outputting original JSON",
                    before_tokens, after_tokens
                );
            }

            let config = TokenlessConfig::load();
            let compression_on = config.is_compression_enabled();
            let mode = resolve_mode(compression_on, before_tokens, after_tokens);
            // Active: emit the TOON result (or original if no savings).
            // Dry-run: emit the original so context stays uncompressed, but
            // still record the TOON result as the predicted savings below.
            let emit_text = if compression_on && !no_savings {
                output.clone()
            } else {
                input.clone()
            };
            println!("{}", emit_text);

            // Recorded `after` = the predicted TOON result (or original when
            // TOON did not reduce size), so dry-run captures the prediction.
            let record_after = if no_savings { input.clone() } else { output };
            let database_paths = DatabasePathResolver::default();
            record_compression_stats(
                &config,
                &database_paths,
                OperationType::CompressToon,
                agent_id,
                session_id,
                tool_use_id,
                input,
                record_after,
                mode,
                None,
                None,
                None,
            );
        }
        Commands::DecompressToon { file } => {
            let input = read_input(&file).map_err(|e| (e, 2))?;
            let value: serde_json::Value = toon_format::decode_default(&input)
                .map_err(|e| (format!("toon decode failed: {}", e), 2))?;
            let output = serde_json::to_string_pretty(&value)
                .map_err(|e| (format!("Serialization error: {}", e), 2))?;
            let output = output.trim_end().to_string();
            if !output.is_empty() {
                println!("{}", output);
            }
        }
        Commands::EnvCheck {
            tool,
            all,
            fix,
            checklist,
            json,
        } => {
            env_check::run(tool.as_deref(), all, fix, checklist, json)?;
        }
        Commands::Init {
            framework,
            all,
            list,
        } => {
            init::run(framework, all, list)?;
        }
        Commands::Mcp(McpCommands::Serve) => {
            mcp::serve()?;
        }
    }

    Ok(())
}

/// Resolve the recording mode from the compression toggle.
///
/// When compression is disabled (dry-run), the original input is emitted so
/// the LLM context stays uncompressed, but the predicted savings are still
/// recorded — enabling A/B comparison of the same task with/without
/// compression.
fn resolve_mode(
    compression_on: bool,
    before_tokens: usize,
    after_tokens: usize,
) -> CompressionMode {
    if compression_on {
        CompressionMode::Active
    } else {
        eprintln!(
            "tokenless: dry-run mode (compression disabled) — emitted original, predicted {} -> {} est. tokens",
            before_tokens, after_tokens
        );
        CompressionMode::DryRun
    }
}

/// Warn (to stderr) when a session's records were not recorded in the expected
/// mode, e.g. a "baseline" session that was not run with compression disabled.
/// A non-blocking sanity hint — comparison still proceeds.
fn warn_mode_mismatch(label: &str, records: &[StatsRecord], expected: CompressionMode) {
    if records.is_empty() {
        return;
    }
    let mismatched = records.iter().filter(|r| r.mode != expected).count();
    if mismatched > 0 {
        eprintln!(
            "tokenless: warning — {} session has {} record(s) not in {} mode (comparison may be inaccurate)",
            label,
            mismatched,
            expected.as_str()
        );
    }
}

/// Record compression stats — fail-silent so compression output
/// is never blocked by database errors.
///
/// All metrics (chars, tokens) are derived from actual text content,
/// never from caller-supplied estimates.
#[allow(clippy::too_many_arguments)]
fn record_compression_stats(
    config: &TokenlessConfig,
    database_paths: &DatabasePathResolver,
    op: OperationType,
    agent_id: Option<String>,
    session_id: Option<String>,
    tool_use_id: Option<String>,
    before_text: String,
    after_text: String,
    mode: CompressionMode,
    stash_writes: Option<usize>,
    stash_errors: Option<usize>,
    stash_size: Option<usize>,
) {
    // Short-circuit only if both stats and SLS are disabled.
    if !config.is_stats_enabled() && !config.is_sls_enabled() {
        return;
    }

    let before_bytes = before_text.len();
    let after_bytes = after_text.len();

    // Skip recording if there was no actual token savings
    let before_tokens = estimate_tokens_from_bytes(before_bytes);
    let after_tokens = estimate_tokens_from_bytes(after_bytes);
    if after_tokens >= before_tokens {
        return;
    }

    let pid = std::process::id();
    let agent = agent_id
        .as_deref()
        .map(|a| a.to_string())
        .unwrap_or_else(|| "cli".to_string());
    let mut record = StatsRecord::new(
        op,
        agent,
        before_bytes,
        before_tokens,
        after_bytes,
        after_tokens,
    )
    .with_before_text(before_text)
    .with_after_text(after_text);
    if let Some(sid) = session_id {
        record = record.with_session_id(sid);
    }
    if let Some(tuid) = tool_use_id {
        record = record.with_tool_use_id(tuid);
    }
    record = record
        .with_source_pid(pid as i64)
        .with_mode(mode)
        .with_stash(stash_writes, stash_errors, stash_size);

    // SQLite stats recording — gated by stats_enabled
    if config.is_stats_enabled()
        && let Ok(recorder) = open_recorder_with(database_paths)
    {
        let _ = recorder.record(&record);
    }

    // SLS recording — fail-silent, independent of SQLite
    if config.is_sls_enabled() {
        let writer = tokenless_stats::SlsWriter::new();
        writer.write(&record);
    }
}

fn main() {
    if let Err((msg, code)) = run() {
        eprintln!("Error: {}", msg);
        process::exit(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("tests/main_tests.rs");
}
