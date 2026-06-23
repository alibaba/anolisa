//! SkillFS CLI — AI agent skill management via virtual filesystem.

use std::path::PathBuf;
use std::sync::Arc;

fn cleanup_pid_file(pid_file: &Option<PathBuf>) {
    if let Some(p) = pid_file {
        match std::fs::remove_file(p) {
            Ok(()) => tracing::info!(path = %p.display(), "removed PID file"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(path = %p.display(), error = %e, "failed to remove PID file"),
        }
    }
}

use clap::{Parser, Subcommand};
use skillfs_core::store::SkillStore;
use skillfs_core::views::ViewsConfig;
use skillfs_core::{ParseConfig, SharedSkillStore};
use skillfs_fuse::security::{
    ActivationMode, ActivationReloadController, ActivationWatcher, ActiveSkillResolver,
    AuditRuntimeConfig, CliLedgerAdapter, ControlSocketConfig, ControlSocketContext,
    ControlSocketServer, DEFAULT_NOTIFY_DEBOUNCE_MS, DEFAULT_NOTIFY_TIMEOUT_MS,
    DEFAULT_RELOAD_INTERVAL_MS, DEFAULT_RELOAD_TIMEOUT_MS, DecisionCommand,
    InstallerStagingController, JsonlProtocolEventWriter, JsonlSecurityEventWriter, LedgerAdapter,
    LedgerBackingRoot, NoopProtocolEventWriter, NoopSecurityEventWriter, NotifyController,
    ProtocolEventWriter, RefreshController, ReloadMode, SecurityConfig, SecurityEventWriter,
    SecurityModeConfig, SourceDriftObserver, StagingMatcher, TrustedPeerConfig,
    TrustedWriterConfig, UnixSocketNotifyClient, bootstrap_activation, resolve_events_path,
    resolve_protocol_events_path, spawn_drift_watcher,
};
use skillfs_fuse::{FuseError as FuseErr, MountConfig, MountOptions, mount_configured};
use tokio::signal;
use tracing::{error, info, warn};

// ---------------------------------------------------------------------------
// CLI Arguments
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "skillfs")]
#[command(about = "AI agent skill management via virtual filesystem and MCP")]
#[command(version = env!("CARGO_PKG_VERSION"))]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Write log output to this file instead of stderr.
    /// The filename may contain `{pid}` which will be replaced with the
    /// process ID, e.g. `/tmp/skillfs-{pid}.log`.
    #[arg(long, value_name = "PATH", global = true)]
    log_file: Option<PathBuf>,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    /// Mount the SkillFS virtual filesystem
    Mount {
        /// Source directory containing skills
        #[arg(value_name = "SOURCE")]
        source: PathBuf,

        /// Mount point for the filesystem
        #[arg(value_name = "MOUNTPOINT")]
        mountpoint: PathBuf,

        /// Allow other users to access the mount
        #[arg(long)]
        allow_other: bool,

        /// Run in foreground (don't daemonize)
        #[arg(long)]
        foreground: bool,

        /// Write the process PID to this file after mount starts.
        /// Use `kill $(cat <file>)` or `kill -TERM $(cat <file>)` to unmount.
        #[arg(long, value_name = "PATH")]
        pid_file: Option<PathBuf>,

        /// Enable best-effort JSONL audit logging by writing one event per
        /// line to this file. The file is opened in append mode and created
        /// if missing. When omitted, audit logging is disabled (the default
        /// in-process sink drops every event).
        ///
        /// If the path cannot be opened, the mount fails before the FUSE
        /// session starts rather than silently downgrading to a no-op sink.
        #[arg(long, value_name = "PATH")]
        audit_log: Option<PathBuf>,

        /// Bounded queue capacity for the audit writer thread. `0` (the
        /// default) maps to the built-in default capacity. Only meaningful
        /// when `--audit-log` is also set.
        #[arg(long, value_name = "N", default_value_t = 0)]
        audit_queue_capacity: usize,

        /// Refuse to mount unless `SOURCE` and `MOUNTPOINT` resolve to the
        /// same directory (in-place / over-mount layout). In that layout
        /// FUSE intercepts every read and write to the physical source
        /// path, so `.skill-meta` policy and the audit log cover all
        /// userspace operations.
        ///
        /// Without this flag the existing non-in-place layout is allowed
        /// for compatibility, but it can only observe operations that go
        /// through the FUSE mountpoint — direct writes to the source path
        /// bypass SkillFS entirely.
        #[arg(long)]
        security_mode: bool,

        /// External decision-provider command prefix. Either a single
        /// binary like `/usr/local/bin/xxx-cli`, or a whitespace-split
        /// command prefix like `agent-sec-cli skill-ledger`. The first
        /// token is the executable; subsequent tokens are fixed
        /// arguments. SkillFS appends `scan <skill_dir> --json` and
        /// `resolve <skill_dir> --json` per call and spawns the
        /// program directly (no shell, no quoting). Required when
        /// `--security` is set; empty or whitespace-only values are
        /// rejected at startup.
        #[arg(long, value_name = "COMMAND")]
        decision_command: Option<String>,

        /// Enable the security pipeline. When set, the mount wires the
        /// active skill resolver and a debounced scan-then-resolve
        /// refresh controller against `--decision-command`. Without
        /// this flag the mount falls back to passthrough behavior.
        #[arg(long)]
        security: bool,

        /// Path to the security events JSONL output. Only meaningful
        /// with `--security`; an unopenable path is a startup error.
        #[arg(long, value_name = "PATH")]
        events_log: Option<PathBuf>,

        /// [DEPRECATED / compatibility] Trusted writer process-name
        /// gate. Matches the FUSE caller's process `comm` via
        /// `/proc/<tgid>/comm`. Process `comm` can be spoofed via
        /// `prctl(PR_SET_NAME)` or by exec'ing a same-basename
        /// binary; NOT production-strength. Use
        /// `--trusted-writer-exe` instead.
        #[arg(long, value_name = "NAME")]
        trusted_writer: Option<String>,

        /// [RECOMMENDED] Trusted writer executable identity gate.
        /// Matches the FUSE caller's `/proc/<tgid>/exe` readlink
        /// against the configured canonical path and on-disk file
        /// identity `(dev, ino)`. Resistant to process-name spoofing.
        /// Requires Linux. The path must exist and be a regular file.
        #[arg(long, value_name = "PATH")]
        trusted_writer_exe: Option<PathBuf>,

        /// Path to a TOML configuration file for security settings.
        /// CLI flags override values from the config file.
        #[arg(long, value_name = "PATH")]
        config: Option<PathBuf>,

        /// Activation file consumption mode. When set to `file`, SkillFS
        /// reads `<skill_dir>/.skill-meta/activation.json` at startup to
        /// populate the active-skill resolver. When set to `off` (the
        /// default), the existing `--decision-command` path is unchanged.
        /// Requires `--security`. Mutually exclusive with
        /// `--decision-command`.
        #[arg(long, value_name = "MODE")]
        activation_mode: Option<String>,

        /// Unix domain socket path for the N2 notify change client.
        /// When set, SkillFS sends `skill_ledger.skillfs_notify_change`
        /// notifications to the external daemon after debounced FUSE
        /// mutations. The daemon owns scan, reconcile, and activation
        /// refresh. Requires `--security --activation-mode file`.
        /// Mutually exclusive with `--decision-command`.
        #[arg(long, value_name = "PATH")]
        notify_socket: Option<PathBuf>,

        /// Path to the N3 activation protocol event log (JSONL).
        /// When set, SkillFS writes an append-only JSONL line for each
        /// debounced FUSE mutation that passes notify filtering. The
        /// log is a reconcile aid for the daemon; write failures only
        /// warn and never affect FUSE or the notify client.
        /// Requires `--security --activation-mode file`.
        /// Mutually exclusive with `--decision-command`.
        #[arg(long, value_name = "PATH")]
        activation_events_log: Option<PathBuf>,

        /// A3: Runtime activation reload mode. When set to `poll`,
        /// SkillFS re-reads activation.json / xattr after each debounced
        /// notify send and updates the active-skill resolver without
        /// requiring a remount. Default `off`.
        /// Requires `--security --activation-mode file`.
        #[arg(long, value_name = "MODE")]
        activation_reload_mode: Option<String>,

        /// A6/B1: Private source-side work path for the external security
        /// daemon. When set, all daemon-facing operations (notify skillDir,
        /// activation bootstrap, activation reload, startup reconcile,
        /// activation watcher) use this path instead of the source path.
        /// In in-place mounts, SkillFS creates a bind mount from the
        /// source to this path before the FUSE over-mount so the daemon
        /// can scan the live source tree. Fail-closed: unsafe backing
        /// root (world-writable, inside mount path, wrong owner) rejects
        /// startup.
        /// Requires `--security --activation-mode file`.
        /// Mutually exclusive with `--decision-command`.
        #[arg(long, value_name = "PATH")]
        ledger_backing_root: Option<PathBuf>,

        /// Unix domain socket path for the trusted peer control
        /// channel. When set, SkillFS creates a control socket at this
        /// path and accepts connections from trusted peers. Peer
        /// identity is verified via `SO_PEERCRED` + executable identity.
        /// Requires `--trusted-peer-exe`. Linux only.
        #[arg(long, value_name = "PATH")]
        control_socket: Option<PathBuf>,

        /// Trusted peer executable path for control socket
        /// authentication. The peer's `/proc/<pid>/exe` must match this
        /// canonical path and its on-disk `(dev, ino)` file identity.
        /// Requires `--control-socket`. The path must exist and be a
        /// regular file.
        #[arg(long, value_name = "PATH")]
        trusted_peer_exe: Option<PathBuf>,

        /// Optional trusted peer UID constraint for control
        /// socket authentication. When set, the peer's UID (from
        /// `SO_PEERCRED`) must match this value.
        #[arg(long, value_name = "UID")]
        trusted_peer_uid: Option<u32>,

        /// Optional trusted peer GID constraint for control
        /// socket authentication. When set, the peer's GID (from
        /// `SO_PEERCRED`) must match this value.
        #[arg(long, value_name = "GID")]
        trusted_peer_gid: Option<u32>,
    },

    /// Generate or update skillfs-views.toml from a skill directory
    Classify {
        /// Source directory containing skills
        #[arg(value_name = "SOURCE")]
        source: PathBuf,

        /// Number of skills to place in the primary (default) view
        #[arg(long, default_value = "6")]
        primary_count: usize,

        /// Preview only — do not write skillfs-views.toml
        #[arg(long)]
        dry_run: bool,
    },

    /// Validate skill files
    Validate {
        /// Source directory containing skills
        #[arg(value_name = "SOURCE")]
        source: PathBuf,

        /// Output format
        #[arg(short, long, value_enum, default_value = "text")]
        format: OutputFormat,
    },

    /// List all skills
    List {
        /// Source directory containing skills
        #[arg(value_name = "SOURCE")]
        source: PathBuf,

        /// Only show enabled skills
        #[arg(long)]
        enabled_only: bool,
    },
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let pid = std::process::id();
    let max_level = if cli.verbose {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };

    // Initialize logging — to a file if --log-file was given, otherwise stderr.
    if let Some(ref log_path_template) = cli.log_file {
        // Replace `{pid}` placeholder in the path.
        let log_path_str = log_path_template
            .to_string_lossy()
            .replace("{pid}", &pid.to_string());
        let log_path = PathBuf::from(&log_path_str);

        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            Ok(file) => {
                let subscriber = tracing_subscriber::fmt()
                    .with_max_level(max_level)
                    .with_ansi(false) // no ANSI colour codes in log files
                    .with_timer(tracing_subscriber::fmt::time::LocalTime::rfc_3339())
                    .with_writer(std::sync::Mutex::new(file))
                    .finish();
                let _ = tracing::subscriber::set_global_default(subscriber);
                // Can't use info!() yet — subscriber just set
                eprintln!("skillfs: logging to {}", log_path.display());
            }
            Err(e) => {
                // Fall back to stderr and warn.
                let subscriber = tracing_subscriber::fmt()
                    .with_max_level(max_level)
                    .with_timer(tracing_subscriber::fmt::time::LocalTime::rfc_3339())
                    .with_writer(std::io::stderr)
                    .finish();
                let _ = tracing::subscriber::set_global_default(subscriber);
                eprintln!(
                    "skillfs: failed to open log file '{}': {} — falling back to stderr",
                    log_path.display(),
                    e
                );
            }
        }
    } else {
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(max_level)
            .with_timer(tracing_subscriber::fmt::time::LocalTime::rfc_3339())
            .with_writer(std::io::stderr)
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
    }

    info!(pid, "starting skillfs CLI");

    if let Err(e) = run(cli).await {
        error!(error = %e, "command failed");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Commands::Mount {
            source,
            mountpoint,
            allow_other,
            foreground,
            pid_file,
            audit_log,
            audit_queue_capacity,
            security_mode,
            decision_command,
            security,
            events_log,
            trusted_writer,
            trusted_writer_exe,
            config,
            activation_mode,
            notify_socket,
            activation_events_log,
            activation_reload_mode,
            ledger_backing_root,
            control_socket,
            trusted_peer_exe,
            trusted_peer_uid,
            trusted_peer_gid,
        } => {
            cmd_mount(
                source,
                mountpoint,
                allow_other,
                foreground,
                pid_file,
                audit_log,
                audit_queue_capacity,
                security_mode,
                decision_command,
                security,
                events_log,
                trusted_writer,
                trusted_writer_exe,
                config,
                activation_mode,
                notify_socket,
                activation_events_log,
                activation_reload_mode,
                ledger_backing_root,
                control_socket,
                trusted_peer_exe,
                trusted_peer_uid,
                trusted_peer_gid,
            )
            .await
        }
        Commands::Classify {
            source,
            primary_count,
            dry_run,
        } => cmd_classify(source, primary_count, dry_run).await,
        Commands::Validate { source, format } => cmd_validate(source, format).await,
        Commands::List {
            source,
            enabled_only,
        } => cmd_list(source, enabled_only).await,
    }
}

// ---------------------------------------------------------------------------
// Mount Command
// ---------------------------------------------------------------------------

/// Debounce window for the runtime source drift watcher (Package W1).
///
/// Mirrors the value used in the existing `skillfs-core::watcher` integration
/// tests. Drift observation is best-effort, so a few-hundred-ms coalescing
/// window keeps audit volume reasonable without losing the signal that an
/// out-of-band change happened.
const DRIFT_DEBOUNCE_MS: u64 = 200;

#[allow(clippy::too_many_arguments)]
async fn cmd_mount(
    source: PathBuf,
    mountpoint: PathBuf,
    allow_other: bool,
    foreground: bool,
    pid_file: Option<PathBuf>,
    audit_log: Option<PathBuf>,
    audit_queue_capacity: usize,
    security_mode: bool,
    decision_command: Option<String>,
    security: bool,
    events_log: Option<PathBuf>,
    trusted_writer: Option<String>,
    trusted_writer_exe: Option<PathBuf>,
    config_path: Option<PathBuf>,
    activation_mode_raw: Option<String>,
    notify_socket: Option<PathBuf>,
    activation_events_log: Option<PathBuf>,
    activation_reload_mode_raw: Option<String>,
    ledger_backing_root: Option<PathBuf>,
    control_socket: Option<PathBuf>,
    trusted_peer_exe: Option<PathBuf>,
    trusted_peer_uid: Option<u32>,
    trusted_peer_gid: Option<u32>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!(source = %source.display(), mountpoint = %mountpoint.display(), security_mode, "mounting SkillFS");

    // Load TOML config if --config is set. CLI flags override config values.
    let file_config = match config_path {
        Some(ref p) => {
            let cfg = SecurityConfig::load(p)
                .map_err(|e| format!("failed to load config '{}': {e}", p.display()))?;
            info!(path = %p.display(), "loaded security config");
            Some(cfg)
        }
        None => None,
    };

    // Parse activation mode: CLI flag (if present) overrides config file.
    let activation_mode = match activation_mode_raw.as_deref() {
        Some(raw) => ActivationMode::parse(raw)
            .ok_or_else(|| format!("invalid --activation-mode '{raw}'; allowed: off, file"))?,
        None => file_config
            .as_ref()
            .map(|c| c.activation_mode())
            .unwrap_or_default(),
    };

    // Parse reload mode: CLI flag (if present) overrides config file.
    let reload_mode = match activation_reload_mode_raw.as_deref() {
        Some(raw) => ReloadMode::parse(raw).ok_or_else(|| {
            format!("invalid --activation-reload-mode '{raw}'; allowed: off, poll")
        })?,
        None => file_config
            .as_ref()
            .map(|c| c.reload_mode())
            .unwrap_or_default(),
    };
    let reload_interval_ms = file_config
        .as_ref()
        .and_then(|c| c.reload_interval_ms())
        .unwrap_or(DEFAULT_RELOAD_INTERVAL_MS);
    let reload_timeout_ms = file_config
        .as_ref()
        .and_then(|c| c.reload_timeout_ms())
        .unwrap_or(DEFAULT_RELOAD_TIMEOUT_MS);
    let watcher_interval_ms = file_config
        .as_ref()
        .and_then(|c| c.watcher_interval_ms())
        .unwrap_or(skillfs_fuse::security::DEFAULT_WATCHER_INTERVAL_MS);

    // Merge: CLI flag overrides config file value.
    let decision_command = decision_command.or_else(|| {
        file_config
            .as_ref()
            .and_then(|c| c.decision_command().map(String::from))
    });
    let events_log = events_log.or_else(|| {
        file_config
            .as_ref()
            .and_then(|c| c.events_log_path().map(PathBuf::from))
    });
    let trusted_writer = trusted_writer.or_else(|| {
        file_config
            .as_ref()
            .and_then(|c| c.trusted_writer_name().map(String::from))
    });
    let audit_log = audit_log.or_else(|| {
        file_config
            .as_ref()
            .and_then(|c| c.audit_log_path().map(PathBuf::from))
    });
    let audit_queue_capacity = if audit_queue_capacity != 0 {
        audit_queue_capacity
    } else {
        file_config
            .as_ref()
            .and_then(|c| c.audit_queue_capacity())
            .unwrap_or(0)
    };

    let parsed_decision_command: Option<DecisionCommand> = match decision_command.as_deref() {
        Some(raw) => {
            let cmd = DecisionCommand::parse(raw)
                .map_err(|e| format!("invalid --decision-command '{raw}': {e}"))?;
            Some(cmd)
        }
        None => None,
    };

    // Activation source validation:
    //   --security + --decision-command           => scan -> resolve path
    //   --security + --activation-mode file        => activation.json consumer
    //   --security + both                          => startup error (dual source)
    //   --activation-mode file without --security  => startup error
    //   --security without either source           => startup error
    if activation_mode == ActivationMode::File && !security {
        return Err("--activation-mode file requires --security".into());
    }
    if activation_mode == ActivationMode::File && parsed_decision_command.is_some() {
        return Err(
            "--activation-mode file and --decision-command are mutually exclusive \
             (activation.json and scan->resolve cannot both populate the resolver)"
                .into(),
        );
    }
    if security && activation_mode == ActivationMode::Off && parsed_decision_command.is_none() {
        return Err(
            "--security requires --decision-command <COMMAND> or --activation-mode file".into(),
        );
    }

    // Reload mode validation.
    if reload_mode == ReloadMode::Poll {
        if !security {
            return Err("--activation-reload-mode poll requires --security".into());
        }
        if activation_mode != ActivationMode::File {
            return Err("--activation-reload-mode poll requires --activation-mode file".into());
        }
    }

    // Merge notify socket: CLI flag overrides config file.
    let notify_socket = notify_socket.or_else(|| {
        file_config
            .as_ref()
            .and_then(|c| c.notify_socket_path().map(PathBuf::from))
    });
    let notify_timeout_ms = file_config
        .as_ref()
        .and_then(|c| c.notify_timeout_ms())
        .unwrap_or(DEFAULT_NOTIFY_TIMEOUT_MS);

    // --notify-socket startup validation.
    if let Some(ref p) = notify_socket {
        if p.as_os_str().is_empty() {
            return Err("--notify-socket path must not be empty".into());
        }
        if !security {
            return Err(format!("--notify-socket {} requires --security", p.display()).into());
        }
        if activation_mode != ActivationMode::File {
            return Err(format!(
                "--notify-socket {} requires --activation-mode file",
                p.display()
            )
            .into());
        }
        if parsed_decision_command.is_some() {
            return Err(
                "--notify-socket and --decision-command are mutually exclusive \
                 (notify is for the daemon-driven activation path; \
                 decision-command has its own scan->resolve refresh)"
                    .into(),
            );
        }
    }

    // Merge activation-events-log: CLI flag overrides config file.
    let activation_events_log = activation_events_log.or_else(|| {
        file_config
            .as_ref()
            .and_then(|c| c.activation_events_log_path().map(PathBuf::from))
    });

    // --activation-events-log startup validation.
    if let Some(ref p) = activation_events_log {
        if p.as_os_str().is_empty() {
            return Err("--activation-events-log path must not be empty".into());
        }
        if !security {
            return Err(format!(
                "--activation-events-log {} requires --security",
                p.display()
            )
            .into());
        }
        if activation_mode != ActivationMode::File {
            return Err(format!(
                "--activation-events-log {} requires --activation-mode file",
                p.display()
            )
            .into());
        }
        if parsed_decision_command.is_some() {
            return Err(
                "--activation-events-log and --decision-command are mutually exclusive \
                 (activation-events-log is for the daemon-driven activation path; \
                 decision-command has its own events-log)"
                    .into(),
            );
        }
        match resolve_protocol_events_path(p) {
            Ok(_) => {}
            Err(e) => {
                return Err(format!(
                    "invalid --activation-events-log path '{}': {}",
                    p.display(),
                    e
                )
                .into());
            }
        }
    }

    // P1 gate: reload=poll requires a notify trigger source. Without
    // --notify-socket or --activation-events-log the NotifyController is
    // never created, so FUSE mutations would never trigger the reload
    // poll — the operator would think reload is active while it is inert.
    if reload_mode == ReloadMode::Poll && notify_socket.is_none() && activation_events_log.is_none()
    {
        return Err("--activation-reload-mode poll requires --notify-socket or \
             --activation-events-log (without a notify trigger source, \
             reload would never fire)"
            .into());
    }

    // I2 gate: staging patterns require a notify source. Without
    // --notify-socket or --activation-events-log the NotifyController is
    // never created, so a staging rename would silently fail to emit the
    // mutation notification.
    let has_staging_patterns = file_config
        .as_ref()
        .and_then(|c| c.install.as_ref())
        .and_then(|i| i.staging_patterns.as_ref())
        .map(|p| !p.is_empty())
        .unwrap_or(false);
    if has_staging_patterns && notify_socket.is_none() && activation_events_log.is_none() {
        return Err("install.staging_patterns requires --notify-socket or \
             --activation-events-log (without a notify source, \
             mutation notifications cannot be delivered)"
            .into());
    }

    // A6/B1: Merge ledger backing root: CLI flag overrides config file.
    let ledger_backing_root = ledger_backing_root.or_else(|| {
        file_config
            .as_ref()
            .and_then(|c| c.ledger_backing_root().map(PathBuf::from))
    });

    // A6/B1: --ledger-backing-root startup validation.
    if let Some(ref p) = ledger_backing_root {
        if p.as_os_str().is_empty() {
            return Err("--ledger-backing-root path must not be empty".into());
        }
        if !security {
            return Err(
                format!("--ledger-backing-root {} requires --security", p.display()).into(),
            );
        }
        if activation_mode != ActivationMode::File {
            return Err(format!(
                "--ledger-backing-root {} requires --activation-mode file",
                p.display()
            )
            .into());
        }
        if parsed_decision_command.is_some() {
            return Err(
                "--ledger-backing-root and --decision-command are mutually exclusive \
                 (backing root is for the daemon-driven activation path)"
                    .into(),
            );
        }
    }

    if security || parsed_decision_command.is_some() || events_log.is_some() {
        info!(
            security,
            decision_command = ?parsed_decision_command.as_ref().map(|c| {
                let mut s = c.program().display().to_string();
                for a in c.fixed_args() {
                    s.push(' ');
                    s.push_str(a);
                }
                s
            }),
            events_log = ?events_log.as_ref().map(|p| p.display().to_string()),
            "security mode: active resolver + refresh controller enabled"
        );
    }

    // `--events-log` is only meaningful in security mode. Surface a clear
    // startup error otherwise so an operator typo cannot silently
    // discard events. Security mode + a path that cannot be resolved (e.g.
    // missing parent dir) is also a startup error: the mount must not
    // begin without the event sink the operator asked for.
    if let Some(ref p) = events_log {
        if !security {
            return Err(format!("--events-log {} requires --security", p.display()).into());
        }
        if activation_mode == ActivationMode::File {
            return Err(format!(
                "--events-log {} is not supported with --activation-mode file \
                     (events log requires --decision-command refresh; \
                     activation event logging is a later package)",
                p.display()
            )
            .into());
        }
        match resolve_events_path(p) {
            Ok(_) => {}
            Err(e) => {
                return Err(format!("invalid --events-log path '{}': {}", p.display(), e).into());
            }
        }
    }

    // Validate source directory
    if !source.exists() {
        return Err(format!("Source directory does not exist: {}", source.display()).into());
    }
    if !source.is_dir() {
        return Err(format!("Source is not a directory: {}", source.display()).into());
    }

    // Package M0 security-mode gate. Ordered intentionally as the first
    // startup gate after the source check (and before any audit setup or
    // mountpoint auto-creation): when `--security-mode` is set, refuse to
    // mount unless `source` and `mountpoint` canonicalize to the same
    // directory. This is the only configuration in which SkillFS can
    // intercept *every* read/write to the physical source path.
    //
    // Putting this first matches the runtime fixture (validate →
    // build_sink → mount) used by the M0 integration tests and avoids
    // leaving startup side-effects (audit log file, auto-created
    // mountpoint directory) behind when the gate rejects the mount. In
    // compat mode (`--security-mode` not set) `validate()` is a no-op, so
    // the existing auto-create-mountpoint UX below is unchanged.
    let security_config = SecurityModeConfig {
        enabled: security_mode,
    };
    security_config
        .validate(&source, &mountpoint)
        .map_err(|e| format!("{}", e))?;

    // Resolve the source canonical path once, up front. Several startup
    // gates need it: the W1 audit-path-vs-source check below, the
    // in-place detection further down, and the W1 drift watcher
    // (which must observe canonical source events). Falls back to the
    // user-supplied path on canonicalize failure so the existing CLI UX
    // is preserved for callers who hand us a relative path that already
    // resolves to a real directory.
    let source_canon = source.canonicalize().unwrap_or_else(|_| source.clone());

    // Build the runtime audit configuration. When `--audit-log` is omitted
    // the default `NoopEventSink` is preserved (Ok(None) below). When it is
    // present but the file cannot be opened, surface a startup error and
    // refuse to mount rather than silently downgrading — operators who ask
    // for audit logging must not be left without it.
    let audit_runtime = AuditRuntimeConfig {
        path: audit_log.clone(),
        queue_capacity: audit_queue_capacity,
    };
    // Package W1 safety gate. Refuse to start if `--audit-log` would land
    // inside the source tree: every audit write would either trigger the
    // drift watcher (creating a `source_changed` feedback loop on each
    // line) or land on top of an actual `<source>/<skill>/SKILL.md`,
    // corrupting the manifest SkillFS is meant to protect. The check is
    // ordered before `build_sink` so a rejected configuration never
    // creates the audit log file on disk. Disabled audit configs always
    // pass.
    audit_runtime
        .validate_audit_path_outside_source(&source_canon)
        .map_err(|e| format!("{}", e))?;
    // N3 source-tree guard: reject --activation-events-log inside source,
    // same rationale as audit. Ordered before the file is opened so a
    // rejected path never creates the log file on disk.
    if let Some(ref p) = activation_events_log {
        skillfs_fuse::security::validate_protocol_events_path_outside_source(p, &source_canon)
            .map_err(|e| format!("{}", e))?;
    }
    let audit_sink = audit_runtime.build_sink().map_err(|e| {
        format!(
            "failed to open audit log '{}': {}",
            audit_log
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            e
        )
    })?;
    if let Some(ref p) = audit_log {
        info!(
            path = %p.display(),
            queue_capacity = audit_runtime.effective_queue_capacity(),
            "audit logging enabled"
        );
    }

    // Validate mount point. Auto-create is intentionally still here, after
    // the security-mode gate: under `--security-mode` the mountpoint must
    // already equal the source (which was checked above), so this branch
    // only ever runs in compat mode where a fresh dedicated mountpoint is
    // the expected ergonomic.
    if !mountpoint.exists() {
        info!("creating mount point directory");
        std::fs::create_dir_all(&mountpoint)?;
    }
    if !mountpoint.is_dir() {
        return Err(format!("Mount point is not a directory: {}", mountpoint.display()).into());
    }

    // Compute mount_canon and in_place early so the A6/B1 backing root
    // setup can validate path shape before the FUSE over-mount.
    let mount_canon = mountpoint
        .canonicalize()
        .unwrap_or_else(|_| mountpoint.clone());
    let in_place = source_canon == mount_canon;

    // A6/B1: Ledger backing root setup.
    //
    // When the operator provides --ledger-backing-root, SkillFS creates a
    // private source alias (bind mount) before the FUSE over-mount becomes
    // active. All daemon-facing operations then use the backing root path.
    // Fail-closed: unsafe backing root rejects startup.
    let backing_root: Option<LedgerBackingRoot> = if let Some(ref br_path) = ledger_backing_root {
        let br = LedgerBackingRoot::setup(&source_canon, br_path, &mount_canon, in_place)
            .map_err(|e| format!("--ledger-backing-root setup failed: {e}"))?;
        info!(
            backing_root = %br.path().display(),
            in_place,
            "ledger backing root enabled — daemon-facing operations will use this path"
        );
        Some(br)
    } else {
        None
    };

    // In-place mount with daemon-facing operations requires a backing root.
    // Without it, daemon_root would fall back to source which becomes the
    // FUSE over-mount path — the daemon cannot scan through FUSE.
    let has_daemon_ops = notify_socket.is_some()
        || activation_events_log.is_some()
        || reload_mode == ReloadMode::Poll;
    if in_place
        && security
        && activation_mode == ActivationMode::File
        && has_daemon_ops
        && backing_root.is_none()
    {
        return Err(
            "in-place mount with activation/notify requires --ledger-backing-root \
             (the FUSE over-mount makes the source path inaccessible to the daemon)"
                .into(),
        );
    }

    // daemon_root: the path used for all daemon-facing operations.
    // When a backing root is set, use it; otherwise fall back to the source.
    let daemon_root: PathBuf = backing_root
        .as_ref()
        .map(|br| br.path().to_path_buf())
        .unwrap_or_else(|| source.clone());

    // Load skills into store
    info!("loading skills from source directory");
    let mut store = SkillStore::new();
    let config = ParseConfig::default();
    let errors = store.load_from_directory(&source, &config);

    if !errors.is_empty() {
        warn!(count = errors.len(), "some skills failed to load");
        for err in &errors {
            warn!(path = %err.path.display(), error = %err.error, "load error");
        }
    }

    info!(count = store.len(), "skills loaded");

    // Auto-assign any skills that are not yet in any view to the default view.
    if let Some(mut views) = ViewsConfig::load(&source) {
        let assigned = views.all_assigned_skills();
        let new_skills: Vec<String> = store
            .list()
            .iter()
            .filter(|name| !assigned.contains(**name))
            .map(|s| s.to_string())
            .collect();
        if !new_skills.is_empty() {
            info!(
                count = new_skills.len(),
                "auto-assigning new skills to default view"
            );
            if let Err(e) = views.assign_to_default(&source, &new_skills) {
                warn!(error = %e, "failed to save updated views config");
            }
        }
    }

    let shared_store: SharedSkillStore = Arc::new(parking_lot::RwLock::new(store));

    // D1.3.1 active-mapping bootstrap (read-only).
    //
    // Only fires when **both** `--security` and
    // `--decision-command` are set (both gates were checked up-front).
    // We build a fresh `ActiveSkillResolver` rooted at `source` and run
    // `scan` then `resolve` against the decision provider per skill;
    // the parsed resolve result is installed into the resolver.
    //
    // Behavior on individual failures is intentionally non-fatal:
    //  * scan failure: we skip resolve, leave the skill out of the
    //    resolver (read paths default to hidden (no activation)), and log.
    //  * resolve spawn / non-zero-exit / JSON parse errors log a
    //    warning and leave the skill out of the resolver.
    //  * a successful resolve whose `decision` cannot be installed
    //    (e.g. an empty source root) logs a warning and the skill
    //    stays hidden — same fallback as a failed resolve.
    //
    // D1.3.1 explicitly does **not** wire watcher hot sync, daemon
    // transport, or `check`/`certify`. Skill-discover is exempt from
    // the gate inside SkillFS itself, so we deliberately do not
    // run scan/resolve on it.
    let active_resolver: Option<Arc<ActiveSkillResolver>> =
        if security && activation_mode == ActivationMode::File {
            // A1: Activation File Consumer bootstrap.
            //
            // When `--activation-mode file` is set, SkillFS reads
            // `<skill_dir>/.skill-meta/activation.json` for every loaded
            // skill at startup and populates the resolver. Invalid or
            // missing activation files map to hidden (fail-safe).
            let resolver = ActiveSkillResolver::new(source.clone());
            let skill_names: Vec<String> = shared_store
                .read()
                .list()
                .iter()
                .filter(|n| **n != "skill-discover")
                .map(|s| s.to_string())
                .collect();
            info!(
                count = skill_names.len(),
                activation_mode = %activation_mode,
                "activation: loading activation files for skill mapping"
            );
            let results = bootstrap_activation(daemon_root.as_path(), &skill_names, &resolver);
            for (name, outcome) in &results {
                match outcome {
                    Ok(target) => {
                        info!(
                            skill = %name,
                            target = %target.as_label(),
                            "activation file loaded"
                        );
                    }
                    Err(e) => {
                        warn!(
                            skill = %name,
                            error = %e,
                            "activation file invalid or missing; skill hidden (fail-safe)"
                        );
                    }
                }
            }
            Some(Arc::new(resolver))
        } else if security && parsed_decision_command.is_some() {
            // D1.3.1 active-mapping bootstrap (decision-command path).
            let cmd = parsed_decision_command
                .as_ref()
                .expect("decision_command presence checked above")
                .clone();
            let adapter: Arc<dyn LedgerAdapter> = Arc::new(CliLedgerAdapter::new(cmd.clone()));
            let resolver = ActiveSkillResolver::new(source.clone());
            let skill_names: Vec<String> = shared_store
                .read()
                .list()
                .iter()
                .filter(|n| **n != "skill-discover")
                .map(|s| s.to_string())
                .collect();
            info!(
                count = skill_names.len(),
                program = %cmd.program().display(),
                "security: resolving active skill mapping via scan -> resolve"
            );
            for name in &skill_names {
                let skill_dir = source.join(name);
                if let Err(e) = adapter.scan(&skill_dir) {
                    warn!(
                        skill = %name,
                        error = %e,
                        "decision-command scan failed; skill will be hidden (no activation)"
                    );
                    continue;
                }
                match adapter.resolve(&skill_dir) {
                    Ok(result) => match resolver.set_from_resolve_for_expected(name, &result) {
                        Ok(target) => {
                            info!(
                                skill = %name,
                                target = %target.as_label(),
                                "decision-command resolve installed"
                            );
                        }
                        Err(e) => warn!(
                            skill = %name,
                            error = %e,
                            "could not install resolve target; skill will be hidden (no activation)"
                        ),
                    },
                    Err(e) => warn!(
                        skill = %name,
                        error = %e,
                        "decision-command resolve failed; skill will be hidden (no activation)"
                    ),
                }
            }
            Some(Arc::new(resolver))
        } else {
            None
        };

    // D1.3.1 refresh controller bootstrap.
    //
    // Only wired when `--security` and `--decision-command`
    // are both set AND we successfully built an active resolver above.
    // The controller takes the same adapter the read-path bootstrap
    // used and runs scan -> resolve on its own worker after a
    // per-skill debounce.
    //
    // `--events-log` selects the JSONL sink; its absence keeps the
    // [`NoopSecurityEventWriter`]. A `--events-log` path that cannot be
    // opened is a startup error: the operator asked for the security
    // event stream and we refuse to silently downgrade.
    let refresh_controller: Option<Arc<RefreshController>> = if security
        && active_resolver.is_some()
        && parsed_decision_command.is_some()
    {
        let cmd = parsed_decision_command
            .as_ref()
            .expect("decision_command presence checked above")
            .clone();
        let adapter: Arc<dyn LedgerAdapter> = Arc::new(CliLedgerAdapter::new(cmd));
        let resolver_for_ctrl = active_resolver
            .clone()
            .expect("active_resolver presence checked above");
        let event_writer: Arc<dyn SecurityEventWriter> = if let Some(p) = events_log.as_ref() {
            let writer = JsonlSecurityEventWriter::new(p, 0).map_err(|e| {
                format!("failed to open --events-log path '{}': {}", p.display(), e)
            })?;
            info!(path = %p.display(), "security events JSONL enabled");
            Arc::new(writer) as Arc<dyn SecurityEventWriter>
        } else {
            Arc::new(NoopSecurityEventWriter) as Arc<dyn SecurityEventWriter>
        };
        let failed_behavior = file_config
            .as_ref()
            .map(|c| c.failed_resolve_behavior())
            .unwrap_or_default();
        let ctrl = RefreshController::new(
            adapter,
            resolver_for_ctrl,
            event_writer,
            std::time::Duration::from_millis(skillfs_fuse::security::DEFAULT_REFRESH_DEBOUNCE_MS),
            failed_behavior,
        );
        info!("security: refresh controller wired (scan -> resolve)");
        Some(ctrl)
    } else {
        // Security mode without a decision-command cannot run scans /
        // resolves; the up-front gate already errored out. Without
        // security mode at all, nothing wires the controller and the
        // mount falls back to the pre-security behavior.
        None
    };

    // N2 notify controller bootstrap.
    //
    // Only wired when `--security --activation-mode file` and
    // `--notify-socket` are all set. The controller debounces per-skill
    // FUSE mutations and sends `skill_ledger.skillfs_notify_change` to the
    // daemon. Notify failure is diagnostic only and never changes the
    // active resolver.
    // N3 protocol event writer bootstrap.
    //
    // Built before the notify controller so it can be injected.
    // When `--activation-events-log` is set but the file cannot be
    // opened, the mount fails at startup.
    let protocol_event_writer: Arc<dyn ProtocolEventWriter> =
        if let Some(ref p) = activation_events_log {
            let writer = JsonlProtocolEventWriter::new(p, 0).map_err(|e| {
                format!(
                    "failed to open --activation-events-log path '{}': {}",
                    p.display(),
                    e
                )
            })?;
            info!(path = %p.display(), "activation protocol event log enabled");
            Arc::new(writer) as Arc<dyn ProtocolEventWriter>
        } else {
            Arc::new(NoopProtocolEventWriter) as Arc<dyn ProtocolEventWriter>
        };

    // A3: Activation reload controller bootstrap.
    //
    // Built before the notify controller so it can be injected.
    // Only constructed when --security --activation-mode file
    // --activation-reload-mode poll and an active resolver exists.
    let reload_controller: Option<Arc<ActivationReloadController>> =
        if reload_mode == ReloadMode::Poll && active_resolver.is_some() {
            let resolver_for_reload = active_resolver
                .clone()
                .expect("active_resolver presence checked above");
            let ctrl = Arc::new(ActivationReloadController::new(
                daemon_root.clone(),
                resolver_for_reload,
                std::time::Duration::from_millis(reload_interval_ms),
                std::time::Duration::from_millis(reload_timeout_ms),
            ));
            info!(
                reload_mode = %reload_mode,
                interval_ms = reload_interval_ms,
                timeout_ms = reload_timeout_ms,
                "activation reload controller enabled"
            );
            Some(ctrl)
        } else {
            None
        };

    let notify_controller: Option<Arc<NotifyController>> =
        if let Some(ref socket_path) = notify_socket {
            let client = Arc::new(UnixSocketNotifyClient::new(
                socket_path.clone(),
                std::time::Duration::from_millis(notify_timeout_ms),
            ));
            let source_for_notify = daemon_root.clone();
            let ctrl = if let Some(ref reload) = reload_controller {
                NotifyController::new_with_reload(
                    client,
                    source_for_notify,
                    std::time::Duration::from_millis(DEFAULT_NOTIFY_DEBOUNCE_MS),
                    notify_timeout_ms,
                    protocol_event_writer.clone(),
                    reload.clone(),
                )
            } else {
                NotifyController::new_with_protocol_writer(
                    client,
                    source_for_notify,
                    std::time::Duration::from_millis(DEFAULT_NOTIFY_DEBOUNCE_MS),
                    notify_timeout_ms,
                    protocol_event_writer.clone(),
                )
            };
            info!(
                socket = %socket_path.display(),
                timeout_ms = notify_timeout_ms,
                reload = reload_mode != ReloadMode::Off,
                "notify: change client enabled (Unix socket)"
            );
            Some(ctrl)
        } else if activation_events_log.is_some() {
            let client = Arc::new(skillfs_fuse::security::NoopNotifyClient);
            let ctrl = if let Some(ref reload) = reload_controller {
                NotifyController::new_with_reload(
                    client,
                    daemon_root.clone(),
                    std::time::Duration::from_millis(DEFAULT_NOTIFY_DEBOUNCE_MS),
                    DEFAULT_NOTIFY_TIMEOUT_MS,
                    protocol_event_writer.clone(),
                    reload.clone(),
                )
            } else {
                NotifyController::new_with_protocol_writer(
                    client,
                    daemon_root.clone(),
                    std::time::Duration::from_millis(DEFAULT_NOTIFY_DEBOUNCE_MS),
                    DEFAULT_NOTIFY_TIMEOUT_MS,
                    protocol_event_writer.clone(),
                )
            };
            info!("notify: protocol event log only (no socket)");
            Some(ctrl)
        } else {
            None
        };

    // Merge trusted-writer-exe: CLI flag overrides config file.
    let trusted_writer_exe = trusted_writer_exe.or_else(|| {
        file_config
            .as_ref()
            .and_then(|c| c.trusted_writer_exe().map(PathBuf::from))
    });

    // Trusted writer gate construction.
    let trusted_writer_config: Option<TrustedWriterConfig> =
        match (&trusted_writer, &trusted_writer_exe) {
            (_, Some(exe_path)) => {
                #[cfg(not(target_os = "linux"))]
                return Err("--trusted-writer-exe requires Linux (/proc/<pid>/exe)".into());

                #[cfg(target_os = "linux")]
                {
                    use skillfs_fuse::security::FileId;
                    use std::os::unix::fs::MetadataExt;

                    let canonical = std::fs::canonicalize(exe_path).map_err(|e| {
                        format!("--trusted-writer-exe '{}': {e}", exe_path.display())
                    })?;
                    let meta = std::fs::metadata(&canonical).map_err(|e| {
                        format!("--trusted-writer-exe '{}': {e}", canonical.display())
                    })?;
                    if !meta.is_file() {
                        return Err(format!(
                            "--trusted-writer-exe '{}': not a regular file",
                            canonical.display()
                        )
                        .into());
                    }
                    let file_id = FileId {
                        dev: meta.dev(),
                        ino: meta.ino(),
                    };
                    let cfg = match &trusted_writer {
                        Some(name) if !name.trim().is_empty() => {
                            TrustedWriterConfig::with_executable_and_compat_name(
                                canonical.clone(),
                                file_id,
                                name.clone(),
                            )
                        }
                        _ => TrustedWriterConfig::with_executable(canonical.clone(), file_id),
                    };
                    info!(
                        trusted_writer_exe = %canonical.display(),
                        "trusted writer enabled (executable identity)"
                    );
                    eprintln!();
                    eprintln!("  --trusted-writer-exe: executable identity pinned (production).");
                    eprintln!("   path = {}", canonical.display());
                    eprintln!("   file_id = ({file_id})");
                    if trusted_writer.is_some() {
                        eprintln!(
                            "   --trusted-writer is also set (compatibility/log context only)."
                        );
                        eprintln!("   Executable identity is the sole authorization basis.");
                    }
                    eprintln!();
                    Some(cfg)
                }
            }
            (Some(name), None) if !name.trim().is_empty() => {
                let cfg = TrustedWriterConfig::with_process_name(name.clone());
                info!(
                    trusted_writer = %name,
                    "trusted writer enabled (compat: TID-to-TGID comm match)"
                );
                eprintln!();
                eprintln!("⚠  --trusted-writer is a deprecated / compatibility gate (comm match).");
                eprintln!("   Process comm can be spoofed (prctl PR_SET_NAME, exec'd basename).");
                eprintln!("   Production: use --trusted-writer-exe <PATH> instead.");
                eprintln!();
                Some(cfg)
            }
            _ => None,
        };

    // ── Trusted peer control socket ────────────────────────────────
    //
    // Merge CLI flags with config file. CLI overrides config.
    let control_socket = control_socket.or_else(|| {
        file_config
            .as_ref()
            .and_then(|c| c.control_socket_path().map(PathBuf::from))
    });
    let trusted_peer_exe = trusted_peer_exe.or_else(|| {
        file_config
            .as_ref()
            .and_then(|c| c.control_socket_trusted_peer_exe().map(PathBuf::from))
    });
    let trusted_peer_uid = trusted_peer_uid.or_else(|| {
        file_config
            .as_ref()
            .and_then(|c| c.control_socket_trusted_peer_uid())
    });
    let trusted_peer_gid = trusted_peer_gid.or_else(|| {
        file_config
            .as_ref()
            .and_then(|c| c.control_socket_trusted_peer_gid())
    });

    // Startup validation: --control-socket requires --trusted-peer-exe
    // and vice versa.
    match (&control_socket, &trusted_peer_exe) {
        (Some(p), None) => {
            return Err(format!(
                "--control-socket {} requires --trusted-peer-exe",
                p.display()
            )
            .into());
        }
        (None, Some(p)) => {
            return Err(format!(
                "--trusted-peer-exe {} requires --control-socket",
                p.display()
            )
            .into());
        }
        _ => {}
    }

    // Build ControlSocketConfig when both are set.
    let control_socket_config: Option<ControlSocketConfig> =
        match (&control_socket, &trusted_peer_exe) {
            (Some(socket_path), Some(exe_path)) => {
                #[cfg(not(target_os = "linux"))]
                return Err(
                    "--control-socket requires Linux (SO_PEERCRED, /proc/<pid>/exe)".into(),
                );

                #[cfg(target_os = "linux")]
                {
                    use skillfs_fuse::security::FileId;
                    use std::os::unix::fs::MetadataExt;

                    let canonical = std::fs::canonicalize(exe_path)
                        .map_err(|e| format!("--trusted-peer-exe '{}': {e}", exe_path.display()))?;
                    let meta = std::fs::metadata(&canonical).map_err(|e| {
                        format!("--trusted-peer-exe '{}': {e}", canonical.display())
                    })?;
                    if !meta.is_file() {
                        return Err(format!(
                            "--trusted-peer-exe '{}': not a regular file",
                            canonical.display()
                        )
                        .into());
                    }
                    let file_id = FileId {
                        dev: meta.dev(),
                        ino: meta.ino(),
                    };

                    info!(
                        control_socket = %socket_path.display(),
                        trusted_peer_exe = %canonical.display(),
                        trusted_peer_file_id = %file_id,
                        "control socket enabled"
                    );

                    Some(ControlSocketConfig {
                        socket_path: socket_path.clone(),
                        trusted_peer: TrustedPeerConfig {
                            exe_path: canonical,
                            exe_file_id: file_id,
                            uid: trusted_peer_uid,
                            gid: trusted_peer_gid,
                        },
                    })
                }
            }
            _ => None,
        };

    // Mount options
    let options = MountOptions {
        allow_other,
        foreground,
        fuse_options: vec!["noatime".to_string()],
    };

    info!("starting FUSE filesystem (blocking)");

    // mount_canon and in_place were computed earlier, before the A6/B1
    // backing root setup.
    let drift_enabled = audit_sink.is_some();
    if in_place {
        info!("in-place mount detected: FUSE will over-mount the source directory");
        eprintln!();
        eprintln!(
            "⚠  In-place mount: '{}' will be READ-ONLY while SkillFS is running.",
            source.display()
        );
        eprintln!("   To install, update, or remove skills, you MUST unmount first:");
        eprintln!("     fusermount3 -u '{}'", mountpoint.display());
        eprintln!("   or send SIGTERM / press Ctrl+C to stop this process.");
        if security_mode {
            eprintln!();
            eprintln!("   --security-mode is enabled: SkillFS audit and policy now cover");
            eprintln!(
                "   every read/write to '{}' that goes through userspace.",
                source.display()
            );
            if drift_enabled {
                eprintln!(
                    "   --audit-log is also enabled: best-effort source drift observation is"
                );
                eprintln!("   active, surfacing out-of-band create/modify/delete of");
                eprintln!("   <source>/<skill>/SKILL.md and immediate skill directories as");
                eprintln!(
                    "   `source_changed` audit lines (visibility-only, no real-time blocking)."
                );
            }
        }
        eprintln!();
    } else {
        // Non-in-place / "compatibility" mount. SkillFS still serves the
        // virtual skill view at '{mountpoint}/skills/...', but the physical
        // source directory remains directly writable outside FUSE. Be
        // explicit so an operator who relies on .skill-meta protection or
        // the audit log knows where the boundary actually is.
        warn!(
            source = %source.display(),
            mountpoint = %mountpoint.display(),
            "non-in-place mount: SkillFS policy/audit only cover the FUSE mountpoint"
        );
        eprintln!();
        eprintln!("⚠  Non-in-place (compatibility / dev) mount:");
        eprintln!("     source     = '{}'", source.display());
        eprintln!("     mountpoint = '{}'", mountpoint.display());
        eprintln!("   • Direct writes to the source path are NOT routed through SkillFS,");
        eprintln!("     so '.skill-meta' protection and the audit log only cover");
        eprintln!(
            "     operations that go through '{}'.",
            mountpoint.display()
        );
        if drift_enabled {
            eprintln!("   • Source drift observation is enabled (Package W1, best-effort):");
            eprintln!("     out-of-band create/modify/delete of <source>/<skill>/SKILL.md and");
            eprintln!("     immediate skill directories surface as `source_changed` audit lines.");
            eprintln!("     Arbitrary files inside skills, '.skill-meta/**', and nested layouts");
            eprintln!("     are NOT observed; SkillFS does not block in real time.");
        } else {
            eprintln!("   • Source drift observation is OFF (no --audit-log): out-of-band");
            eprintln!("     changes to the source path are not observed at all. Re-run with");
            eprintln!("     --audit-log <PATH> to record SKILL.md / skill-dir drift.");
        }
        eprintln!("   • You can add or remove skill directories at the source at any");
        eprintln!("     time; the change is picked up on the next mount.");
        eprintln!("   For a mount that enforces SkillFS policy on every read/write,");
        eprintln!("   re-run with '--security-mode' and source == mountpoint.");
        eprintln!();
    }

    // Write PID file so the process can be managed without shell job control.
    // e.g. `kill -TERM $(cat /tmp/skillfs.pid)` to unmount cleanly.
    if let Some(ref pid_path) = pid_file {
        let pid = std::process::id();
        std::fs::write(pid_path, format!("{}\n", pid))
            .map_err(|e| format!("failed to write pid file '{}': {}", pid_path.display(), e))?;
        info!(path = %pid_path.display(), pid, "wrote PID file");
    }

    // Capture the mountpoint for signal-triggered cleanup.
    let mountpoint_for_signal = mountpoint.clone();
    let pid_file_for_signal = pid_file.clone();

    // Package W1 source drift observer wiring. Best-effort and visibility-only.
    //
    // When the operator opts in to audit logging via `--audit-log`, attach
    // the existing `skillfs-core` watcher to the same audit sink so
    // out-of-band source-tree changes (especially direct writes to the
    // physical source path in compat mode, and writes through pre-mount
    // file descriptors in security mode) surface as `SourceChanged` JSONL
    // records. Without `--audit-log` this whole block is skipped, so the
    // pre-W1 default behavior is preserved exactly: no watcher is spawned,
    // no drift observation runs, no extra threads are started.
    //
    // Failures are non-fatal. Drift observation is a visibility aid: a
    // failed watcher startup must not abort the FUSE mount that an
    // operator already asked for. We log a warning and continue with the
    // sink-only audit pipeline that S2.1 delivered.
    let drift_handle = if let Some(ref sink) = audit_sink {
        let observer = Arc::new(SourceDriftObserver::new(source_canon.clone(), sink.clone()));
        match spawn_drift_watcher(source_canon.clone(), observer, DRIFT_DEBOUNCE_MS).await {
            Ok(handle) => {
                info!(
                    source = %source_canon.display(),
                    debounce_ms = DRIFT_DEBOUNCE_MS,
                    "source drift observation enabled"
                );
                Some(handle)
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "failed to start source drift watcher; continuing without drift observation"
                );
                None
            }
        }
    } else {
        None
    };

    // A5: Activation state watcher. Background convergence loop that
    // periodically checks activation freshness and reloads when the
    // daemon writes new activation. Independent of FUSE event loop.
    let activation_watcher: Option<Arc<ActivationWatcher>> =
        if reload_mode == ReloadMode::Poll && reload_controller.is_some() {
            let reload_for_watcher = reload_controller
                .clone()
                .expect("reload_controller presence checked above");
            let watcher = Arc::new(ActivationWatcher::new(
                reload_for_watcher,
                protocol_event_writer.clone(),
                std::time::Duration::from_millis(watcher_interval_ms),
            ));
            info!(
                watcher_interval_ms = watcher_interval_ms,
                "activation watcher enabled (continuous convergence)"
            );
            Some(watcher)
        } else {
            None
        };

    // A5: inject watcher registrar into notify controller so new skills
    // observed through FUSE mutations are automatically tracked.
    if let (Some(watcher), Some(ctrl)) = (&activation_watcher, &notify_controller) {
        ctrl.set_watcher_registrar(watcher.clone());
    }

    // A4: capture reconcile inputs before notify_controller is moved into
    // MountConfig. Reconcile fires once after mount startup when
    // --security --activation-mode file and a notify controller exists.
    let reconcile_notify = notify_controller.clone();
    let reconcile_skill_names: Option<Vec<String>> =
        if activation_mode == ActivationMode::File && notify_controller.is_some() {
            let names: Vec<String> = shared_store
                .read()
                .list()
                .iter()
                .filter(|n| **n != "skill-discover")
                .map(|s| s.to_string())
                .collect();
            Some(names)
        } else {
            None
        };

    // I2: build staging matcher and controller from [install] config.
    let staging_matcher: Option<Arc<StagingMatcher>> = file_config
        .as_ref()
        .and_then(|c| c.staging_config())
        .map(|cfg| {
            info!(
                patterns = cfg.patterns.len(),
                "staging: installer staging compatibility enabled"
            );
            Arc::new(StagingMatcher::new(cfg))
        });
    let staging_controller: Option<Arc<InstallerStagingController>> =
        match (&staging_matcher, &notify_controller) {
            (Some(matcher), Some(notify_ctrl)) => Some(InstallerStagingController::new(
                matcher.clone(),
                notify_ctrl.clone(),
            )),
            _ => None,
        };

    let quiet_timeout_controller = match &notify_controller {
        Some(notify_ctrl) => file_config
            .as_ref()
            .and_then(|c| c.quiet_timeout_ms())
            .map(|ms| {
                info!(
                    quiet_timeout_ms = ms,
                    "install: quiet-timeout mutation notify enabled"
                );
                skillfs_fuse::security::QuietTimeoutController::new(
                    notify_ctrl.clone(),
                    std::time::Duration::from_millis(ms),
                )
            }),
        None => {
            if file_config
                .as_ref()
                .and_then(|c| c.quiet_timeout_ms())
                .is_some()
            {
                warn!(
                    "install: quiet_timeout_ms configured but no notify controller; \
                     quiet timeout disabled"
                );
            }
            None
        }
    };

    // I4: Build post-publish grace controller from [install] config.
    // Must be built before PendingInstallController so we can inject it.
    let post_publish_controller = match (
        file_config.as_ref().and_then(|c| c.post_publish_grace_ms()),
        file_config
            .as_ref()
            .and_then(|c| c.post_publish_write_patterns()),
    ) {
        (Some(ms), Some(patterns)) => {
            let parsed = skillfs_fuse::security::validate_post_publish_patterns(patterns)
                .map_err(|e| format!("invalid install.post_publish_write_patterns: {e}"))?;
            info!(
                post_publish_grace_ms = ms,
                patterns = parsed.len(),
                "install: post-publish grace window enabled"
            );
            Some(skillfs_fuse::security::PostPublishGraceController::new(
                std::time::Duration::from_millis(ms),
                parsed,
            ))
        }
        _ => None,
    };

    let pending_install_controller = match (&notify_controller, &active_resolver) {
        (Some(notify_ctrl), Some(_)) => file_config
            .as_ref()
            .and_then(|c| c.quiet_timeout_ms())
            .map(|ms| {
                info!(
                    pending_timeout_ms = ms,
                    "install: direct final-skill pending install enabled"
                );
                skillfs_fuse::security::PendingInstallController::new_with_post_publish(
                    notify_ctrl.clone(),
                    std::time::Duration::from_millis(ms),
                    daemon_root.clone(),
                    post_publish_controller.clone(),
                )
            }),
        _ => None,
    };

    // Start control socket server before the FUSE mount.
    let control_socket_handle = if let Some(cs_config) = control_socket_config {
        let ctx = ControlSocketContext {
            source_root: daemon_root.clone(),
            resolver: active_resolver.clone(),
            protocol_event_writer: Some(protocol_event_writer.clone()),
        };
        let server = ControlSocketServer::new(cs_config).with_context(ctx);
        let handle = server
            .start()
            .map_err(|e| format!("failed to start control socket server: {e}"))?;
        info!(
            socket = %handle.socket_path().display(),
            "control socket server started"
        );
        Some(handle)
    } else {
        None
    };

    // mount_configured() blocks until the FUSE session exits (Ctrl+C or
    // SIGTERM). We wrap it in spawn_blocking and race against OS signals
    // so that SIGTERM triggers the same clean unmount path as Ctrl+C.
    //
    // A6/B1: `backing_root` stays in this scope and is dropped when
    // cmd_mount returns. The Drop impl calls cleanup(), which unmounts
    // the bind mount and removes the temp dir. The bind mount is
    // independent of the FUSE mount, so cleanup order does not matter.
    let mount_task = tokio::task::spawn_blocking(move || {
        mount_configured(
            &mountpoint,
            &source,
            shared_store,
            options,
            in_place,
            MountConfig {
                event_sink: audit_sink,
                policy: None,
                active_resolver,
                refresh_controller,
                notify_controller,
                trusted_writer: trusted_writer_config,
                staging_matcher,
                staging_controller,
                quiet_timeout_controller,
                pending_install_controller,
                post_publish_controller,
            },
        )
    });

    // A5: start activation watcher after mount is spawned.
    if let Some(ref watcher) = activation_watcher {
        if let Some(ref names) = reconcile_skill_names {
            watcher.register_skills(names);
        }
        watcher.start();
    }

    // A4: fire startup reconcile after mount is spawned. Runs on a
    // background thread so daemon socket latency cannot block startup.
    // A5: after reconcile, schedule an immediate watcher check so
    // daemon-written activation is picked up without waiting for the
    // periodic interval.
    if let (Some(ctrl), Some(names)) = (reconcile_notify, &reconcile_skill_names) {
        let names_owned = names.clone();
        let watcher_for_reconcile = activation_watcher.clone();
        ctrl.spawn_startup_reconcile(names_owned.clone());
        if let Some(ref w) = watcher_for_reconcile {
            w.schedule_immediate_check(names_owned);
        }
    }

    /// Trigger a clean FUSE unmount by calling fusermount3 -u.
    /// This causes fuser::mount2 event loop to exit, which unblocks the
    /// spawn_blocking thread and allows the process to exit cleanly.
    fn trigger_unmount(mountpoint: &std::path::Path) {
        let _ = std::process::Command::new("fusermount3")
            .args(["-u", &mountpoint.to_string_lossy()])
            .output();
    }

    let result = tokio::select! {
        res = mount_task => {
            match res {
                Ok(inner) => inner,
                Err(e) => Err(FuseErr::MountFailed(e.to_string())),
            }
        }
        _ = signal::ctrl_c() => {
            info!("received Ctrl+C, unmounting");
            trigger_unmount(&mountpoint_for_signal);
            if let Some(h) = drift_handle {
                h.shutdown().await;
            }
            if let Some(h) = control_socket_handle {
                h.shutdown();
            }
            cleanup_pid_file(&pid_file_for_signal);
            return Ok(());
        }
        _ = async {
            let mut term = signal::unix::signal(signal::unix::SignalKind::terminate())
                .expect("failed to register SIGTERM handler");
            term.recv().await
        } => {
            info!("received SIGTERM, unmounting");
            trigger_unmount(&mountpoint_for_signal);
            if let Some(h) = drift_handle {
                h.shutdown().await;
            }
            if let Some(h) = control_socket_handle {
                h.shutdown();
            }
            cleanup_pid_file(&pid_file_for_signal);
            return Ok(());
        }
    };

    // Mount exited on its own (FUSE event loop returned). Make sure the
    // drift watcher does not outlive the mount it was paired with: shut
    // it down explicitly so the underlying notify watcher and the drift
    // adapter task are torn down deterministically before this function
    // returns.
    if let Some(h) = drift_handle {
        h.shutdown().await;
    }
    // Shut down control socket server deterministically.
    if let Some(h) = control_socket_handle {
        h.shutdown();
    }

    cleanup_pid_file(&pid_file);

    match result {
        Ok(()) => {
            info!("filesystem unmounted successfully");
            Ok(())
        }
        Err(e) => Err(format!("Mount failed: {}", e).into()),
    }
}

// ---------------------------------------------------------------------------
// Classify Command
// ---------------------------------------------------------------------------

async fn cmd_classify(
    source: PathBuf,
    primary_count: usize,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    info!(source = %source.display(), "classifying skills");

    if !source.exists() {
        return Err(format!("Source directory does not exist: {}", source.display()).into());
    }
    if !source.is_dir() {
        return Err(format!("Source is not a directory: {}", source.display()).into());
    }

    let mut store = SkillStore::new();
    let config = ParseConfig::default();
    let _errors = store.load_from_directory(&source, &config);

    let mut all_names: Vec<String> = store.list().iter().map(|s| s.to_string()).collect();
    all_names.sort();

    if all_names.is_empty() {
        println!("No skills found in {}", source.display());
        return Ok(());
    }

    // If a views config already exists, report its status instead of overwriting.
    if let Some(existing) = ViewsConfig::load(&source) {
        println!("skillfs-views.toml already exists in {}", source.display());
        println!();
        for view in &existing.views {
            let marker = if view.default { " [default]" } else { "" };
            println!("View: {}{}", view.name, marker);
            if !view.description.is_empty() {
                println!("  Description: {}", view.description);
            }
            println!("  Skills ({}):", view.skills.len());
            for s in &view.skills {
                println!("    - {}", s);
            }
            println!();
        }
        let assigned = existing.all_assigned_skills();
        let unassigned: Vec<&String> = all_names
            .iter()
            .filter(|n| !assigned.contains(*n))
            .collect();
        if !unassigned.is_empty() {
            println!("Unassigned skills (will be added to default view on next mount):");
            for s in &unassigned {
                println!("  - {}", s);
            }
        }
        return Ok(());
    }

    // Generate a fresh config: first N skills in "major" (default), rest in "other".
    let n = primary_count.min(all_names.len());
    let primary: Vec<String> = all_names[..n].to_vec();
    let secondary: Vec<String> = all_names[n..].to_vec();

    let cfg = ViewsConfig {
        views: vec![
            skillfs_core::views::ViewConfig {
                name: "major".to_string(),
                default: true,
                description: "Core skills shown at mount time".to_string(),
                skills: primary.clone(),
            },
            skillfs_core::views::ViewConfig {
                name: "other".to_string(),
                default: false,
                description: "Additional skills accessible via skill-discover".to_string(),
                skills: secondary.clone(),
            },
        ],
    };

    if dry_run {
        println!(
            "[dry-run] Would write skillfs-views.toml to {}",
            source.display()
        );
        println!();
        println!("Primary view 'major' ({} skills):", primary.len());
        for s in &primary {
            println!("  - {}", s);
        }
        println!();
        println!("Secondary view 'other' ({} skills):", secondary.len());
        for s in &secondary {
            println!("  - {}", s);
        }
    } else {
        cfg.save(&source)?;
        println!("Written skillfs-views.toml to {}", source.display());
        println!();
        println!("Primary view 'major' ({} skills):", primary.len());
        for s in &primary {
            println!("  - {}", s);
        }
        println!();
        println!("Secondary view 'other' ({} skills):", secondary.len());
        for s in &secondary {
            println!("  - {}", s);
        }
        println!();
        println!("Edit skillfs-views.toml to move skills between views as needed.");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Validate Command
// ---------------------------------------------------------------------------

async fn cmd_validate(
    source: PathBuf,
    format: OutputFormat,
) -> Result<(), Box<dyn std::error::Error>> {
    info!(source = %source.display(), "validating skills");

    // Validate source directory
    if !source.exists() {
        return Err(format!("Source directory does not exist: {}", source.display()).into());
    }
    if !source.is_dir() {
        return Err(format!("Source is not a directory: {}", source.display()).into());
    }

    // Load skills
    let mut store = SkillStore::new();
    let config = ParseConfig::default();
    let errors = store.load_from_directory(&source, &config);

    // Output results
    match format {
        OutputFormat::Text => {
            println!("Validated {} skills from {}", store.len(), source.display());

            if errors.is_empty() {
                println!("✓ All skills loaded successfully");
            } else {
                println!("✗ {} skills failed to load:", errors.len());
                for err in &errors {
                    println!("  - {}: {}", err.path.display(), err.error);
                }
            }

            // Show skill summary
            if !store.is_empty() {
                println!("\nSkills:");
                let names = store.list();
                for name in names {
                    if let Some(entry) = store.get(name) {
                        let status = match &entry.parse_status {
                            skillfs_core::ParseStatus::Ok => "✓",
                            skillfs_core::ParseStatus::Degraded(_) => "⚠",
                            skillfs_core::ParseStatus::Error(_) => "✗",
                        };
                        println!(
                            "  {} {} - {} ({})",
                            status,
                            name,
                            entry
                                .metadata
                                .description
                                .chars()
                                .take(50)
                                .collect::<String>(),
                            if entry.metadata.enabled {
                                "enabled"
                            } else {
                                "disabled"
                            }
                        );
                    }
                }
            }
        }
        OutputFormat::Json => {
            let result = serde_json::json!({
                "total": store.len() + errors.len(),
                "success": store.len(),
                "failed": errors.len(),
                "errors": errors.iter().map(|e| {
                    serde_json::json!({
                        "path": e.path.to_string_lossy().to_string(),
                        "error": e.error
                    })
                }).collect::<Vec<_>>(),
                "skills": store.list().iter().map(|name| {
                    if let Some(entry) = store.get(name) {
                        serde_json::json!({
                            "name": name,
                            "description": entry.metadata.description,
                            "enabled": entry.metadata.enabled,
                            "status": format!("{:?}", entry.parse_status).to_lowercase()
                        })
                    } else {
                        serde_json::json!({})
                    }
                }).collect::<Vec<_>>()
            });
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }

    // Exit with error code if there were failures
    if !errors.is_empty() {
        std::process::exit(1);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// List Command
// ---------------------------------------------------------------------------

async fn cmd_list(source: PathBuf, enabled_only: bool) -> Result<(), Box<dyn std::error::Error>> {
    info!(source = %source.display(), "listing skills");

    // Validate source directory
    if !source.exists() {
        return Err(format!("Source directory does not exist: {}", source.display()).into());
    }
    if !source.is_dir() {
        return Err(format!("Source is not a directory: {}", source.display()).into());
    }

    // Load skills
    let mut store = SkillStore::new();
    let config = ParseConfig::default();
    let _errors = store.load_from_directory(&source, &config);

    let names = store.list();

    if names.is_empty() {
        println!("No skills found in {}", source.display());
        return Ok(());
    }

    println!("Skills in {}:", source.display());
    println!();

    for name in names {
        if let Some(entry) = store.get(name) {
            if enabled_only && !entry.metadata.enabled {
                continue;
            }

            let status_icon = match &entry.parse_status {
                skillfs_core::ParseStatus::Ok => "✓",
                skillfs_core::ParseStatus::Degraded(_) => "⚠",
                skillfs_core::ParseStatus::Error(_) => "✗",
            };

            println!("{} {}", status_icon, name);
            println!("  Description: {}", entry.metadata.description);
            println!("  Version: {}", entry.metadata.version);
            println!(
                "  Tags: {}",
                if entry.metadata.tags.is_empty() {
                    "(none)".to_string()
                } else {
                    entry.metadata.tags.join(", ")
                }
            );
            println!(
                "  Status: {} | {}",
                format!("{:?}", entry.parse_status).to_lowercase(),
                if entry.metadata.enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            );
            println!();
        }
    }

    Ok(())
}
