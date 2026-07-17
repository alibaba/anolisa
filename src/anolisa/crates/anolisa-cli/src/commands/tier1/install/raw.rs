//! Raw backend resolution and manifest contract parsing for the `install`
//! command. Execution moved to the planner-driven pipeline: `dispatch.rs`
//! drives the plan, `owned_ops.rs` performs the side effects.

use std::path::Path;

use anolisa_core::download::DownloadCache;
use anolisa_core::install_runner::{
    ResolvedInstallFile, SUPPORTED_ARTIFACT_TYPES, read_embedded_component_manifest_text,
};
use anolisa_core::path_safety::validate_owned_path;
use anolisa_core::{
    ArtifactType, CapabilityRequest, ComponentManifest, DistributionIndex, FileKind, HookPhase,
    HookSpec, ResolveQuery, ServiceRequest, expand_layout_placeholders, resolve_manifest_hooks,
};
use anolisa_platform::fs_layout::FsLayout;

use crate::context::CliContext;
use crate::repo_config::{
    HostVars, RepoConfig, raw_artifact_url, raw_index_url, raw_relative_root,
};
use crate::response::CliError;

use super::COMMAND;
use super::render::{artifact_ext, artifact_type_wire, repo_config_err};
use super::types::*;
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
    let index = DistributionIndex::load(&downloaded_index.cached_path)
        .map(installable_raw_index)
        .map_err(|err| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!("failed to parse distribution index {index_url}: {err}"),
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

fn installable_raw_index(mut index: DistributionIndex) -> DistributionIndex {
    index.entries.retain(|entry| {
        SUPPORTED_ARTIFACT_TYPES.contains(&artifact_type_wire(&entry.artifact_type))
    });
    index
}

impl InstallContractSource {
    fn label(self) -> &'static str {
        match self {
            Self::EmbeddedArtifact => "embedded artifact manifest",
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

    let contract = load_execution_install_contract(&resolution, &artifact.cached_path)?;
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
            let manifest =
                ComponentManifest::from_toml_str(&toml).map_err(|err| CliError::Runtime {
                    command: COMMAND.to_string(),
                    reason: format!(
                        "failed to parse embedded component manifest from {}: {err}",
                        resolution.artifact_url
                    ),
                })?;
            Ok(LoadedInstallContract {
                manifest,
                source: InstallContractSource::EmbeddedArtifact,
                toml,
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
