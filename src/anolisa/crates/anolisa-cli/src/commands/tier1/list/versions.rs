//! `anolisa list <component> --versions` — published-version discovery.
//!
//! Read-only companion to `install --version`: resolves the input against
//! the component index (exact name first, then the shared alias lookup),
//! then asks each backend the component declares for the versions it
//! publishes. Raw versions come from the distribution index that install
//! resolution consults; rpm versions from `dnf repoquery` against the
//! configured ANOLISA repository. A failing backend degrades to a warning
//! so one unreachable source does not hide the other's answer.

use std::collections::BTreeSet;

use anolisa_core::ResolveQuery;
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::pkg_query::{PackageQuery, PackageQueryError, rpm_evr_cmp};
use anolisa_platform::rpm_query::RpmPackageQuery;
use serde::Serialize;

use crate::commands::state_view::StateView;
use crate::commands::tier1::install::{
    configured_rpm_repo_source, fetch_installable_raw_index, installed_version_label,
};
use crate::context::CliContext;
use crate::repo_config::{HostVars, RepoConfig};
use crate::resolution::{ComponentIndex, ComponentIndexEntry, lookup_component_alias};
use crate::response::{CliError, render_json};

use super::{COMMAND, render_warnings};

/// JSON payload for one component's published versions.
#[derive(Debug, Serialize)]
pub(super) struct VersionsPayload {
    pub(super) component: String,
    pub(super) backends: Vec<BackendVersions>,
    /// Omitted when empty, matching the `list` table envelope.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) warnings: Vec<String>,
}

/// Versions one backend publishes for the component.
#[derive(Debug, Serialize)]
pub(super) struct BackendVersions {
    pub(super) backend: String,
    /// Backend-native package the versions belong to (what `install
    /// --version` will pin).
    pub(super) package: String,
    /// Highest-first, so the head is what a bare `install` would pick.
    pub(super) versions: Vec<VersionRow>,
}

/// One published version and whether a visible installation matches it.
#[derive(Debug, Serialize)]
pub(super) struct VersionRow {
    pub(super) version: String,
    pub(super) installed: bool,
}

/// Read-only inputs for version discovery, grouped so tests can inject a
/// fake package query without a live dnf.
pub(super) struct VersionSources<'a> {
    pub(super) layout: &'a FsLayout,
    pub(super) env: &'a anolisa_env::EnvFacts,
    pub(super) repo_config: &'a RepoConfig,
    pub(super) index: &'a ComponentIndex,
    pub(super) view: &'a StateView,
    pub(super) rpm_query: &'a dyn PackageQuery,
}

pub(super) fn handle_versions(
    input: &str,
    ctx: &CliContext,
    layout: &FsLayout,
    env: &anolisa_env::EnvFacts,
    repo_config: &RepoConfig,
    index: &ComponentIndex,
    view: &StateView,
) -> Result<(), CliError> {
    // repoquery against the configured ANOLISA repo when one exists, so the
    // listed versions match what a delegated install can actually pull.
    let rpm_query = match configured_rpm_repo_source(repo_config, env)
        .map_err(|err| err.with_command(COMMAND))?
    {
        Some(repo) => RpmPackageQuery::system_with_repo(repo),
        None => RpmPackageQuery::system(),
    };
    let sources = VersionSources {
        layout,
        env,
        repo_config,
        index,
        view,
        rpm_query: &rpm_query,
    };
    let payload = build_versions_payload(input, ctx, &sources)?;

    if ctx.json {
        return render_json(COMMAND, &payload);
    }
    if !ctx.quiet {
        render_warnings(&payload.warnings);
        render_human_versions(&payload);
    }
    Ok(())
}

/// Assemble the versions report for one component across its backends.
pub(super) fn build_versions_payload(
    input: &str,
    ctx: &CliContext,
    sources: &VersionSources<'_>,
) -> Result<VersionsPayload, CliError> {
    let entry =
        resolve_index_entry(input, sources.index).ok_or_else(|| CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!(
                "unknown component '{input}': not in the component index — run `anolisa list` for available components"
            ),
        })?;

    // Version labels of visible installations, for the `installed` marker on
    // raw rows (rpm rows compare against the live rpmdb EVR instead).
    let installed_labels: BTreeSet<String> = sources
        .view
        .visible_components()
        .iter()
        .filter(|record| record.object.name == entry.name)
        .map(|record| installed_version_label(record.object))
        .collect();

    let mut warnings = Vec::new();
    let mut backends = Vec::new();
    for (kind, package) in declared_backends(entry, sources.repo_config) {
        if !sources.repo_config.backends.contains_key(&kind) {
            warnings.push(format!(
                "backend '{kind}' of component '{}' is not configured in repo.toml; skipping",
                entry.name
            ));
            continue;
        }
        match kind.as_str() {
            "raw" => match raw_backend_versions(ctx, sources, &package) {
                Ok(published) => backends.push(BackendVersions {
                    backend: kind,
                    versions: published
                        .into_iter()
                        .map(|version| VersionRow {
                            installed: installed_labels.contains(&version),
                            version,
                        })
                        .collect(),
                    package,
                }),
                Err(err) => warnings.push(format!(
                    "raw backend versions unavailable: {}",
                    err.reason()
                )),
            },
            "rpm" => match rpm_backend_versions(sources.rpm_query, &package) {
                Ok((published, installed_evr)) => backends.push(BackendVersions {
                    backend: kind,
                    versions: published
                        .into_iter()
                        .map(|version| VersionRow {
                            installed: installed_evr.as_deref() == Some(version.as_str()),
                            version,
                        })
                        .collect(),
                    package,
                }),
                Err(err) => warnings.push(format!("rpm backend versions unavailable: {err}")),
            },
            other => warnings.push(format!(
                "backend '{other}' has no version listing support yet"
            )),
        }
    }

    Ok(VersionsPayload {
        component: entry.name.clone(),
        backends,
        warnings,
    })
}

/// Exact index name first, then the shared alias lookup — the same order
/// install uses to resolve component identity.
fn resolve_index_entry<'a>(
    input: &str,
    index: &'a ComponentIndex,
) -> Option<&'a ComponentIndexEntry> {
    if let Some(entry) = index.components.iter().find(|entry| entry.name == input) {
        return Some(entry);
    }
    let canonical = lookup_component_alias(input, Some(index))?;
    index
        .components
        .iter()
        .find(|entry| entry.name == canonical)
}

/// Backend/package pairs to query: the component's declared backends, or —
/// for an index entry without backend rows — the repo default backend with
/// the component name as its package.
fn declared_backends(
    entry: &ComponentIndexEntry,
    repo_config: &RepoConfig,
) -> Vec<(String, String)> {
    if entry.backends.is_empty() {
        return vec![(repo_config.default_backend.clone(), entry.name.clone())];
    }
    entry
        .backends
        .iter()
        .map(|backend| (backend.kind.clone(), backend.package.clone()))
        .collect()
}

/// Versions the raw distribution index publishes for `package` on this host
/// (same filters install resolution applies), highest-first.
fn raw_backend_versions(
    ctx: &CliContext,
    sources: &VersionSources<'_>,
    package: &str,
) -> Result<Vec<String>, CliError> {
    let Some(backend) = sources.repo_config.backends.get("raw") else {
        // The caller only routes here for configured backends.
        return Ok(Vec::new());
    };
    let host = HostVars {
        os: sources.env.os.clone(),
        arch: sources.env.arch.clone(),
    };
    let base_url = sources
        .repo_config
        .resolved_base_url("raw", backend, &host)
        .map_err(|err| CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: err.to_string(),
        })?;
    let (index, _index_url) = fetch_installable_raw_index(sources.layout, &base_url)
        .map_err(|err| err.with_command(COMMAND))?;
    let query = ResolveQuery {
        component: package,
        version: None,
        channel: None,
        install_mode: ctx.install_mode.as_str(),
        os: &sources.env.os,
        arch: &sources.env.arch,
        libc: sources.env.libc.as_deref(),
        pkg_base: sources.env.pkg_base.as_deref(),
        preferred_types: &[],
    };
    Ok(index.matching_versions(&query))
}

/// Published EVRs highest-first plus the installed EVR, from the injected
/// package query (the live path queries the configured ANOLISA repo).
fn rpm_backend_versions(
    query: &dyn PackageQuery,
    package: &str,
) -> Result<(Vec<String>, Option<String>), PackageQueryError> {
    let mut available = query.query_available(package)?;
    available.sort_by(|a, b| rpm_evr_cmp(&b.version, &a.version));
    let mut versions: Vec<String> = available
        .iter()
        .map(|info| info.version.to_string())
        .collect();
    versions.dedup();
    let installed = query
        .query_installed(package)?
        .map(|info| info.version.to_string());
    // An installed EVR the repo no longer publishes still belongs in the
    // list (marked installed) so the picture stays complete.
    if let Some(evr) = &installed
        && !versions.iter().any(|version| version == evr)
    {
        versions.push(evr.clone());
    }
    Ok((versions, installed))
}

fn render_human_versions(payload: &VersionsPayload) {
    println!("versions for {}:", payload.component);
    for backend in &payload.backends {
        println!("  {} (package {}):", backend.backend, backend.package);
        if backend.versions.is_empty() {
            println!("    (none published)");
        }
        for row in &backend.versions {
            if row.installed {
                println!("    {} (installed)", row.version);
            } else {
                println!("    {}", row.version);
            }
        }
    }
    if payload.backends.is_empty() {
        println!("  (no queryable backend)");
    }
}
