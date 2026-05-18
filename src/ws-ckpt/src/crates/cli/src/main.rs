use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use ws_ckpt_common::{
    decode_payload, default_auto_cleanup_keep, encode_frame, load_config_file, save_config_file,
    ChangeType, CleanupRetention, DaemonConfig, ErrorCode, Request, Response,
    ADVISORY_SNAPSHOT_LIMIT, CONFIG_FILE_PATH, DEFAULT_AUTO_CLEANUP,
    DEFAULT_AUTO_CLEANUP_INTERVAL_SECS, DEFAULT_HEALTH_CHECK_INTERVAL_SECS,
    DEFAULT_IMG_MAX_PERCENT, DEFAULT_IMG_SIZE_GB, DEFAULT_MOUNT_PATH, DEFAULT_SOCKET_PATH,
};

/// Backend-usage advisory threshold (percent); CLI-side since daemon returns raw bytes.
const ADVISORY_FS_USAGE_PCT: f64 = 90.0;

/// Upper bound for the best-effort advisory IPC; a stuck daemon must not delay
/// user-visible commands. Local UDS RTT is sub-millisecond.
const ADVISORY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(10);

// Parse CLI value for `--auto-cleanup-keep`: integer -> Count mode, duration
// string (e.g. "30d", units s/m/h/d/w) -> Age mode. Mirrors TOML semantics in
// `CleanupRetention::deserialize`.
fn parse_cleanup_retention(s: &str) -> Result<CleanupRetention, String> {
    if let Ok(n) = s.parse::<u32>() {
        return Ok(CleanupRetention::Count(n));
    }
    CleanupRetention::age(s).map_err(|e| format!("invalid value '{}': {}", s, e))
}

#[derive(Parser)]
#[command(name = "ws-ckpt", version, about = "btrfs workspace snapshot manager")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the ws-ckpt daemon (manual mode)
    Daemon {
        /// btrfs filesystem mount point
        #[arg(long, default_value = DEFAULT_MOUNT_PATH)]
        mount_path: PathBuf,

        /// Unix socket path for IPC
        #[arg(long, default_value = DEFAULT_SOCKET_PATH)]
        socket: PathBuf,

        /// Log level (debug/info/warn/error)
        #[arg(long, default_value = "info")]
        log_level: String,
    },

    /// Initialize a workspace for btrfs snapshot management
    Init {
        /// Workspace path or ID (absolute path, relative path, or workspace ID)
        #[arg(long, short = 'w')]
        workspace: String,
    },

    /// Create a checkpoint (readonly snapshot)
    Checkpoint {
        /// Workspace path or ID (absolute path, relative path, or workspace ID)
        #[arg(long, short = 'w')]
        workspace: String,

        /// Snapshot ID (must be unique within the workspace)
        #[arg(long, short = 'i')]
        id: String,

        /// Commit message describing the checkpoint
        #[arg(long, short = 'm')]
        message: Option<String>,

        /// Additional metadata as JSON string
        #[arg(long)]
        metadata: Option<String>,
    },

    /// Rollback workspace to a specific snapshot
    Rollback {
        /// Workspace path or ID (absolute path, relative path, or workspace ID)
        #[arg(long, short = 'w')]
        workspace: String,

        /// Target snapshot (ID like msg1-step2, or name like before-refactor)
        #[arg(long = "snapshot", short = 's')]
        to: String,
    },

    /// Delete a specific snapshot
    Delete {
        /// Workspace path or ID (optional; omit for global snapshot lookup)
        #[arg(long, short = 'w')]
        workspace: Option<String>,

        /// Snapshot ID or unique prefix
        #[arg(long, short = 's')]
        snapshot: String,

        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },

    /// List all snapshots for a workspace (or all workspaces if omitted)
    List {
        /// Workspace path or ID (optional; omit to list all workspaces)
        #[arg(long, short = 'w')]
        workspace: Option<String>,

        /// Output format: table or json (default: table)
        #[arg(long, default_value = "table")]
        format: String,
    },

    /// Show diff between two snapshots
    Diff {
        /// Workspace path or ID (absolute path, relative path, or workspace ID)
        #[arg(long, short = 'w')]
        workspace: String,

        /// Source snapshot (ID or name)
        #[arg(long, short = 'f')]
        from: String,

        /// Target snapshot (ID or name)
        #[arg(long, short = 't')]
        to: String,
    },

    /// Show daemon and workspace status
    Status {
        /// Workspace path or ID (optional filter)
        #[arg(long, short = 'w')]
        workspace: Option<String>,

        /// Output format: table or json (default: table)
        #[arg(long, default_value = "table")]
        format: String,
    },

    /// Clean up old snapshots, keeping the most recent ones
    Cleanup {
        /// Workspace path or ID (absolute path, relative path, or workspace ID)
        #[arg(long, short = 'w')]
        workspace: String,

        /// Number of recent unpinned snapshots to keep (default: 20)
        #[arg(long, default_value = "20")]
        keep: u32,
    },

    /// View or update daemon configuration
    Config {
        /// Set health check interval in seconds (0 disables the scheduler loop)
        #[arg(long)]
        health_check_interval: Option<u64>,

        /// Set target image size in GB (image will be grown/shrunk at next daemon restart)
        #[arg(long)]
        img_size: Option<u64>,

        /// Set initial-creation cap as percentage of host partition (0-100); only used on first bootstrap
        #[arg(long)]
        img_max_percent: Option<f64>,

        /// Enable periodic auto-cleanup
        #[arg(long, conflicts_with = "disable_auto_cleanup")]
        enable_auto_cleanup: bool,

        /// Disable periodic auto-cleanup
        #[arg(long, conflicts_with = "enable_auto_cleanup")]
        disable_auto_cleanup: bool,

        /// Set cleanup retention: integer (count mode, 0 = disabled) or duration like "30d" (age mode, units s/m/h/d/w)
        #[arg(long, value_parser = parse_cleanup_retention)]
        auto_cleanup_keep: Option<CleanupRetention>,

        /// Set auto-cleanup interval in seconds (0 disables the scheduler loop)
        #[arg(long)]
        auto_cleanup_interval: Option<u64>,
    },

    /// Trigger daemon to reload /etc/ws-ckpt/config.toml
    Reload,

    /// Recover workspace to a normal directory (undo init)
    Recover {
        /// Workspace path or ID
        #[arg(short, long, conflicts_with = "all")]
        workspace: Option<String>,

        /// Recover all registered workspaces
        #[arg(long, conflicts_with = "workspace")]
        all: bool,

        /// Skip interactive confirmation
        #[arg(long)]
        force: bool,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match run(cli).await {
        Ok(()) => {
            // Post-command soft advisory: best-effort, silent on failure.
            print_health_advisory_if_needed().await;
        }
        Err(e) => {
            eprintln!("\x1b[31mError: {:#}\x1b[0m", e);
            process::exit(1);
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Daemon {
            mount_path,
            socket,
            log_level,
        } => {
            // Load auto-cleanup settings from config file
            let file_config = load_config_file(std::path::Path::new(CONFIG_FILE_PATH))
                .unwrap_or_else(|e| {
                    eprintln!(
                        "Warning: failed to load {}: {}, using defaults",
                        CONFIG_FILE_PATH, e
                    );
                    Default::default()
                });
            let config = DaemonConfig {
                mount_path,
                socket_path: socket,
                log_level,
                auto_cleanup: file_config.auto_cleanup.unwrap_or(DEFAULT_AUTO_CLEANUP),
                auto_cleanup_keep: file_config
                    .auto_cleanup_keep
                    .clone()
                    .unwrap_or_else(default_auto_cleanup_keep),
                auto_cleanup_interval_secs: file_config
                    .auto_cleanup_interval_secs
                    .unwrap_or(DEFAULT_AUTO_CLEANUP_INTERVAL_SECS),
                health_check_interval_secs: file_config
                    .health_check_interval_secs
                    .unwrap_or(DEFAULT_HEALTH_CHECK_INTERVAL_SECS),
                backend_type: file_config.backend.r#type.clone(),
                img_size: file_config
                    .backend
                    .btrfs_loop
                    .as_ref()
                    .and_then(|b| b.img_size)
                    .unwrap_or(DEFAULT_IMG_SIZE_GB),
                img_max_percent: file_config
                    .backend
                    .btrfs_loop
                    .as_ref()
                    .and_then(|b| b.img_max_percent)
                    .unwrap_or(DEFAULT_IMG_MAX_PERCENT * 100.0),
                min_free_bytes: 512 * 1024 * 1024,
                min_free_percent: 1.0,
            };
            ws_ckpt_daemon::run_daemon(config).await?;
        }
        Commands::Init { workspace } => {
            let request = Request::Init {
                workspace: resolve_workspace_arg(&workspace),
            };
            let response = send_request_to_daemon(&request).await?;
            handle_response(response, &request).await?;
        }
        Commands::Checkpoint {
            workspace,
            id,
            message,
            metadata,
        } => {
            // Validate metadata is valid JSON if provided
            if let Some(ref s) = metadata {
                serde_json::from_str::<serde_json::Value>(s)
                    .context("invalid JSON in --metadata")?;
            }
            let request = Request::Checkpoint {
                workspace: resolve_workspace_arg(&workspace),
                id,
                message,
                metadata,
                pin: false,
            };
            let response = send_request_to_daemon(&request).await?;
            handle_response(response, &request).await?;
        }
        Commands::Rollback { workspace, to } => {
            let request = Request::Rollback {
                workspace: resolve_workspace_arg(&workspace),
                to,
            };
            let response = send_request_to_daemon(&request).await?;
            handle_response(response, &request).await?;
        }
        Commands::Delete {
            workspace,
            snapshot,
            force,
        } => {
            let request = Request::Delete {
                workspace: workspace.as_deref().map(resolve_workspace_arg),
                snapshot,
                force,
            };
            let response = send_request_to_daemon(&request).await?;
            handle_response(response, &request).await?;
        }
        Commands::List { workspace, format } => {
            let request = Request::List {
                workspace: workspace.as_deref().map(resolve_workspace_arg),
                format: Some(format.clone()),
            };
            let response = send_request_to_daemon(&request).await?;
            handle_list_response(response, &format)?;
        }
        Commands::Diff {
            workspace,
            from,
            to,
        } => {
            let request = Request::Diff {
                workspace: resolve_workspace_arg(&workspace),
                from,
                to,
            };
            let response = send_request_to_daemon(&request).await?;
            handle_diff_response(response)?;
        }
        Commands::Status { workspace, format } => {
            let request = Request::Status {
                workspace: workspace.as_deref().map(resolve_workspace_arg),
            };
            let response = send_request_to_daemon(&request).await?;
            handle_status_response(response, &format)?;
        }
        Commands::Cleanup { workspace, keep } => {
            let request = Request::Cleanup {
                workspace: resolve_workspace_arg(&workspace),
                keep: Some(keep),
            };
            let response = send_request_to_daemon(&request).await?;
            handle_cleanup_response(response)?;
        }
        Commands::Config {
            health_check_interval,
            img_size,
            img_max_percent,
            enable_auto_cleanup,
            disable_auto_cleanup,
            auto_cleanup_keep,
            auto_cleanup_interval,
        } => {
            let auto_cleanup = match (enable_auto_cleanup, disable_auto_cleanup) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            };
            if health_check_interval.is_none()
                && img_size.is_none()
                && img_max_percent.is_none()
                && auto_cleanup.is_none()
                && auto_cleanup_keep.is_none()
                && auto_cleanup_interval.is_none()
            {
                // View mode: read config file and show
                handle_config_view()?;
            } else {
                // Update mode: modify config file + notify daemon
                handle_config_update(
                    health_check_interval,
                    img_size,
                    img_max_percent,
                    auto_cleanup,
                    auto_cleanup_keep,
                    auto_cleanup_interval,
                )
                .await?;
            }
        }
        Commands::Reload => {
            handle_reload().await?;
        }
        Commands::Recover {
            workspace,
            all,
            force,
        } => {
            handle_recover(workspace, all, force).await?;
        }
    }
    Ok(())
}

/// Get the socket path from env or default
fn get_socket_path() -> PathBuf {
    std::env::var("WS_CKPT_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_SOCKET_PATH))
}

/// Resolve workspace identifier: convert filesystem paths to absolute,
/// pass workspace IDs through unchanged.
///
/// IMPORTANT: We must NOT follow symlinks here. With symlink-based workspaces,
/// the user-facing path is a symlink (e.g. `/tmp/test-ws -> /mnt/btrfs-workspace/ws-xxx`).
/// The daemon registers the symlink path, so we must preserve it.
fn resolve_workspace_arg(workspace: &str) -> String {
    let path = std::path::Path::new(workspace);
    // If it looks like a workspace ID (no path separators), pass through unchanged
    if !workspace.contains('/') && !workspace.contains('\\') {
        return workspace.to_string();
    }
    // Convert to absolute path WITHOUT following symlinks
    if path.is_absolute() {
        workspace.to_string()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(workspace).to_string_lossy().to_string(),
            Err(_) => workspace.to_string(),
        }
    }
}

/// Send a request to the daemon via Unix Socket and receive a response
async fn send_request_to_daemon(request: &Request) -> Result<Response> {
    let socket_path = get_socket_path();

    let mut stream = UnixStream::connect(&socket_path).await.map_err(|e| {
        match e.kind() {
            std::io::ErrorKind::NotFound => {
                eprintln!("\x1b[31m\u{2717} Daemon is not running.\x1b[0m");
                eprintln!("  Start it with 'systemctl start ws-ckpt'.");
                process::exit(1);
            }
            std::io::ErrorKind::ConnectionRefused => {
                eprintln!(
                    "\x1b[33m\u{26a0} Daemon is starting up. Please retry in a few seconds.\x1b[0m"
                );
                process::exit(1);
            }
            _ => {}
        }
        anyhow::anyhow!(
            "Failed to connect to ws-ckpt daemon ({}): {}",
            socket_path.display(),
            e
        )
    })?;

    // Send request
    let frame = encode_frame(request)?;
    stream
        .write_all(&frame)
        .await
        .context("failed to send request")?;

    // Read response: 4-byte LE length + payload
    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .await
        .context("failed to read response length")?;
    let len = u32::from_le_bytes(len_buf) as usize;

    let mut payload = vec![0u8; len];
    stream
        .read_exact(&mut payload)
        .await
        .context("failed to read response payload")?;

    let response: Response = decode_payload(&payload)?;
    Ok(response)
}

/// Silent IPC used by best-effort callers (e.g. post-command health advisory).
/// Never prints to stderr and never calls `process::exit`; all errors bubble up
/// so the caller can decide to ignore them.
async fn try_send_request_to_daemon_silent(request: &Request) -> Result<Response> {
    let socket_path = get_socket_path();

    let mut stream = UnixStream::connect(&socket_path)
        .await
        .context("connect to daemon (silent)")?;

    let frame = encode_frame(request)?;
    stream
        .write_all(&frame)
        .await
        .context("send request (silent)")?;

    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .await
        .context("read response length (silent)")?;
    let len = u32::from_le_bytes(len_buf) as usize;

    let mut payload = vec![0u8; len];
    stream
        .read_exact(&mut payload)
        .await
        .context("read response payload (silent)")?;

    let response: Response = decode_payload(&payload)?;
    Ok(response)
}

/// Emit a best-effort advisory to stderr when any threshold is crossed.
/// Silent on timeout/IPC error; `fs_total_bytes == 0` skips only the fs warning.
async fn print_health_advisory_if_needed() {
    let (over_limit_workspace_count, fs_total_bytes, fs_used_bytes) = match tokio::time::timeout(
        ADVISORY_TIMEOUT,
        try_send_request_to_daemon_silent(&Request::HealthAdvisory),
    )
    .await
    {
        Ok(Ok(Response::HealthAdvisoryOk {
            over_limit_workspace_count,
            fs_total_bytes,
            fs_used_bytes,
        })) => (over_limit_workspace_count, fs_total_bytes, fs_used_bytes),
        _ => return,
    };

    let snapshot_warning: Option<String> = if over_limit_workspace_count > 0 {
        Some(format!(
            "{} workspace(s) have more than {} snapshots. Run `ws-ckpt status` to see details",
            over_limit_workspace_count, ADVISORY_SNAPSHOT_LIMIT
        ))
    } else {
        None
    };

    let fs_warning: Option<String> = if fs_total_bytes > 0 {
        let pct = (fs_used_bytes as f64 / fs_total_bytes as f64) * 100.0;
        if pct > ADVISORY_FS_USAGE_PCT {
            Some(format!(
                "btrfs backend usage above {}% (current: {:.1}%)",
                ADVISORY_FS_USAGE_PCT, pct
            ))
        } else {
            None
        }
    } else {
        None
    };

    if snapshot_warning.is_none() && fs_warning.is_none() {
        return;
    }

    eprintln!();
    if let Some(msg) = &snapshot_warning {
        eprintln!("\x1b[33m\u{26a0} Warning: {}.\x1b[0m", msg);
    }
    if let Some(msg) = &fs_warning {
        eprintln!("\x1b[33m\u{26a0} Warning: {}.\x1b[0m", msg);
    }
    eprintln!();
    eprintln!("\x1b[33m  Manual cleanup:   ws-ckpt cleanup -w <workspace> --keep <N>\x1b[0m");
    eprintln!("\x1b[33m  Or enable auto-cleanup for all workspaces:\x1b[0m");
    eprintln!("\x1b[33m      ws-ckpt config --enable-auto-cleanup \\\x1b[0m");
    eprintln!("\x1b[33m                     --auto-cleanup-keep <NUM|DURATION> \\\x1b[0m");
    eprintln!("\x1b[33m                     --auto-cleanup-interval <SECONDS>\x1b[0m");
    eprintln!("\x1b[33m  Suggested values:\x1b[0m");
    eprintln!("\x1b[33m      ws-ckpt config --enable-auto-cleanup --auto-cleanup-keep 1000 --auto-cleanup-interval 86400\x1b[0m");
}

/// Handle the response, printing formatted output.
/// If ConfirmationRequired, prompt user and re-send with force=true.
async fn handle_response(response: Response, original_request: &Request) -> Result<()> {
    match response {
        Response::InitOk { ws_id } => {
            println!("\x1b[32m✓ Workspace initialized: {}\x1b[0m", ws_id);
        }
        Response::CheckpointOk { snapshot_id } => {
            println!("\x1b[32m✓ Checkpoint created: {}\x1b[0m", snapshot_id);
        }
        Response::RollbackOk { from, to } => {
            println!(
                "\x1b[32m✓ Rolled back workspace {} to snapshot: {}\x1b[0m",
                from, to
            );
        }
        Response::DeleteOk { target } => {
            println!("\x1b[32m✓ Deleted: {}\x1b[0m", target);
        }
        Response::RecoverOk { workspace } => {
            println!("\x1b[32m\u{2713} Workspace recovered: {}\x1b[0m", workspace);
        }
        Response::Error {
            code: ErrorCode::ConfirmationRequired,
            message,
        } => {
            eprintln!("\x1b[33m⚠ {}\x1b[0m", message);
            eprint!("确认操作? [y/N]: ");
            io::stderr().flush()?;

            let stdin = io::stdin();
            let mut line = String::new();
            stdin.lock().read_line(&mut line)?;

            if line.trim().eq_ignore_ascii_case("y") {
                // Re-send with force=true
                let force_request = match original_request {
                    Request::Delete {
                        workspace,
                        snapshot,
                        ..
                    } => Request::Delete {
                        workspace: workspace.clone(),
                        snapshot: snapshot.clone(),
                        force: true,
                    },
                    _ => {
                        anyhow::bail!("unexpected ConfirmationRequired for non-delete request");
                    }
                };
                let response = send_request_to_daemon(&force_request).await?;
                Box::pin(handle_response(response, &force_request)).await?;
            } else {
                println!("操作已取消");
            }
        }
        Response::Error {
            code: ErrorCode::SnapshotAlreadyExists,
            message,
        } => {
            eprintln!("\x1b[31m✗ {}\x1b[0m", message);
            eprintln!("  Please use a different snapshot ID.");
            process::exit(1);
        }
        Response::Error {
            code: ErrorCode::DiskSpaceInsufficient,
            message,
        } => {
            eprintln!("\x1b[31m\u{2717} {}\x1b[0m", message);
            eprintln!("  Use 'ws-ckpt delete' to remove old snapshots, or free disk space.");
            process::exit(1);
        }
        Response::Error {
            code: ErrorCode::SnapshotNotFound,
            message,
        } => {
            eprintln!("\x1b[31m✗ {}\x1b[0m", message);
            eprintln!("  Use 'ws-ckpt list' to view available snapshots.");
            process::exit(1);
        }
        Response::Error {
            code: ErrorCode::WorkspaceNotFound,
            message,
        } => {
            eprintln!("\x1b[31m✗ {}\x1b[0m", message);
            eprintln!("  Use 'ws-ckpt init' to initialize, or 'ws-ckpt list' to view workspaces.");
            process::exit(1);
        }
        Response::Error { code, message } => {
            eprintln!("\x1b[31mError [{:?}]: {}\x1b[0m", code, message);
            process::exit(1);
        }
        Response::CheckpointSkipped { reason } => {
            eprintln!("\x1b[33m\u{26a0} {}\x1b[0m", reason);
        }
        _ => {
            // Phase 2 responses are handled by dedicated functions
            eprintln!("\x1b[33mUnexpected response type\x1b[0m");
        }
    }
    Ok(())
}

/// Handle ListOk response, formatting as table or json.
fn handle_list_response(response: Response, format: &str) -> Result<()> {
    match response {
        Response::ListOk { snapshots } => {
            if format == "json" {
                println!("{}", serde_json::to_string_pretty(&snapshots)?);
            } else {
                // Table format
                if snapshots.is_empty() {
                    println!("No snapshots found.");
                } else {
                    // Dynamically compute column widths
                    let hdr_ws = "WORKSPACE";
                    let hdr_snap = "SNAPSHOT";
                    let offset_secs = chrono::Local::now().offset().local_minus_utc();
                    let sign = if offset_secs >= 0 { '+' } else { '-' };
                    let h = offset_secs.abs() / 3600;
                    let m = (offset_secs.abs() % 3600) / 60;
                    let local_offset = if m == 0 {
                        format!("{sign}{h}")
                    } else {
                        format!("{sign}{h}:{m:02}")
                    };
                    let hdr_date = format!("CREATED (UTC{local_offset})");
                    let hdr_date = hdr_date.as_str();
                    let hdr_msg = "MESSAGE";

                    let w_ws = snapshots
                        .iter()
                        .map(|e| e.workspace.len())
                        .max()
                        .unwrap_or(0)
                        .max(hdr_ws.len());
                    let w_snap = snapshots
                        .iter()
                        .map(|e| {
                            if e.meta.missing {
                                e.id.len() + " [MISSING]".len()
                            } else {
                                e.id.len()
                            }
                        })
                        .max()
                        .unwrap_or(0)
                        .max(hdr_snap.len());
                    let w_date = 19_usize.max(hdr_date.len()); // "YYYY-MM-DD HH:MM:SS"

                    println!(
                        "{:<w_ws$} {:<w_snap$} {:<w_date$} {}",
                        hdr_ws, hdr_snap, hdr_date, hdr_msg,
                    );
                    println!("{}", "-".repeat(w_ws + w_snap + w_date + hdr_msg.len() + 3));
                    for entry in &snapshots {
                        let id_display = if entry.meta.missing {
                            format!("{} [MISSING]", entry.id)
                        } else {
                            entry.id.clone()
                        };
                        println!(
                            "{:<w_ws$} {:<w_snap$} {:<w_date$} {}",
                            entry.workspace,
                            id_display,
                            entry
                                .meta
                                .created_at
                                .with_timezone(&chrono::Local)
                                .format("%Y-%m-%d %H:%M:%S"),
                            entry.meta.message.as_deref().unwrap_or("-"),
                        );
                    }
                    println!("\nTotal: {} snapshot(s)", snapshots.len());
                }
            }
        }
        Response::Error { code, message } => {
            eprintln!("\x1b[31mError [{:?}]: {}\x1b[0m", code, message);
            process::exit(1);
        }
        _ => {
            eprintln!("\x1b[33mUnexpected response type\x1b[0m");
        }
    }
    Ok(())
}

/// Handle DiffOk response, formatting diff entries.
fn handle_diff_response(response: Response) -> Result<()> {
    match response {
        Response::DiffOk { changes } => {
            if changes.is_empty() {
                println!("No differences found.");
            } else {
                for entry in &changes {
                    let marker = match entry.change_type {
                        ChangeType::Added => "\x1b[32m+",
                        ChangeType::Deleted => "\x1b[31m-",
                        ChangeType::Modified => "\x1b[33mM",
                        ChangeType::Renamed => "\x1b[36mR",
                    };
                    let detail = entry
                        .detail
                        .as_deref()
                        .map(|d| format!(" ({})", d))
                        .unwrap_or_default();
                    println!("{}  {}{}\x1b[0m", marker, entry.path, detail);
                }
                println!("\n{} change(s)", changes.len());
            }
        }
        Response::Error { code, message } => {
            eprintln!("\x1b[31mError [{:?}]: {}\x1b[0m", code, message);
            process::exit(1);
        }
        _ => {
            eprintln!("\x1b[33mUnexpected response type\x1b[0m");
        }
    }
    Ok(())
}

/// Handle StatusOk response, formatting the status report.
fn handle_status_response(response: Response, format: &str) -> Result<()> {
    match response {
        Response::StatusOk { report } => {
            if format == "json" {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("\x1b[1mDaemon Status\x1b[0m");
                println!("  Uptime: {} seconds", report.uptime_secs);
                println!(
                    "  Filesystem: {:.2} GB used / {:.2} GB total",
                    report.fs_used_bytes as f64 / 1_073_741_824.0,
                    report.fs_total_bytes as f64 / 1_073_741_824.0,
                );
                println!();
                if report.workspaces.is_empty() {
                    println!("  No workspaces registered.");
                } else {
                    println!("  {:<25} {:<40} SNAPSHOTS", "WORKSPACE", "PATH");
                    println!("  {}", "-".repeat(75));
                    for ws in &report.workspaces {
                        println!("  {:<25} {:<40} {}", ws.ws_id, ws.path, ws.snapshot_count);
                    }
                    println!("\n  Total: {} workspace(s)", report.workspaces.len());
                }
            }
        }
        Response::Error { code, message } => {
            eprintln!("\x1b[31mError [{:?}]: {}\x1b[0m", code, message);
            process::exit(1);
        }
        _ => {
            eprintln!("\x1b[33mUnexpected response type\x1b[0m");
        }
    }
    Ok(())
}

/// Handle CleanupOk response.
fn handle_cleanup_response(response: Response) -> Result<()> {
    match response {
        Response::CleanupOk { removed } => {
            if removed.is_empty() {
                println!("\x1b[32m\u{2713} No snapshots needed cleanup.\x1b[0m");
            } else {
                println!(
                    "\x1b[32m\u{2713} Cleaned up {} snapshot(s):\x1b[0m",
                    removed.len()
                );
                for snap in &removed {
                    println!("  - {}", snap);
                }
            }
        }
        Response::Error { code, message } => {
            eprintln!("\x1b[31mError [{:?}]: {}\x1b[0m", code, message);
            process::exit(1);
        }
        _ => {
            eprintln!("\x1b[33mUnexpected response type\x1b[0m");
        }
    }
    Ok(())
}

/// View current configuration from config file (no daemon required).
fn handle_config_view() -> Result<()> {
    let path = std::path::Path::new(CONFIG_FILE_PATH);
    let fc = load_config_file(path).map_err(|e| anyhow::anyhow!("Failed to read config: {}", e))?;

    let auto_cleanup = fc.auto_cleanup.unwrap_or(DEFAULT_AUTO_CLEANUP);
    let keep = fc
        .auto_cleanup_keep
        .clone()
        .unwrap_or_else(default_auto_cleanup_keep);
    let keep_display = match &keep {
        CleanupRetention::Count(n) => format!("{} (count mode)", n),
        CleanupRetention::Age { raw, .. } => format!("\"{}\" (age mode)", raw),
    };
    let interval = fc
        .auto_cleanup_interval_secs
        .unwrap_or(DEFAULT_AUTO_CLEANUP_INTERVAL_SECS);
    let health = fc
        .health_check_interval_secs
        .unwrap_or(DEFAULT_HEALTH_CHECK_INTERVAL_SECS);
    let btrfs_loop = fc.backend.btrfs_loop.as_ref();
    let img_size = btrfs_loop
        .and_then(|b| b.img_size)
        .unwrap_or(DEFAULT_IMG_SIZE_GB);
    let img_max = btrfs_loop
        .and_then(|b| b.img_max_percent)
        .unwrap_or(DEFAULT_IMG_MAX_PERCENT * 100.0);

    println!("\x1b[1mDaemon Configuration\x1b[0m");
    println!("  Config file:             {}", CONFIG_FILE_PATH);
    println!(
        "  Mount path:              {} (default)",
        DEFAULT_MOUNT_PATH
    );
    println!(
        "  Socket path:             {} (default)",
        DEFAULT_SOCKET_PATH
    );
    println!(
        "  Auto-cleanup:            {}{}",
        if auto_cleanup { "enabled" } else { "disabled" },
        if fc.auto_cleanup.is_none() {
            " (default)"
        } else {
            ""
        }
    );
    println!(
        "  Auto-cleanup keep:       {}{}",
        keep_display,
        if fc.auto_cleanup_keep.is_none() {
            " (default)"
        } else {
            ""
        }
    );
    println!(
        "  Auto-cleanup interval:   {}s ({}m){}",
        interval,
        interval / 60,
        if fc.auto_cleanup_interval_secs.is_none() {
            " (default)"
        } else {
            ""
        }
    );
    println!(
        "  Health-check interval:   {}s ({}m){}",
        health,
        health / 60,
        if fc.health_check_interval_secs.is_none() {
            " (default)"
        } else {
            ""
        }
    );
    println!(
        "  Image size:              {} GB{}",
        img_size,
        if btrfs_loop.and_then(|b| b.img_size).is_none() {
            " (default)"
        } else {
            ""
        }
    );
    println!(
        "  Image max percent:       {}%{}",
        img_max,
        if btrfs_loop.and_then(|b| b.img_max_percent).is_none() {
            " (default)"
        } else {
            ""
        }
    );
    Ok(())
}

/// Update configuration: write to config file + notify daemon to reload.
async fn handle_config_update(
    health_check_interval: Option<u64>,
    img_size: Option<u64>,
    img_max_percent: Option<f64>,
    auto_cleanup: Option<bool>,
    auto_cleanup_keep: Option<CleanupRetention>,
    auto_cleanup_interval_secs: Option<u64>,
) -> Result<()> {
    let path = std::path::Path::new(CONFIG_FILE_PATH);

    // Load existing config
    let mut fc =
        load_config_file(path).map_err(|e| anyhow::anyhow!("Failed to read config: {}", e))?;

    // Apply updates
    if let Some(interval) = health_check_interval {
        fc.health_check_interval_secs = Some(interval);
    }
    if let Some(v) = auto_cleanup {
        fc.auto_cleanup = Some(v);
    }
    if let Some(v) = auto_cleanup_keep {
        fc.auto_cleanup_keep = Some(v);
    }
    if let Some(v) = auto_cleanup_interval_secs {
        fc.auto_cleanup_interval_secs = Some(v);
    }
    // Record whether any btrfs-loop image settings are being changed,
    // as these only take effect at daemon bootstrap (not on reload).
    let has_img_settings = img_size.is_some() || img_max_percent.is_some();

    // Handle btrfs-loop settings
    if has_img_settings {
        let mut bl = fc.backend.btrfs_loop.unwrap_or_default();
        if let Some(s) = img_size {
            bl.img_size = Some(s);
        }
        if let Some(c) = img_max_percent {
            bl.img_max_percent = Some(c);
        }
        fc.backend.btrfs_loop = Some(bl);
    }

    // Save
    save_config_file(path, &fc).map_err(|e| anyhow::anyhow!("Failed to save config: {}", e))?;

    println!(
        "\x1b[32m\u{2713} Configuration saved to {}\x1b[0m",
        CONFIG_FILE_PATH
    );

    // Try to notify running daemon
    match send_request_to_daemon(&Request::ReloadConfig).await {
        Ok(Response::ReloadConfigOk) => {
            println!("\x1b[32m\u{2713} Daemon reloaded configuration\x1b[0m");
            if has_img_settings {
                println!("\x1b[33m\u{26a0} Note: btrfs-loop image settings (img-size, img-max-percent) require daemon restart to take effect.\x1b[0m");
            }
        }
        Ok(Response::Error { message, .. }) => {
            eprintln!("\x1b[33m\u{26a0} Daemon reload failed: {}\x1b[0m", message);
        }
        Err(_) => {
            println!("\x1b[33m\u{26a0} Daemon not running, changes will take effect on next start\x1b[0m");
        }
        _ => {}
    }
    Ok(())
}

/// Handle `ws-ckpt reload` (also used by systemd `ExecReload=`): send
/// `Request::ReloadConfig` IPC. If the daemon is not running,
/// `send_request_to_daemon` exits 1 with a red message.
async fn handle_reload() -> Result<()> {
    match send_request_to_daemon(&Request::ReloadConfig).await? {
        Response::ReloadConfigOk => {
            println!("\x1b[32m\u{2713} Daemon reloaded configuration\x1b[0m");
            Ok(())
        }
        Response::Error { code, message } => {
            anyhow::bail!("Daemon reload failed [{:?}]: {}", code, message);
        }
        other => anyhow::bail!("Unexpected response from daemon: {:?}", other),
    }
}

/// Handle recover command: single workspace or all workspaces.
async fn handle_recover(workspace: Option<String>, all: bool, force: bool) -> Result<()> {
    if workspace.is_none() && !all {
        eprintln!("\x1b[31mError: either --workspace/-w or --all must be specified\x1b[0m");
        process::exit(1);
    }

    if all {
        // Batch mode: get all workspaces via Status
        let status_req = Request::Status { workspace: None };
        let status_resp = send_request_to_daemon(&status_req).await?;
        let workspaces = match status_resp {
            Response::StatusOk { report } => report.workspaces,
            Response::Error { code, message } => {
                eprintln!("\x1b[31mError [{:?}]: {}\x1b[0m", code, message);
                process::exit(1);
            }
            _ => {
                eprintln!("\x1b[33mUnexpected response type\x1b[0m");
                process::exit(1);
            }
        };

        if workspaces.is_empty() {
            println!("No workspaces to recover.");
            return Ok(());
        }

        if !force {
            println!("Recovering {} workspace(s):", workspaces.len());
            for ws in &workspaces {
                println!("  {} ({} snapshots)", ws.path, ws.snapshot_count);
            }
            println!(
                "This will delete all snapshots and restore all workspaces to normal directories."
            );
            eprint!("Proceed? [y/N] ");
            io::stderr().flush()?;
            let stdin = io::stdin();
            let mut line = String::new();
            stdin.lock().read_line(&mut line)?;
            if line.trim() != "y" && line.trim() != "Y" {
                println!("Operation cancelled.");
                return Ok(());
            }
        }

        for ws in &workspaces {
            let req = Request::Recover {
                workspace: ws.path.clone(),
            };
            let resp = send_request_to_daemon(&req).await?;
            match resp {
                Response::RecoverOk { workspace } => {
                    println!("Workspace recovered: {}", workspace);
                }
                Response::Error { code, message } => {
                    eprintln!(
                        "\x1b[31mError [{:?}] recovering {}: {}\x1b[0m",
                        code, ws.path, message
                    );
                }
                _ => {
                    eprintln!("\x1b[33mUnexpected response for {}\x1b[0m", ws.path);
                }
            }
        }
        println!("All workspaces recovered.");
    } else {
        // Single workspace mode
        let ws_arg = resolve_workspace_arg(workspace.as_deref().unwrap());

        // Get status for snapshot count
        let status_req = Request::Status {
            workspace: Some(ws_arg.clone()),
        };
        let status_resp = send_request_to_daemon(&status_req).await?;
        let snapshot_count = match &status_resp {
            Response::StatusOk { report } => report
                .workspaces
                .first()
                .map(|w| w.snapshot_count)
                .unwrap_or(0),
            Response::Error { code, message } => {
                eprintln!("\x1b[31mError [{:?}]: {}\x1b[0m", code, message);
                process::exit(1);
            }
            _ => 0,
        };

        if !force {
            println!("Workspace: {} ({} snapshots)", ws_arg, snapshot_count);
            println!(
                "This will delete all snapshots and restore the workspace to a normal directory."
            );
            eprint!("Proceed? [y/N] ");
            io::stderr().flush()?;
            let stdin = io::stdin();
            let mut line = String::new();
            stdin.lock().read_line(&mut line)?;
            if line.trim() != "y" && line.trim() != "Y" {
                println!("Operation cancelled.");
                return Ok(());
            }
        }

        let req = Request::Recover { workspace: ws_arg };
        let resp = send_request_to_daemon(&req).await?;
        match resp {
            Response::RecoverOk { workspace } => {
                println!("\x1b[32m\u{2713} Workspace recovered: {}\x1b[0m", workspace);
            }
            Response::Error { code, message } => {
                eprintln!("\x1b[31mError [{:?}]: {}\x1b[0m", code, message);
                process::exit(1);
            }
            _ => {
                eprintln!("\x1b[33mUnexpected response type\x1b[0m");
                process::exit(1);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    // ── Subcommand basic parsing ──

    #[test]
    fn parse_daemon_default() {
        let cli = Cli::try_parse_from(["ws-ckpt", "daemon"]).unwrap();
        match cli.command {
            Commands::Daemon {
                mount_path,
                socket,
                log_level,
            } => {
                assert_eq!(mount_path, PathBuf::from(DEFAULT_MOUNT_PATH));
                assert_eq!(socket, PathBuf::from(DEFAULT_SOCKET_PATH));
                assert_eq!(log_level, "info");
            }
            _ => panic!("expected Daemon"),
        }
    }

    #[test]
    fn parse_init() {
        let cli = Cli::try_parse_from(["ws-ckpt", "init", "--workspace", "/tmp/test"]).unwrap();
        match cli.command {
            Commands::Init { workspace } => assert_eq!(workspace, "/tmp/test"),
            _ => panic!("expected Init"),
        }
    }

    #[test]
    fn parse_checkpoint_full() {
        let cli = Cli::try_parse_from([
            "ws-ckpt",
            "checkpoint",
            "--workspace",
            "/tmp/test",
            "--id",
            "msg1-step0",
            "-m",
            "save point",
            "--metadata",
            r#"{"key":"value"}"#,
        ])
        .unwrap();
        match cli.command {
            Commands::Checkpoint {
                workspace,
                id,
                message,
                metadata,
            } => {
                assert_eq!(workspace, "/tmp/test");
                assert_eq!(id, "msg1-step0");
                assert_eq!(message.as_deref(), Some("save point"));
                assert_eq!(metadata.as_deref(), Some(r#"{"key":"value"}"#));
            }
            _ => panic!("expected Checkpoint"),
        }
    }

    #[test]
    fn parse_checkpoint_minimal() {
        let cli = Cli::try_parse_from([
            "ws-ckpt",
            "checkpoint",
            "--workspace",
            "/ws",
            "--id",
            "snap-1",
        ])
        .unwrap();
        match cli.command {
            Commands::Checkpoint {
                id,
                message,
                metadata,
                ..
            } => {
                assert_eq!(id, "snap-1");
                assert!(message.is_none());
                assert!(metadata.is_none());
            }
            _ => panic!("expected Checkpoint"),
        }
    }

    #[test]
    fn parse_checkpoint_short_message_flag() {
        let cli = Cli::try_parse_from([
            "ws-ckpt",
            "checkpoint",
            "--workspace",
            "/ws",
            "--id",
            "snap-1",
            "-m",
            "备注",
        ])
        .unwrap();
        match cli.command {
            Commands::Checkpoint { message, .. } => {
                assert_eq!(message.as_deref(), Some("备注"));
            }
            _ => panic!("expected Checkpoint"),
        }
    }

    #[test]
    fn parse_rollback() {
        let cli = Cli::try_parse_from([
            "ws-ckpt",
            "rollback",
            "--workspace",
            "/tmp/test",
            "--snapshot",
            "msg1-step1",
        ])
        .unwrap();
        match cli.command {
            Commands::Rollback { workspace, to } => {
                assert_eq!(workspace, "/tmp/test");
                assert_eq!(to, "msg1-step1");
            }
            _ => panic!("expected Rollback"),
        }
    }

    #[test]
    fn parse_delete_default_no_force() {
        let cli = Cli::try_parse_from([
            "ws-ckpt",
            "delete",
            "--workspace",
            "/ws",
            "--snapshot",
            "abc123",
        ])
        .unwrap();
        match cli.command {
            Commands::Delete {
                workspace,
                snapshot,
                force,
            } => {
                assert_eq!(workspace.as_deref(), Some("/ws"));
                assert_eq!(snapshot, "abc123");
                assert!(!force, "force should default to false");
            }
            _ => panic!("expected Delete"),
        }
    }

    #[test]
    fn parse_delete_with_force() {
        let cli = Cli::try_parse_from([
            "ws-ckpt",
            "delete",
            "-w",
            "/ws",
            "--snapshot",
            "abc123",
            "--force",
        ])
        .unwrap();
        match cli.command {
            Commands::Delete {
                workspace, force, ..
            } => {
                assert_eq!(workspace.as_deref(), Some("/ws"));
                assert!(force);
            }
            _ => panic!("expected Delete"),
        }
    }

    #[test]
    fn parse_delete_short_snapshot_flag() {
        let cli = Cli::try_parse_from(["ws-ckpt", "delete", "-w", "/ws", "-s", "abc123"]).unwrap();
        match cli.command {
            Commands::Delete {
                workspace,
                snapshot,
                force,
            } => {
                assert_eq!(workspace.as_deref(), Some("/ws"));
                assert_eq!(snapshot, "abc123");
                assert!(!force);
            }
            _ => panic!("expected Delete"),
        }
    }

    #[test]
    fn delete_missing_snapshot_fails() {
        let result = Cli::try_parse_from(["ws-ckpt", "delete", "--workspace", "/ws"]);
        assert!(result.is_err(), "delete without --snapshot should fail");
    }

    #[test]
    fn delete_without_workspace_parses_ok() {
        let cli = Cli::try_parse_from(["ws-ckpt", "delete", "--snapshot", "abc"]).unwrap();
        match cli.command {
            Commands::Delete {
                workspace,
                snapshot,
                ..
            } => {
                assert!(workspace.is_none());
                assert_eq!(snapshot, "abc");
            }
            _ => panic!("expected Delete"),
        }
    }

    // ── Error cases ──

    #[test]
    fn init_missing_workspace_fails() {
        let result = Cli::try_parse_from(["ws-ckpt", "init"]);
        assert!(result.is_err(), "init without --workspace should fail");
    }

    #[test]
    fn checkpoint_missing_workspace_fails() {
        let result = Cli::try_parse_from(["ws-ckpt", "checkpoint", "--id", "snap-1"]);
        assert!(
            result.is_err(),
            "checkpoint without --workspace should fail"
        );
    }

    #[test]
    fn checkpoint_missing_id_fails() {
        let result = Cli::try_parse_from(["ws-ckpt", "checkpoint", "--workspace", "/ws"]);
        assert!(result.is_err(), "checkpoint without --id should fail");
    }

    #[test]
    fn rollback_missing_to_fails() {
        let result = Cli::try_parse_from(["ws-ckpt", "rollback", "--workspace", "/ws"]);
        assert!(result.is_err(), "rollback without --snapshot should fail");
    }

    // ── Metadata JSON validation ──
    // Note: clap accepts --metadata as a raw string. JSON validation happens
    // at runtime in the run() function via serde_json::from_str, not at parse time.
    // So both valid and invalid JSON will parse successfully at the clap level.

    #[test]
    fn metadata_valid_json_parses() {
        let cli = Cli::try_parse_from([
            "ws-ckpt",
            "checkpoint",
            "--workspace",
            "/ws",
            "--id",
            "snap-1",
            "--metadata",
            r#"{"key":"value"}"#,
        ])
        .unwrap();
        match cli.command {
            Commands::Checkpoint { metadata, .. } => {
                let json_str = metadata.unwrap();
                // Verify it's valid JSON
                let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
                assert_eq!(v["key"], "value");
            }
            _ => panic!("expected Checkpoint"),
        }
    }

    #[test]
    fn metadata_invalid_json_parsed_but_invalid() {
        // clap will accept the string, but serde_json::from_str should fail
        let cli = Cli::try_parse_from([
            "ws-ckpt",
            "checkpoint",
            "--workspace",
            "/ws",
            "--id",
            "snap-1",
            "--metadata",
            "not-json",
        ])
        .unwrap();
        match cli.command {
            Commands::Checkpoint { metadata, .. } => {
                let json_str = metadata.unwrap();
                let result = serde_json::from_str::<serde_json::Value>(&json_str);
                assert!(result.is_err(), "invalid JSON should fail to parse");
            }
            _ => panic!("expected Checkpoint"),
        }
    }

    // ── handle_response: DeleteOk output branching ──

    #[tokio::test]
    async fn delete_ok_snapshot_shows_deleted_message() {
        let response = Response::DeleteOk {
            target: "msg1-step0".to_string(),
        };
        let request = Request::Delete {
            workspace: Some("/ws".to_string()),
            snapshot: "msg1-step0".to_string(),
            force: false,
        };
        let result = handle_response(response, &request).await;
        assert!(result.is_ok());
    }

    // ── Phase 2 CLI parsing tests ──

    #[test]
    fn parse_list() {
        let cli = Cli::try_parse_from(["ws-ckpt", "list", "--workspace", "/tmp/test"]).unwrap();
        match cli.command {
            Commands::List { workspace, format } => {
                assert_eq!(workspace.as_deref(), Some("/tmp/test"));
                assert_eq!(format, "table"); // default
            }
            _ => panic!("expected List"),
        }
    }

    #[test]
    fn parse_list_json_format() {
        let cli = Cli::try_parse_from([
            "ws-ckpt",
            "list",
            "--workspace",
            "/tmp/test",
            "--format",
            "json",
        ])
        .unwrap();
        match cli.command {
            Commands::List { format, .. } => {
                assert_eq!(format, "json");
            }
            _ => panic!("expected List"),
        }
    }

    #[test]
    fn parse_diff() {
        let cli = Cli::try_parse_from([
            "ws-ckpt",
            "diff",
            "--workspace",
            "/tmp/test",
            "--from",
            "msg1-step0",
            "--to",
            "msg2-step0",
        ])
        .unwrap();
        match cli.command {
            Commands::Diff {
                workspace,
                from,
                to,
            } => {
                assert_eq!(workspace, "/tmp/test");
                assert_eq!(from, "msg1-step0");
                assert_eq!(to, "msg2-step0");
            }
            _ => panic!("expected Diff"),
        }
    }

    #[test]
    fn parse_status() {
        let cli = Cli::try_parse_from(["ws-ckpt", "status"]).unwrap();
        match cli.command {
            Commands::Status { workspace, format } => {
                assert!(workspace.is_none());
                assert_eq!(format, "table");
            }
            _ => panic!("expected Status"),
        }
    }

    #[test]
    fn parse_status_with_workspace() {
        let cli = Cli::try_parse_from(["ws-ckpt", "status", "--workspace", "/tmp/ws"]).unwrap();
        match cli.command {
            Commands::Status { workspace, format } => {
                assert_eq!(workspace.as_deref(), Some("/tmp/ws"));
                assert_eq!(format, "table");
            }
            _ => panic!("expected Status"),
        }
    }

    #[test]
    fn parse_status_json_format() {
        let cli = Cli::try_parse_from(["ws-ckpt", "status", "--format", "json"]).unwrap();
        match cli.command {
            Commands::Status { format, .. } => {
                assert_eq!(format, "json");
            }
            _ => panic!("expected Status"),
        }
    }

    #[test]
    fn parse_cleanup() {
        let cli = Cli::try_parse_from(["ws-ckpt", "cleanup", "--workspace", "/tmp/test"]).unwrap();
        match cli.command {
            Commands::Cleanup { workspace, keep } => {
                assert_eq!(workspace, "/tmp/test");
                assert_eq!(keep, 20); // default
            }
            _ => panic!("expected Cleanup"),
        }
    }

    #[test]
    fn parse_cleanup_with_keep() {
        let cli = Cli::try_parse_from([
            "ws-ckpt",
            "cleanup",
            "--workspace",
            "/tmp/test",
            "--keep",
            "5",
        ])
        .unwrap();
        match cli.command {
            Commands::Cleanup { keep, .. } => {
                assert_eq!(keep, 5);
            }
            _ => panic!("expected Cleanup"),
        }
    }

    #[test]
    fn list_without_workspace_parses_ok() {
        let cli = Cli::try_parse_from(["ws-ckpt", "list"]).unwrap();
        match cli.command {
            Commands::List { workspace, format } => {
                assert!(workspace.is_none());
                assert_eq!(format, "table");
            }
            _ => panic!("expected List"),
        }
    }

    #[test]
    fn diff_missing_from_fails() {
        let result = Cli::try_parse_from([
            "ws-ckpt",
            "diff",
            "--workspace",
            "/ws",
            "--to",
            "msg1-step0",
        ]);
        assert!(result.is_err(), "diff without --from should fail");
    }

    #[test]
    fn cleanup_missing_workspace_fails() {
        let result = Cli::try_parse_from(["ws-ckpt", "cleanup"]);
        assert!(result.is_err(), "cleanup without --workspace should fail");
    }

    #[test]
    fn parse_config_no_args() {
        let cli = Cli::try_parse_from(["ws-ckpt", "config"]).unwrap();
        match cli.command {
            Commands::Config {
                health_check_interval,
                img_size,
                img_max_percent,
                ..
            } => {
                assert!(health_check_interval.is_none());
                assert!(img_size.is_none());
                assert!(img_max_percent.is_none());
            }
            _ => panic!("expected Config"),
        }
    }

    #[test]
    fn parse_config_with_all_flags() {
        let cli = Cli::try_parse_from([
            "ws-ckpt",
            "config",
            "--health-check-interval",
            "120",
            "--img-size",
            "30",
            "--img-max-percent",
            "40",
        ])
        .unwrap();
        match cli.command {
            Commands::Config {
                health_check_interval,
                img_size,
                img_max_percent,
                ..
            } => {
                assert_eq!(health_check_interval, Some(120));
                assert_eq!(img_size, Some(30));
                assert_eq!(img_max_percent, Some(40.0));
            }
            _ => panic!("expected Config"),
        }
    }

    #[test]
    fn parse_reload() {
        let cli = Cli::try_parse_from(["ws-ckpt", "reload"]).unwrap();
        assert!(matches!(cli.command, Commands::Reload));
    }

    #[test]
    fn parse_reload_rejects_args() {
        let result = Cli::try_parse_from(["ws-ckpt", "reload", "--foo"]);
        assert!(result.is_err(), "reload should accept no arguments");
    }

    #[test]
    fn parse_config_enable_auto_cleanup() {
        let cli = Cli::try_parse_from(["ws-ckpt", "config", "--enable-auto-cleanup"]).unwrap();
        match cli.command {
            Commands::Config {
                enable_auto_cleanup,
                disable_auto_cleanup,
                ..
            } => {
                assert!(enable_auto_cleanup);
                assert!(!disable_auto_cleanup);
            }
            _ => panic!("expected Config"),
        }
    }

    #[test]
    fn parse_config_disable_auto_cleanup() {
        let cli = Cli::try_parse_from(["ws-ckpt", "config", "--disable-auto-cleanup"]).unwrap();
        match cli.command {
            Commands::Config {
                enable_auto_cleanup,
                disable_auto_cleanup,
                ..
            } => {
                assert!(!enable_auto_cleanup);
                assert!(disable_auto_cleanup);
            }
            _ => panic!("expected Config"),
        }
    }

    #[test]
    fn parse_config_enable_disable_conflict() {
        let result = Cli::try_parse_from([
            "ws-ckpt",
            "config",
            "--enable-auto-cleanup",
            "--disable-auto-cleanup",
        ]);
        assert!(
            result.is_err(),
            "--enable-auto-cleanup and --disable-auto-cleanup must be mutually exclusive"
        );
    }

    #[test]
    fn parse_config_auto_cleanup_keep_count() {
        let cli = Cli::try_parse_from(["ws-ckpt", "config", "--auto-cleanup-keep", "20"]).unwrap();
        match cli.command {
            Commands::Config {
                auto_cleanup_keep, ..
            } => {
                assert_eq!(auto_cleanup_keep, Some(CleanupRetention::Count(20)));
            }
            _ => panic!("expected Config"),
        }
    }

    #[test]
    fn parse_config_auto_cleanup_keep_zero_disables() {
        let cli = Cli::try_parse_from(["ws-ckpt", "config", "--auto-cleanup-keep", "0"]).unwrap();
        match cli.command {
            Commands::Config {
                auto_cleanup_keep, ..
            } => {
                let keep = auto_cleanup_keep.expect("keep should be set");
                assert!(keep.is_disabled());
                assert_eq!(keep, CleanupRetention::Count(0));
            }
            _ => panic!("expected Config"),
        }
    }

    #[test]
    fn parse_config_auto_cleanup_keep_age() {
        let cli = Cli::try_parse_from(["ws-ckpt", "config", "--auto-cleanup-keep", "30d"]).unwrap();
        match cli.command {
            Commands::Config {
                auto_cleanup_keep, ..
            } => match auto_cleanup_keep {
                Some(CleanupRetention::Age { raw, secs }) => {
                    assert_eq!(raw, "30d");
                    assert_eq!(secs, 30 * 24 * 3600);
                }
                other => panic!("expected Age variant, got {:?}", other),
            },
            _ => panic!("expected Config"),
        }
    }

    #[test]
    fn parse_config_auto_cleanup_keep_invalid() {
        let result = Cli::try_parse_from(["ws-ckpt", "config", "--auto-cleanup-keep", "abc"]);
        assert!(
            result.is_err(),
            "non-numeric value without duration unit should be rejected"
        );
    }

    #[test]
    fn parse_config_auto_cleanup_interval() {
        let cli =
            Cli::try_parse_from(["ws-ckpt", "config", "--auto-cleanup-interval", "3600"]).unwrap();
        match cli.command {
            Commands::Config {
                auto_cleanup_interval,
                ..
            } => {
                assert_eq!(auto_cleanup_interval, Some(3600));
            }
            _ => panic!("expected Config"),
        }
    }

    // ── Recover CLI parsing tests ──

    #[test]
    fn parse_recover_with_workspace() {
        let cli =
            Cli::try_parse_from(["ws-ckpt", "recover", "--workspace", "/tmp/my-project"]).unwrap();
        match cli.command {
            Commands::Recover {
                workspace,
                all,
                force,
            } => {
                assert_eq!(workspace.as_deref(), Some("/tmp/my-project"));
                assert!(!all);
                assert!(!force);
            }
            _ => panic!("expected Recover"),
        }
    }

    #[test]
    fn parse_recover_with_short_workspace() {
        let cli = Cli::try_parse_from(["ws-ckpt", "recover", "-w", "/tmp/ws"]).unwrap();
        match cli.command {
            Commands::Recover { workspace, .. } => {
                assert_eq!(workspace.as_deref(), Some("/tmp/ws"));
            }
            _ => panic!("expected Recover"),
        }
    }

    #[test]
    fn parse_recover_all() {
        let cli = Cli::try_parse_from(["ws-ckpt", "recover", "--all"]).unwrap();
        match cli.command {
            Commands::Recover {
                workspace,
                all,
                force,
            } => {
                assert!(workspace.is_none());
                assert!(all);
                assert!(!force);
            }
            _ => panic!("expected Recover"),
        }
    }

    #[test]
    fn parse_recover_all_force() {
        let cli = Cli::try_parse_from(["ws-ckpt", "recover", "--all", "--force"]).unwrap();
        match cli.command {
            Commands::Recover {
                workspace,
                all,
                force,
            } => {
                assert!(workspace.is_none());
                assert!(all);
                assert!(force);
            }
            _ => panic!("expected Recover"),
        }
    }

    #[test]
    fn parse_recover_no_args_parses_ok() {
        // No args is valid at parse time; runtime validates
        let cli = Cli::try_parse_from(["ws-ckpt", "recover"]).unwrap();
        match cli.command {
            Commands::Recover {
                workspace,
                all,
                force,
            } => {
                assert!(workspace.is_none());
                assert!(!all);
                assert!(!force);
            }
            _ => panic!("expected Recover"),
        }
    }

    #[test]
    fn parse_recover_workspace_and_all_conflicts() {
        let result = Cli::try_parse_from(["ws-ckpt", "recover", "--workspace", "/ws", "--all"]);
        assert!(result.is_err(), "--workspace and --all should conflict");
    }

    #[test]
    fn parse_recover_workspace_force() {
        let cli = Cli::try_parse_from(["ws-ckpt", "recover", "-w", "/tmp/ws", "--force"]).unwrap();
        match cli.command {
            Commands::Recover {
                workspace,
                all,
                force,
            } => {
                assert_eq!(workspace.as_deref(), Some("/tmp/ws"));
                assert!(!all);
                assert!(force);
            }
            _ => panic!("expected Recover"),
        }
    }
}
