//! `anolisa adapter` sub-surface: scan, install, and remove adapters.
//!
//! Adapters bridge ANOLISA-managed components into agent frameworks
//! (e.g. `tokenless/openclaw`). The adapter state is tracked in
//! `installed.toml` as [`ObjectKind::Adapter`] objects.
//!
//! ## `adapter scan`
//!
//! Read-only probe of every `[[adapters]]` entry across the catalog,
//! reporting which frameworks are detected on the host.
//!
//! ## `adapter install <component> <framework>`
//!
//! Resolves the manifest adapter, downloads the component artifact from
//! the distribution index, extracts files per the adapter source/dest
//! mapping, writes state and central log. On failure after partial file
//! copy, installed files are cleaned up so no phantom state remains.
//!
//! ## `adapter remove <component> <framework>`
//!
//! Safe file deletion with four-layer guard:
//!
//! 1. **Owner check** — only `FileOwner::Anolisa` files are removed.
//! 2. **Path boundary** — [`validate_owned_path`] rejects escapes.
//! 3. **Symlink guard** — refuses to follow symlinks.
//! 4. **Directory guard** — refuses to `remove_file` a directory.

use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{SecondsFormat, Utc};
use clap::{Parser, Subcommand};
use serde::Serialize;

use anolisa_core::adapter::{detect_framework, expand_layout_placeholders};
use anolisa_core::central_log::{CentralLog, LogKind, LogRecord, LogStatus, Severity};
use anolisa_core::distribution::ArtifactType;
use anolisa_core::download::DownloadCache;
use anolisa_core::install_runner::{InstallRunner, ResolvedInstallFile};
use anolisa_core::lock::InstallLock;
use anolisa_core::path_safety::validate_owned_path;
use anolisa_core::state::{
    FileOwner, InstallMode as StateInstallMode, InstalledObject, ObjectKind, ObjectStatus,
    OperationRecord, OwnedFile,
};
use anolisa_core::{DistributionIndex, ResolveQuery};

use crate::color::Palette;
use crate::commands::common;
use crate::context::CliContext;
use crate::response::{CliError, render_json};

/// CLI arguments for the `adapter` sub-surface.
#[derive(Parser)]
pub struct AdapterArgs {
    /// Adapter subcommand.
    #[command(subcommand)]
    pub command: AdapterCommands,
}

/// Subcommands under `anolisa adapter`.
#[derive(Subcommand)]
pub enum AdapterCommands {
    /// List registered adapters.
    List,
    /// Install an adapter for a component into a framework.
    Install {
        /// Component name (e.g., tokenless).
        component: String,
        /// Target framework (e.g., openclaw, hermes).
        framework: String,
    },
    /// Remove an installed adapter.
    Remove {
        /// Component name (e.g., tokenless).
        component: String,
        /// Target framework (e.g., openclaw, hermes).
        framework: String,
        /// Also remove adapter-specific configuration and state (not yet implemented).
        #[arg(long)]
        purge: bool,
    },
    /// Auto-detect available adapter integrations.
    Scan,
}

// ---------------------------------------------------------------------------
// JSON payloads
// ---------------------------------------------------------------------------

/// One entry in the adapter scan result.
#[derive(Debug, Clone, Serialize)]
struct ScanEntry {
    component: String,
    framework: String,
    detected: bool,
    reason: String,
}

/// Top-level scan output.
#[derive(Serialize)]
struct ScanResult {
    adapters: Vec<ScanEntry>,
}

/// Dry-run plan for adapter install.
#[derive(Serialize)]
struct InstallPlan {
    component: String,
    framework: String,
    source: Option<String>,
    dest: String,
    detected: bool,
    detect_reason: String,
    /// Framework CLI registration command that real execution would run
    /// after extracting the plugin (e.g. `openclaw plugins install …`).
    /// `None` for frameworks wired in without a CLI step.
    #[serde(skip_serializing_if = "Option::is_none")]
    register_command: Option<String>,
}

/// JSON output for successful adapter install.
#[derive(Serialize)]
struct InstallResult {
    component: String,
    framework: String,
    adapter: String,
    version: String,
    operation_id: String,
    files_installed: Vec<String>,
}

/// JSON output for adapter remove (both dry-run and real execution).
#[derive(Serialize)]
struct RemoveResult {
    adapter: String,
    files_removed: Vec<String>,
    files_skipped: Vec<SkippedFile>,
    dry_run: bool,
}

/// A file that was skipped during removal with an explanation.
#[derive(Serialize)]
struct SkippedFile {
    path: String,
    reason: String,
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Handle the `anolisa adapter <subcommand>` dispatch.
pub fn handle(args: AdapterArgs, ctx: &CliContext) -> Result<(), CliError> {
    match args.command {
        AdapterCommands::Scan => handle_scan(ctx),
        AdapterCommands::Install {
            component,
            framework,
        } => handle_install(ctx, &component, &framework),
        AdapterCommands::Remove {
            component,
            framework,
            purge,
        } => handle_remove(&component, &framework, purge, ctx),
        AdapterCommands::List => Err(CliError::not_implemented("adapter list")),
    }
}

// ---------------------------------------------------------------------------
// adapter scan
// ---------------------------------------------------------------------------

/// Read-only scan of all adapter entries in the catalog, probing the host
/// for each framework.
fn handle_scan(ctx: &CliContext) -> Result<(), CliError> {
    let catalog = common::load_bundled_catalog(ctx, "adapter scan")?;

    let mut entries: Vec<ScanEntry> = Vec::new();
    for comp in catalog.list_components() {
        if comp.adapters.is_empty() {
            continue;
        }
        for adapter in &comp.adapters {
            let framework = adapter
                .framework
                .as_deref()
                .unwrap_or("unknown")
                .to_string();
            let result = detect_framework(adapter);
            entries.push(ScanEntry {
                component: comp.component.name.clone(),
                framework,
                detected: result.detected,
                reason: result.reason,
            });
        }
    }

    if ctx.json {
        return render_json("adapter scan", ScanResult { adapters: entries });
    }

    println!(
        "{:<16} {:<16} {:<12} REASON",
        "COMPONENT", "FRAMEWORK", "DETECTED"
    );
    for e in &entries {
        println!(
            "{:<16} {:<16} {:<12} {}",
            e.component, e.framework, e.detected, e.reason
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// adapter install
// ---------------------------------------------------------------------------

/// Install an adapter for `component` into `framework`.
///
/// The adapter spec is read from the **published** artifact's embedded
/// `.anolisa/component.toml` (not the dev-tree catalog), so `source`/`dest`
/// and the version come from what was actually shipped. After the plugin
/// directory is extracted into the ANOLISA datadir, frameworks that register
/// through their own CLI (e.g. OpenClaw) are wired in by invoking that CLI —
/// a bare file copy is not loaded by OpenClaw.
fn handle_install(ctx: &CliContext, component: &str, framework: &str) -> Result<(), CliError> {
    let command = format!("adapter install {component} {framework}");
    let layout = common::resolve_layout(ctx);

    // Resolve artifact from the distribution index. Version-agnostic: pick
    // the highest semver published for this component/platform — the
    // published artifact, not the dev-tree catalog, is authoritative.
    let dist_index =
        common::load_distribution_index(ctx, &command)?.ok_or_else(|| CliError::Runtime {
            command: command.clone(),
            reason: "no distribution index available — cannot resolve artifact".to_string(),
        })?;

    let entry = resolve_adapter_artifact(&dist_index, component, None, ctx, &command)?;

    // Adapter install only supports tar_gz — we must read the embedded
    // manifest and map a source directory into a directory-typed dest.
    if entry.artifact_type != ArtifactType::TarGz {
        return Err(CliError::Runtime {
            command,
            reason: format!(
                "adapter install requires a tar_gz artifact, got '{}'",
                artifact_type_wire(&entry.artifact_type)
            ),
        });
    }

    let sha256 = entry.sha256.as_deref().ok_or_else(|| CliError::Runtime {
        command: command.clone(),
        reason: format!(
            "distribution entry for '{component}' has no sha256 — refusing to install unverified artifact"
        ),
    })?;

    // Download artifact to cache. Done before the dry-run branch too, so the
    // displayed source/dest come from the real embedded manifest.
    let cache = DownloadCache::new(layout.cache_dir.clone());
    let downloaded = cache
        .fetch(&entry.url, Some(sha256))
        .map_err(|err| CliError::Runtime {
            command: command.clone(),
            reason: format!("failed to download artifact: {err}"),
        })?;

    // Read the published install contract embedded in the artifact and take
    // the adapter spec from there (source = the in-tar path, version = the
    // shipped version).
    let manifest =
        anolisa_core::install_runner::read_embedded_component_manifest(&downloaded.cached_path)
            .map_err(|err| CliError::Runtime {
                command: command.clone(),
                reason: format!("failed to read embedded component manifest: {err}"),
            })?
            .ok_or_else(|| CliError::Runtime {
                command: command.clone(),
                reason: format!(
                    "published artifact for '{component}' has no embedded .anolisa/component.toml — cannot resolve adapters"
                ),
            })?;

    let adapter = manifest
        .adapters
        .iter()
        .find(|a| a.framework.as_deref() == Some(framework))
        .ok_or_else(|| CliError::InvalidArgument {
            command: command.clone(),
            reason: format!(
                "no adapter for framework '{framework}' in published component '{component}'"
            ),
        })?;

    let version = manifest.component.version.clone();
    let adapter_name = format!("{component}/{framework}");

    let dest_template = adapter.dest.as_deref().unwrap_or_default();
    let expanded_dest =
        expand_layout_placeholders(dest_template, &layout, &[("component", component)]).map_err(
            |err| CliError::InvalidArgument {
                command: command.clone(),
                reason: format!("failed to expand adapter dest: {err}"),
            },
        )?;

    validate_owned_path(&layout, &expanded_dest).map_err(|err| CliError::InvalidArgument {
        command: command.clone(),
        reason: format!(
            "adapter destination '{}' failed path safety check: {err}",
            expanded_dest.display()
        ),
    })?;

    let detect_result = detect_framework(adapter);

    // A framework that registers via its own CLI cannot be wired in when the
    // framework is absent — fail fast before extracting anything.
    if framework_needs_cli(framework) && !detect_result.detected {
        return Err(CliError::Runtime {
            command,
            reason: format!(
                "framework '{framework}' not detected ({}) — cannot register the adapter via its CLI",
                detect_result.reason
            ),
        });
    }
    if !detect_result.detected && !ctx.quiet {
        eprintln!(
            "warning: framework '{framework}' not detected on this host: {}",
            detect_result.reason
        );
    }

    let register_invocation = openclaw_install_for(framework, &expanded_dest);

    if ctx.dry_run {
        let plan = InstallPlan {
            component: component.to_string(),
            framework: framework.to_string(),
            source: adapter.source.clone(),
            dest: expanded_dest.display().to_string(),
            detected: detect_result.detected,
            detect_reason: detect_result.reason.clone(),
            register_command: register_invocation.as_ref().map(|i| i.display_command()),
        };

        if ctx.json {
            return render_json(&command, plan);
        }

        println!("adapter install plan (dry-run):");
        println!("  component:     {}", plan.component);
        println!("  framework:     {}", plan.framework);
        println!(
            "  source:        {}",
            plan.source.as_deref().unwrap_or("<none>")
        );
        println!("  dest:          {}", plan.dest);
        println!("  detected:      {}", plan.detected);
        println!("  detect_reason: {}", plan.detect_reason);
        if let Some(cmd) = &plan.register_command {
            println!("  register:      {cmd}");
        }
        return Ok(());
    }

    // -- Real execution: extract, register via framework CLI, write state/log --

    let started_at = now_iso8601();

    // Construct source→dest file mapping.
    let files = vec![ResolvedInstallFile {
        source: adapter.source.clone(),
        dest: expanded_dest.clone(),
        mode: None,
    }];

    // Acquire lock, then load state inside the lock so a concurrent writer
    // cannot be overwritten and so state-load failures happen before file copy.
    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.clone(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;

    let mut state = load_state_for_install(ctx, &command)?;

    // Generate operation_id after lock acquisition with nanosecond precision
    // to avoid collisions between concurrent processes.
    let lock_ts = Utc::now();
    let operation_id = format!(
        "op-adapter-install-{}-{}",
        lock_ts.format("%Y%m%d%H%M%S"),
        lock_ts.timestamp_subsec_nanos()
    );

    // Execute file copy.
    let runner = InstallRunner::new(&layout);
    let outcome = runner
        .install_files("tar_gz", &downloaded.cached_path, &files)
        .map_err(|err| CliError::Runtime {
            command: command.clone(),
            reason: format!("install failed: {err}"),
        })?;

    // From this point, files are on disk — any failure must roll them back.

    // Register the adapter into its framework via the framework's own CLI
    // (OpenClaw loads plugins from its registry, not from a dropped dir).
    if let Some(inv) = &register_invocation {
        if !ctx.quiet {
            println!(
                "registering adapter into {framework}: {}",
                inv.display_command()
            );
        }
        if let Err(err) = run_openclaw(inv, ctx.quiet) {
            rollback_installed_files(&outcome.files);
            return Err(CliError::Runtime {
                command,
                reason: format!(
                    "extracted adapter files but {framework} CLI registration failed (rolled back): {}",
                    err.message()
                ),
            });
        }
    }

    let owned_files: Vec<OwnedFile> = outcome
        .files
        .iter()
        .map(|f| OwnedFile {
            path: f.path.clone(),
            owner: FileOwner::Anolisa,
            sha256: Some(f.sha256.clone()),
        })
        .collect();

    let installed_file_paths: Vec<String> = outcome
        .files
        .iter()
        .map(|f| f.path.display().to_string())
        .collect();

    let obj = InstalledObject {
        kind: ObjectKind::Adapter,
        name: adapter_name.clone(),
        version: version.clone(),
        status: ObjectStatus::Installed,
        manifest_digest: None,
        distribution_source: Some(entry.url.clone()),
        installed_at: started_at.clone(),
        last_operation_id: Some(operation_id.clone()),
        managed: true,
        adopted: false,
        subscription_scope: Default::default(),
        enabled_features: Vec::new(),
        component_refs: vec![component.to_string()],
        files: owned_files,
        external_modified_files: Vec::new(),
        services: Vec::new(),
        health: Vec::new(),
    };

    state.install_mode = match ctx.install_mode {
        crate::context::InstallMode::System => StateInstallMode::System,
        crate::context::InstallMode::User => StateInstallMode::User,
    };
    state.prefix = layout.prefix.clone();
    state.upsert_object(obj);
    state.operations.push(OperationRecord {
        id: operation_id.clone(),
        command: command.clone(),
        status: "ok".to_string(),
        started_at: started_at.clone(),
        finished_at: Some(now_iso8601()),
    });

    let state_path = layout.state_dir.join("installed.toml");
    if let Err(err) = state.save(&state_path) {
        rollback_installed_files(&outcome.files);
        return Err(CliError::Runtime {
            command,
            reason: format!(
                "failed to save state; attempted best-effort rollback of installed files (some may remain on disk): {err}"
            ),
        });
    }

    // Central log.
    let log = CentralLog::open(layout.central_log.clone());
    let record = LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.clone()),
        command: format!("adapter install {adapter_name}"),
        source: "anolisa-cli".to_string(),
        component: Some(component.to_string()),
        severity: Severity::Info,
        message: format!("adapter {adapter_name} installed"),
        actor: "cli".to_string(),
        install_mode: Some(ctx.install_mode.as_str().to_string()),
        started_at: started_at.clone(),
        finished_at: Some(now_iso8601()),
        status: Some(LogStatus::Ok),
        objects: vec![adapter_name.clone()],
        backup_ids: Vec::new(),
        warnings: Vec::new(),
        details: serde_json::Value::Null,
    };
    if let Err(err) = log.append(&record) {
        eprintln!("warning: failed to write central log: {err}");
    }

    // Output.
    if ctx.json {
        return render_json(
            &command,
            InstallResult {
                component: component.to_string(),
                framework: framework.to_string(),
                adapter: adapter_name,
                version,
                operation_id,
                files_installed: installed_file_paths,
            },
        );
    }

    if !ctx.quiet {
        let color = Palette::new(ctx.no_color);
        println!(
            "{} {} {}",
            color.command("adapter install"),
            adapter_name,
            color.ok("succeeded")
        );
        println!(
            "{} {}",
            color.label("operation_id:"),
            color.id(&operation_id)
        );
        println!(
            "{} {}",
            color.label("files installed:"),
            installed_file_paths.len()
        );
        for p in &installed_file_paths {
            println!("  - {}", color.path(p));
        }
    }

    Ok(())
}

/// Resolve the artifact entry for a component from the distribution index.
///
/// `version = None` lets the index pick the highest published semver for the
/// host platform — adapter install pins to whatever was actually shipped
/// rather than a version read from the dev-tree catalog.
fn resolve_adapter_artifact(
    dist_index: &DistributionIndex,
    component: &str,
    version: Option<&str>,
    ctx: &CliContext,
    command: &str,
) -> Result<anolisa_core::DistributionEntry, CliError> {
    let env = anolisa_env::EnvService::detect();
    let preferred = [ArtifactType::TarGz];
    let query = ResolveQuery {
        component,
        version,
        channel: None,
        install_mode: ctx.install_mode.as_str(),
        os: &env.os,
        arch: &env.arch,
        libc: env.libc.as_deref(),
        pkg_base: env.pkg_base.as_deref(),
        preferred_types: &preferred,
    };
    dist_index.resolve(&query).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to resolve artifact for '{component}': {err}"),
    })
}

// ---------------------------------------------------------------------------
// Framework CLI registration (OpenClaw)
// ---------------------------------------------------------------------------

/// True when `framework` is wired in by invoking its own CLI rather than by a
/// bare file copy. OpenClaw loads plugins from its managed registry
/// (`openclaw plugins install`), so a dropped directory alone is not loaded.
fn framework_needs_cli(framework: &str) -> bool {
    framework == "openclaw"
}

/// A resolved OpenClaw CLI invocation. Built purely (the only env read is
/// `OPENCLAW_BIN`) so the argv/env contract — mirroring
/// `openclaw/scripts/install.sh` — can be unit-tested without spawning.
struct OpenclawInvocation {
    /// Executable to run (`OPENCLAW_BIN` override, else `openclaw`).
    program: String,
    /// Arguments, e.g. `plugins install <dest> --force …`.
    args: Vec<String>,
    /// `OPENCLAW_STATE_DIR` value set on the child; `OPENCLAW_HOME` is unset.
    state_dir: PathBuf,
    /// Directories prepended to `PATH` before spawning.
    path_prepend: Vec<PathBuf>,
}

impl OpenclawInvocation {
    /// Human-readable form for dry-run/preview output.
    fn display_command(&self) -> String {
        let mut s = format!(
            "OPENCLAW_STATE_DIR={} {}",
            self.state_dir.display(),
            self.program
        );
        for a in &self.args {
            s.push(' ');
            s.push_str(a);
        }
        s
    }
}

/// Why an OpenClaw CLI invocation failed.
enum OpenclawError {
    /// The binary was not found on `PATH`.
    NotFound,
    /// The process failed to spawn for another reason.
    Spawn(String),
    /// The process ran but exited non-zero.
    Exit(String),
}

impl OpenclawError {
    fn message(&self) -> String {
        match self {
            OpenclawError::NotFound => "openclaw CLI not found on PATH".to_string(),
            OpenclawError::Spawn(s) => s.clone(),
            OpenclawError::Exit(s) => s.clone(),
        }
    }
}

/// `OPENCLAW_BIN` override, else `openclaw`.
fn openclaw_bin() -> String {
    std::env::var("OPENCLAW_BIN")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "openclaw".to_string())
}

/// Resolve the OpenClaw home (also the state dir): `OPENCLAW_HOME`, else
/// `$HOME/.openclaw`. Trailing slashes are trimmed to match the script.
fn openclaw_home() -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("OPENCLAW_HOME") {
        let s = h.to_string_lossy();
        let trimmed = s.trim_end_matches('/');
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".openclaw"))
}

/// PATH prefix dirs, mirroring `install.sh`:
/// `$HOME/.local/bin`, `<state_dir>/bin`, `/usr/local/bin`.
fn openclaw_path_prepend(home: &Path, user_home: Option<&Path>) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(uh) = user_home {
        v.push(uh.join(".local/bin"));
    }
    v.push(home.join("bin"));
    v.push(PathBuf::from("/usr/local/bin"));
    v
}

/// Build the `openclaw plugins install <dest> …` invocation. Pure.
fn build_openclaw_install(
    program: &str,
    dest: &Path,
    home: &Path,
    user_home: Option<&Path>,
) -> OpenclawInvocation {
    OpenclawInvocation {
        program: program.to_string(),
        args: vec![
            "plugins".to_string(),
            "install".to_string(),
            dest.display().to_string(),
            "--force".to_string(),
            "--dangerously-force-unsafe-install".to_string(),
        ],
        state_dir: home.to_path_buf(),
        path_prepend: openclaw_path_prepend(home, user_home),
    }
}

/// Build the `openclaw plugins uninstall <plugin_id> --force` invocation. Pure.
///
/// `--force` skips OpenClaw's interactive `[y/N]` confirmation — required
/// because anolisa drives the CLI non-interactively (no TTY). Mirrors the
/// official `openclaw/scripts/uninstall.sh` contract.
fn build_openclaw_uninstall(
    program: &str,
    plugin_id: &str,
    home: &Path,
    user_home: Option<&Path>,
) -> OpenclawInvocation {
    OpenclawInvocation {
        program: program.to_string(),
        args: vec![
            "plugins".to_string(),
            "uninstall".to_string(),
            plugin_id.to_string(),
            "--force".to_string(),
        ],
        state_dir: home.to_path_buf(),
        path_prepend: openclaw_path_prepend(home, user_home),
    }
}

/// The install invocation for `framework`, or `None` when not CLI-registered.
fn openclaw_install_for(framework: &str, dest: &Path) -> Option<OpenclawInvocation> {
    if framework != "openclaw" {
        return None;
    }
    let home = openclaw_home()?;
    let user_home = std::env::var_os("HOME").map(PathBuf::from);
    Some(build_openclaw_install(
        &openclaw_bin(),
        dest,
        &home,
        user_home.as_deref(),
    ))
}

/// The uninstall invocation for `framework`, or `None` when not CLI-registered.
fn openclaw_uninstall_for(framework: &str, plugin_id: &str) -> Option<OpenclawInvocation> {
    if framework != "openclaw" {
        return None;
    }
    let home = openclaw_home()?;
    let user_home = std::env::var_os("HOME").map(PathBuf::from);
    Some(build_openclaw_uninstall(
        &openclaw_bin(),
        plugin_id,
        &home,
        user_home.as_deref(),
    ))
}

/// Spawn an OpenClaw CLI invocation, replicating the env contract from
/// `openclaw/scripts/install.sh` (unset `OPENCLAW_HOME`, set
/// `OPENCLAW_STATE_DIR`, prepend PATH) without executing the script itself.
fn run_openclaw(inv: &OpenclawInvocation, quiet: bool) -> Result<(), OpenclawError> {
    let mut cmd = Command::new(&inv.program);
    cmd.args(&inv.args);
    cmd.env_remove("OPENCLAW_HOME");
    cmd.env("OPENCLAW_STATE_DIR", &inv.state_dir);

    let existing = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = inv.path_prepend.clone();
    paths.extend(std::env::split_paths(&existing));
    if let Ok(joined) = std::env::join_paths(&paths) {
        cmd.env("PATH", joined);
    }
    if quiet {
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
    }

    match cmd.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(OpenclawError::Exit(format!(
            "'{}' exited with {status}",
            inv.program
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(OpenclawError::NotFound),
        Err(e) => Err(OpenclawError::Spawn(format!(
            "failed to run '{}': {e}",
            inv.program
        ))),
    }
}

/// Load installed state, mapping errors to CliError::Runtime. Called inside
/// the lock and before file copy so a corrupted state file doesn't leave
/// orphan adapter files on disk.
fn load_state_for_install(
    ctx: &CliContext,
    command: &str,
) -> Result<anolisa_core::InstalledState, CliError> {
    common::load_installed_state(ctx, command).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to load installed state: {err}"),
    })
}

/// Wire-form artifact type string for the install runner.
fn artifact_type_wire(t: &ArtifactType) -> &'static str {
    match t {
        ArtifactType::TarGz => "tar_gz",
        ArtifactType::Binary => "binary",
        ArtifactType::Rpm => "rpm",
        ArtifactType::Deb => "deb",
        ArtifactType::Zip => "zip",
        ArtifactType::Oci => "oci",
        ArtifactType::File => "file",
    }
}

/// Best-effort cleanup of installed files after a state-save failure.
fn rollback_installed_files(files: &[anolisa_core::InstalledFile]) {
    for f in files {
        let _ = std::fs::remove_file(&f.path);
    }
}

// ---------------------------------------------------------------------------
// adapter remove
// ---------------------------------------------------------------------------

/// Handle `adapter remove <component> <framework>`.
fn handle_remove(
    component: &str,
    framework: &str,
    purge: bool,
    ctx: &CliContext,
) -> Result<(), CliError> {
    let command_str = format!("adapter remove {component} {framework}");
    if purge {
        return Err(CliError::not_implemented_with_hint(
            "adapter remove --purge",
            "adapter remove --purge is not yet implemented; omit --purge to remove the adapter files",
        ));
    }

    let adapter_name = format!("{component}/{framework}");
    let started_at = now_iso8601();
    let layout = common::resolve_layout(ctx);
    let state_path = layout.state_dir.join("installed.toml");

    // Dry-run: unlocked read-only preview.
    if ctx.dry_run {
        let state = common::load_installed_state(ctx, &command_str)?;
        let adapter_obj = state
            .find_object(ObjectKind::Adapter, &adapter_name)
            .ok_or_else(|| CliError::InvalidArgument {
                command: command_str.clone(),
                reason: format!("adapter '{adapter_name}' is not installed"),
            })?;

        let (would_remove, would_skip) = classify_files(&adapter_obj.files, &layout);

        if ctx.json {
            return render_json(
                &command_str,
                RemoveResult {
                    adapter: adapter_name,
                    files_removed: would_remove,
                    files_skipped: would_skip,
                    dry_run: true,
                },
            );
        }
        if !ctx.quiet {
            let color = Palette::new(ctx.no_color);
            println!(
                "{} {} {}",
                color.command("adapter remove"),
                adapter_name,
                color.muted("(dry-run)")
            );
            if let Some(inv) = openclaw_uninstall_for(framework, component) {
                println!("{} {}", color.label("would run:"), inv.display_command());
            }
            if !would_remove.is_empty() {
                println!("{}", color.label("would remove:"));
                for p in &would_remove {
                    println!("  - {}", color.path(p));
                }
            }
            if !would_skip.is_empty() {
                println!("{}", color.warn("would skip:"));
                for s in &would_skip {
                    println!("  - {} ({})", color.path(&s.path), s.reason);
                }
            }
            if would_remove.is_empty() && would_skip.is_empty() {
                println!("  {}", color.muted("(no files recorded)"));
            }
        }
        return Ok(());
    }

    // Real execution: lock first, then re-load state inside the lock so a
    // concurrent writer cannot be overwritten.
    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command_str.clone(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;

    let mut state = common::load_installed_state(ctx, &command_str)?;
    let adapter_obj = state
        .find_object(ObjectKind::Adapter, &adapter_name)
        .ok_or_else(|| CliError::InvalidArgument {
            command: command_str.clone(),
            reason: format!("adapter '{adapter_name}' is not installed"),
        })?
        .clone();

    // Symmetric framework de-registration: unwire from the framework's CLI
    // before deleting the local copy. A missing framework binary means there
    // is nothing to unregister (not fatal); a real uninstall failure is
    // retryable, so we keep state and bail.
    if let Some(inv) = openclaw_uninstall_for(framework, component) {
        match run_openclaw(&inv, ctx.quiet) {
            Ok(()) => {}
            Err(OpenclawError::NotFound) => {
                if !ctx.quiet {
                    eprintln!(
                        "warning: openclaw CLI not found — skipping plugin uninstall for {adapter_name}"
                    );
                }
            }
            Err(err) => {
                return Err(CliError::Runtime {
                    command: command_str.clone(),
                    reason: format!(
                        "framework de-registration failed (state kept so removal can be retried): {}",
                        err.message()
                    ),
                });
            }
        }
    }

    let mut removed: Vec<String> = Vec::new();
    let mut skipped: Vec<SkippedFile> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut remove_errors: Vec<String> = Vec::new();

    for file in &adapter_obj.files {
        if file.owner != FileOwner::Anolisa {
            let msg = format!("skipped {} — externally owned file", file.path.display());
            warnings.push(msg);
            skipped.push(SkippedFile {
                path: file.path.display().to_string(),
                reason: "file is externally owned — refusing to delete".to_string(),
            });
            continue;
        }
        if let Err(err) = validate_owned_path(&layout, &file.path) {
            let msg = format!("skipped {} — path boundary: {err}", file.path.display());
            warnings.push(msg);
            skipped.push(SkippedFile {
                path: file.path.display().to_string(),
                reason: format!("path boundary check failed: {err}"),
            });
            continue;
        }
        if file.path.is_symlink() {
            let msg = format!(
                "skipped {} — refusing to follow symlink",
                file.path.display()
            );
            warnings.push(msg);
            skipped.push(SkippedFile {
                path: file.path.display().to_string(),
                reason: "refusing to follow symlink".to_string(),
            });
            continue;
        }
        if file.path.is_dir() {
            let msg = format!(
                "skipped {} — refusing to remove directory",
                file.path.display()
            );
            warnings.push(msg);
            skipped.push(SkippedFile {
                path: file.path.display().to_string(),
                reason: "refusing to remove directory".to_string(),
            });
            continue;
        }
        if !file.path.exists() {
            continue;
        }
        if let Err(err) = std::fs::remove_file(&file.path) {
            let msg = format!("failed to remove {}: {err}", file.path.display());
            warnings.push(msg);
            remove_errors.push(format!("{} ({err})", file.path.display()));
            skipped.push(SkippedFile {
                path: file.path.display().to_string(),
                reason: format!("remove_file failed: {err}"),
            });
        } else {
            removed.push(file.path.display().to_string());
        }
    }

    if !remove_errors.is_empty() {
        let details = remove_errors.join(", ");
        return Err(CliError::Runtime {
            command: command_str,
            reason: format!(
                "failed to remove {count} adapter file(s); state kept so removal can be retried: {details}",
                count = remove_errors.len(),
            ),
        });
    }

    // Update state — set metadata from ctx in case this is a fresh file.
    state.install_mode = match ctx.install_mode {
        crate::context::InstallMode::System => StateInstallMode::System,
        crate::context::InstallMode::User => StateInstallMode::User,
    };
    state.prefix = layout.prefix.clone();
    state.remove_object(ObjectKind::Adapter, &adapter_name);
    state.save(&state_path).map_err(|err| CliError::Runtime {
        command: command_str.clone(),
        reason: format!("failed to save installed state: {err}"),
    })?;

    // Central log.
    let operation_id = format!(
        "op-adapter-remove-{}",
        started_at.replace([':', '-', 'T', 'Z'], "")
    );
    let log = CentralLog::open(layout.central_log.clone());
    let record = LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.clone()),
        command: format!("adapter remove {adapter_name}"),
        source: "anolisa-cli".to_string(),
        component: Some(component.to_string()),
        severity: if warnings.is_empty() {
            Severity::Info
        } else {
            Severity::Warn
        },
        message: format!("adapter {adapter_name} removed"),
        actor: "cli".to_string(),
        install_mode: Some(ctx.install_mode.as_str().to_string()),
        started_at: started_at.clone(),
        finished_at: Some(now_iso8601()),
        status: Some(LogStatus::Ok),
        objects: vec![adapter_name.clone()],
        backup_ids: Vec::new(),
        warnings: warnings.clone(),
        details: serde_json::Value::Null,
    };
    if let Err(err) = log.append(&record) {
        eprintln!("warning: failed to write central log: {err}");
    }

    // Output.
    if ctx.json {
        return render_json(
            &command_str,
            RemoveResult {
                adapter: adapter_name,
                files_removed: removed,
                files_skipped: skipped,
                dry_run: false,
            },
        );
    }

    if !ctx.quiet {
        let color = Palette::new(ctx.no_color);
        println!(
            "{} {} {}",
            color.command("adapter remove"),
            adapter_name,
            color.ok("succeeded")
        );
        println!(
            "{} {}",
            color.label("operation_id:"),
            color.id(&operation_id)
        );
        println!("{} {}", color.label("files removed:"), removed.len());
        for p in &removed {
            println!("  - {}", color.path(p));
        }
        if !skipped.is_empty() {
            println!("{} {}", color.label("files skipped:"), skipped.len());
            for s in &skipped {
                println!("  - {} ({})", color.path(&s.path), s.reason);
            }
        }
        if !warnings.is_empty() {
            println!("{}", color.warn("warnings:"));
            for w in &warnings {
                println!("  - {w}");
            }
        }
    }

    Ok(())
}

/// Classify adapter files into removable vs skipped without mutating
/// anything. Used by the dry-run preview.
fn classify_files(
    files: &[OwnedFile],
    layout: &anolisa_platform::fs_layout::FsLayout,
) -> (Vec<String>, Vec<SkippedFile>) {
    let mut would_remove = Vec::new();
    let mut would_skip = Vec::new();
    for file in files {
        if file.owner != FileOwner::Anolisa {
            would_skip.push(SkippedFile {
                path: file.path.display().to_string(),
                reason: "file is externally owned — refusing to delete".to_string(),
            });
        } else if let Err(err) = validate_owned_path(layout, &file.path) {
            would_skip.push(SkippedFile {
                path: file.path.display().to_string(),
                reason: format!("path boundary check failed: {err}"),
            });
        } else if file.path.is_symlink() {
            would_skip.push(SkippedFile {
                path: file.path.display().to_string(),
                reason: "refusing to follow symlink".to_string(),
            });
        } else if file.path.is_dir() {
            would_skip.push(SkippedFile {
                path: file.path.display().to_string(),
                reason: "refusing to remove directory".to_string(),
            });
        } else {
            would_remove.push(file.path.display().to_string());
        }
    }
    (would_remove, would_skip)
}

/// ISO 8601 UTC timestamp with second precision.
fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use anolisa_core::state::{InstalledObject, InstalledState, ObjectStatus, OwnedFile};
    use anolisa_platform::fs_layout::FsLayout;
    use tempfile::tempdir;

    use crate::context::InstallMode;

    fn ctx_with_prefix(
        json: bool,
        dry_run: bool,
        install_mode: InstallMode,
        prefix: Option<PathBuf>,
    ) -> CliContext {
        CliContext {
            install_mode,
            prefix,
            json,
            dry_run,
            verbose: false,
            quiet: true,
            no_color: true,
        }
    }

    fn adapter_object(name: &str, files: Vec<OwnedFile>) -> InstalledObject {
        InstalledObject {
            kind: ObjectKind::Adapter,
            name: name.to_string(),
            version: "0.1.0".to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: None,
            distribution_source: None,
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: None,
            managed: true,
            adopted: false,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files,
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        }
    }

    // -- remove: adapter not installed → InvalidArgument ---------------------

    #[test]
    fn remove_unknown_adapter_returns_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let ctx = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        let err = handle_remove("tokenless", "cosh", false, &ctx)
            .expect_err("must error for unknown adapter");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("not installed"));
    }

    // -- remove: --purge returns NOT_IMPLEMENTED ----------------------------

    #[test]
    fn remove_purge_returns_not_implemented() {
        let tmp = tempdir().expect("tmpdir");
        let ctx = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        let err = handle_remove("tokenless", "cosh", true, &ctx).expect_err("purge must error");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
    }

    // -- remove: dry-run previews without modifying state --------------------

    #[test]
    fn remove_dry_run_does_not_delete_files_or_modify_state() {
        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));

        std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bin");
        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");
        let owned = layout.bin_dir.join("tokenless-adapter");
        std::fs::write(&owned, b"adapter-binary").expect("write owned");

        let mut state = InstalledState::default();
        state.upsert_object(adapter_object(
            "tokenless/cosh",
            vec![OwnedFile {
                path: owned.clone(),
                owner: FileOwner::Anolisa,
                sha256: None,
            }],
        ));
        let state_path = layout.state_dir.join("installed.toml");
        state.save(&state_path).expect("save state");
        let prior_bytes = std::fs::read(&state_path).expect("read prior");

        let ctx = ctx_with_prefix(
            false,
            true,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        handle_remove("tokenless", "cosh", false, &ctx).expect("dry-run must succeed");

        assert!(owned.exists(), "dry-run must not delete files");
        let after_bytes = std::fs::read(&state_path).expect("read after");
        assert_eq!(after_bytes, prior_bytes, "dry-run must not modify state");
    }

    // -- remove: real delete + state update ---------------------------------

    #[test]
    fn remove_deletes_owned_files_and_drops_state_object() {
        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));

        std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bin");
        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");
        let owned = layout.bin_dir.join("tokenless-adapter");
        std::fs::write(&owned, b"adapter-binary").expect("write owned");

        let mut state = InstalledState::default();
        state.upsert_object(adapter_object(
            "tokenless/cosh",
            vec![OwnedFile {
                path: owned.clone(),
                owner: FileOwner::Anolisa,
                sha256: None,
            }],
        ));
        let state_path = layout.state_dir.join("installed.toml");
        state.save(&state_path).expect("save state");

        let ctx = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        handle_remove("tokenless", "cosh", false, &ctx).expect("remove must succeed");

        assert!(!owned.exists(), "owned file must be removed");

        let after = InstalledState::load(&state_path).expect("reload state");
        assert!(
            after
                .find_object(ObjectKind::Adapter, "tokenless/cosh")
                .is_none(),
            "adapter object must be dropped"
        );

        assert!(layout.central_log.exists(), "central log must be written");
    }

    #[cfg(unix)]
    #[test]
    fn remove_file_failure_keeps_state_for_retry() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));

        std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bin");
        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");

        let removable = layout.bin_dir.join("removable-adapter-file");
        std::fs::write(&removable, b"removable").expect("write removable");

        let locked_dir = layout.bin_dir.join("locked-dir");
        std::fs::create_dir_all(&locked_dir).expect("mkdir locked dir");
        let blocked = locked_dir.join("blocked-adapter-file");
        std::fs::write(&blocked, b"blocked").expect("write blocked");
        std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o555))
            .expect("make locked dir read-only");

        let mut state = InstalledState::default();
        state.upsert_object(adapter_object(
            "tokenless/cosh",
            vec![
                OwnedFile {
                    path: removable.clone(),
                    owner: FileOwner::Anolisa,
                    sha256: None,
                },
                OwnedFile {
                    path: blocked.clone(),
                    owner: FileOwner::Anolisa,
                    sha256: None,
                },
            ],
        ));
        let state_path = layout.state_dir.join("installed.toml");
        state.save(&state_path).expect("save state");

        let ctx = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        let err = handle_remove("tokenless", "cosh", false, &ctx)
            .expect_err("remove must fail when a file cannot be deleted");

        std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o755))
            .expect("restore locked dir permissions");

        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("state kept"),
            "error should explain retryable state retention: {}",
            err.reason()
        );
        assert!(
            !removable.exists(),
            "files removed before the failure stay removed"
        );
        assert!(blocked.exists(), "failed file must remain on disk");

        let after = InstalledState::load(&state_path).expect("reload state");
        assert!(
            after
                .find_object(ObjectKind::Adapter, "tokenless/cosh")
                .is_some(),
            "adapter state must remain so remove can be retried"
        );
    }

    // -- remove: idempotent for already-deleted files -----------------------

    #[test]
    fn remove_is_idempotent_for_missing_files() {
        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));

        std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bin");
        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");

        let ghost = layout.bin_dir.join("already-gone");

        let mut state = InstalledState::default();
        state.upsert_object(adapter_object(
            "tokenless/cosh",
            vec![OwnedFile {
                path: ghost,
                owner: FileOwner::Anolisa,
                sha256: None,
            }],
        ));
        let state_path = layout.state_dir.join("installed.toml");
        state.save(&state_path).expect("save state");

        let ctx = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        handle_remove("tokenless", "cosh", false, &ctx)
            .expect("remove must succeed for missing files");
    }

    // -- remove: external-owned files skipped -------------------------------

    #[test]
    fn remove_skips_externally_owned_files() {
        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));

        std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bin");
        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");
        let external = layout.bin_dir.join("external-config");
        std::fs::write(&external, b"external").expect("write external");
        let owned = layout.bin_dir.join("owned-binary");
        std::fs::write(&owned, b"owned").expect("write owned");

        let mut state = InstalledState::default();
        state.upsert_object(adapter_object(
            "tokenless/cosh",
            vec![
                OwnedFile {
                    path: external.clone(),
                    owner: FileOwner::External,
                    sha256: None,
                },
                OwnedFile {
                    path: owned.clone(),
                    owner: FileOwner::Anolisa,
                    sha256: None,
                },
            ],
        ));
        let state_path = layout.state_dir.join("installed.toml");
        state.save(&state_path).expect("save state");

        let ctx = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        handle_remove("tokenless", "cosh", false, &ctx).expect("remove must succeed");

        assert!(external.exists(), "external file must not be deleted");
        assert!(!owned.exists(), "owned file must be deleted");
    }

    // -- remove: path outside owned roots is skipped ------------------------

    #[test]
    fn remove_skips_files_outside_owned_roots() {
        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));

        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");
        let outside = tmp.path().join("not-owned").join("rogue.conf");
        std::fs::create_dir_all(outside.parent().unwrap()).expect("mkdir outside");
        std::fs::write(&outside, b"rogue").expect("write outside");

        let mut state = InstalledState::default();
        state.upsert_object(adapter_object(
            "tokenless/cosh",
            vec![OwnedFile {
                path: outside.clone(),
                owner: FileOwner::Anolisa,
                sha256: None,
            }],
        ));
        let state_path = layout.state_dir.join("installed.toml");
        state.save(&state_path).expect("save state");

        let ctx = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        handle_remove("tokenless", "cosh", false, &ctx).expect("remove must succeed");

        assert!(outside.exists(), "file outside roots must not be deleted");
    }

    // -- remove: symlinks are skipped ---------------------------------------

    #[cfg(unix)]
    #[test]
    fn remove_skips_symlinks() {
        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));

        std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bin");
        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");

        let target = layout.bin_dir.join("real-file");
        std::fs::write(&target, b"target").expect("write target");
        let link = layout.bin_dir.join("link-file");
        std::os::unix::fs::symlink(&target, &link).expect("create symlink");

        let mut state = InstalledState::default();
        state.upsert_object(adapter_object(
            "tokenless/cosh",
            vec![OwnedFile {
                path: link.clone(),
                owner: FileOwner::Anolisa,
                sha256: None,
            }],
        ));
        let state_path = layout.state_dir.join("installed.toml");
        state.save(&state_path).expect("save state");

        let ctx = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        handle_remove("tokenless", "cosh", false, &ctx).expect("remove must succeed");

        assert!(link.is_symlink(), "symlink must not be removed");
        assert!(target.exists(), "symlink target must not be removed");
    }

    // -- dispatch: list returns NOT_IMPLEMENTED -----------------------------

    #[test]
    fn list_returns_not_implemented() {
        let tmp = tempdir().expect("tmpdir");
        let ctx = ctx_with_prefix(
            false,
            false,
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
        );
        let err = handle(
            AdapterArgs {
                command: AdapterCommands::List,
            },
            &ctx,
        )
        .expect_err("list must return not implemented");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
    }

    // -- openclaw CLI invocation construction (pure, no spawn) --------------

    #[test]
    fn framework_needs_cli_is_openclaw_only() {
        assert!(framework_needs_cli("openclaw"));
        assert!(!framework_needs_cli("cosh"));
        assert!(!framework_needs_cli("hermes"));
    }

    #[test]
    fn build_openclaw_install_mirrors_install_sh_contract() {
        let inv = build_openclaw_install(
            "openclaw",
            std::path::Path::new("/data/adapters/tokenless/openclaw"),
            std::path::Path::new("/home/u/.openclaw"),
            Some(std::path::Path::new("/home/u")),
        );
        assert_eq!(inv.program, "openclaw");
        assert_eq!(
            inv.args,
            vec![
                "plugins".to_string(),
                "install".to_string(),
                "/data/adapters/tokenless/openclaw".to_string(),
                "--force".to_string(),
                "--dangerously-force-unsafe-install".to_string(),
            ]
        );
        // OPENCLAW_STATE_DIR == home; OPENCLAW_HOME is unset at spawn time.
        assert_eq!(inv.state_dir, PathBuf::from("/home/u/.openclaw"));
        // PATH prefix order matches install.sh: ~/.local/bin, <state>/bin,
        // /usr/local/bin.
        assert_eq!(
            inv.path_prepend,
            vec![
                PathBuf::from("/home/u/.local/bin"),
                PathBuf::from("/home/u/.openclaw/bin"),
                PathBuf::from("/usr/local/bin"),
            ]
        );
        let shown = inv.display_command();
        assert!(shown.contains("OPENCLAW_STATE_DIR=/home/u/.openclaw"));
        assert!(shown.contains("plugins install /data/adapters/tokenless/openclaw"));
        assert!(shown.contains("--dangerously-force-unsafe-install"));
    }

    #[test]
    fn build_openclaw_uninstall_targets_plugin_id() {
        let inv = build_openclaw_uninstall(
            "openclaw",
            "tokenless",
            std::path::Path::new("/home/u/.openclaw"),
            Some(std::path::Path::new("/home/u")),
        );
        assert_eq!(
            inv.args,
            vec![
                "plugins".to_string(),
                "uninstall".to_string(),
                "tokenless".to_string(),
                "--force".to_string(),
            ]
        );
        assert_eq!(inv.state_dir, PathBuf::from("/home/u/.openclaw"));
    }

    // =========================================================================
    // Integration tests: adapter install + remove end-to-end
    // =========================================================================

    mod install_integration {
        use super::*;
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use sha2::{Digest, Sha256};
        use std::fs;
        use tar::{Builder, Header};

        fn sha256_hex(bytes: &[u8]) -> String {
            let mut h = Sha256::new();
            h.update(bytes);
            let digest = h.finalize();
            digest.iter().map(|b| format!("{b:02x}")).collect()
        }

        fn build_tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
            let buf: Vec<u8> = Vec::new();
            let enc = GzEncoder::new(buf, Compression::default());
            let mut tar = Builder::new(enc);
            for (path, data) in entries {
                let mut hdr = Header::new_gnu();
                hdr.set_size(data.len() as u64);
                hdr.set_mode(0o644);
                hdr.set_cksum();
                tar.append_data(&mut hdr, path, *data).unwrap();
            }
            let enc = tar.into_inner().unwrap();
            enc.finish().unwrap()
        }

        /// Sets up a complete test environment:
        /// - system prefix under tmp
        /// - overlay manifests with distribution index pointing to a file:// tar.gz
        /// - the tar.gz built from provided entries
        ///
        /// Returns (ctx, tar_gz_sha256)
        fn setup_env(
            tmp: &std::path::Path,
            tar_entries: &[(&str, &[u8])],
            component: &str,
            version: &str,
        ) -> (CliContext, String) {
            let layout = FsLayout::system(Some(tmp.to_path_buf()));

            // Build tar.gz artifact. The fixture embeds a published install
            // contract (`.anolisa/component.toml`) declaring a `cosh` adapter so
            // `handle_install` reads source/dest/version from the artifact — not
            // from the dev-tree catalog. `cosh` is a non-CLI framework, so no
            // real `openclaw` binary is ever spawned during the test.
            let embedded = format!(
                "[component]\n\
                 name = \"{component}\"\n\
                 version = \"{version}\"\n\
                 \n\
                 [[adapters]]\n\
                 framework = \"cosh\"\n\
                 kind = \"third-party\"\n\
                 plugin_id = \"{component}\"\n\
                 source = \"target/release/cosh-ext/\"\n\
                 dest = \"{{datadir}}/adapters/{component}/cosh/\"\n",
            );
            let mut entries: Vec<(&str, &[u8])> = tar_entries.to_vec();
            entries.push((".anolisa/component.toml", embedded.as_bytes()));
            let tar_bytes = build_tar_gz(&entries);
            let tar_sha = sha256_hex(&tar_bytes);
            let artifact_dir = tmp.join("artifacts");
            fs::create_dir_all(&artifact_dir).unwrap();
            let tar_path = artifact_dir.join("adapter.tar.gz");
            fs::write(&tar_path, &tar_bytes).unwrap();
            let tar_url = format!("file://{}", tar_path.display());

            // Write overlay distribution index.
            let dist_dir = layout.manifests_overlay.join("distribution-index");
            fs::create_dir_all(&dist_dir).unwrap();
            let index_content = format!(
                r#"schema_version = 1

[[entries]]
component = "{component}"
version = "{version}"
channel = "stable"
artifact_type = "tar_gz"
backend = "tar"
url = "{tar_url}"
os = "{os}"
arch = "{arch}"
install_modes = ["system"]
sha256 = "{tar_sha}"
"#,
                os = anolisa_env::EnvService::detect().os,
                arch = anolisa_env::EnvService::detect().arch,
            );
            fs::write(dist_dir.join("index.toml"), &index_content).unwrap();

            // Ensure state/cache dirs exist.
            fs::create_dir_all(&layout.state_dir).unwrap();
            fs::create_dir_all(&layout.cache_dir).unwrap();

            let ctx = ctx_with_prefix(false, false, InstallMode::System, Some(tmp.to_path_buf()));
            (ctx, tar_sha)
        }

        // -- install: end-to-end success -----------------------------------------

        #[test]
        fn install_downloads_copies_files_and_writes_state() {
            let tmp = tempdir().expect("tmpdir");
            let plugin_json = br#"{"name":"tokenless"}"#;
            let index_js = b"console.log('adapter loaded');";
            let (ctx, _sha) = setup_env(
                tmp.path(),
                &[
                    ("target/release/cosh-ext/plugin.json", plugin_json),
                    ("target/release/cosh-ext/dist/index.js", index_js),
                ],
                "tokenless",
                TEST_VERSION,
            );

            handle_install(&ctx, "tokenless", "cosh").expect("install must succeed");

            // Verify files exist at the expanded destination.
            let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
            let dest_root = layout.datadir.join("adapters/tokenless/cosh");
            assert!(
                dest_root.join("plugin.json").exists(),
                "plugin.json must be installed"
            );
            assert!(
                dest_root.join("dist/index.js").exists(),
                "dist/index.js must be installed"
            );
            assert_eq!(
                fs::read(dest_root.join("plugin.json")).unwrap(),
                plugin_json
            );
            assert_eq!(fs::read(dest_root.join("dist/index.js")).unwrap(), index_js);

            // Verify state has the adapter object.
            let state_path = layout.state_dir.join("installed.toml");
            let state = InstalledState::load(&state_path).expect("load state");
            let obj = state
                .find_object(ObjectKind::Adapter, "tokenless/cosh")
                .expect("adapter object must exist in state");
            assert_eq!(obj.status, ObjectStatus::Installed);
            assert_eq!(obj.component_refs, vec!["tokenless".to_string()]);
            assert_eq!(obj.files.len(), 2);
            assert!(obj.files.iter().all(|f| f.owner == FileOwner::Anolisa));
            assert!(obj.files.iter().all(|f| f.sha256.is_some()));

            // Verify central log written.
            assert!(layout.central_log.exists(), "central log must be written");
        }

        // -- install then remove: full lifecycle ---------------------------------

        #[test]
        fn install_then_remove_leaves_no_files_or_state() {
            let tmp = tempdir().expect("tmpdir");
            let plugin_json = br#"{"name":"tokenless"}"#;
            let (ctx, _sha) = setup_env(
                tmp.path(),
                &[("target/release/cosh-ext/plugin.json", plugin_json)],
                "tokenless",
                TEST_VERSION,
            );

            handle_install(&ctx, "tokenless", "cosh").expect("install must succeed");

            let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
            let dest_root = layout.datadir.join("adapters/tokenless/cosh");
            assert!(dest_root.join("plugin.json").exists());

            handle_remove("tokenless", "cosh", false, &ctx).expect("remove must succeed");

            assert!(
                !dest_root.join("plugin.json").exists(),
                "file must be removed"
            );
            let state_path = layout.state_dir.join("installed.toml");
            let state = InstalledState::load(&state_path).expect("load state");
            assert!(
                state
                    .find_object(ObjectKind::Adapter, "tokenless/cosh")
                    .is_none(),
                "adapter object must be removed from state"
            );
        }

        // -- install: missing sha256 in distribution entry -----------------------

        #[test]
        fn install_rejects_entry_without_sha256() {
            let tmp = tempdir().expect("tmpdir");
            let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
            fs::create_dir_all(&layout.state_dir).unwrap();
            fs::create_dir_all(&layout.cache_dir).unwrap();

            // Write a distribution index with no sha256.
            let dist_dir = layout.manifests_overlay.join("distribution-index");
            fs::create_dir_all(&dist_dir).unwrap();
            let env = anolisa_env::EnvService::detect();
            let index_content = format!(
                r#"schema_version = 1

[[entries]]
component = "tokenless"
version = "{version}"
channel = "stable"
artifact_type = "tar_gz"
backend = "tar"
url = "file:///nonexistent/artifact.tar.gz"
os = "{os}"
arch = "{arch}"
install_modes = ["system"]
"#,
                version = TEST_VERSION,
                os = env.os,
                arch = env.arch,
            );
            fs::write(dist_dir.join("index.toml"), &index_content).unwrap();

            let ctx = ctx_with_prefix(
                false,
                false,
                InstallMode::System,
                Some(tmp.path().to_path_buf()),
            );
            let err =
                handle_install(&ctx, "tokenless", "cosh").expect_err("must reject missing sha256");
            assert_eq!(err.code(), "EXECUTION_FAILED");
            assert!(err.reason().contains("sha256"));

            // No state written.
            let state_path = layout.state_dir.join("installed.toml");
            let state = InstalledState::load(&state_path).expect("load state");
            assert!(
                state
                    .find_object(ObjectKind::Adapter, "tokenless/cosh")
                    .is_none(),
                "no phantom state on sha256 rejection"
            );
        }

        // -- install: checksum mismatch does not leave state ---------------------

        #[test]
        fn install_checksum_mismatch_does_not_leave_state() {
            let tmp = tempdir().expect("tmpdir");
            let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
            fs::create_dir_all(&layout.state_dir).unwrap();
            fs::create_dir_all(&layout.cache_dir).unwrap();

            // Build a tar.gz but declare a wrong sha256 in the index.
            let tar_bytes = build_tar_gz(&[("target/release/cosh-ext/plugin.json", b"data")]);
            let artifact_dir = tmp.path().join("artifacts");
            fs::create_dir_all(&artifact_dir).unwrap();
            let tar_path = artifact_dir.join("adapter.tar.gz");
            fs::write(&tar_path, &tar_bytes).unwrap();
            let tar_url = format!("file://{}", tar_path.display());

            let dist_dir = layout.manifests_overlay.join("distribution-index");
            fs::create_dir_all(&dist_dir).unwrap();
            let env = anolisa_env::EnvService::detect();
            let index_content = format!(
                r#"schema_version = 1

[[entries]]
component = "tokenless"
version = "{version}"
channel = "stable"
artifact_type = "tar_gz"
backend = "tar"
url = "{tar_url}"
os = "{os}"
arch = "{arch}"
install_modes = ["system"]
sha256 = "{wrong_sha}"
"#,
                version = TEST_VERSION,
                os = env.os,
                arch = env.arch,
                wrong_sha = "0".repeat(64),
            );
            fs::write(dist_dir.join("index.toml"), &index_content).unwrap();

            let ctx = ctx_with_prefix(
                false,
                false,
                InstallMode::System,
                Some(tmp.path().to_path_buf()),
            );
            let err = handle_install(&ctx, "tokenless", "cosh")
                .expect_err("must reject checksum mismatch");
            assert_eq!(err.code(), "EXECUTION_FAILED");

            // No state or files left.
            let state_path = layout.state_dir.join("installed.toml");
            let state = InstalledState::load(&state_path).expect("load state");
            assert!(
                state
                    .find_object(ObjectKind::Adapter, "tokenless/cosh")
                    .is_none(),
                "no state on checksum failure"
            );
            let dest_root = layout.datadir.join("adapters/tokenless/cosh");
            assert!(!dest_root.exists(), "no adapter files on checksum failure");
        }

        // -- install: dest already exists rejects without leaving state -----------

        #[test]
        fn install_dest_exists_does_not_leave_state() {
            let tmp = tempdir().expect("tmpdir");
            let plugin_json = br#"{"name":"pre-existing"}"#;
            let (ctx, _sha) = setup_env(
                tmp.path(),
                &[("target/release/cosh-ext/plugin.json", b"new-data")],
                "tokenless",
                TEST_VERSION,
            );

            // Pre-create the dest file to trigger DestExists.
            let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
            let dest_root = layout.datadir.join("adapters/tokenless/cosh");
            fs::create_dir_all(&dest_root).unwrap();
            fs::write(dest_root.join("plugin.json"), plugin_json).unwrap();

            let err = handle_install(&ctx, "tokenless", "cosh")
                .expect_err("must reject when dest exists");
            assert_eq!(err.code(), "EXECUTION_FAILED");

            // Pre-existing file untouched.
            assert_eq!(
                fs::read(dest_root.join("plugin.json")).unwrap(),
                plugin_json
            );

            // No state written.
            let state_path = layout.state_dir.join("installed.toml");
            let state = InstalledState::load(&state_path).expect("load state");
            assert!(
                state
                    .find_object(ObjectKind::Adapter, "tokenless/cosh")
                    .is_none(),
                "no state when dest exists"
            );
        }

        // -- install: binary-only entry rejected for adapter install -------------

        #[test]
        fn install_rejects_binary_artifact_type() {
            let tmp = tempdir().expect("tmpdir");
            let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
            fs::create_dir_all(&layout.state_dir).unwrap();
            fs::create_dir_all(&layout.cache_dir).unwrap();

            // Write a distribution index with only a binary entry.
            let dist_dir = layout.manifests_overlay.join("distribution-index");
            fs::create_dir_all(&dist_dir).unwrap();
            let env = anolisa_env::EnvService::detect();
            let index_content = format!(
                r#"schema_version = 1

[[entries]]
component = "tokenless"
version = "{version}"
channel = "stable"
artifact_type = "binary"
backend = "binary"
url = "file:///nonexistent/tokenless"
os = "{os}"
arch = "{arch}"
install_modes = ["system"]
sha256 = "{sha}"
"#,
                version = TEST_VERSION,
                os = env.os,
                arch = env.arch,
                sha = "0".repeat(64),
            );
            fs::write(dist_dir.join("index.toml"), &index_content).unwrap();

            let ctx = ctx_with_prefix(
                false,
                false,
                InstallMode::System,
                Some(tmp.path().to_path_buf()),
            );
            let err = handle_install(&ctx, "tokenless", "cosh")
                .expect_err("must reject binary artifact type");
            assert_eq!(err.code(), "EXECUTION_FAILED");
            assert!(
                err.reason().contains("tar_gz"),
                "error should mention tar_gz requirement: {}",
                err.reason()
            );

            // No state or files left.
            let state_path = layout.state_dir.join("installed.toml");
            let state = InstalledState::load(&state_path).expect("load state");
            assert!(
                state
                    .find_object(ObjectKind::Adapter, "tokenless/cosh")
                    .is_none(),
                "no state on binary rejection"
            );
        }

        // -- install: corrupted state file rejected without leaving files ---------

        #[test]
        fn install_corrupted_state_does_not_leave_files() {
            let tmp = tempdir().expect("tmpdir");
            let plugin_json = br#"{"name":"tokenless"}"#;
            let (ctx, _sha) = setup_env(
                tmp.path(),
                &[("target/release/cosh-ext/plugin.json", plugin_json)],
                "tokenless",
                TEST_VERSION,
            );

            // Write a corrupted installed.toml so load_installed_state fails.
            let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
            let state_path = layout.state_dir.join("installed.toml");
            fs::write(&state_path, b"this is not valid toml [[[[").unwrap();

            let err =
                handle_install(&ctx, "tokenless", "cosh").expect_err("must reject corrupted state");
            assert_eq!(err.code(), "EXECUTION_FAILED");

            // No adapter files written (state load happens before file copy).
            let dest_root = layout.datadir.join("adapters/tokenless/cosh");
            assert!(
                !dest_root.join("plugin.json").exists(),
                "no adapter files when state is corrupted"
            );
        }

        /// Fixed published version for the fixtures. Decoupled from
        /// `manifests/runtime/tokenless.toml`: adapter install resolves the
        /// shipped artifact by highest semver, so the index entry and the
        /// embedded manifest only need to agree with each other, not with the
        /// dev-tree catalog.
        const TEST_VERSION: &str = "0.5.0";
    }
}
