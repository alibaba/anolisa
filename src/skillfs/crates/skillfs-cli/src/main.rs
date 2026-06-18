//! SkillFS CLI — AI agent skill management via virtual filesystem.

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use skillfs_core::store::SkillStore;
use skillfs_core::views::ViewsConfig;
use skillfs_core::{ParseConfig, SharedSkillStore};
use skillfs_fuse::{FuseError as FuseErr, MountOptions, mount};
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
        } => cmd_mount(source, mountpoint, allow_other, foreground, pid_file).await,
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

async fn cmd_mount(
    source: PathBuf,
    mountpoint: PathBuf,
    allow_other: bool,
    foreground: bool,
    pid_file: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!(source = %source.display(), mountpoint = %mountpoint.display(), "mounting SkillFS");

    // Validate source directory
    if !source.exists() {
        return Err(format!("Source directory does not exist: {}", source.display()).into());
    }
    if !source.is_dir() {
        return Err(format!("Source is not a directory: {}", source.display()).into());
    }

    // Validate mount point
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
    let source_canon = source.canonicalize().unwrap_or_else(|_| source.clone());
    let mount_canon = mountpoint
        .canonicalize()
        .unwrap_or_else(|_| mountpoint.clone());
    let in_place = source_canon == mount_canon;
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
        eprintln!();
    } else {
        eprintln!();
        eprintln!(
            "ℹ  Source directory '{}' is NOT affected by the FUSE mount.",
            source.display()
        );
        eprintln!("   You can add or remove skill directories there at any time;");
        eprintln!("   changes are picked up automatically on the next mount.");
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

    // Note: mount() blocks until the FUSE session exits (Ctrl+C or SIGTERM).
    // We wrap it in spawn_blocking and race against OS signals so that
    // SIGTERM triggers the same clean unmount path as Ctrl+C.
    let mount_task = tokio::task::spawn_blocking(move || {
        mount(&mountpoint, &source, shared_store, options, in_place)
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
            return Ok(());
        }
        _ = async {
            // CLI startup: a failure here is unrecoverable (libc-level signal-handler limit).
            let mut term = signal::unix::signal(signal::unix::SignalKind::terminate())
                .expect("failed to register SIGTERM handler");
            term.recv().await
        } => {
            info!("received SIGTERM, unmounting");
            trigger_unmount(&mountpoint_for_signal);
            return Ok(());
        }
    };

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
