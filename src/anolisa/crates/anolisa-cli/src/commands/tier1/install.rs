//! `anolisa install` — install a component through a configured backend.
//!
//! `install` takes a component noun that must resolve in the catalog as a
//! component manifest. The resolution chain — repo.toml loading, backend selection
//! (`--backend` > `default_backend`), base_url variable substitution, and
//! package-name mapping (`--package` > `package_map` > scope > component
//! name) — feeds the **raw** backend executor: fetch the distribution index
//! from the raw repository root, resolve an artifact, download it with
//! mandatory sha256 verification, install the manifest-declared files, and
//! record state plus a central-log audit entry. `yum` / `npm` backends are
//! selectable but their executors are NOT_IMPLEMENTED.
//!
//! Deliberately out of scope for this milestone: execution-policy gating,
//! pre/post hooks, health checks, and service start/enable. Installed
//! services are recorded in state with `enabled: false`.

use clap::Parser;
use serde::Serialize;

use anolisa_core::central_log::{CentralLog, LogKind, LogRecord, LogStatus, Severity};
use anolisa_core::download::DownloadCache;
use anolisa_core::install_runner::{InstallRunner, ResolvedInstallFile, SUPPORTED_ARTIFACT_TYPES};
use anolisa_core::lock::InstallLock;
use anolisa_core::path_safety::validate_owned_path;
use anolisa_core::state::{
    FileOwner, InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectKind,
    ObjectStatus, OperationRecord, OwnedFile, ServiceRef,
};
use anolisa_core::{
    ArtifactType, ComponentManifest, DistributionEntry, DistributionIndex, FileKind, ResolveQuery,
    expand_layout_placeholders,
};
use anolisa_platform::fs_layout::FsLayout;
use chrono::{SecondsFormat, Utc};

use crate::color::Palette;
use crate::commands::common;
use crate::context::CliContext;
use crate::repo_config::{
    HostVars, RepoConfig, RepoConfigError, normalize_override_url, raw_artifact_url, raw_index_url,
    raw_relative_root,
};
use crate::response::{CliError, render_json};

const COMMAND: &str = "install";

#[derive(Debug, Parser)]
// `--version` here means the *component* version (the `cargo install`
// convention), so the auto-generated CLI-version flag must be disabled
// to free the name. `anolisa --version` still works at the top level.
#[command(disable_version_flag = true)]
pub struct InstallArgs {
    /// Component name to install
    #[arg(value_name = "COMPONENT")]
    pub component: String,
    /// Install a specific version instead of the latest in the channel
    #[arg(long, value_name = "VERSION")]
    pub version: Option<String>,
    /// Backend override (raw | yum | npm); defaults to repo.toml default_backend
    #[arg(long, value_name = "BACKEND")]
    pub backend: Option<String>,
    /// One-off base_url override for the selected backend
    #[arg(long, value_name = "URL")]
    pub repo: Option<String>,
    /// Override the backend-native package name for the component
    #[arg(long, value_name = "NAME")]
    pub package: Option<String>,
}

/// Resolution context shared by the dry-run preview and the real
/// executor: everything is decided before any file is written.
struct ResolvedInstall {
    component: String,
    package: String,
    backend: String,
    base_url: String,
    entry: DistributionEntry,
    artifact_url: String,
    files: Vec<ResolvedInstallFile>,
    services: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct ArtifactInfo {
    r#type: String,
    url: String,
    sha256: Option<String>,
}

/// Wire shape for `--dry-run`: the full resolution result without IO
/// beyond the index fetch.
#[derive(Serialize)]
struct InstallPlanPayload {
    component: String,
    package: String,
    version: String,
    backend: String,
    base_url: String,
    install_mode: String,
    artifact: ArtifactInfo,
    files: Vec<String>,
    services: Vec<String>,
    dry_run: bool,
    warnings: Vec<String>,
}

/// Wire shape for a completed install.
#[derive(Serialize)]
struct InstallResultPayload {
    component: String,
    package: String,
    version: String,
    backend: String,
    base_url: String,
    install_mode: String,
    operation_id: String,
    artifact_url: String,
    files_installed: Vec<String>,
    services: Vec<String>,
    warnings: Vec<String>,
}

pub fn handle(args: InstallArgs, ctx: &CliContext) -> Result<(), CliError> {
    let command = format!("install {}", args.component);
    let component = args.component.clone();

    // The component name is the ANOLISA-side identity and must exist in the
    // catalog; `--package` only changes the backend-native package name.
    let catalog = common::load_bundled_catalog(ctx, COMMAND)?;
    let manifest = catalog
        .component(&component)
        .ok_or_else(|| CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!("component '{component}' is not in the catalog"),
        })?
        .clone();

    let mode = ctx.install_mode.as_str();
    if !manifest.install.modes.iter().any(|m| m == mode) {
        return Err(CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!(
                "component '{component}' does not support {mode}-mode install (supported: {})",
                manifest.install.modes.join(", ")
            ),
        });
    }

    let layout = common::resolve_layout(ctx);
    let env = anolisa_env::EnvService::detect();

    // — Resolution chain: repo.toml → backend → base_url → package. —
    let repo_config = RepoConfig::load(&layout).map_err(|err| repo_config_err(err, false))?;
    let (backend_name, backend) = repo_config
        .select_backend(args.backend.as_deref())
        // Only reachable via --backend (validation guarantees the default
        // is configured), so this is caller input.
        .map_err(|err| repo_config_err(err, true))?;

    let mut warnings: Vec<String> = Vec::new();
    let base_url = match args.repo.as_deref() {
        Some(override_url) => {
            let normalized =
                normalize_override_url(override_url).map_err(|err| repo_config_err(err, true))?;
            if normalized.starts_with("http://") {
                warnings.push(format!(
                    "--repo uses plaintext http ({normalized}) — artifacts are still sha256-verified on the raw backend, but the index itself is unauthenticated",
                ));
            }
            normalized
        }
        None => {
            let host = HostVars {
                os: env.os.clone(),
                arch: env.arch.clone(),
            };
            repo_config
                .resolved_base_url(backend_name, backend, &host)
                // Variable errors are fixed by editing [vars] in repo.toml.
                .map_err(|err| repo_config_err(err, true))?
        }
    };
    let package = repo_config.package_name(backend, &component, args.package.as_deref());

    let installed = common::load_installed_state(ctx, COMMAND)?;
    ensure_component_backend_compatible(&installed, &component, backend_name, COMMAND)?;

    // Backend gate: only raw can execute today. The selection above already
    // validated the name/configuration, so this is purely "executor missing".
    if backend_name != "raw" {
        return Err(CliError::not_implemented_with_hint(
            format!("install --backend {backend_name}"),
            format!(
                "the '{backend_name}' backend is configured but its executor is not implemented yet — only 'raw' can install today",
            ),
        ));
    }

    let resolved = resolve_raw(
        ctx,
        &layout,
        &env,
        &manifest,
        ResolveInputs {
            component,
            package,
            backend: backend_name.to_string(),
            base_url,
            version: args.version.as_deref(),
            warnings,
        },
    )?;

    if ctx.dry_run {
        return render_plan(ctx, &resolved);
    }

    execute_raw(ctx, &layout, &command, resolved)
}

/// Caller-side inputs to [`resolve_raw`], grouped to keep the signature flat.
struct ResolveInputs<'a> {
    component: String,
    package: String,
    backend: String,
    base_url: String,
    version: Option<&'a str>,
    warnings: Vec<String>,
}

/// Resolve everything the raw executor needs without writing outside the
/// download cache: fetch the index, pick an artifact, and render the
/// manifest's file mappings against the layout.
fn resolve_raw(
    ctx: &CliContext,
    layout: &FsLayout,
    env: &anolisa_env::EnvFacts,
    manifest: &ComponentManifest,
    inputs: ResolveInputs<'_>,
) -> Result<ResolvedInstall, CliError> {
    let ResolveInputs {
        component,
        package,
        backend,
        base_url,
        version,
        mut warnings,
    } = inputs;

    // The index is always re-fetched (DownloadCache overwrites on conflict),
    // so a republished repo is picked up without a cache flush.
    let index_url = raw_index_url(&base_url);
    let cache = DownloadCache::new(layout.cache_dir.clone());
    let downloaded_index = cache
        .fetch(&index_url, None)
        .map_err(|err| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!("failed to fetch distribution index {index_url}: {err}"),
        })?;
    let index = DistributionIndex::load(&downloaded_index.cached_path).map_err(|err| {
        CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!("failed to parse distribution index {index_url}: {err}"),
        }
    })?;

    // The index is keyed by the backend-native package name so that
    // `package_map` / `--package` select between alternate publications.
    let query = ResolveQuery {
        component: &package,
        version,
        channel: None,
        install_mode: ctx.install_mode.as_str(),
        os: &env.os,
        arch: &env.arch,
        libc: env.libc.as_deref(),
        pkg_base: env.pkg_base.as_deref(),
        preferred_types: &[],
    };
    let entry = index.resolve(&query).map_err(|err| CliError::InvalidArgument {
        command: COMMAND.to_string(),
        reason: format!(
            "cannot resolve package '{package}' (component '{component}', version {}, {}/{}, {} mode) from {index_url}: {err}",
            version.unwrap_or("latest"),
            env.os,
            env.arch,
            ctx.install_mode.as_str(),
        ),
    })?;

    let wire_type = artifact_type_wire(&entry.artifact_type);
    if !SUPPORTED_ARTIFACT_TYPES.contains(&wire_type) {
        return Err(CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!(
                "resolved artifact type '{wire_type}' is not installable by the raw backend (supported: {})",
                SUPPORTED_ARTIFACT_TYPES.join(", ")
            ),
        });
    }

    // Missing checksum is a publisher-side defect, not caller input: refuse
    // rather than install unverified bytes.
    if entry.sha256.is_none() {
        return Err(CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "distribution entry for '{package}' {} has no sha256 — refusing to install an unverifiable artifact",
                entry.version
            ),
        });
    }

    if version.is_none() && entry.version != manifest.component.version {
        warnings.push(format!(
            "artifact version {} differs from catalog manifest version {}",
            entry.version, manifest.component.version
        ));
    }

    // Three URL forms, most-mirror-friendly first: an omitted url uses the
    // code-owned raw layout, a repo-relative url resolves against the index
    // directory (self-contained mirrors), and an absolute url is used as-is
    // (escape hatch for off-repo artifacts).
    let artifact_url = if entry.url.is_empty() {
        let values = std::collections::BTreeMap::from([
            ("component", Some(entry.component.clone())),
            ("version", Some(entry.version.clone())),
            ("os", Some(entry.os.clone())),
            ("arch", Some(entry.arch.clone())),
            ("libc", entry.libc.clone()),
            ("ext", Some(artifact_ext(&entry.artifact_type).to_string())),
        ]);
        raw_artifact_url(&backend, &base_url, &values).map_err(|err| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "cannot derive artifact URL for '{package}' {} from raw repository layout: {err}",
                entry.version
            ),
        })?
    } else if entry.url.contains("://") {
        entry.url.clone()
    } else {
        format!(
            "{}/{}",
            raw_relative_root(&base_url),
            entry.url.trim_start_matches('/')
        )
    };

    let files = resolve_manifest_files(manifest, layout, &component)?;
    if files.is_empty() {
        return Err(CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "component '{component}' declares no [install.files] — nothing to install"
            ),
        });
    }

    Ok(ResolvedInstall {
        component,
        package,
        backend,
        base_url,
        artifact_url,
        entry,
        files,
        services: manifest.install.services.clone(),
        warnings,
    })
}

/// Render the manifest's `[install.files]` against the layout: expand
/// `{bindir}`-style placeholders and reject any destination escaping the
/// ANOLISA-owned roots before a single byte is written.
fn resolve_manifest_files(
    manifest: &ComponentManifest,
    layout: &FsLayout,
    component: &str,
) -> Result<Vec<ResolvedInstallFile>, CliError> {
    let mut files = Vec::with_capacity(manifest.install.files.len());
    for spec in &manifest.install.files {
        let template = spec.install_path().ok_or_else(|| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "component '{component}' has an [install.files] entry with neither source nor dest"
            ),
        })?;
        let dest = expand_layout_placeholders(template, layout, &[("component", component)])
            .map_err(|err| CliError::Runtime {
                command: COMMAND.to_string(),
                reason: format!("failed to expand install path '{template}': {err}"),
            })?;
        validate_owned_path(layout, &dest).map_err(|err| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "install destination '{}' failed path safety check: {err}",
                dest.display()
            ),
        })?;
        // A symlink's source is its referent — a layout template like the
        // dest, not an archive path. Expand and bound-check it the same way.
        let source = match (spec.kind, spec.source.as_deref()) {
            (FileKind::Symlink, Some(template)) => {
                let referent =
                    expand_layout_placeholders(template, layout, &[("component", component)])
                        .map_err(|err| CliError::Runtime {
                            command: COMMAND.to_string(),
                            reason: format!(
                                "failed to expand symlink referent '{template}': {err}"
                            ),
                        })?;
                validate_owned_path(layout, &referent).map_err(|err| CliError::Runtime {
                    command: COMMAND.to_string(),
                    reason: format!(
                        "symlink referent '{}' failed path safety check: {err}",
                        referent.display()
                    ),
                })?;
                Some(referent.to_string_lossy().into_owned())
            }
            _ => spec.source.clone(),
        };
        files.push(ResolvedInstallFile {
            source,
            dest,
            mode: spec.mode.clone(),
            kind: spec.kind,
        });
    }
    Ok(files)
}

/// Execute the resolved install: download+verify, copy files under the
/// install lock, persist state, and append the audit record. Files already
/// on disk are rolled back when a later step fails, so no phantom install
/// survives an error.
fn execute_raw(
    ctx: &CliContext,
    layout: &FsLayout,
    command: &str,
    mut resolved: ResolvedInstall,
) -> Result<(), CliError> {
    let started_at = now_iso8601();
    let sha256 = resolved
        .entry
        .sha256
        .as_deref()
        .expect("resolve_raw rejects entries without sha256");

    let cache = DownloadCache::new(layout.cache_dir.clone());
    let artifact = cache
        .fetch(&resolved.artifact_url, Some(sha256))
        .map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "failed to download artifact {}: {err}",
                resolved.artifact_url
            ),
        })?;

    // Acquire lock, then load state inside the lock so a concurrent writer
    // cannot be overwritten and state-load failures precede any file copy.
    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;
    let mut state =
        common::load_installed_state(ctx, command).map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!("failed to load installed state: {err}"),
        })?;
    ensure_component_backend_compatible(&state, &resolved.component, &resolved.backend, command)?;

    // Nanosecond suffix avoids collisions between near-simultaneous
    // processes that serialize on the lock within the same second.
    let lock_ts = Utc::now();
    let operation_id = format!(
        "op-install-{}-{}",
        lock_ts.format("%Y%m%d%H%M%S"),
        lock_ts.timestamp_subsec_nanos()
    );

    let runner = InstallRunner::new(layout);
    let outcome = runner
        .install_files(
            artifact_type_wire(&resolved.entry.artifact_type),
            &artifact.cached_path,
            &resolved.files,
        )
        .map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!("install failed: {err}"),
        })?;

    // From this point files are on disk — failures must roll them back.

    let owned_files: Vec<OwnedFile> = outcome
        .files
        .iter()
        .map(|f| OwnedFile {
            path: f.path.clone(),
            owner: FileOwner::Anolisa,
            sha256: Some(f.sha256.clone()),
        })
        .collect();
    let installed_paths: Vec<String> = outcome
        .files
        .iter()
        .map(|f| f.path.display().to_string())
        .collect();

    let service_manager = match ctx.install_mode {
        crate::context::InstallMode::System => "systemd",
        crate::context::InstallMode::User => "systemd-user",
    };

    // Migrate away legacy capability rows on this state write; surfaced
    // in the result warnings and audited in the central log below. A
    // state-save failure rolls the prune back with the rest of the write.
    let pruned_legacy = state.prune_legacy_capabilities();
    if !pruned_legacy.is_empty() {
        resolved.warnings.push(format!(
            "pruned legacy capability state object(s) written by an older release: {}",
            pruned_legacy.join(", ")
        ));
    }

    state.install_mode = match ctx.install_mode {
        crate::context::InstallMode::System => StateInstallMode::System,
        crate::context::InstallMode::User => StateInstallMode::User,
    };
    state.prefix = layout.prefix.clone();
    state.upsert_object(InstalledObject {
        kind: ObjectKind::Component,
        name: resolved.component.clone(),
        version: resolved.entry.version.clone(),
        status: ObjectStatus::Installed,
        // Embedded-manifest digest verification is future work; recording
        // an unverified digest would overstate what install checked.
        manifest_digest: None,
        distribution_source: Some(resolved.artifact_url.clone()),
        install_backend: Some(resolved.backend.clone()),
        installed_at: started_at.clone(),
        last_operation_id: Some(operation_id.clone()),
        managed: true,
        adopted: false,
        subscription_scope: Default::default(),
        enabled_features: Vec::new(),
        component_refs: Vec::new(),
        files: owned_files,
        external_modified_files: Vec::new(),
        services: resolved
            .services
            .iter()
            .map(|svc| ServiceRef {
                name: svc.clone(),
                manager: service_manager.to_string(),
                restartable: true,
                // Service enablement is deferred to a later milestone.
                enabled: false,
            })
            .collect(),
        health: Vec::new(),
    });
    state.operations.push(OperationRecord {
        id: operation_id.clone(),
        command: command.to_string(),
        status: "ok".to_string(),
        started_at: started_at.clone(),
        finished_at: Some(now_iso8601()),
    });

    let state_path = layout.state_dir.join("installed.toml");
    if let Err(err) = state.save(&state_path) {
        rollback_installed_files(&outcome.files);
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "failed to save state; attempted best-effort rollback of installed files (some may remain on disk): {err}"
            ),
        });
    }

    // Audit log is best-effort: the install already succeeded and state is
    // saved, so a log failure downgrades to a warning instead of unwinding.
    let log = CentralLog::open(layout.central_log.clone());
    if !pruned_legacy.is_empty() {
        // Warn-severity so `logs --level warn` surfaces the migration.
        let prune_record = LogRecord {
            kind: LogKind::Operation,
            operation_id: Some(operation_id.clone()),
            command: command.to_string(),
            source: "anolisa-cli".to_string(),
            component: None,
            severity: Severity::Warn,
            message: format!(
                "pruned legacy capability state object(s) written by an older release: {}",
                pruned_legacy.join(", ")
            ),
            actor: "cli".to_string(),
            install_mode: Some(ctx.install_mode.as_str().to_string()),
            started_at: started_at.clone(),
            finished_at: Some(now_iso8601()),
            status: None,
            objects: pruned_legacy.clone(),
            backup_ids: Vec::new(),
            warnings: Vec::new(),
            details: serde_json::Value::Null,
        };
        if let Err(err) = log.append(&prune_record) {
            eprintln!("warning: failed to write central log: {err}");
        }
    }
    let record = LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.clone()),
        command: command.to_string(),
        source: "anolisa-cli".to_string(),
        component: Some(resolved.component.clone()),
        severity: Severity::Info,
        message: format!(
            "component {} {} installed via {} backend",
            resolved.component, resolved.entry.version, resolved.backend
        ),
        actor: "cli".to_string(),
        install_mode: Some(ctx.install_mode.as_str().to_string()),
        started_at,
        finished_at: Some(now_iso8601()),
        status: Some(LogStatus::Ok),
        objects: vec![resolved.component.clone()],
        backup_ids: Vec::new(),
        warnings: resolved.warnings.clone(),
        details: serde_json::Value::Null,
    };
    if let Err(err) = log.append(&record) {
        eprintln!("warning: failed to write central log: {err}");
    }

    let payload = InstallResultPayload {
        component: resolved.component,
        package: resolved.package,
        version: resolved.entry.version,
        backend: resolved.backend,
        base_url: resolved.base_url,
        install_mode: ctx.install_mode.as_str().to_string(),
        operation_id,
        artifact_url: resolved.artifact_url,
        files_installed: installed_paths,
        services: resolved.services,
        warnings: resolved.warnings,
    };
    if ctx.json {
        return render_json(command, &payload);
    }
    if !ctx.quiet {
        render_result(&payload, ctx.no_color);
    }
    Ok(())
}

fn ensure_component_backend_compatible(
    state: &InstalledState,
    component: &str,
    requested_backend: &str,
    command: &str,
) -> Result<(), CliError> {
    let Some(obj) = state.find_object(ObjectKind::Component, component) else {
        return Ok(());
    };

    match installed_backend_label(obj) {
        Some(installed_backend) if installed_backend == requested_backend => Ok(()),
        Some(installed_backend) => Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "component '{component}' is already installed via backend '{installed_backend}'; reinstalling it via backend '{requested_backend}' is not allowed — uninstall it first or use backend '{installed_backend}'",
            ),
        }),
        None => Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "component '{component}' is already installed but its install backend is unknown; uninstall it before installing via backend '{requested_backend}'",
            ),
        }),
    }
}

fn installed_backend_label(obj: &InstalledObject) -> Option<&str> {
    obj.install_backend
        .as_deref()
        .or_else(|| infer_backend_from_distribution_source(obj.distribution_source.as_deref()))
}

fn infer_backend_from_distribution_source(source: Option<&str>) -> Option<&'static str> {
    let source = source?;
    if source.starts_with("http://")
        || source.starts_with("https://")
        || source.starts_with("file://")
    {
        Some("raw")
    } else {
        None
    }
}

fn render_plan(ctx: &CliContext, resolved: &ResolvedInstall) -> Result<(), CliError> {
    let payload = InstallPlanPayload {
        component: resolved.component.clone(),
        package: resolved.package.clone(),
        version: resolved.entry.version.clone(),
        backend: resolved.backend.clone(),
        base_url: resolved.base_url.clone(),
        install_mode: ctx.install_mode.as_str().to_string(),
        artifact: ArtifactInfo {
            r#type: artifact_type_wire(&resolved.entry.artifact_type).to_string(),
            url: resolved.artifact_url.clone(),
            sha256: resolved.entry.sha256.clone(),
        },
        files: resolved
            .files
            .iter()
            .map(|f| f.dest.display().to_string())
            .collect(),
        services: resolved.services.clone(),
        dry_run: true,
        warnings: resolved.warnings.clone(),
    };

    if ctx.json {
        return render_json(COMMAND, &payload);
    }
    if ctx.quiet {
        return Ok(());
    }
    let color = Palette::new(ctx.no_color);
    println!(
        "{} {} v{} {}",
        color.command("install"),
        payload.component,
        payload.version,
        color.muted("(dry-run — nothing installed)"),
    );
    println!("{} {}", color.label("backend:"), payload.backend);
    println!(
        "{} {}",
        color.label("base_url:"),
        color.path(&payload.base_url)
    );
    println!("{} {}", color.label("package:"), payload.package);
    println!("{} {}", color.label("install_mode:"), payload.install_mode);
    println!(
        "{} {} ({})",
        color.label("artifact:"),
        color.path(&payload.artifact.url),
        payload.artifact.r#type
    );
    println!("{}", color.header("files:"));
    for f in &payload.files {
        println!("  - {}", color.path(f));
    }
    if !payload.services.is_empty() {
        println!("{}", color.header("services (recorded, not started):"));
        for s in &payload.services {
            println!("  - {s}");
        }
    }
    render_warnings(&payload.warnings, &color);
    Ok(())
}

fn render_result(payload: &InstallResultPayload, no_color: bool) {
    let color = Palette::new(no_color);
    println!(
        "{} {} v{} {}",
        color.command("install"),
        payload.component,
        payload.version,
        color.ok("succeeded"),
    );
    println!("{} {}", color.label("backend:"), payload.backend);
    println!("{} {}", color.label("package:"), payload.package);
    println!(
        "{} {}",
        color.label("operation_id:"),
        color.id(&payload.operation_id)
    );
    println!(
        "{} {}",
        color.label("files installed:"),
        payload.files_installed.len()
    );
    for p in &payload.files_installed {
        println!("  - {}", color.path(p));
    }
    if !payload.services.is_empty() {
        println!("{}", color.header("services (recorded, not started):"));
        for s in &payload.services {
            println!("  - {s}");
        }
    }
    render_warnings(&payload.warnings, &color);
}

fn render_warnings(warnings: &[String], color: &Palette) {
    if warnings.is_empty() {
        return;
    }
    println!("{}", color.warn("warnings:"));
    for w in warnings {
        println!("  - {w}");
    }
}

/// Route a [`RepoConfigError`] to the CLI error surface.
///
/// `caller_fixable` decides the bucket: selection/substitution/override
/// errors are actionable by the caller (pass a different `--backend`,
/// fix `[vars]`, fix the `--repo` URL) → INVALID_ARGUMENT (exit 2);
/// discovery/IO/parse failures mean the config asset itself is broken →
/// EXECUTION_FAILED (exit 1), mirroring the execution-policy split.
fn repo_config_err(err: RepoConfigError, caller_fixable: bool) -> CliError {
    if caller_fixable {
        CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: err.to_string(),
        }
    } else {
        CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!("failed to load repo config: {err}"),
        }
    }
}

/// `{ext}` placeholder value for the conventional file name. Single-file
/// artifacts ship bare; OCI rows are references, not downloadable files,
/// and never resolve through URL derivation.
fn artifact_ext(t: &ArtifactType) -> &'static str {
    match t {
        ArtifactType::TarGz => ".tar.gz",
        ArtifactType::Zip => ".zip",
        ArtifactType::Rpm => ".rpm",
        ArtifactType::Deb => ".deb",
        ArtifactType::Binary | ArtifactType::File | ArtifactType::Oci => "",
    }
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

/// ISO 8601 UTC timestamp with second precision.
fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::context::InstallMode;
    use sha2::{Digest, Sha256};
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    fn ctx(json: bool) -> CliContext {
        ctx_with_prefix(json, None)
    }

    // `--prefix` only rebases system-mode layouts (user mode resolves from
    // $HOME), so isolation tests run in System mode under a tempdir to keep
    // every filesystem probe (repo.toml, state, cache) away from the host.
    fn ctx_with_prefix(json: bool, prefix: Option<PathBuf>) -> CliContext {
        CliContext {
            install_mode: if prefix.is_some() {
                InstallMode::System
            } else {
                InstallMode::User
            },
            prefix,
            json,
            dry_run: false,
            verbose: false,
            quiet: true, // suppress stdout during tests
            no_color: true,
        }
    }

    fn args(component: &str) -> InstallArgs {
        InstallArgs {
            component: component.to_string(),
            version: None,
            backend: None,
            repo: None,
            package: None,
        }
    }

    /// Lay out a local file:// raw repo containing one binary artifact for
    /// `agentsight` targeting the *detected* host os/arch, and return the
    /// repo's raw v1 root. Uses a repo-relative artifact URL to also exercise
    /// the relative-URL join.
    fn write_local_repo(root: &Path) -> String {
        let v1 = root.join("v1");
        std::fs::create_dir_all(&v1).expect("create repo dirs");

        let payload = b"#!/bin/sh\necho agentsight\n";
        std::fs::write(v1.join("agentsight-bin"), payload).expect("write artifact");
        let sha = format!("{:x}", Sha256::digest(payload));

        let env = anolisa_env::EnvService::detect();
        let index = format!(
            r#"schema_version = 1
channel = "stable"
publisher = "test"

[[entries]]
component = "agentsight"
version = "0.2.0"
channel = "stable"
artifact_type = "binary"
backend = "raw"
url = "agentsight-bin"
os = "{os}"
arch = "{arch}"
install_modes = ["system"]
sha256 = "{sha}"
"#,
            os = env.os,
            arch = env.arch,
        );
        std::fs::write(v1.join("index.toml"), index).expect("write index");
        format!("file://{}", v1.display())
    }

    /// Like [`write_local_repo`], but the index row omits `url` and the
    /// artifact sits at the conventional publish path
    /// `{component}/{version}/{os}/{arch}/{component}-{version}-{os}-{arch}`
    /// under the raw v1 root.
    fn write_conventional_repo(root: &Path) -> String {
        let env = anolisa_env::EnvService::detect();
        let artifact_dir = root
            .join("v1/agentsight/0.2.0")
            .join(&env.os)
            .join(&env.arch);
        std::fs::create_dir_all(&artifact_dir).expect("create repo dirs");

        let payload = b"#!/bin/sh\necho agentsight\n";
        let file_name = format!("agentsight-0.2.0-{}-{}", env.os, env.arch);
        std::fs::write(artifact_dir.join(file_name), payload).expect("write artifact");
        let sha = format!("{:x}", Sha256::digest(payload));

        let index = format!(
            r#"schema_version = 1
channel = "stable"
publisher = "test"

[[entries]]
component = "agentsight"
version = "0.2.0"
channel = "stable"
artifact_type = "binary"
backend = "raw"
os = "{os}"
arch = "{arch}"
install_modes = ["system"]
sha256 = "{sha}"
"#,
            os = env.os,
            arch = env.arch,
        );
        std::fs::write(root.join("v1/index.toml"), index).expect("write index");
        format!("file://{}", root.join("v1").display())
    }

    #[test]
    fn install_cli_rejects_multiple_components() {
        let err = InstallArgs::try_parse_from(["install", "agentsight", "tokenless"])
            .expect_err("must reject extra positional arguments");
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn install_unknown_component_is_invalid_argument() {
        let err = handle(args("no-such-component"), &ctx(false)).expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("no-such-component"));
    }

    /// agentsight's manifest declares `modes = ["system"]`; the mode gate
    /// fires before any repo/index IO, so plain user-mode ctx is safe here.
    #[test]
    fn install_unsupported_mode_is_invalid_argument() {
        let err = handle(args("agentsight"), &ctx(false)).expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("does not support user-mode"),
            "got: {}",
            err.reason()
        );
    }

    /// `--backend` naming a known-but-unconfigured backend is caller
    /// input → INVALID_ARGUMENT, with the hint naming repo.toml.
    #[test]
    fn install_unconfigured_backend_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let mut a = args("agentsight");
        a.backend = Some("npm".to_string());
        let err = handle(a, &ctx_with_prefix(false, Some(tmp.path().to_path_buf())))
            .expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("npm"), "got: {}", err.reason());
        assert!(
            err.reason().contains("repo.toml"),
            "reason must point at repo.toml: {}",
            err.reason()
        );
    }

    #[test]
    fn install_unknown_backend_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let mut a = args("agentsight");
        a.backend = Some("pip".to_string());
        let err = handle(a, &ctx_with_prefix(false, Some(tmp.path().to_path_buf())))
            .expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("pip"));
    }

    /// A configured non-raw backend selects fine but has no executor yet.
    #[test]
    fn install_configured_yum_backend_is_not_implemented() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().to_path_buf();
        let layout = FsLayout::system(Some(prefix.clone()));
        std::fs::create_dir_all(&layout.etc_dir).expect("etc dir");
        std::fs::write(
            layout.etc_dir.join("repo.toml"),
            r#"schema_version = 1
default_backend = "raw"

[backends.raw]
base_url = "https://example.com/anolisa"

[backends.yum]
base_url = "https://example.com/yum-repo"
"#,
        )
        .expect("write repo.toml");

        let mut a = args("agentsight");
        a.backend = Some("yum".to_string());
        let err = handle(a, &ctx_with_prefix(false, Some(prefix))).expect_err("must error");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
        assert!(err.reason().contains("yum"), "got: {}", err.reason());
    }

    /// A malformed `--repo` URL fails the same shape rules as configured
    /// base_urls and routes to INVALID_ARGUMENT.
    #[test]
    fn install_invalid_repo_override_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let mut a = args("agentsight");
        a.repo = Some("ftp://example.com/repo".to_string());
        let err = handle(a, &ctx_with_prefix(false, Some(tmp.path().to_path_buf())))
            .expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("ftp"), "got: {}", err.reason());
    }

    /// Dry-run resolves through the real index (fetch + ResolveQuery +
    /// file rendering) but must not install anything or create state.
    #[test]
    fn install_dry_run_resolves_without_writing_files() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let repo_url = write_local_repo(&tmp.path().join("repo"));

        let mut a = args("agentsight");
        a.repo = Some(repo_url);
        let mut ctx = ctx_with_prefix(false, Some(prefix.clone()));
        ctx.dry_run = true;
        handle(a, &ctx).expect("dry-run must succeed");

        let layout = FsLayout::system(Some(prefix));
        assert!(
            !layout.bin_dir.join("agentsight").exists(),
            "dry-run must not install the binary"
        );
        assert!(
            !layout.state_dir.join("installed.toml").exists(),
            "dry-run must not write state"
        );
    }

    /// End-to-end raw install from a local file:// repo: resolve via the
    /// repo-relative artifact URL, verify sha256, install the binary to
    /// {bindir}, and persist component state.
    #[test]
    fn install_raw_end_to_end_from_local_repo() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let repo_url = write_local_repo(&tmp.path().join("repo"));

        let mut a = args("agentsight");
        a.repo = Some(repo_url.clone());
        handle(a, &ctx_with_prefix(false, Some(prefix.clone()))).expect("install must succeed");

        let layout = FsLayout::system(Some(prefix));
        let bin = layout.bin_dir.join("agentsight");
        assert!(bin.exists(), "binary must be installed at {{bindir}}");

        let state = anolisa_core::InstalledState::load(&layout.state_dir.join("installed.toml"))
            .expect("state must load");
        let obj = state
            .find_object(ObjectKind::Component, "agentsight")
            .expect("component object must be recorded");
        assert_eq!(obj.version, "0.2.0");
        assert_eq!(obj.status, ObjectStatus::Installed);
        assert_eq!(obj.files.len(), 1);
        assert!(
            obj.distribution_source
                .as_deref()
                .is_some_and(|u| u.starts_with(&repo_url)),
            "distribution_source must record the resolved artifact URL"
        );
        assert_eq!(
            obj.install_backend.as_deref(),
            Some("raw"),
            "install_backend must record the selected backend"
        );
        assert!(
            obj.services.iter().all(|s| !s.enabled),
            "install must not mark services enabled"
        );
        assert_eq!(state.operations.len(), 1);
        assert!(state.operations[0].id.starts_with("op-install-"));
    }

    #[test]
    fn install_existing_component_with_different_backend_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let layout = FsLayout::system(Some(prefix.clone()));
        std::fs::create_dir_all(&layout.etc_dir).expect("etc dir");
        std::fs::create_dir_all(&layout.state_dir).expect("state dir");
        std::fs::write(
            layout.etc_dir.join("repo.toml"),
            r#"schema_version = 1
default_backend = "raw"

[backends.raw]
base_url = "https://example.com/anolisa"

[backends.yum]
base_url = "https://example.com/yum-repo"
"#,
        )
        .expect("write repo.toml");

        let mut state = anolisa_core::InstalledState {
            install_mode: StateInstallMode::System,
            prefix: layout.prefix.clone(),
            ..Default::default()
        };
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Component,
            name: "agentsight".to_string(),
            version: "0.2.0".to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: None,
            distribution_source: Some("file:///repo/v1/agentsight-bin".to_string()),
            install_backend: Some("raw".to_string()),
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: Some("op-prior".to_string()),
            managed: true,
            adopted: false,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        });
        state
            .save(&layout.state_dir.join("installed.toml"))
            .expect("save state");

        let mut a = args("agentsight");
        a.backend = Some("yum".to_string());
        let err = handle(a, &ctx_with_prefix(false, Some(prefix))).expect_err("must error");

        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("already installed via backend 'raw'")
                && err.reason().contains("backend 'yum'"),
            "reason must explain backend conflict: {}",
            err.reason()
        );
    }

    /// An index row without `url` installs from the code-owned raw layout
    /// under the raw v1 root.
    #[test]
    fn install_derives_artifact_url_from_convention_when_index_omits_url() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let repo_url = write_conventional_repo(&tmp.path().join("repo"));

        let mut a = args("agentsight");
        a.repo = Some(repo_url.clone());
        handle(a, &ctx_with_prefix(false, Some(prefix.clone()))).expect("install must succeed");

        let layout = FsLayout::system(Some(prefix));
        assert!(layout.bin_dir.join("agentsight").exists());

        let state = anolisa_core::InstalledState::load(&layout.state_dir.join("installed.toml"))
            .expect("state must load");
        let obj = state
            .find_object(ObjectKind::Component, "agentsight")
            .expect("component object must be recorded");
        let env = anolisa_env::EnvService::detect();
        assert_eq!(
            obj.distribution_source.as_deref(),
            Some(
                format!(
                    "{repo_url}/agentsight/0.2.0/{os}/{arch}/agentsight-0.2.0-{os}-{arch}",
                    os = env.os,
                    arch = env.arch
                )
                .as_str()
            ),
            "distribution_source must record the convention-derived URL"
        );
    }

    /// A legacy template-form repo URL still resolves by taking the static
    /// prefix before `{component}` as the raw v1 root.
    #[test]
    fn install_resolves_legacy_template_form_repo_url() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let repo_root = tmp.path().join("repo");
        // write_conventional_repo puts the tree under <root>/v1/; point the
        // template's static prefix at that same directory.
        let _ = write_conventional_repo(&repo_root);
        let template_url = format!(
            "file://{}/v1/{{component}}/{{version}}/{{os}}/{{arch}}/",
            repo_root.display()
        );

        let mut a = args("agentsight");
        a.repo = Some(template_url);
        handle(a, &ctx_with_prefix(false, Some(prefix.clone()))).expect("install must succeed");

        let layout = FsLayout::system(Some(prefix));
        assert!(layout.bin_dir.join("agentsight").exists());

        let state = anolisa_core::InstalledState::load(&layout.state_dir.join("installed.toml"))
            .expect("state must load");
        let obj = state
            .find_object(ObjectKind::Component, "agentsight")
            .expect("component object must be recorded");
        let env = anolisa_env::EnvService::detect();
        assert_eq!(
            obj.distribution_source.as_deref(),
            Some(
                format!(
                    "file://{}/v1/agentsight/0.2.0/{os}/{arch}/agentsight-0.2.0-{os}-{arch}",
                    repo_root.display(),
                    os = env.os,
                    arch = env.arch
                )
                .as_str()
            ),
            "distribution_source must record the convention-derived URL"
        );
    }

    /// Requesting a version the index does not publish is caller input.
    #[test]
    fn install_unpublished_version_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let repo_url = write_local_repo(&tmp.path().join("repo"));

        let mut a = args("agentsight");
        a.repo = Some(repo_url);
        a.version = Some("9.9.9".to_string());
        let err =
            handle(a, &ctx_with_prefix(false, Some(prefix))).expect_err("must fail to resolve");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("9.9.9"), "got: {}", err.reason());
    }
}
