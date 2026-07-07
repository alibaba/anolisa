//! Raw backend resolution, manifest contract parsing, and execution for
//! the `install` command.

use std::path::Path;

use anolisa_core::central_log::{CentralLog, LogKind, LogRecord, LogStatus, Severity};
use anolisa_core::download::{DownloadCache, DownloadError};
use anolisa_core::install_runner::{
    InstallRunner, ResolvedInstallFile, SUPPORTED_ARTIFACT_TYPES,
    read_embedded_component_manifest_text,
};
use anolisa_core::lock::InstallLock;
use anolisa_core::path_safety::validate_owned_path;
use anolisa_core::state::{
    FileOwner, InstallMode as StateInstallMode, InstalledObject, ObjectKind, ObjectStatus,
    OperationRecord, OwnedFile, OwnedFileKind, Ownership, ServiceRef,
};
use anolisa_core::{
    ArtifactType, CapabilityRequest, ComponentManifest, DependencyResolution, DependencyResolver,
    DistributionIndex, FileKind, HookPhase, HookSpec, ProvisionPlan, ResolveQuery,
    ServiceActivation, ServiceManager, ServiceRequest, ServiceScope, apply_capabilities,
    apply_services, capability_for_install_mode, expand_layout_placeholders,
    resolve_manifest_hooks, run_hooks, service_for_install_mode, user_service_for_install_mode,
};
use anolisa_platform::fs_layout::FsLayout;
use chrono::Utc;

use crate::commands::common;
use crate::context::CliContext;
use crate::repo_config::{
    HostVars, RepoConfig, raw_artifact_url, raw_index_url, raw_relative_root,
};
use crate::response::{CliError, render_json};

use super::io_util::{
    now_iso8601, rollback_activated_services, rollback_installed_files,
    rollback_installed_manifest, service_cleanup_suffix, write_installed_component_manifest,
};
use super::provision::{resolver_env_from_facts, retained_packages_note, run_provision};
use super::render::{artifact_ext, artifact_type_wire, render_result, repo_config_err};
use super::types::*;

use super::{COMMAND, ensure_component_backend_compatible};
pub(crate) fn resolve_raw(
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

/// Rebuild [`ResolveInputs`] for an already-installed component from its
/// recorded backend plus repo.toml, for the `update` path (which has no CLI
/// `--backend` / `--repo` / `--version` to read). Always targets the latest
/// published version (`version: None`).
///
/// `recorded_package` is the package captured at install time
/// ([`InstalledObject::raw_package`](anolisa_core::state::InstalledObject::raw_package));
/// when present it takes precedence over repo.toml derivation, so a component
/// installed with `--package` updates against the same package rather than a
/// re-derived (possibly different) one.
///
/// # Errors
///
/// Returns [`CliError`] when `backend_name` is unknown or unconfigured in
/// repo.toml, when its `base_url` variables cannot be resolved, or — until a
/// non-raw raw-like executor exists — when the backend is not `raw`.
pub(crate) fn resolve_raw_inputs_for_component(
    component: String,
    backend_name: &str,
    recorded_package: Option<&str>,
    env: &anolisa_env::EnvFacts,
    repo_config: &RepoConfig,
    command: &str,
) -> Result<ResolveInputs<'static>, CliError> {
    let (backend_name, backend) = repo_config
        .select_backend(Some(backend_name))
        .map_err(|err| repo_config_err(err, true).with_command(command))?;
    if backend_name != "raw" {
        return Err(CliError::not_implemented_with_hint(
            command.to_string(),
            format!(
                "the '{backend_name}' backend has no update executor yet — only 'raw' updates today"
            ),
        ));
    }
    let host = HostVars {
        os: env.os.clone(),
        arch: env.arch.clone(),
    };
    let base_url = repo_config
        .resolved_base_url(backend_name, backend, &host)
        .map_err(|err| repo_config_err(err, true).with_command(command))?;
    // recorded_package wins via package_name's CLI-override slot, so a
    // `--package` install resolves the same package on update; None falls
    // through to repo.toml's package_map / component-name derivation.
    let package = repo_config.package_name(backend, &component, recorded_package);
    Ok(ResolveInputs {
        component,
        package,
        backend: backend_name.to_string(),
        base_url,
        version: None,
        warnings: Vec::new(),
    })
}

/// Best-effort list of versions published for `package` under the current
/// host selectors, highest-first. Returns empty on any fetch/parse failure:
/// candidates only enrich the dry-run preview and must never block an update.
///
/// Uses [`DistributionIndex::matching_versions`] with the same [`ResolveQuery`]
/// shape as `resolve_raw` so the preview list agrees with what an actual
/// update would resolve (same channel / libc / pkg_base / install_mode
/// filtering and semver ordering).
pub(crate) fn available_raw_versions(
    layout: &FsLayout,
    base_url: &str,
    package: &str,
    env: &anolisa_env::EnvFacts,
    install_mode: &str,
) -> Vec<String> {
    let index_url = raw_index_url(base_url);
    let cache = DownloadCache::new(layout.cache_dir.clone());
    let Ok(downloaded) = cache.fetch(&index_url, None) else {
        return Vec::new();
    };
    let Ok(index) = DistributionIndex::load(&downloaded.cached_path) else {
        return Vec::new();
    };
    let query = ResolveQuery {
        component: package,
        version: None,
        channel: None,
        install_mode,
        os: &env.os,
        arch: &env.arch,
        libc: env.libc.as_deref(),
        pkg_base: env.pkg_base.as_deref(),
        preferred_types: &[],
    };
    index.matching_versions(&query)
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

pub(crate) fn build_install_preview(
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
            capabilities: Vec::new(),
            dependencies: Vec::new(),
            provision_plan: None,
        });
    };

    let (files, services, capabilities) = match resolve_manifest_contract(
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
            (Vec::new(), Vec::new(), Vec::new())
        }
        Err(err) => return Err(err),
    };

    let (dependencies, provision_plan) = preview_dependencies(&contract.manifest, &mut resolution);

    Ok(InstallPreview {
        resolution,
        files,
        services,
        capabilities,
        dependencies,
        provision_plan,
    })
}

/// Run the runtime-dependency preflight for `--dry-run` (read-only). Reports
/// per-dependency status without ever failing the preview: a missing dependency
/// is informational here, and a declaration error degrades to a warning rather
/// than aborting the plan.
fn preview_dependencies(
    manifest: &ComponentManifest,
    resolution: &mut RawResolution,
) -> (Vec<DependencyResolution>, Option<ProvisionPlan>) {
    if manifest.runtime_deps.is_empty() {
        return (Vec::new(), None);
    }
    let env = anolisa_env::EnvService::detect();
    let resolver_env = resolver_env_from_facts(&env);
    match DependencyResolver::system().resolve(&manifest.runtime_deps, &resolver_env) {
        Ok(plan) => {
            resolution.warnings.extend(plan.warnings.clone());
            let provision =
                ProvisionPlan::from_resolution(&plan, &manifest.runtime_deps, &resolver_env);
            (plan.resolutions, Some(provision))
        }
        Err(err) => {
            resolution
                .warnings
                .push(format!("dependency preflight skipped: {err}"));
            (Vec::new(), None)
        }
    }
}

pub(crate) fn prepare_raw_execution(
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
    let (files, services, capabilities) = resolve_manifest_contract(
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
        capabilities,
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

pub(crate) fn sidecar_meta_url(
    artifact_url: &str,
    component: &str,
    version: &str,
) -> Option<String> {
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

/// Resolved install contract: laid files, recorded service unit names, and
/// capability requests to apply once those files are on disk.
type ResolvedContract = (
    Vec<ResolvedInstallFile>,
    Vec<ServiceRequest>,
    Vec<CapabilityRequest>,
);

fn resolve_manifest_contract(
    manifest: &ComponentManifest,
    layout: &FsLayout,
    resolution: &RawResolution,
    mode: &str,
    source: InstallContractSource,
) -> Result<ResolvedContract, CliError> {
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

    let services = resolve_manifest_services(manifest, &resolution.component, mode)?;
    let capabilities = resolve_manifest_capabilities(manifest, layout, &resolution.component)?;

    Ok((files, services, capabilities))
}

/// Render the manifest's `[[component.services]]` into activation requests:
/// substitute the template instance into the unit name and carry
/// scope/enable/start through to the executor. No filesystem or layout
/// expansion — unit names are systemd identifiers, not paths.
///
/// # Errors
///
/// Returns [`CliError::Runtime`] if a service entry has an empty `unit`.
pub(crate) fn resolve_manifest_services(
    manifest: &ComponentManifest,
    component: &str,
    mode: &str,
) -> Result<Vec<ServiceRequest>, CliError> {
    // The `%u` instance specifier resolves to the caller's login name, but
    // only in a user-mode install where the unit is activated as that user.
    // A system-mode install merely *places* a user-scope template for later
    // per-user `systemctl --user enable`, so it leaves `%u` un-resolved
    // (the bare template) rather than baking in root's name. Detect the user
    // at most once, and only when a `%u` instance actually needs it.
    let caller = if mode == "user"
        && manifest
            .install
            .services
            .iter()
            .any(|s| s.instance.as_deref().is_some_and(|i| i.contains("%u")))
    {
        Some(anolisa_env::EnvService::detect().user)
    } else {
        None
    };

    let mut requests = Vec::with_capacity(manifest.install.services.len());
    for spec in &manifest.install.services {
        if spec.unit.trim().is_empty() {
            return Err(CliError::Runtime {
                command: COMMAND.to_string(),
                reason: format!(
                    "component '{component}' has a [[component.services]] entry with an empty unit"
                ),
            });
        }
        // Template unit (`name@.service`) + instance → `name@<instance>.service`.
        let unit = match &spec.instance {
            Some(instance) if spec.unit.contains("@.") => {
                match resolve_service_instance(instance, caller.as_deref()) {
                    Some(resolved) => spec.unit.replacen("@.", &format!("@{resolved}."), 1),
                    // `%u` with no resolved user (system-mode place-only):
                    // keep the bare template; per-user enable instantiates it.
                    None => spec.unit.clone(),
                }
            }
            _ => spec.unit.clone(),
        };
        requests.push(ServiceRequest {
            unit,
            scope: spec.scope,
            enable: spec.enable,
            start: spec.start,
        });
    }
    Ok(requests)
}

/// Resolve a systemd template instance, expanding the `%u` specifier to the
/// caller's login name.
///
/// `%u` is a systemd specifier that systemd does *not* expand in the instance
/// portion of a command-line unit name, so anolisa resolves it itself. Returns
/// `None` when the instance uses `%u` but no caller name is available (a
/// system-mode install that only places the template) — the caller then keeps
/// the bare template. A literal instance is returned verbatim in every mode.
pub(crate) fn resolve_service_instance(instance: &str, caller: Option<&str>) -> Option<String> {
    if !instance.contains("%u") {
        return Some(instance.to_string());
    }
    caller.map(|user| instance.replace("%u", user))
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
pub(crate) fn resolve_adapter_files(
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

/// Render the manifest's `[[component.capabilities]]` against the layout:
/// expand `{bindir}`-style placeholders in the target path and reject any
/// path escaping the ANOLISA-owned roots before `setcap` ever runs.
///
/// Rows with empty `caps` are skipped — there is nothing to grant. A row
/// that lists caps but no `path` is a contract error: we will not guess
/// which binary to harden.
pub(crate) fn resolve_manifest_capabilities(
    manifest: &ComponentManifest,
    layout: &FsLayout,
    component: &str,
) -> Result<Vec<CapabilityRequest>, CliError> {
    let mut requests = Vec::new();
    for spec in &manifest.install.capabilities {
        if spec.caps.is_empty() {
            continue;
        }
        let template = spec.path.as_deref().ok_or_else(|| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "component '{component}' has a [[component.capabilities]] entry with caps but no path"
            ),
        })?;
        let path = expand_layout_placeholders(template, layout, &[("component", component)])
            .map_err(|err| CliError::Runtime {
                command: COMMAND.to_string(),
                reason: format!("failed to expand capability path '{template}': {err}"),
            })?;
        validate_owned_path(layout, &path).map_err(|err| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "capability target '{}' failed path safety check: {err}",
                path.display()
            ),
        })?;
        requests.push(CapabilityRequest {
            path,
            caps: spec.caps.clone(),
            optional: spec.optional,
        });
    }
    Ok(requests)
}

/// Contract-declared lifecycle hooks for the three raw-install phases,
/// placeholder-expanded with `strict`/`timeout` carried from the contract.
///
/// `pre_install` runs before any files are laid down. On a fresh raw install
/// the hook script ships in the same artifact and is therefore not on disk
/// yet, so [`run_hook`](anolisa_core::run_hook) reports it as `Missing`. With
/// `strict = false` — the only sensible choice for `pre_install`, since the
/// script cannot exist on a first install — that is a silent no-op; a
/// `strict = true` `pre_install` would instead abort the install (the script
/// it requires is unreachable). The phase becomes meaningful on the update
/// path (out of scope here) where a prior version already laid the script.
#[derive(Debug)]
pub(crate) struct InstallHooks {
    pub(crate) pre_install: Vec<HookSpec>,
    pub(crate) post_install: Vec<HookSpec>,
    pub(crate) post_enable: Vec<HookSpec>,
}

/// Resolve a component's `[[component.hooks]]` for the three install phases.
///
/// Unlike the uninstall side (which degrades a missing/invalid snapshot to
/// "no hooks"), install resolves strictly: an unresolvable script path is a
/// contract authoring bug and aborts before any IO so it surfaces early.
pub(crate) fn resolve_install_hooks(
    manifest: &ComponentManifest,
    layout: &FsLayout,
    component: &str,
) -> Result<InstallHooks, CliError> {
    let resolve = |phase: HookPhase| -> Result<Vec<HookSpec>, CliError> {
        resolve_manifest_hooks(&manifest.install.hooks, layout, component, phase).map_err(|err| {
            CliError::Runtime {
                command: COMMAND.to_string(),
                reason: format!(
                    "component '{component}' has an invalid [[component.hooks]] script path: {err}"
                ),
            }
        })
    };
    Ok(InstallHooks {
        pre_install: resolve(HookPhase::PreInstall)?,
        post_install: resolve(HookPhase::PostInstall)?,
        post_enable: resolve(HookPhase::PostEnable)?,
    })
}

/// Execute the resolved install: download+verify, copy files under the
/// install lock, persist state, and append the audit record. Files already
/// on disk are rolled back when a later step fails, so no phantom install
/// survives an error.
pub(crate) fn execute_raw(
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
        capabilities,
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

    // Resolve the contract's lifecycle hooks before any IO so an invalid
    // script path (a contract authoring bug) aborts the install before files
    // are touched. The log handle is opened here too: pre_install runs before
    // file layout, and the capability/service steps below reuse it.
    let manifest =
        ComponentManifest::from_toml_str(&manifest_toml).map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!("failed to parse component manifest for hook resolution: {err}"),
        })?;

    // Validate hook declarations before any host mutation (contract authoring
    // errors must be caught before provisioning installs system packages).
    let hooks = resolve_install_hooks(&manifest, layout, &resolution.component)?;

    // Runtime-dependency provisioning — probe declared dependencies while the
    // lock is held but before any filesystem mutation. In system mode, missing
    // system packages are auto-installed via the host package manager. In user
    // mode, missing deps are reported with remediation commands and the install
    // is aborted. The RPM backend never reaches here (dnf resolves its
    // `Requires`), so a dependency is never resolved twice.
    let env = anolisa_env::EnvService::detect();
    let provisioned_packages =
        run_provision(&manifest, &env, ctx, command, &mut resolution.warnings)?;

    let retained_pkg_note = retained_packages_note(&provisioned_packages);

    let log = CentralLog::open(layout.central_log.clone());

    // pre_install hook — before files land, so a strict failure aborts with
    // nothing on disk to roll back. On a fresh raw install the script ships in
    // this artifact and is not yet laid, so it skips as Missing (no warning).
    let pre_install = run_hooks(
        &hooks.pre_install,
        layout,
        Some(&log),
        &operation_id,
        "cli",
        ctx.install_mode.as_str(),
    );
    resolution.warnings.extend(pre_install.warnings);
    if let Some(hf) = pre_install.hard_failure.as_ref() {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "pre_install hook failed: {}{retained_pkg_note}",
                hf.summary()
            ),
        });
    }

    let runner = InstallRunner::new(layout);
    let outcome = runner
        .install_files(
            artifact_type_wire(&resolution.entry.artifact_type),
            &artifact_path,
            &files,
        )
        .map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!("install failed: {err}{retained_pkg_note}"),
        })?;

    // From this point files are on disk — failures must roll them back.
    let manifest_path =
        match write_installed_component_manifest(layout, &resolution.component, &manifest_toml) {
            Ok(path) => path,
            Err(err) => {
                rollback_installed_files(&outcome.files);
                return Err(CliError::Runtime {
                    command: command.to_string(),
                    reason: format!("{err}{retained_pkg_note}"),
                });
            }
        };

    // Files and the contract manifest are on disk; apply declared Linux file
    // capabilities now. The manager gates itself to raw + system + Linux +
    // non-container — user mode, containers, and non-Linux are quiet skips.
    // A required (non-optional) failure aborts: roll back files + manifest
    // while the lock is still held and before any state is persisted, so no
    // half-installed component survives. Optional failures degrade to
    // warnings. Reuses the `log` handle opened before file layout and the
    // `env` facts detected for the dependency preflight above.
    let cap_manager = capability_for_install_mode(ctx.install_mode.as_str(), &env);
    let cap_outcome = apply_capabilities(
        cap_manager.as_ref(),
        &capabilities,
        Some(&log),
        &resolution.component,
        &operation_id,
        "cli",
        ctx.install_mode.as_str(),
    );
    if let Some(reason) = cap_outcome.aborted {
        rollback_installed_files(&outcome.files);
        rollback_installed_manifest(&manifest_path);
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "required capability application failed; rolled back installed files and manifest: {reason}{retained_pkg_note}"
            ),
        });
    }
    resolution.warnings.extend(cap_outcome.warnings);

    // post_install hook — after setcap, before services (§6.2). Files and
    // capabilities are committed, so a strict failure rolls them back exactly
    // like a required capability abort.
    let post_install = run_hooks(
        &hooks.post_install,
        layout,
        Some(&log),
        &operation_id,
        "cli",
        ctx.install_mode.as_str(),
    );
    resolution.warnings.extend(post_install.warnings);
    if let Some(hf) = post_install.hard_failure.as_ref() {
        rollback_installed_files(&outcome.files);
        rollback_installed_manifest(&manifest_path);
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "post_install hook failed; rolled back installed files and manifest: {}{retained_pkg_note}",
                hf.summary()
            ),
        });
    }

    // Capabilities done; bring declared services up (issue order: setcap →
    // service enable/start). Activation is best-effort — a failed enable/start
    // is a warning, not an abort: the component's files are installed and an
    // operator can fix the unit out of band. Reuse the env + log opened for
    // the capability step.
    //
    // A contract's services are single-scope in practice (a component is
    // either a system daemon or a per-user service). Pick the matching
    // backend: an all-user-scope set drives `systemctl --user` (and only in a
    // user-mode install); otherwise the system backend. A request the chosen
    // backend does not handle (a hypothetical mixed-scope contract) is skipped
    // by `apply_services` via `handles_scope`, so this never mis-drives.
    let service_manager: Box<dyn ServiceManager> =
        if !services.is_empty() && services.iter().all(|s| s.scope == ServiceScope::User) {
            user_service_for_install_mode(ctx.install_mode.as_str(), &env)
        } else {
            service_for_install_mode(ctx.install_mode.as_str(), &env)
        };
    let service_run = apply_services(
        service_manager.as_ref(),
        &services,
        ServiceActivation::Start,
        Some(&log),
        &resolution.component,
        &operation_id,
        "cli",
        ctx.install_mode.as_str(),
    );
    resolution
        .warnings
        .extend(service_run.warnings.iter().cloned());

    // post_enable hook — after service enable/start (§6.2). A strict failure
    // rolls back files + manifest like post_install. Because services are an
    // external side effect, clean up only the units this install successfully
    // enabled or started before removing their unit files.
    let post_enable = run_hooks(
        &hooks.post_enable,
        layout,
        Some(&log),
        &operation_id,
        "cli",
        ctx.install_mode.as_str(),
    );
    resolution.warnings.extend(post_enable.warnings);
    if let Some(hf) = post_enable.hard_failure.as_ref() {
        let cleanup_warnings = rollback_activated_services(
            service_manager.as_ref(),
            &service_run,
            Some(&log),
            &resolution.component,
            &operation_id,
            ctx.install_mode.as_str(),
        );
        rollback_installed_files(&outcome.files);
        rollback_installed_manifest(&manifest_path);
        let cleanup_suffix = service_cleanup_suffix(&cleanup_warnings);
        let hook_summary = hf.summary();
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "post_enable hook failed; stopped/disabled activated services and rolled back installed files and manifest{cleanup_suffix}: {hook_summary}{retained_pkg_note}",
            ),
        });
    }

    let mut owned_files: Vec<OwnedFile> = outcome
        .files
        .iter()
        .map(|f| OwnedFile {
            path: f.path.clone(),
            owner: FileOwner::Anolisa,
            sha256: if f.referent.is_some() {
                None
            } else {
                Some(f.sha256.clone())
            },
            kind: if f.referent.is_some() {
                OwnedFileKind::Symlink
            } else {
                OwnedFileKind::File
            },
            referent: f.referent.clone(),
        })
        .collect();
    let manifest_sha256 = {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(manifest_toml.as_bytes());
        Some(hash.iter().fold(String::new(), |mut s, b| {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
            s
        }))
    };
    owned_files.push(OwnedFile {
        path: manifest_path.clone(),
        owner: FileOwner::Anolisa,
        sha256: manifest_sha256,
        kind: OwnedFileKind::File,
        referent: None,
    });
    let mut installed_paths: Vec<String> = outcome
        .files
        .iter()
        .map(|f| f.path.display().to_string())
        .collect();
    installed_paths.push(manifest_path.display().to_string());

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
        // Record the resolved package so update reuses it verbatim, preserving
        // any `--package` override instead of re-deriving from repo.toml.
        raw_package: Some(resolution.package.clone()),
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
                name: svc.unit.clone(),
                // Label follows the unit's scope, not install mode: a
                // place-only user-scope unit in a system install is still
                // `systemd-user`, keeping `manager` consistent with `scope`.
                manager: svc.scope.manager_label().to_string(),
                restartable: true,
                // Reflect what the executor actually enabled this run.
                enabled: service_run.enabled_units.contains(&svc.unit),
                scope: svc.scope,
            })
            .collect(),
        health: Vec::new(),
        provisioned_packages: provisioned_packages.clone(),
    });
    state.operations.push(OperationRecord {
        id: operation_id.clone(),
        command: command.to_string(),
        status: "ok".to_string(),
        started_at: started_at.clone(),
        finished_at: Some(now_iso8601()),
    });

    common::migrate_v3_symlinks(&mut state, layout);
    let state_path = layout.state_dir.join("installed.toml");
    if let Err(err) = state.save(&state_path) {
        let cleanup_warnings = rollback_activated_services(
            service_manager.as_ref(),
            &service_run,
            Some(&log),
            &resolution.component,
            &operation_id,
            ctx.install_mode.as_str(),
        );
        rollback_installed_files(&outcome.files);
        rollback_installed_manifest(&manifest_path);
        let cleanup_suffix = service_cleanup_suffix(&cleanup_warnings);
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "failed to save state; stopped/disabled activated services and attempted best-effort rollback of installed files and manifest{cleanup_suffix} (some files may remain on disk): {err}{retained_pkg_note}",
            ),
        });
    }

    // Audit log is best-effort: the install already succeeded and state is
    // saved, so a log failure downgrades to a warning instead of unwinding.
    // `log` was opened above for the capability audit and is reused here.
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
        services: services.iter().map(|s| s.unit.clone()).collect(),
        provisioned_packages,
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

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::*;
    use anolisa_core::ComponentManifest;
    use anolisa_platform::fs_layout::FsLayout;
    use tempfile::tempdir;

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

    #[test]
    fn resolve_manifest_capabilities_expands_bindir_path() {
        let prefix = tempdir().unwrap();
        let layout = FsLayout::system(Some(prefix.path().to_path_buf()));
        let toml = capability_manifest(Some("{bindir}/agentsight"), &["CAP_BPF"], false);
        let manifest = ComponentManifest::from_toml_str(&toml).expect("parse manifest");
        let reqs =
            resolve_manifest_capabilities(&manifest, &layout, "agentsight").expect("resolve");
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].path, layout.bin_dir.join("agentsight"));
        assert_eq!(reqs[0].caps, vec!["CAP_BPF".to_string()]);
        assert!(!reqs[0].optional);
    }

    #[test]
    fn resolve_manifest_capabilities_rejects_out_of_bounds_path() {
        let prefix = tempdir().unwrap();
        let layout = FsLayout::system(Some(prefix.path().to_path_buf()));
        let toml = capability_manifest(Some("/etc/passwd"), &["CAP_BPF"], false);
        let manifest = ComponentManifest::from_toml_str(&toml).expect("parse manifest");
        let err = resolve_manifest_capabilities(&manifest, &layout, "agentsight")
            .expect_err("path escaping owned roots must be rejected");
        assert!(matches!(err, CliError::Runtime { .. }), "got {err:?}");
    }

    #[test]
    fn resolve_manifest_capabilities_skips_rows_with_empty_caps() {
        let prefix = tempdir().unwrap();
        let layout = FsLayout::system(Some(prefix.path().to_path_buf()));
        // A path but nothing to grant — nothing to do, no setcap invocation.
        let toml = capability_manifest(Some("{bindir}/agentsight"), &[], false);
        let manifest = ComponentManifest::from_toml_str(&toml).expect("parse manifest");
        let reqs =
            resolve_manifest_capabilities(&manifest, &layout, "agentsight").expect("resolve");
        assert!(reqs.is_empty());
    }

    #[test]
    fn resolve_manifest_capabilities_requires_path_when_caps_present() {
        let prefix = tempdir().unwrap();
        let layout = FsLayout::system(Some(prefix.path().to_path_buf()));
        let toml = capability_manifest(None, &["CAP_BPF"], false);
        let manifest = ComponentManifest::from_toml_str(&toml).expect("parse manifest");
        let err = resolve_manifest_capabilities(&manifest, &layout, "agentsight")
            .expect_err("caps without a path is a contract error");
        assert!(matches!(err, CliError::Runtime { .. }), "got {err:?}");
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

    #[test]
    fn resolve_manifest_services_carries_spec_and_expands_instance() {
        let toml = service_manifest("anolisa-memory@.service", true, false, Some("alice"));
        let manifest = ComponentManifest::from_toml_str(&toml).expect("parse manifest");
        let reqs = resolve_manifest_services(&manifest, "agentsight", "system").expect("resolve");
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].unit, "anolisa-memory@alice.service");
        assert!(reqs[0].enable);
        assert!(!reqs[0].start);
    }

    #[test]
    fn resolve_manifest_services_plain_unit_unchanged() {
        let toml = service_manifest("agentsight.service", true, true, None);
        let manifest = ComponentManifest::from_toml_str(&toml).expect("parse manifest");
        let reqs = resolve_manifest_services(&manifest, "agentsight", "system").expect("resolve");
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].unit, "agentsight.service");
        assert!(reqs[0].enable && reqs[0].start);
        assert_eq!(reqs[0].scope, anolisa_core::ServiceScope::System);
    }

    #[test]
    fn resolve_service_instance_expands_percent_u_only_with_a_caller() {
        // `%u` resolves to the caller; a literal instance passes through in
        // every mode; `%u` with no caller (system-mode place-only) stays None.
        assert_eq!(
            resolve_service_instance("%u", Some("alice")).as_deref(),
            Some("alice")
        );
        assert_eq!(resolve_service_instance("%u", None), None);
        assert_eq!(
            resolve_service_instance("0", Some("alice")).as_deref(),
            Some("0")
        );
        assert_eq!(resolve_service_instance("0", None).as_deref(), Some("0"));
    }

    #[test]
    fn resolve_manifest_services_resolves_percent_u_in_user_mode() {
        let toml = service_manifest("anolisa-memory@.service", false, false, Some("%u"));
        let manifest = ComponentManifest::from_toml_str(&toml).expect("parse manifest");
        let reqs = resolve_manifest_services(&manifest, "agent-memory", "user").expect("resolve");
        // The exact name is the live login user, but `%u` must be gone and the
        // template must be instantiated.
        assert!(
            !reqs[0].unit.contains("%u"),
            "unit must not keep the literal specifier: {}",
            reqs[0].unit
        );
        assert!(reqs[0].unit.starts_with("anolisa-memory@"));
        assert!(reqs[0].unit.ends_with(".service"));
        assert_ne!(reqs[0].unit, "anolisa-memory@.service");
    }

    #[test]
    fn resolve_manifest_services_keeps_percent_u_template_in_system_mode() {
        // System mode is place-only for user-scope templates: leave `%u`
        // un-resolved so per-user `systemctl --user enable` instantiates it.
        let toml = service_manifest("anolisa-memory@.service", false, false, Some("%u"));
        let manifest = ComponentManifest::from_toml_str(&toml).expect("parse manifest");
        let reqs = resolve_manifest_services(&manifest, "agent-memory", "system").expect("resolve");
        assert_eq!(reqs[0].unit, "anolisa-memory@.service");
    }

    #[test]
    fn resolve_install_hooks_classifies_phases_and_filters_uninstall() {
        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        let toml = hooks_manifest(&[
            ("pre_install", "{datadir}/hooks/demo/pre-install.sh", false),
            ("post_install", "{datadir}/hooks/demo/post-install.sh", true),
            ("post_enable", "{datadir}/hooks/demo/post-enable.sh", false),
            (
                "pre_uninstall",
                "{datadir}/hooks/demo/pre-uninstall.sh",
                false,
            ),
        ]);
        let manifest = ComponentManifest::from_toml_str(&toml).expect("parse manifest");
        let hooks = resolve_install_hooks(&manifest, &layout, "demo").expect("resolve");

        assert_eq!(hooks.pre_install.len(), 1);
        assert_eq!(hooks.post_install.len(), 1);
        assert!(hooks.post_install[0].strict, "strict carried from contract");
        assert_eq!(hooks.post_enable.len(), 1);
        assert_eq!(
            hooks.pre_install[0].script,
            layout.datadir.join("hooks/demo/pre-install.sh"),
        );
        // The pre_uninstall entry must not leak into any install-phase list.
        let total = hooks.pre_install.len() + hooks.post_install.len() + hooks.post_enable.len();
        assert_eq!(total, 3, "uninstall-phase hook must be excluded");
    }

    #[test]
    fn resolve_install_hooks_rejects_invalid_placeholder() {
        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        let toml = hooks_manifest(&[("post_install", "{nope}/x.sh", false)]);
        let manifest = ComponentManifest::from_toml_str(&toml).expect("parse manifest");
        let err = resolve_install_hooks(&manifest, &layout, "demo").expect_err("must error");
        assert!(matches!(err, CliError::Runtime { .. }));
    }
}
