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
//! Resolves the manifest adapter, expands the destination path against
//! the active [`FsLayout`], and runs detection. Only `--dry-run` is
//! operational today; real execution returns `NOT_IMPLEMENTED` without
//! writing state because artifact staging/download is not yet available.
//!
//! ## `adapter remove <component> <framework>`
//!
//! Safe file deletion with four-layer guard:
//!
//! 1. **Owner check** — only `FileOwner::Anolisa` files are removed.
//! 2. **Path boundary** — [`validate_owned_path`] rejects escapes.
//! 3. **Symlink guard** — refuses to follow symlinks.
//! 4. **Directory guard** — refuses to `remove_file` a directory.

use chrono::{SecondsFormat, Utc};
use clap::{Parser, Subcommand};
use serde::Serialize;

use anolisa_core::adapter::{detect_framework, expand_layout_placeholders};
use anolisa_core::central_log::{CentralLog, LogKind, LogRecord, LogStatus, Severity};
use anolisa_core::lock::InstallLock;
use anolisa_core::path_safety::validate_owned_path;
use anolisa_core::state::{FileOwner, InstallMode as StateInstallMode, ObjectKind, OwnedFile};

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
fn handle_install(ctx: &CliContext, component: &str, framework: &str) -> Result<(), CliError> {
    let command = format!("adapter install {component} {framework}");
    let catalog = common::load_bundled_catalog(ctx, &command)?;

    let comp = catalog
        .component(component)
        .ok_or_else(|| CliError::InvalidArgument {
            command: command.clone(),
            reason: format!("component '{component}' not found in catalog"),
        })?;

    let adapter = comp
        .adapters
        .iter()
        .find(|a| a.framework.as_deref() == Some(framework))
        .ok_or_else(|| CliError::InvalidArgument {
            command: command.clone(),
            reason: format!("no adapter for framework '{framework}' in component '{component}'"),
        })?;

    let layout = common::resolve_layout(ctx);
    let dest_template = adapter.dest.as_deref().unwrap_or_default();
    let expanded_dest =
        expand_layout_placeholders(dest_template, &layout, &[("component", component)]).map_err(
            |err| CliError::InvalidArgument {
                command: command.clone(),
                reason: format!("failed to expand adapter dest: {err}"),
            },
        )?;

    let detect_result = detect_framework(adapter);
    if !detect_result.detected && !ctx.quiet {
        eprintln!(
            "warning: framework '{framework}' not detected on this host: {}",
            detect_result.reason
        );
    }

    validate_owned_path(&layout, &expanded_dest).map_err(|err| CliError::InvalidArgument {
        command: command.clone(),
        reason: format!(
            "adapter destination '{}' failed path safety check: {err}",
            expanded_dest.display()
        ),
    })?;

    let plan = InstallPlan {
        component: component.to_string(),
        framework: framework.to_string(),
        source: adapter.source.clone(),
        dest: expanded_dest.display().to_string(),
        detected: detect_result.detected,
        detect_reason: detect_result.reason,
    };

    if ctx.dry_run {
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
        return Ok(());
    }

    // Real execution requires artifact staging which is not implemented.
    // Do NOT write state/log here — a phantom "installed" record without
    // real files would mislead status/remove/list.
    Err(CliError::not_implemented_with_hint(
        command,
        "adapter install real execution requires artifact staging; use --dry-run to preview the plan",
    ))
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

    let mut removed: Vec<String> = Vec::new();
    let mut skipped: Vec<SkippedFile> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

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
            skipped.push(SkippedFile {
                path: file.path.display().to_string(),
                reason: format!("remove_file failed: {err}"),
            });
        } else {
            removed.push(file.path.display().to_string());
        }
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
        let err = handle_remove("tokenless", "openclaw", false, &ctx)
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
        let err = handle_remove("tokenless", "openclaw", true, &ctx).expect_err("purge must error");
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
            "tokenless/openclaw",
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
        handle_remove("tokenless", "openclaw", false, &ctx).expect("dry-run must succeed");

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
            "tokenless/openclaw",
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
        handle_remove("tokenless", "openclaw", false, &ctx).expect("remove must succeed");

        assert!(!owned.exists(), "owned file must be removed");

        let after = InstalledState::load(&state_path).expect("reload state");
        assert!(
            after
                .find_object(ObjectKind::Adapter, "tokenless/openclaw")
                .is_none(),
            "adapter object must be dropped"
        );

        assert!(layout.central_log.exists(), "central log must be written");
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
            "tokenless/openclaw",
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
        handle_remove("tokenless", "openclaw", false, &ctx)
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
            "tokenless/openclaw",
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
        handle_remove("tokenless", "openclaw", false, &ctx).expect("remove must succeed");

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
            "tokenless/openclaw",
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
        handle_remove("tokenless", "openclaw", false, &ctx).expect("remove must succeed");

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
            "tokenless/openclaw",
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
        handle_remove("tokenless", "openclaw", false, &ctx).expect("remove must succeed");

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
}
