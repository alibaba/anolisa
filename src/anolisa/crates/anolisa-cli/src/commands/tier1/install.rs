//! `anolisa install` — install a component through a configured backend.
//!
//! `install` takes a component noun and resolves it through the configured
//! backend. The resolution chain — repo.toml loading, backend selection
//! (`--backend` > `default_backend`), base_url variable substitution, and
//! package-name mapping (`--package` > `package_map` > scope > component
//! name) — feeds the **raw** backend executor: fetch the distribution index
//! from the raw repository root, resolve an artifact, then execute by
//! downloading it with mandatory sha256 verification, loading the install
//! contract, installing the declared files, and recording state plus a
//! central-log audit entry. `yum` / `npm` backends are selectable but their
//! executors are NOT_IMPLEMENTED.
//!
//! Deliberately out of scope for this milestone: execution-policy gating,
//! pre/post hooks, health checks, and service start/enable. Installed
//! services are recorded in state with `enabled: false`.

use clap::Parser;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anolisa_core::central_log::{CentralLog, LogKind, LogRecord, LogStatus, Severity};
use anolisa_core::download::{DownloadCache, DownloadError};
use anolisa_core::install_runner::{
    InstallRunner, ResolvedInstallFile, SUPPORTED_ARTIFACT_TYPES,
    read_embedded_component_manifest_text,
};
use anolisa_core::lock::InstallLock;
use anolisa_core::path_safety::validate_owned_path;
use anolisa_core::state::{
    FileOwner, InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectKind,
    ObjectStatus, OperationRecord, OwnedFile, Ownership, ServiceRef,
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
use crate::response::{CliError, render_json, render_json_with_status};

const COMMAND: &str = "install";

#[derive(Debug, Parser)]
// `--version` here means the *component* version (the `cargo install`
// convention), so the auto-generated CLI-version flag must be disabled
// to free the name. `anolisa --version` still works at the top level.
#[command(disable_version_flag = true)]
#[command(group(
    clap::ArgGroup::new("target")
        .required(true)
        .args(["component", "all"]),
))]
pub struct InstallArgs {
    /// Component name to install
    #[arg(value_name = "COMPONENT")]
    pub component: Option<String>,
    /// Install every component listed in the catalog (mutually exclusive with COMPONENT)
    #[arg(long, conflicts_with_all = ["component", "version", "package"])]
    pub all: bool,
    /// With --all, stop on the first failure instead of continuing
    #[arg(long, requires = "all")]
    pub fail_fast: bool,
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

/// Raw backend resolution shared by dry-run preview and real execution.
struct RawResolution {
    component: String,
    package: String,
    backend: String,
    base_url: String,
    entry: DistributionEntry,
    artifact_url: String,
    warnings: Vec<String>,
}

/// Dry-run preview after optional lightweight metadata expansion.
struct InstallPreview {
    resolution: RawResolution,
    files: Vec<ResolvedInstallFile>,
    services: Vec<String>,
}

/// Execution input after the artifact has been verified and its install
/// contract has been resolved.
struct PreparedInstall {
    resolution: RawResolution,
    artifact_path: PathBuf,
    files: Vec<ResolvedInstallFile>,
    services: Vec<String>,
    manifest_toml: String,
}

/// Parsed install contract plus the TOML persisted as the local install fact.
struct LoadedInstallContract {
    manifest: ComponentManifest,
    source: InstallContractSource,
    toml: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallContractSource {
    EmbeddedArtifact,
    SidecarMeta,
    LocalCatalog,
}

#[derive(Serialize)]
struct ArtifactInfo {
    r#type: String,
    url: String,
    sha256: Option<String>,
}

/// Wire shape for `--dry-run`: the resolution result without downloading
/// the install artifact.
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
    if args.fail_fast && !args.all {
        return Err(CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: "--fail-fast is only meaningful with --all".to_string(),
        });
    }
    if args.all {
        return handle_all(args, ctx);
    }
    // clap ArgGroup guarantees at least one of `component` / `--all`; with
    // `--all` ruled out above, `component` is necessarily Some.
    let component = args
        .component
        .clone()
        .expect("clap ArgGroup ensures component is set when --all is absent");
    handle_one(component, args, ctx)
}

fn handle_one(component: String, args: InstallArgs, ctx: &CliContext) -> Result<(), CliError> {
    let command = format!("install {component}");

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
        let preview = build_install_preview(ctx, &layout, resolved)?;
        return render_plan(ctx, &preview);
    }

    let prepared = prepare_raw_execution(ctx, &layout, resolved)?;
    execute_raw(ctx, &layout, &command, prepared)
}

// ── --all support ───────────────────────────────────────────────────

/// Minimal catalog shape used by `--all`. We only need the component name
/// and (optionally) the `available` status, so this is a thin parse rather
/// than a re-use of `list.rs`'s richer types. Keeping it local avoids a
/// cross-module type dependency for one entry point.
#[derive(Debug, Deserialize)]
struct AllCatalogV1 {
    schema_version: u32,
    #[serde(default)]
    components: Vec<AllCatalogEntry>,
}

#[derive(Debug, Deserialize)]
struct AllCatalogEntry {
    name: String,
    #[serde(default)]
    status: Option<String>,
}

/// Wire shape for a batch entry.  `status` is one of:
/// `installed` | `planned` (dry-run) | `failed` | `skipped`.
#[derive(Serialize)]
struct AllSummaryItem {
    component: String,
    status: &'static str,
    reason: Option<String>,
}

#[derive(Serialize)]
struct AllSummaryPayload {
    total: usize,
    installed: usize,
    planned: usize,
    failed: usize,
    skipped: usize,
    dry_run: bool,
    items: Vec<AllSummaryItem>,
}

fn handle_all(args: InstallArgs, ctx: &CliContext) -> Result<(), CliError> {
    let names = resolve_all_components(ctx)?;
    if names.is_empty() {
        if !ctx.quiet && !ctx.json {
            let color = Palette::new(ctx.no_color);
            println!(
                "{}",
                color.muted("no available components in catalog; nothing to install")
            );
        }
        if ctx.json {
            return render_json(
                "install --all",
                AllSummaryPayload {
                    total: 0,
                    installed: 0,
                    planned: 0,
                    failed: 0,
                    skipped: 0,
                    dry_run: ctx.dry_run,
                    items: Vec::new(),
                },
            );
        }
        return Ok(());
    }

    // Suppress per-component rendering: handle_all owns the final output.
    // Each handle_one call runs in quiet mode so it doesn't print individual
    // JSON envelopes or human-mode messages — only the batch summary at the
    // end goes to stdout.
    let suppressed_ctx = CliContext {
        json: false,
        quiet: true,
        ..ctx.clone()
    };

    // Dry-run successes are "planned" rather than "installed": no files or
    // state were written.
    let ok_status: &'static str = if ctx.dry_run { "planned" } else { "installed" };

    let mut items: Vec<AllSummaryItem> = Vec::with_capacity(names.len());
    let mut first_error: Option<CliError> = None;
    let mut last_processed = 0usize;

    for (idx, name) in names.iter().enumerate() {
        last_processed = idx;
        if !ctx.quiet && !ctx.json {
            let color = Palette::new(ctx.no_color);
            println!("{} {name}", color.label("==>"));
        }
        let per_args = InstallArgs {
            component: Some(name.clone()),
            all: false,
            fail_fast: false,
            version: None,
            backend: args.backend.clone(),
            repo: args.repo.clone(),
            package: None,
        };
        match handle_one(name.clone(), per_args, &suppressed_ctx) {
            Ok(()) => items.push(AllSummaryItem {
                component: name.clone(),
                status: ok_status,
                reason: None,
            }),
            Err(err) => {
                let reason = err.reason().to_string();
                items.push(AllSummaryItem {
                    component: name.clone(),
                    status: "failed",
                    reason: Some(reason),
                });
                if first_error.is_none() {
                    first_error = Some(err);
                }
                if args.fail_fast {
                    break;
                }
            }
        }
    }

    // --fail-fast may have left components unprocessed.  Mark them as
    // skipped so `total` always equals the full target set.
    for name in &names[last_processed + 1..] {
        items.push(AllSummaryItem {
            component: name.clone(),
            status: "skipped",
            reason: Some("--fail-fast: not attempted".to_string()),
        });
    }

    let installed = items.iter().filter(|i| i.status == "installed").count();
    let planned = items.iter().filter(|i| i.status == "planned").count();
    let failed = items.iter().filter(|i| i.status == "failed").count();
    let skipped = items.iter().filter(|i| i.status == "skipped").count();

    if ctx.json {
        // The batch summary is the single, complete JSON response.  We
        // return BatchPartial (not Ok) so that main's render_error still
        // sets a non-zero exit code — but render_error recognises
        // BatchPartial and skips the second JSON render.
        render_json_with_status(
            "install --all",
            failed == 0,
            AllSummaryPayload {
                total: names.len(),
                installed,
                planned,
                failed,
                skipped,
                dry_run: ctx.dry_run,
                items,
            },
        )?;
        return match first_error {
            Some(_) => Err(CliError::BatchPartial {
                command: "install --all".to_string(),
            }),
            None => Ok(()),
        };
    }

    if !ctx.quiet {
        let color = Palette::new(ctx.no_color);
        println!();
        let failed_names: Vec<&str> = items
            .iter()
            .filter(|i| i.status == "failed")
            .map(|i| i.component.as_str())
            .collect();
        let ok_word = if ctx.dry_run { "planned" } else { "installed" };
        let ok_count = if ctx.dry_run { planned } else { installed };
        if failed_names.is_empty() {
            println!(
                "{} total={}  {ok_word}={}  skipped={}",
                color.label("summary:"),
                names.len(),
                ok_count,
                skipped,
            );
        } else {
            println!(
                "{} total={}  {ok_word}={}  failed={} ({})  skipped={}",
                color.label("summary:"),
                names.len(),
                ok_count,
                failed,
                failed_names.join(", "),
                skipped,
            );
            for item in items.iter().filter(|i| i.status == "failed") {
                if let Some(reason) = &item.reason {
                    eprintln!("{} {}: {reason}", color.err("failed:"), item.component);
                }
            }
        }
    }

    // Human mode: preserve non-zero exit code on failure.
    match first_error {
        Some(_) => Err(CliError::BatchPartial {
            command: "install --all".to_string(),
        }),
        None => Ok(()),
    }
}

/// Fetch and parse the component catalog, returning the names of components
/// whose `status` is `available`. Returns an error if the catalog URL is not
/// configured or the catalog cannot be fetched/parsed.
fn resolve_all_components(ctx: &CliContext) -> Result<Vec<String>, CliError> {
    let url = common::resolve_catalog_url(ctx, "install --all")?.ok_or_else(|| {
        CliError::InvalidArgument {
            command: "install --all".to_string(),
            reason: "component catalog is not configured; set ANOLISA_CATALOG_URL or \
                 configure [backends.raw].base_url in repo.toml"
                .to_string(),
        }
    })?;
    let bytes = common::fetch_catalog_bytes(&url, "install --all")?;
    let catalog: AllCatalogV1 =
        serde_json::from_slice(&bytes).map_err(|err| CliError::InvalidArgument {
            command: "install --all".to_string(),
            reason: format!("failed to parse component catalog JSON: {err}"),
        })?;
    if catalog.schema_version != 1 {
        return Err(CliError::InvalidArgument {
            command: "install --all".to_string(),
            reason: format!(
                "unsupported component catalog schema_version {}; expected 1",
                catalog.schema_version
            ),
        });
    }
    let mut names: Vec<String> = Vec::new();
    for entry in catalog.components {
        if entry.name.trim().is_empty() {
            if !ctx.quiet {
                eprintln!("warning: catalog contains an entry with an empty name; skipping");
            }
            continue;
        }
        if entry.status.as_deref() == Some("available") {
            names.push(entry.name);
        }
    }
    Ok(names)
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

/// Resolve raw backend metadata without fetching the artifact.
///
/// This fetches the distribution index into the download cache, selects a
/// supported artifact, and derives the artifact URL. Execution later
/// downloads the artifact and reads its install contract; dry-run may read
/// lightweight `meta.toml` metadata for a richer preview.
fn resolve_raw(
    ctx: &CliContext,
    layout: &FsLayout,
    env: &anolisa_env::EnvFacts,
    inputs: ResolveInputs<'_>,
) -> Result<RawResolution, CliError> {
    let ResolveInputs {
        component,
        package,
        backend,
        base_url,
        version,
        warnings,
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

    Ok(RawResolution {
        component,
        package,
        backend,
        base_url,
        artifact_url,
        entry,
        warnings,
    })
}

impl InstallContractSource {
    fn label(self) -> &'static str {
        match self {
            Self::EmbeddedArtifact => "embedded artifact manifest",
            Self::SidecarMeta => "sidecar meta.toml",
            Self::LocalCatalog => "local catalog manifest",
        }
    }
}

fn build_install_preview(
    ctx: &CliContext,
    layout: &FsLayout,
    mut resolution: RawResolution,
) -> Result<InstallPreview, CliError> {
    if resolution.entry.sha256.is_none() {
        resolution.warnings.push(format!(
            "distribution entry for '{}' {} has no sha256; execute will refuse to install it",
            resolution.package, resolution.entry.version
        ));
    }

    let Some(contract) = load_lightweight_install_contract(ctx, layout, &resolution)? else {
        resolution.warnings.push(format!(
            "dry-run did not download artifact {}; file and service details are unavailable",
            resolution.artifact_url
        ));
        return Ok(InstallPreview {
            resolution,
            files: Vec::new(),
            services: Vec::new(),
        });
    };

    let (files, services) = match resolve_manifest_contract(
        &contract.manifest,
        layout,
        &resolution,
        ctx.install_mode.as_str(),
        contract.source,
    ) {
        Ok(contract_files) => contract_files,
        Err(err) if contract.source == InstallContractSource::LocalCatalog => {
            resolution.warnings.push(format!(
                "local catalog manifest does not match resolved artifact; file and service details are unavailable: {}",
                err.reason()
            ));
            (Vec::new(), Vec::new())
        }
        Err(err) => return Err(err),
    };

    Ok(InstallPreview {
        resolution,
        files,
        services,
    })
}

fn prepare_raw_execution(
    ctx: &CliContext,
    layout: &FsLayout,
    resolution: RawResolution,
) -> Result<PreparedInstall, CliError> {
    let sha256 = resolution.entry.sha256.as_deref().ok_or_else(|| {
        CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "distribution entry for '{}' {} has no sha256 — refusing to install an unverifiable artifact",
                resolution.package, resolution.entry.version
            ),
        }
    })?;

    let cache = DownloadCache::new(layout.cache_dir.clone());
    let artifact = cache
        .fetch(&resolution.artifact_url, Some(sha256))
        .map_err(|err| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "failed to download artifact {}: {err}",
                resolution.artifact_url
            ),
        })?;

    let contract =
        load_execution_install_contract(ctx, layout, &resolution, &artifact.cached_path)?;
    let (files, services) = resolve_manifest_contract(
        &contract.manifest,
        layout,
        &resolution,
        ctx.install_mode.as_str(),
        contract.source,
    )?;

    Ok(PreparedInstall {
        resolution,
        artifact_path: artifact.cached_path,
        files,
        services,
        manifest_toml: contract.toml,
    })
}

fn load_execution_install_contract(
    ctx: &CliContext,
    layout: &FsLayout,
    resolution: &RawResolution,
    artifact_path: &Path,
) -> Result<LoadedInstallContract, CliError> {
    match resolution.entry.artifact_type {
        ArtifactType::TarGz => {
            let toml = read_embedded_component_manifest_text(artifact_path)
                .map_err(|err| CliError::Runtime {
                    command: COMMAND.to_string(),
                    reason: format!(
                        "failed to read embedded component manifest from {}: {err}",
                        resolution.artifact_url
                    ),
                })?
                .ok_or_else(|| CliError::Runtime {
                    command: COMMAND.to_string(),
                    reason: format!(
                        "published artifact for package '{}' has no embedded .anolisa/component.toml",
                        resolution.package
                    ),
                })?;
            let manifest = ComponentManifest::from_toml_str(&toml).map_err(|err| {
                CliError::Runtime {
                    command: COMMAND.to_string(),
                    reason: format!(
                        "failed to parse embedded component manifest from {}: {err}",
                        resolution.artifact_url
                    ),
                }
            })?;
            Ok(LoadedInstallContract {
                manifest,
                source: InstallContractSource::EmbeddedArtifact,
                toml,
            })
        }
        ArtifactType::Binary => {
            load_lightweight_install_contract(ctx, layout, resolution)?.ok_or_else(|| {
                CliError::Runtime {
                    command: COMMAND.to_string(),
                    reason: format!(
                        "binary artifact for package '{}' {} requires sidecar meta.toml or a matching local component manifest",
                        resolution.package, resolution.entry.version
                    ),
                }
            })
        }
        other => Err(CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!(
                "resolved artifact type '{}' is not installable by the raw backend (supported: {})",
                artifact_type_wire(&other),
                SUPPORTED_ARTIFACT_TYPES.join(", ")
            ),
        }),
    }
}

fn load_lightweight_install_contract(
    ctx: &CliContext,
    layout: &FsLayout,
    resolution: &RawResolution,
) -> Result<Option<LoadedInstallContract>, CliError> {
    if let Some(contract) = fetch_sidecar_meta_manifest(layout, resolution)? {
        return Ok(Some(contract));
    }

    load_catalog_manifest(ctx, &resolution.component)
}

fn fetch_sidecar_meta_manifest(
    layout: &FsLayout,
    resolution: &RawResolution,
) -> Result<Option<LoadedInstallContract>, CliError> {
    let Some(meta_url) = sidecar_meta_url(
        &resolution.artifact_url,
        &resolution.entry.component,
        &resolution.entry.version,
    ) else {
        return Ok(None);
    };
    let expected_sha = manifest_digest_sha256(resolution.entry.manifest_digest.as_deref())?;
    let cache = DownloadCache::new(layout.cache_dir.clone());
    let downloaded = match cache.fetch(&meta_url, expected_sha) {
        Ok(downloaded) => downloaded,
        Err(DownloadError::HttpStatus { status: 404, .. }) => return Ok(None),
        Err(DownloadError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(err) => {
            return Err(CliError::Runtime {
                command: COMMAND.to_string(),
                reason: format!("failed to fetch sidecar metadata {meta_url}: {err}"),
            });
        }
    };
    let toml =
        std::fs::read_to_string(&downloaded.cached_path).map_err(|err| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "failed to read sidecar metadata {} from cache: {err}",
                downloaded.cached_path.display()
            ),
        })?;
    let manifest = ComponentManifest::from_toml_str(&toml).map_err(|err| CliError::Runtime {
        command: COMMAND.to_string(),
        reason: format!("failed to parse sidecar metadata {meta_url}: {err}"),
    })?;
    Ok(Some(LoadedInstallContract {
        manifest,
        source: InstallContractSource::SidecarMeta,
        toml,
    }))
}

fn load_catalog_manifest(
    ctx: &CliContext,
    component: &str,
) -> Result<Option<LoadedInstallContract>, CliError> {
    let catalog = common::load_bundled_catalog(ctx, COMMAND)?;
    let Some(manifest) = catalog.component(component).cloned() else {
        return Ok(None);
    };
    let toml = serialize_manifest_toml(&manifest, InstallContractSource::LocalCatalog)?;
    Ok(Some(LoadedInstallContract {
        manifest,
        source: InstallContractSource::LocalCatalog,
        toml,
    }))
}

fn serialize_manifest_toml(
    manifest: &ComponentManifest,
    source: InstallContractSource,
) -> Result<String, CliError> {
    toml::to_string_pretty(manifest).map_err(|err| CliError::Runtime {
        command: COMMAND.to_string(),
        reason: format!(
            "failed to serialize {} for local install metadata: {err}",
            source.label()
        ),
    })
}

fn manifest_digest_sha256(digest: Option<&str>) -> Result<Option<&str>, CliError> {
    match digest {
        None => Ok(None),
        Some(value) => value
            .strip_prefix("sha256:")
            .map(Some)
            .ok_or_else(|| CliError::Runtime {
                command: COMMAND.to_string(),
                reason: format!(
                    "unsupported manifest_digest '{value}' for sidecar metadata verification"
                ),
            }),
    }
}

fn sidecar_meta_url(artifact_url: &str, component: &str, version: &str) -> Option<String> {
    let version_marker = format!("/{component}/{version}/");
    if let Some(idx) = artifact_url.rfind(&version_marker) {
        return Some(format!(
            "{}meta.toml",
            &artifact_url[..idx + version_marker.len()]
        ));
    }

    artifact_url
        .rfind('/')
        .map(|idx| format!("{}/meta.toml", &artifact_url[..idx]))
}

fn resolve_manifest_contract(
    manifest: &ComponentManifest,
    layout: &FsLayout,
    resolution: &RawResolution,
    mode: &str,
    source: InstallContractSource,
) -> Result<(Vec<ResolvedInstallFile>, Vec<String>), CliError> {
    if manifest.component.name.as_str() != resolution.component {
        return Err(CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "{} for package '{}' declares component '{}', expected '{}'",
                source.label(),
                resolution.package,
                manifest.component.name,
                resolution.component
            ),
        });
    }
    if manifest.component.version.as_str() != resolution.entry.version.as_str() {
        return Err(CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "{} for component '{}' declares version {}, but the distribution index resolved {}",
                source.label(),
                resolution.component,
                manifest.component.version,
                resolution.entry.version
            ),
        });
    }

    if !manifest.install.modes.iter().any(|m| m == mode) {
        return Err(CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!(
                "{} for component '{}' is inconsistent with the distribution index: index resolved {mode}-mode support, but manifest declares modes: {}",
                source.label(),
                resolution.component,
                manifest.install.modes.join(", ")
            ),
        });
    }

    let mut files = resolve_manifest_files(manifest, layout, &resolution.component)?;
    if files.is_empty() {
        return Err(CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "component '{}' declares no [install.files] — nothing to install",
                resolution.component
            ),
        });
    }
    // Adapter resources are laid alongside the component's own files, from
    // the same artifact. Install only *places* them under the standard
    // `{datadir}/adapters/<component>/<framework>/` tree — enabling them
    // against a framework is the separate `anolisa adapter enable` step.
    files.extend(resolve_adapter_files(
        manifest,
        layout,
        &resolution.component,
    )?);

    Ok((files, manifest.install.services.clone()))
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

/// Render the manifest's `[[adapters]]` entries into install file mappings.
///
/// Install only *places* adapter resources under the standard
/// `{datadir}/adapters/<component>/<framework>/` tree; it never runs a
/// framework CLI or touches user framework state — that is
/// `anolisa adapter enable`.
///
/// Each entry is linted up front for the fields install needs: a framework,
/// a source, and a destination. The framework does not have to be supported
/// by this ANOLISA build; install only lays data down, while
/// `anolisa adapter enable` decides whether a built-in driver exists.
fn resolve_adapter_files(
    manifest: &ComponentManifest,
    layout: &FsLayout,
    component: &str,
) -> Result<Vec<ResolvedInstallFile>, CliError> {
    if manifest.adapters.is_empty() {
        return Ok(Vec::new());
    }
    let mut files = Vec::with_capacity(manifest.adapters.len());
    for adapter in &manifest.adapters {
        let framework = adapter
            .framework
            .as_deref()
            .ok_or_else(|| CliError::InvalidArgument {
                command: COMMAND.to_string(),
                reason: format!(
                    "component '{component}' has an [[adapters]] entry with no framework"
                ),
            })?;
        let source = adapter
            .source
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CliError::InvalidArgument {
                command: COMMAND.to_string(),
                reason: format!(
                    "component '{component}' adapter for '{framework}' declares no source"
                ),
            })?;
        let dest_template = adapter
            .dest
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CliError::InvalidArgument {
                command: COMMAND.to_string(),
                reason: format!(
                    "component '{component}' adapter for '{framework}' declares no dest"
                ),
            })?;
        let dest = expand_layout_placeholders(dest_template, layout, &[("component", component)])
            .map_err(|err| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!("failed to expand adapter dest '{dest_template}': {err}"),
        })?;
        validate_owned_path(layout, &dest).map_err(|err| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "adapter destination '{}' failed path safety check: {err}",
                dest.display()
            ),
        })?;
        // The runner lays an entire archive subtree only when the source key
        // ends with '/'. An adapter bundle is always a directory, so force
        // directory-prefix semantics regardless of how the manifest wrote it.
        let source = if source.ends_with('/') {
            source.to_string()
        } else {
            format!("{source}/")
        };
        files.push(ResolvedInstallFile {
            source: Some(source),
            dest,
            // Bundle contents are framework-loaded data, not directly
            // executed by ANOLISA; lay them 0644. Per-file modes inside a
            // bundle are not expressible in `[[adapters]]` in the MVP.
            mode: Some("0644".to_string()),
            kind: FileKind::Data,
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
    prepared: PreparedInstall,
) -> Result<(), CliError> {
    let PreparedInstall {
        mut resolution,
        artifact_path,
        files,
        services,
        manifest_toml,
    } = prepared;
    let started_at = now_iso8601();

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
    ensure_component_backend_compatible(
        &state,
        &resolution.component,
        &resolution.backend,
        command,
    )?;

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
            artifact_type_wire(&resolution.entry.artifact_type),
            &artifact_path,
            &files,
        )
        .map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!("install failed: {err}"),
        })?;

    // From this point files are on disk — failures must roll them back.
    let manifest_path =
        match write_installed_component_manifest(layout, &resolution.component, &manifest_toml) {
            Ok(path) => path,
            Err(err) => {
                rollback_installed_files(&outcome.files);
                return Err(err);
            }
        };

    let mut owned_files: Vec<OwnedFile> = outcome
        .files
        .iter()
        .map(|f| OwnedFile {
            path: f.path.clone(),
            owner: FileOwner::Anolisa,
            sha256: Some(f.sha256.clone()),
        })
        .collect();
    owned_files.push(OwnedFile {
        path: manifest_path.clone(),
        owner: FileOwner::Anolisa,
        sha256: None,
    });
    let mut installed_paths: Vec<String> = outcome
        .files
        .iter()
        .map(|f| f.path.display().to_string())
        .collect();
    installed_paths.push(manifest_path.display().to_string());

    let service_manager = match ctx.install_mode {
        crate::context::InstallMode::System => "systemd",
        crate::context::InstallMode::User => "systemd-user",
    };

    // Migrate away legacy capability rows on this state write; surfaced
    // in the result warnings and audited in the central log below. A
    // state-save failure rolls the prune back with the rest of the write.
    let pruned_legacy = state.prune_legacy_capabilities();
    if !pruned_legacy.is_empty() {
        resolution.warnings.push(format!(
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
        name: resolution.component.clone(),
        version: resolution.entry.version.clone(),
        status: ObjectStatus::Installed,
        // Embedded-manifest digest verification is future work; recording
        // an unverified digest would overstate what install checked.
        manifest_digest: None,
        distribution_source: Some(resolution.artifact_url.clone()),
        install_backend: Some(resolution.backend.clone()),
        ownership: Some(Ownership::RawManaged),
        rpm_metadata: None,
        installed_at: started_at.clone(),
        last_operation_id: Some(operation_id.clone()),
        managed: true,
        adopted: false,
        subscription_scope: Default::default(),
        enabled_features: Vec::new(),
        component_refs: Vec::new(),
        files: owned_files,
        external_modified_files: Vec::new(),
        services: services
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
        rollback_installed_manifest(&manifest_path);
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
        component: Some(resolution.component.clone()),
        severity: Severity::Info,
        message: format!(
            "component {} {} installed via {} backend",
            resolution.component, resolution.entry.version, resolution.backend
        ),
        actor: "cli".to_string(),
        install_mode: Some(ctx.install_mode.as_str().to_string()),
        started_at,
        finished_at: Some(now_iso8601()),
        status: Some(LogStatus::Ok),
        objects: vec![resolution.component.clone()],
        backup_ids: Vec::new(),
        warnings: resolution.warnings.clone(),
        details: serde_json::Value::Null,
    };
    if let Err(err) = log.append(&record) {
        eprintln!("warning: failed to write central log: {err}");
    }

    let payload = InstallResultPayload {
        component: resolution.component,
        package: resolution.package,
        version: resolution.entry.version,
        backend: resolution.backend,
        base_url: resolution.base_url,
        install_mode: ctx.install_mode.as_str().to_string(),
        operation_id,
        artifact_url: resolution.artifact_url,
        files_installed: installed_paths,
        services,
        warnings: resolution.warnings,
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

fn render_plan(ctx: &CliContext, preview: &InstallPreview) -> Result<(), CliError> {
    let resolved = &preview.resolution;
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
        files: preview
            .files
            .iter()
            .map(|f| f.dest.display().to_string())
            .collect(),
        services: preview.services.clone(),
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

fn write_installed_component_manifest(
    layout: &FsLayout,
    component: &str,
    toml: &str,
) -> Result<PathBuf, CliError> {
    let path = common::installed_component_manifest_path(layout, component, COMMAND)?;
    write_atomic_text(&path, toml).map_err(|err| CliError::Runtime {
        command: COMMAND.to_string(),
        reason: format!(
            "failed to write installed component manifest at {}: {err}",
            path.display()
        ),
    })?;
    Ok(path)
}

fn write_atomic_text(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("component.toml");
    let nanos = Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let tmp = parent.join(format!(".{name}.tmp-{}-{nanos}", std::process::id()));

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o644);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(content.as_bytes())?;
    drop(file);
    if let Err(err) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

fn rollback_installed_manifest(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// ISO 8601 UTC timestamp with second precision.
fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::context::InstallMode;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use sha2::{Digest, Sha256};
    use std::path::{Path, PathBuf};
    use tar::{Builder, Header};
    use tempfile::tempdir;

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
            component: Some(component.to_string()),
            all: false,
            fail_fast: false,
            version: None,
            backend: None,
            repo: None,
            package: None,
        }
    }

    fn toml_string_array(values: &[&str]) -> String {
        let quoted: Vec<String> = values.iter().map(|value| format!("\"{value}\"")).collect();
        format!("[{}]", quoted.join(", "))
    }

    fn component_manifest_toml(component: &str, version: &str, modes: &[&str]) -> String {
        let modes = toml_string_array(modes);
        format!(
            r#"[component]
name = "{component}"
version = "{version}"

[component.layout]
modes = {modes}

[[component.layout.files]]
source = "bin/{component}"
target = "{{bindir}}/{component}"
mode = "0755"
type = "executable"
"#
        )
    }

    fn build_tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let buf = Vec::new();
        let enc = GzEncoder::new(buf, Compression::default());
        let mut tar = Builder::new(enc);
        for (path, data) in entries {
            let mut header = Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, *path, *data)
                .expect("append tar entry");
        }
        let enc = tar.into_inner().expect("finish tar");
        enc.finish().expect("finish gzip")
    }

    fn build_component_artifact(component: &str, version: &str, modes: &[&str]) -> Vec<u8> {
        let manifest = component_manifest_toml(component, version, modes);
        let bin_path = format!("bin/{component}");
        let payload = format!("#!/bin/sh\necho {component}\n");
        build_tar_gz(&[
            (".anolisa/component.toml", manifest.as_bytes()),
            (bin_path.as_str(), payload.as_bytes()),
        ])
    }

    /// Build a manifest with a single `[[adapters]]` entry, optionally
    /// overriding the framework and whether source/dest are present.
    fn adapter_manifest(framework: &str, source: Option<&str>, dest: Option<&str>) -> String {
        let mut toml = String::from(
            "[component]\nname = \"tokenless\"\nversion = \"0.1.0\"\n\n\
             [component.layout]\nmodes = [\"system\"]\n\n\
             [[adapters]]\n",
        );
        toml.push_str(&format!("framework = \"{framework}\"\n"));
        if let Some(s) = source {
            toml.push_str(&format!("source = \"{s}\"\n"));
        }
        if let Some(d) = dest {
            toml.push_str(&format!("dest = \"{d}\"\n"));
        }
        toml
    }

    #[test]
    fn resolve_adapter_files_lays_bundle_under_datadir() {
        let prefix = tempdir().unwrap();
        let layout = FsLayout::system(Some(prefix.path().to_path_buf()));
        let toml = adapter_manifest(
            "openclaw",
            Some("adapters/tokenless/openclaw"),
            Some("{datadir}/adapters/{component}/openclaw/"),
        );
        let manifest = ComponentManifest::from_toml_str(&toml).expect("parse manifest");
        let files = resolve_adapter_files(&manifest, &layout, "tokenless").expect("resolve");

        assert_eq!(files.len(), 1);
        let f = &files[0];
        // Source is normalized to a directory prefix so the whole bundle
        // tree is laid down by the runner.
        assert_eq!(f.source.as_deref(), Some("adapters/tokenless/openclaw/"));
        assert_eq!(f.dest, layout.datadir.join("adapters/tokenless/openclaw"));
        assert_eq!(f.kind, FileKind::Data);
        assert_eq!(f.mode.as_deref(), Some("0644"));
    }

    #[test]
    fn resolve_adapter_files_allows_unknown_framework() {
        let prefix = tempdir().unwrap();
        let layout = FsLayout::system(Some(prefix.path().to_path_buf()));
        let toml = adapter_manifest(
            "hermes",
            Some("adapters/tokenless/hermes"),
            Some("{datadir}/adapters/{component}/hermes/"),
        );
        let manifest = ComponentManifest::from_toml_str(&toml).expect("parse manifest");
        let files = resolve_adapter_files(&manifest, &layout, "tokenless").expect("resolve");

        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].source.as_deref(),
            Some("adapters/tokenless/hermes/")
        );
        assert_eq!(
            files[0].dest,
            layout.datadir.join("adapters/tokenless/hermes")
        );
    }

    #[test]
    fn resolve_adapter_files_rejects_missing_source() {
        let prefix = tempdir().unwrap();
        let layout = FsLayout::system(Some(prefix.path().to_path_buf()));
        let toml = adapter_manifest(
            "openclaw",
            None,
            Some("{datadir}/adapters/{component}/openclaw/"),
        );
        let manifest = ComponentManifest::from_toml_str(&toml).expect("parse manifest");
        let err = resolve_adapter_files(&manifest, &layout, "tokenless")
            .expect_err("missing source must be rejected");
        assert!(
            matches!(err, CliError::InvalidArgument { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn resolve_adapter_files_empty_when_no_adapters() {
        let prefix = tempdir().unwrap();
        let layout = FsLayout::system(Some(prefix.path().to_path_buf()));
        let toml = component_manifest_toml("tokenless", "0.1.0", &["system"]);
        let manifest = ComponentManifest::from_toml_str(&toml).expect("parse manifest");
        let files = resolve_adapter_files(&manifest, &layout, "tokenless").expect("resolve");
        assert!(files.is_empty());
    }

    fn write_empty_repo(root: &Path) -> String {
        let v1 = root.join("v1");
        std::fs::create_dir_all(&v1).expect("create repo dirs");
        std::fs::write(
            v1.join("index.toml"),
            r#"schema_version = 1
channel = "stable"
publisher = "test"
"#,
        )
        .expect("write index");
        format!("file://{}", v1.display())
    }

    /// Lay out a local file:// raw repo containing one tar.gz artifact for
    /// `agentsight` targeting the *detected* host os/arch, and return the
    /// repo's raw v1 root. Uses a repo-relative artifact URL to also exercise
    /// the relative-URL join.
    fn write_local_repo(root: &Path) -> String {
        write_local_repo_component(root, "agentsight", "0.2.0", &["system"])
    }

    fn write_local_repo_component(
        root: &Path,
        component: &str,
        version: &str,
        modes: &[&str],
    ) -> String {
        write_local_repo_component_with_modes(root, component, version, modes, modes)
    }

    fn write_local_repo_component_with_modes(
        root: &Path,
        component: &str,
        version: &str,
        index_modes: &[&str],
        manifest_modes: &[&str],
    ) -> String {
        let v1 = root.join("v1");
        std::fs::create_dir_all(&v1).expect("create repo dirs");

        let artifact = build_component_artifact(component, version, manifest_modes);
        let artifact_name = format!("{component}.tar.gz");
        std::fs::write(v1.join(&artifact_name), &artifact).expect("write artifact");
        let sha = format!("{:x}", Sha256::digest(&artifact));
        let modes = toml_string_array(index_modes);

        let env = anolisa_env::EnvService::detect();
        let index = format!(
            r#"schema_version = 1
channel = "stable"
publisher = "test"

[[entries]]
component = "{component}"
version = "{version}"
channel = "stable"
artifact_type = "tar_gz"
backend = "raw"
url = "{artifact_name}"
os = "{os}"
arch = "{arch}"
install_modes = {modes}
sha256 = "{sha}"
"#,
            os = env.os,
            arch = env.arch,
        );
        std::fs::write(v1.join("index.toml"), index).expect("write index");
        format!("file://{}", v1.display())
    }

    fn write_published_layout_repo_with_meta(
        root: &Path,
        component: &str,
        version: &str,
        modes: &[&str],
    ) -> String {
        let env = anolisa_env::EnvService::detect();
        let version_dir = root.join("v1").join(component).join(version);
        let artifact_dir = version_dir.join(&env.os).join(&env.arch);
        std::fs::create_dir_all(&artifact_dir).expect("create artifact dirs");

        let manifest = component_manifest_toml(component, version, modes);
        std::fs::write(version_dir.join("meta.toml"), &manifest).expect("write meta");

        let artifact = build_component_artifact(component, version, modes);
        let artifact_name = format!(
            "{component}-{version}-{os}-{arch}.tar.gz",
            os = env.os,
            arch = env.arch
        );
        std::fs::write(artifact_dir.join(&artifact_name), &artifact).expect("write artifact");
        let sha = format!("{:x}", Sha256::digest(&artifact));
        let modes = toml_string_array(modes);
        let url = format!(
            "{component}/{version}/{os}/{arch}/{artifact_name}",
            os = env.os,
            arch = env.arch
        );

        let index = format!(
            r#"schema_version = 1
channel = "stable"
publisher = "test"

[[entries]]
component = "{component}"
version = "{version}"
channel = "stable"
artifact_type = "tar_gz"
backend = "raw"
url = "{url}"
os = "{os}"
arch = "{arch}"
install_modes = {modes}
sha256 = "{sha}"
"#,
            os = env.os,
            arch = env.arch,
        );
        std::fs::write(root.join("v1/index.toml"), index).expect("write index");
        format!("file://{}", root.join("v1").display())
    }

    fn write_binary_repo_component(
        root: &Path,
        component: &str,
        version: &str,
        modes: &[&str],
    ) -> String {
        let v1 = root.join("v1");
        std::fs::create_dir_all(&v1).expect("create repo dirs");

        let artifact = format!("#!/bin/sh\necho {component}\n").into_bytes();
        let artifact_name = component.to_string();
        std::fs::write(v1.join(&artifact_name), &artifact).expect("write artifact");
        let sha = format!("{:x}", Sha256::digest(&artifact));
        let modes = toml_string_array(modes);

        let env = anolisa_env::EnvService::detect();
        let index = format!(
            r#"schema_version = 1
channel = "stable"
publisher = "test"

[[entries]]
component = "{component}"
version = "{version}"
channel = "stable"
artifact_type = "binary"
backend = "raw"
url = "{artifact_name}"
os = "{os}"
arch = "{arch}"
install_modes = {modes}
sha256 = "{sha}"
"#,
            os = env.os,
            arch = env.arch,
        );
        std::fs::write(v1.join("index.toml"), index).expect("write index");
        format!("file://{}", v1.display())
    }

    fn write_overlay_manifest(layout: &FsLayout, component: &str, version: &str, modes: &[&str]) {
        let runtime_dir = layout.manifests_overlay.join("runtime");
        std::fs::create_dir_all(&runtime_dir).expect("create overlay runtime dir");
        std::fs::write(
            runtime_dir.join(format!("{component}.toml")),
            component_manifest_toml(component, version, modes),
        )
        .expect("write overlay manifest");
    }

    #[test]
    fn sidecar_meta_url_uses_version_directory_for_published_layout() {
        let artifact_url = "https://example.test/anolisa/v1/tokenless/0.5.0/linux/x86_64/tokenless-0.5.0-linux-x86_64.tar.gz";

        assert_eq!(
            sidecar_meta_url(artifact_url, "tokenless", "0.5.0").as_deref(),
            Some("https://example.test/anolisa/v1/tokenless/0.5.0/meta.toml")
        );
    }

    #[test]
    fn sidecar_meta_url_keeps_flat_layout_fallback() {
        let artifact_url = "file:///tmp/repo/v1/legacy-bin";

        assert_eq!(
            sidecar_meta_url(artifact_url, "legacy-bin", "1.0.0").as_deref(),
            Some("file:///tmp/repo/v1/meta.toml")
        );
    }

    /// Like [`write_local_repo`], but the index row omits `url` and the
    /// artifact sits at the conventional publish path
    /// `{component}/{version}/{os}/{arch}/{component}-{version}-{os}-{arch}.tar.gz`
    /// under the raw v1 root.
    fn write_conventional_repo(root: &Path) -> String {
        let env = anolisa_env::EnvService::detect();
        let artifact_dir = root
            .join("v1/agentsight/0.2.0")
            .join(&env.os)
            .join(&env.arch);
        std::fs::create_dir_all(&artifact_dir).expect("create repo dirs");

        let artifact = build_component_artifact("agentsight", "0.2.0", &["system"]);
        let file_name = format!("agentsight-0.2.0-{}-{}.tar.gz", env.os, env.arch);
        std::fs::write(artifact_dir.join(file_name), &artifact).expect("write artifact");
        let sha = format!("{:x}", Sha256::digest(&artifact));

        let index = format!(
            r#"schema_version = 1
channel = "stable"
publisher = "test"

[[entries]]
component = "agentsight"
version = "0.2.0"
channel = "stable"
artifact_type = "tar_gz"
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
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let mut a = args("no-such-component");
        a.repo = Some(write_empty_repo(&tmp.path().join("repo")));

        let err = handle(a, &ctx_with_prefix(false, Some(prefix))).expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("no-such-component"));
    }

    /// Install mode support comes from the remote distribution index before
    /// any artifact is downloaded.
    #[test]
    fn install_unsupported_mode_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let mut a = args("agentsight");
        a.repo = Some(write_local_repo_component(
            &tmp.path().join("repo"),
            "agentsight",
            "0.2.0",
            &["user"],
        ));

        let err = handle(a, &ctx_with_prefix(false, Some(prefix))).expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("install mode is not supported"),
            "got: {}",
            err.reason()
        );
    }

    /// The embedded manifest is a publisher consistency check after index
    /// resolution, but it should use the same caller-visible error bucket as
    /// the index-level mode filter.
    #[test]
    fn install_manifest_mode_mismatch_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let mut a = args("agentsight");
        a.repo = Some(write_local_repo_component_with_modes(
            &tmp.path().join("repo"),
            "agentsight",
            "0.2.0",
            &["system"],
            &["user"],
        ));

        let err = handle(a, &ctx_with_prefix(false, Some(prefix))).expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason()
                .contains("inconsistent with the distribution index")
                && err.reason().contains("system-mode support"),
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
        let cached_names: Vec<String> = std::fs::read_dir(layout.cache_dir.join("downloads"))
            .expect("downloads cache exists")
            .map(|entry| {
                entry
                    .expect("cache entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert!(
            cached_names
                .iter()
                .all(|name| !name.ends_with("agentsight.tar.gz")),
            "dry-run must not download the install artifact; cache entries: {cached_names:?}"
        );
    }

    #[test]
    fn install_dry_run_reads_version_meta_without_downloading_artifact() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let repo_url = write_published_layout_repo_with_meta(
            &tmp.path().join("repo"),
            "remote-only",
            "1.0.0",
            &["system"],
        );
        let mut ctx = ctx_with_prefix(false, Some(prefix.clone()));
        ctx.dry_run = true;
        let layout = FsLayout::system(Some(prefix));
        let env = anolisa_env::EnvService::detect();

        let resolution = resolve_raw(
            &ctx,
            &layout,
            &env,
            ResolveInputs {
                component: "remote-only".to_string(),
                package: "remote-only".to_string(),
                backend: "raw".to_string(),
                base_url: repo_url,
                version: None,
                warnings: Vec::new(),
            },
        )
        .expect("resolve");
        let preview = build_install_preview(&ctx, &layout, resolution).expect("preview");

        assert_eq!(preview.files.len(), 1);
        assert_eq!(preview.files[0].dest, layout.bin_dir.join("remote-only"));
        assert!(
            preview
                .resolution
                .warnings
                .iter()
                .all(|warning| !warning.contains("file and service details are unavailable")),
            "version-level meta.toml should provide file details: {:?}",
            preview.resolution.warnings
        );

        let cached_names: Vec<String> = std::fs::read_dir(layout.cache_dir.join("downloads"))
            .expect("downloads cache exists")
            .map(|entry| {
                entry
                    .expect("cache entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert!(
            cached_names
                .iter()
                .all(|name| !name.ends_with("remote-only-1.0.0-linux-x86_64.tar.gz")),
            "dry-run must not download the install artifact; cache entries: {cached_names:?}"
        );
    }

    /// Legacy distribution indexes may still publish a raw `binary` entry.
    /// Keep installing those when a local component manifest supplies the
    /// destination contract.
    #[test]
    fn install_binary_artifact_uses_local_catalog_contract() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let layout = FsLayout::system(Some(prefix.clone()));
        write_overlay_manifest(&layout, "legacy-bin", "1.0.0", &["system"]);

        let mut a = args("legacy-bin");
        a.repo = Some(write_binary_repo_component(
            &tmp.path().join("repo"),
            "legacy-bin",
            "1.0.0",
            &["system"],
        ));

        handle(a, &ctx_with_prefix(false, Some(prefix.clone()))).expect("install must succeed");

        let bin = FsLayout::system(Some(prefix)).bin_dir.join("legacy-bin");
        assert!(bin.exists(), "binary artifact must be installed");
        assert_eq!(
            std::fs::read_to_string(&bin).expect("read installed binary"),
            "#!/bin/sh\necho legacy-bin\n"
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
        let manifest_path =
            common::installed_component_manifest_path(&layout, "agentsight", COMMAND)
                .expect("manifest path");
        assert!(
            manifest_path.exists(),
            "installed component manifest must be persisted"
        );
        let saved_manifest =
            ComponentManifest::from_file(&manifest_path).expect("saved manifest parses");
        assert_eq!(saved_manifest.component.name, "agentsight");
        assert_eq!(saved_manifest.component.version, "0.2.0");

        let state = anolisa_core::InstalledState::load(&layout.state_dir.join("installed.toml"))
            .expect("state must load");
        let obj = state
            .find_object(ObjectKind::Component, "agentsight")
            .expect("component object must be recorded");
        assert_eq!(obj.version, "0.2.0");
        assert_eq!(obj.status, ObjectStatus::Installed);
        assert_eq!(obj.files.len(), 2);
        assert!(
            obj.files.iter().any(|file| file.path == manifest_path),
            "installed manifest must be tracked as an owned file"
        );
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
    fn install_raw_uses_embedded_manifest_without_local_catalog() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let repo_url = write_local_repo_component(
            &tmp.path().join("repo"),
            "remote-only",
            "1.0.0",
            &["system"],
        );

        let mut a = args("remote-only");
        a.repo = Some(repo_url);
        handle(a, &ctx_with_prefix(false, Some(prefix.clone()))).expect("install must succeed");

        let layout = FsLayout::system(Some(prefix));
        assert!(
            layout.bin_dir.join("remote-only").exists(),
            "component absent from local manifests must install from embedded artifact contract"
        );
        let state = anolisa_core::InstalledState::load(&layout.state_dir.join("installed.toml"))
            .expect("state must load");
        assert!(
            state
                .find_object(ObjectKind::Component, "remote-only")
                .is_some(),
            "remote-only component must be recorded"
        );
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
            ownership: None,
            rpm_metadata: None,
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
                    "{repo_url}/agentsight/0.2.0/{os}/{arch}/agentsight-0.2.0-{os}-{arch}.tar.gz",
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
                    "file://{}/v1/agentsight/0.2.0/{os}/{arch}/agentsight-0.2.0-{os}-{arch}.tar.gz",
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

    // ── --all / --fail-fast clap validation tests ─────────────────────

    #[test]
    fn install_all_and_component_are_mutually_exclusive() {
        let err = InstallArgs::try_parse_from(["install", "--all", "tokenless"])
            .expect_err("must reject --all with positional");
        assert!(
            err.kind() == clap::error::ErrorKind::ArgumentConflict
                || err.to_string().contains("cannot be used with")
        );
    }

    #[test]
    fn install_all_conflicts_with_package() {
        let err = InstallArgs::try_parse_from(["install", "--all", "--package", "foo"])
            .expect_err("must reject --all with --package");
        assert!(
            err.kind() == clap::error::ErrorKind::ArgumentConflict
                || err.to_string().contains("cannot be used with")
        );
    }

    #[test]
    fn install_all_conflicts_with_version() {
        let err = InstallArgs::try_parse_from(["install", "--all", "--version", "1.0.0"])
            .expect_err("must reject --all with --version");
        assert!(
            err.kind() == clap::error::ErrorKind::ArgumentConflict
                || err.to_string().contains("cannot be used with")
        );
    }

    #[test]
    fn install_fail_fast_without_all_is_rejected() {
        // clap still parses it (ArgGroup + requires limitation), but
        // handle() now rejects at runtime.
        let a = InstallArgs::try_parse_from(["install", "tokenless", "--fail-fast"])
            .expect("clap allows this parse");
        assert!(!a.all);
        assert!(a.fail_fast);

        let ctx = ctx_with_prefix(false, None);
        let err = handle(a, &ctx).expect_err("handle should reject --fail-fast without --all");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
    }

    #[test]
    fn install_all_parses_successfully() {
        let a = InstallArgs::try_parse_from(["install", "--all"]).expect("should parse");
        assert!(a.all);
        assert!(a.component.is_none());
    }

    #[test]
    fn install_all_with_fail_fast_parses_successfully() {
        let a =
            InstallArgs::try_parse_from(["install", "--all", "--fail-fast"]).expect("should parse");
        assert!(a.all);
        assert!(a.fail_fast);
    }

    // ── catalog parsing tests ────────────────────────────────────────

    #[test]
    fn all_catalog_filters_available_components() {
        let json = r#"{
            "schema_version": 1,
            "components": [
                {"name": "a", "status": "available"},
                {"name": "b", "status": "planned"},
                {"name": "c", "status": "available"},
                {"name": "d"}
            ]
        }"#;
        let catalog: AllCatalogV1 = serde_json::from_str(json).expect("parse");
        assert_eq!(catalog.schema_version, 1);
        let names: Vec<String> = catalog
            .components
            .into_iter()
            .filter(|c| c.status.as_deref() == Some("available"))
            .map(|c| c.name)
            .filter(|n| !n.trim().is_empty())
            .collect();
        assert_eq!(names, vec!["a", "c"]);
    }

    #[test]
    fn all_catalog_rejects_unsupported_schema_version() {
        let json = r#"{"schema_version": 2, "components": []}"#;
        let catalog: AllCatalogV1 = serde_json::from_str(json).expect("parse");
        assert_ne!(catalog.schema_version, 1);
    }

    #[test]
    fn all_catalog_skips_empty_names() {
        let json = r#"{
            "schema_version": 1,
            "components": [
                {"name": "", "status": "available"},
                {"name": "  ", "status": "available"},
                {"name": "valid", "status": "available"}
            ]
        }"#;
        let catalog: AllCatalogV1 = serde_json::from_str(json).expect("parse");
        let names: Vec<String> = catalog
            .components
            .into_iter()
            .filter(|c| c.status.as_deref() == Some("available"))
            .map(|c| c.name)
            .filter(|n| !n.trim().is_empty())
            .collect();
        assert_eq!(names, vec!["valid"]);
    }
}
