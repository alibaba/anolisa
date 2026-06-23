//! SkillFS CLI — AI agent skill management via virtual filesystem.

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use skillfs_core::store::SkillStore;
use skillfs_core::views::ViewsConfig;
use skillfs_core::{ParseConfig, SharedSkillStore};
use skillfs_fuse::security::{
    AuditRuntimeConfig, SecurityModeConfig, SourceDriftObserver, spawn_drift_watcher,
};
use skillfs_fuse::{FuseError as FuseErr, MountOptions, mount_with_security};
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
) -> Result<(), Box<dyn std::error::Error>> {
    info!(source = %source.display(), mountpoint = %mountpoint.display(), security_mode, "mounting SkillFS");

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

    // Mount options
    let options = MountOptions {
        allow_other,
        foreground,
        fuse_options: vec!["noatime".to_string()],
    };

    info!("starting FUSE filesystem (blocking)");

    // Detect in-place mount: when source and mountpoint resolve to the same path.
    // `source_canon` is computed once at the top of cmd_mount so the W1
    // audit-path-vs-source guard can use it before any audit sink is built.
    let mount_canon = mountpoint
        .canonicalize()
        .unwrap_or_else(|_| mountpoint.clone());
    let in_place = source_canon == mount_canon;
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

    // Note: mount_with_security() blocks until the FUSE session exits
    // (Ctrl+C or SIGTERM). We wrap it in spawn_blocking and race against
    // OS signals so that SIGTERM triggers the same clean unmount path as
    // Ctrl+C. When `audit_sink` is `None` the FUSE filesystem keeps its
    // default `NoopEventSink`, matching the prior `mount(...)` behavior.
    let mount_task = tokio::task::spawn_blocking(move || {
        mount_with_security(
            &mountpoint,
            &source,
            shared_store,
            options,
            in_place,
            audit_sink,
            None,
        )
    });

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
            // Explicit, deterministic shutdown of both the core watcher
            // and the drift adapter. Drop fallback (best-effort abort)
            // would still work, but signal handlers are exactly the
            // path long-lived embedders need to be predictable.
            if let Some(h) = drift_handle {
                h.shutdown().await;
            }
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

    match result {
        Ok(()) => {
            info!("filesystem unmounted successfully");
            // Remove PID file on clean exit.
            if let Some(ref pid_path) = pid_file {
                let _ = std::fs::remove_file(pid_path);
            }
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
