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

fn read_input(file: &Option<String>) -> Result<String, String> {
    match file {
        Some(path) => {
            fs::read_to_string(path).map_err(|e| format!("Failed to read file '{}': {}", path, e))
        }
        None => {
            let mut buf = String::new();
            io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| format!("Failed to read stdin: {}", e))?;
            Ok(buf)
        }
    }
}

/// Resolve the current user's home directory.
///
/// Prefers the account-database entry from `getpwuid_r` so an attacker
/// cannot redirect the path by mutating `$HOME`. Falls back to
/// `dirs::home_dir()` (which itself reads `$HOME`) only when the syscall
/// has no result, e.g. minimal containers without an `/etc/passwd` entry.
/// Returns an empty string on failure — the previous `.` CWD fallback was
/// dropped because it caused state files to land wherever the binary was
/// invoked from, which is both unexpected and unsafe.
pub fn get_home_dir() -> String {
    #[cfg(unix)]
    if let Some(home) = home_dir_from_passwd() {
        return home;
    }
    dirs::home_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

#[cfg(unix)]
fn home_dir_from_passwd() -> Option<String> {
    use std::ffi::CStr;
    // SAFETY: getuid is infallible and always safe. getpwuid_r is the
    // thread-safe variant: we hand it a stack-allocated passwd struct and
    // a 4 KiB heap buffer, and it never writes past the buffer length we
    // pass. result is left null when no entry is found, which we detect.
    let uid = unsafe { libc::getuid() };
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0u8; 4096];
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe {
        libc::getpwuid_r(
            uid,
            &mut pwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() || pwd.pw_dir.is_null() {
        return None;
    }
    // SAFETY: pw_dir points into our buf and is NUL-terminated by the libc
    // contract. The CStr borrow is short-lived; we copy the bytes out before
    // pwd/buf are dropped.
    let home = unsafe { CStr::from_ptr(pwd.pw_dir) }.to_str().ok()?;
    (!home.is_empty()).then(|| home.to_string())
}

fn get_db_path() -> String {
    std::env::var("TOKENLESS_STATS_DB")
        .unwrap_or_else(|_| format!("{}/.tokenless/stats.db", get_home_dir()))
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

            let result_json = if batch {
                let arr = value
                    .as_array()
                    .ok_or_else(|| ("Expected a JSON array for --batch mode".to_string(), 1))?;
                let results: Vec<serde_json::Value> =
                    arr.iter().map(|item| compressor.compress(item)).collect();
                serde_json::to_string_pretty(&results)
                    .map_err(|e| (format!("Serialization error: {}", e), 2))?
            } else {
                let result = compressor.compress(&value);
                serde_json::to_string_pretty(&result)
                    .map_err(|e| (format!("Serialization error: {}", e), 2))?
            };

            // Compact JSON for accurate size comparison (pretty-print inflates size)
            let after_compact = serde_json::to_string(
                &serde_json::from_str::<serde_json::Value>(&result_json)
                    .unwrap_or(serde_json::Value::Null),
            )
            .unwrap_or(result_json.clone());

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
            let result_json = serde_json::to_string_pretty(&compressor.compress(&value))
                .map_err(|e| (format!("Serialization error: {}", e), 2))?;

            let after_compact = serde_json::to_string(
                &serde_json::from_str::<serde_json::Value>(&result_json)
                    .unwrap_or(serde_json::Value::Null),
            )
            .unwrap_or(result_json.clone());

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
