//! Tokenless CLI - LLM token optimization via schema and response compression.
mod env_check;

use clap::{Parser, Subcommand};
use std::fs;
use std::io::{self, Read};
use std::process;
use tokenless_schema::{ResponseCompressor, SchemaCompressor};
use tokenless_stats::estimate_tokens_from_bytes;
use tokenless_stats::{OperationType, StatsRecord, StatsRecorder, TokenlessConfig};
use tokenless_stats::{format_list, format_show, format_summary};

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
}

#[derive(Subcommand)]
enum StatsCommands {
    /// Show summary statistics with breakdown by operation
    Summary {
        #[arg(long)]
        limit: Option<usize>,
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
            let mut buf = String::new();
            io::stdin()
                .lock()
                .take(limit)
                .read_to_string(&mut buf)
                .map_err(|e| format!("Failed to read stdin: {}", e))?;
            if buf.len() > MAX_INPUT_BYTES {
                return Err(too_large());
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

/// Resolve the database path. When `TOKENLESS_STATS_DB` is set, the path
/// is validated to ensure it resides under the user's home directory;
/// otherwise the env var is ignored and the default path is used. This
/// prevents an attacker from redirecting the database to a system-critical
/// location (e.g. `/etc/evil.db`).
fn get_db_path() -> String {
    let home = get_home_dir();
    if let Ok(env_path) = std::env::var("TOKENLESS_STATS_DB")
        && !env_path.is_empty()
    {
        match validate_db_path(&env_path, &home) {
            Ok(path) => return path,
            Err(reason) => eprintln!("[tokenless] ignoring TOKENLESS_STATS_DB: {}", reason),
        }
    }
    format!("{}/.tokenless/stats.db", home)
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
    if resolved.starts_with(home) {
        Ok(env_path.to_string())
    } else {
        Err(format!(
            "path '{}' is outside home directory '{}'",
            env_path, home
        ))
    }
}

fn ensure_db_dir() -> Result<(), (String, i32)> {
    let db_path = get_db_path();
    if let Some(parent) = std::path::Path::new(&db_path).parent() {
        fs::create_dir_all(parent)
            .map_err(|e| (format!("Failed to create database directory: {}", e), 1))?;
    }
    Ok(())
}

fn open_recorder() -> Result<StatsRecorder, (String, i32)> {
    ensure_db_dir()?;
    StatsRecorder::new(get_db_path()).map_err(|e| (format!("Failed to open database: {}", e), 1))
}

fn run() -> Result<(), (String, i32)> {
    let cli = Cli::parse();

    match cli.command {
        Commands::CompressSchema {
            file,
            batch,
            agent_id,
            session_id,
            tool_use_id,
        } => {
            let input = read_input(&file).map_err(|e| (e, 2))?;
            let value: serde_json::Value = serde_json::from_str(&input)
                .map_err(|e| (format!("JSON parse error: {}", e), 2))?;

            let compressor = SchemaCompressor::new();

            let (result_json, after_compact) = if batch {
                let arr = value
                    .as_array()
                    .ok_or_else(|| ("Expected a JSON array for --batch mode".to_string(), 1))?;
                let results: Vec<serde_json::Value> =
                    arr.iter().map(|item| compressor.compress(item)).collect();
                let compact = serde_json::to_string(&results).unwrap_or_default();
                let pretty = serde_json::to_string_pretty(&results)
                    .map_err(|e| (format!("Serialization error: {}", e), 2))?;
                (pretty, compact)
            } else {
                let result = compressor.compress(&value);
                let compact = serde_json::to_string(&result).unwrap_or_default();
                let pretty = serde_json::to_string_pretty(&result)
                    .map_err(|e| (format!("Serialization error: {}", e), 2))?;
                (pretty, compact)
            };

            // If no token savings, output original instead of compressed result
            let before_tokens = estimate_tokens_from_bytes(input.len());
            let after_tokens = estimate_tokens_from_bytes(after_compact.len());
            let output_text = if after_tokens >= before_tokens {
                input.clone()
            } else {
                result_json.clone()
            };

            println!("{}", output_text);

            record_compression_stats(
                OperationType::CompressSchema,
                agent_id,
                session_id,
                tool_use_id,
                input,
                output_text,
            );
        }
        Commands::CompressResponse {
            file,
            agent_id,
            session_id,
            tool_use_id,
        } => {
            let input = read_input(&file).map_err(|e| (e, 2))?;
            let value: serde_json::Value = serde_json::from_str(&input)
                .map_err(|e| (format!("JSON parse error: {}", e), 2))?;

            let compressor = ResponseCompressor::new();
            let result = compressor.compress(&value);
            let after_compact = serde_json::to_string(&result).unwrap_or_else(|_| String::new());
            let result_json = serde_json::to_string_pretty(&result)
                .map_err(|e| (format!("Serialization error: {}", e), 2))?;

            // If no token savings, output original instead of compressed result
            let before_tokens = estimate_tokens_from_bytes(input.len());
            let after_tokens = estimate_tokens_from_bytes(after_compact.len());
            let output_text = if after_tokens >= before_tokens {
                input.clone()
            } else {
                result_json.clone()
            };

            println!("{}", output_text);

            record_compression_stats(
                OperationType::CompressResponse,
                agent_id,
                session_id,
                tool_use_id,
                input,
                output_text,
            );
        }
        Commands::Stats(stats_cmd) => {
            let recorder = open_recorder()?;

            match stats_cmd {
                StatsCommands::Summary { limit } => {
                    let records = recorder
                        .all_records(limit)
                        .map_err(|e| (format!("Failed to query records: {}", e), 1))?;
                    println!(
                        "{}",
                        format_summary(&records, Some("Tokenless Statistics Summary"))
                    );
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
                    let env_set = std::env::var("TOKENLESS_STATS_ENABLED").ok();
                    let config = TokenlessConfig::load();
                    if config.is_stats_enabled() {
                        let source = if env_set.is_some() {
                            "env override"
                        } else if TokenlessConfig::config_file_exists() {
                            "config file"
                        } else {
                            "default"
                        };
                        println!("Stats recording: ENABLED (via {})", source);
                    } else {
                        let source = if env_set.is_some() {
                            "env override"
                        } else if TokenlessConfig::config_file_exists() {
                            "config file"
                        } else {
                            "default"
                        };
                        println!("Stats recording: DISABLED (via {})", source);
                    }
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
            let display = if output.is_empty() || after_tokens >= before_tokens {
                input.clone()
            } else {
                output
            };
            println!("{}", display);

            record_compression_stats(
                OperationType::CompressToon,
                agent_id,
                session_id,
                tool_use_id,
                input,
                display,
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
    }

    Ok(())
}

/// Record compression stats — fail-silent so compression output
/// is never blocked by database errors.
///
/// All metrics (chars, tokens) are derived from actual text content,
/// never from caller-supplied estimates.
fn record_compression_stats(
    op: OperationType,
    agent_id: Option<String>,
    session_id: Option<String>,
    tool_use_id: Option<String>,
    before_text: String,
    after_text: String,
) {
    if !TokenlessConfig::load().is_stats_enabled() {
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
    record = record.with_source_pid(pid as i64);

    // Record silently — stats failures must not break compression
    if let Ok(recorder) = open_recorder() {
        let _ = recorder.record(&record);
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

    fn temp_subdir(label: &str) -> std::path::PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!(
            "tokenless-db-validate-{}-{}-{}",
            std::process::id(),
            nanos,
            label
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn validate_db_path_rejects_empty_home() {
        // No trusted home anchor means starts_with("") would match
        // any path, so the function must short-circuit to rejection.
        let err = validate_db_path("/tmp/whatever.db", "").unwrap_err();
        assert!(err.contains("no trusted home"));
    }

    #[test]
    fn validate_db_path_accepts_path_inside_home() {
        let home = temp_subdir("inside");
        let canon_home = std::fs::canonicalize(&home).unwrap();
        let inner = canon_home.join("stats.db");
        let result =
            validate_db_path(inner.to_str().unwrap(), canon_home.to_str().unwrap()).unwrap();
        assert_eq!(result, inner.to_str().unwrap());
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn validate_db_path_rejects_path_outside_home() {
        let home = temp_subdir("outside-home");
        let canon_home = std::fs::canonicalize(&home).unwrap();
        // Pick a known-existing directory that is NOT under home.
        let outside = std::path::Path::new("/etc/hosts");
        if !outside.exists() {
            std::fs::remove_dir_all(&home).ok();
            return;
        }
        let err = validate_db_path("/etc/hosts", canon_home.to_str().unwrap()).unwrap_err();
        assert!(err.contains("outside home"));
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn validate_db_path_rejects_parent_dir_bypass_with_existing_parent() {
        // ~/foo/../../etc/evil.db where /etc exists: canonicalize() of
        // the parent resolves to /etc, which must fail starts_with(home).
        let home = temp_subdir("pd-existing");
        let canon_home = std::fs::canonicalize(&home).unwrap();
        let escape = canon_home.join("foo/../../etc/evil.db");
        let err =
            validate_db_path(escape.to_str().unwrap(), canon_home.to_str().unwrap()).unwrap_err();
        // Either "outside home" (parent canonicalized away from home) or
        // "cannot be resolved" (parent itself unreachable). Both are valid
        // rejections — what matters is no Ok return.
        assert!(err.contains("outside home") || err.contains("cannot be resolved"));
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn validate_db_path_rejects_parent_dir_bypass_with_nonexistent_parent() {
        // ~/nonexistent-path/../../etc/evil.db where nonexistent-path
        // doesn't exist: parent canonicalize() ALSO fails, so without the
        // hardening this path would slip through via the old fallback.
        let home = temp_subdir("pd-nonexistent");
        let canon_home = std::fs::canonicalize(&home).unwrap();
        let escape = canon_home.join("does-not-exist-xyz/../../etc/evil.db");
        let result = validate_db_path(escape.to_str().unwrap(), canon_home.to_str().unwrap());
        assert!(
            result.is_err(),
            "ParentDir bypass via nonexistent intermediate must be rejected; got {:?}",
            result
        );
        std::fs::remove_dir_all(&home).ok();
    }
}
