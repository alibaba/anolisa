//! Tokenless CLI - LLM token optimization via schema and response compression.

use clap::{Parser, Subcommand};
use std::fs;
use std::io::{self, Read};
use std::process;
use tokenless_schema::{ResponseCompressor, SchemaCompressor};
use tokenless_stats::estimate_tokens_from_chars;
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

fn get_db_path() -> String {
    std::env::var("TOKENLESS_STATS_DB").unwrap_or_else(|_| {
        format!(
            "{}/.tokenless/stats.db",
            std::env::var("HOME").unwrap_or_else(|_| ".".to_string())
        )
    })
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
                .map_err(|e| (format!("JSON parse error: {}", e), 1))?;

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

            println!("{}", result_json);

            // Auto-record stats with actual text content
            // Compact JSON for accurate after_text (pretty-print inflates size)
            let after_compact = serde_json::to_string(
                &serde_json::from_str::<serde_json::Value>(&result_json)
                    .unwrap_or(serde_json::Value::Null),
            )
            .unwrap_or(result_json.clone());
            record_compression_stats(
                OperationType::CompressSchema,
                agent_id,
                session_id,
                tool_use_id,
                input,
                after_compact,
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
                .map_err(|e| (format!("JSON parse error: {}", e), 1))?;

            let compressor = ResponseCompressor::new();
            let result_json = serde_json::to_string_pretty(&compressor.compress(&value))
                .map_err(|e| (format!("Serialization error: {}", e), 2))?;

            println!("{}", result_json);

            // Auto-record stats with actual text content
            let after_compact = serde_json::to_string(
                &serde_json::from_str::<serde_json::Value>(&result_json)
                    .unwrap_or(serde_json::Value::Null),
            )
            .unwrap_or(result_json.clone());
            record_compression_stats(
                OperationType::CompressResponse,
                agent_id,
                session_id,
                tool_use_id,
                input,
                after_compact,
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
                        io::stdout().flush().unwrap();
                        let mut input = String::new();
                        io::stdin().read_line(&mut input).unwrap();
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
            let mut child = std::process::Command::new("toon")
                .arg("-e")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| (format!("Failed to spawn toon: {}", e), 2))?;
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(input.as_bytes())
                    .map_err(|e| (format!("Failed to write to toon stdin: {}", e), 2))?;
            }
            let out = child
                .wait_with_output()
                .map_err(|e| (format!("Failed to wait for toon: {}", e), 2))?;
            if !out.status.success() {
                return Err((
                    format!(
                        "toon encode failed: {}",
                        String::from_utf8_lossy(&out.stderr)
                    ),
                    2,
                ));
            }
            let output = String::from_utf8_lossy(&out.stdout);
            let output = output.trim_end();
            if !output.is_empty() {
                println!("{}", output);
            }
            // Auto-record stats
            record_compression_stats(
                OperationType::CompressToon,
                agent_id,
                session_id,
                tool_use_id,
                input,
                output.to_string(),
            );
        }
        Commands::DecompressToon { file } => {
            let input = read_input(&file).map_err(|e| (e, 2))?;
            let mut child = std::process::Command::new("toon")
                .arg("-d")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| (format!("Failed to spawn toon: {}", e), 2))?;
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(input.as_bytes())
                    .map_err(|e| (format!("Failed to write to toon stdin: {}", e), 2))?;
            }
            let out = child
                .wait_with_output()
                .map_err(|e| (format!("Failed to wait for toon: {}", e), 2))?;
            if !out.status.success() {
                return Err((
                    format!(
                        "toon decode failed: {}",
                        String::from_utf8_lossy(&out.stderr)
                    ),
                    2,
                ));
            }
            let output = String::from_utf8_lossy(&out.stdout);
            let output = output.trim_end();
            if !output.is_empty() {
                println!("{}", output);
            }
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

    let before_chars = before_text.len();
    let after_chars = after_text.len();
    let before_tokens = estimate_tokens_from_chars(before_chars);
    let after_tokens = estimate_tokens_from_chars(after_chars);

    let pid = std::process::id();
    let agent = agent_id
        .as_deref()
        .map(|a| format!("{}({})", a, pid))
        .unwrap_or_else(|| format!("cli({})", pid));
    let mut record = StatsRecord::new(
        op,
        agent,
        before_chars,
        before_tokens,
        after_chars,
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
