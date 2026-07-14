//! Component identity resolution across install backends.
//!
//! This module owns the mapping from user input to two identities: the stable
//! ANOLISA component name and the selected backend's native package name.
//! Command handlers should consume this resolved pair instead of duplicating
//! backend-specific candidate chains.

use std::collections::BTreeSet;
use std::path::Path;

use anolisa_core::download::DownloadCache;
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::pkg_query::{PackageQuery, PackageQueryError};
use serde::Deserialize;
use thiserror::Error;

use crate::repo_config::{BackendConfig, HostVars, RepoConfig, component_index_url};

/// On-disk schema version for repo-side `components.toml`.
pub(crate) const COMPONENT_INDEX_SCHEMA_VERSION: u32 = 1;

/// Repository-side component identity and backend mapping index.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct ComponentIndex {
    /// Wire schema version; loaders reject versions they do not understand.
    pub(crate) schema_version: u32,
    /// Optional publish timestamp for diagnostics.
    ///
    /// This is not required in v1 indexes, so absent fields deserialize as
    /// `None` to keep hand-authored indexes small.
    #[serde(default)]
    pub(crate) generated_at: Option<String>,
    /// Optional publishing party for diagnostics.
    ///
    /// This is informational metadata, not part of resolution.
    #[serde(default)]
    pub(crate) publisher: Option<String>,
    /// Component rows.
    ///
    /// Empty indexes are valid so repositories can publish the file before
    /// every backend mapping has been populated.
    #[serde(default)]
    pub(crate) components: Vec<ComponentIndexEntry>,
}

/// One ANOLISA component and its backend-native identities.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct ComponentIndexEntry {
    /// Stable ANOLISA component name.
    pub(crate) name: String,
    /// Optional human label; not used by resolution v1.
    #[serde(default)]
    pub(crate) display_name: Option<String>,
    /// Optional one-line summary; not used by resolution v1.
    #[serde(default)]
    pub(crate) summary: Option<String>,
    /// Backend-native package names for this component.
    ///
    /// Components may initially ship on only one backend, so the list defaults
    /// to empty rather than forcing placeholder rows.
    #[serde(default)]
    pub(crate) backends: Vec<ComponentBackendEntry>,
    /// Alternate user inputs that should resolve to this component.
    ///
    /// Alias rows are optional and mainly cover historical RPM package names.
    #[serde(default)]
    pub(crate) aliases: Vec<ComponentAliasEntry>,
}

/// Backend-native identity for a component.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct ComponentBackendEntry {
    /// Backend kind such as `raw` or `rpm`.
    pub(crate) kind: String,
    /// Backend-native package/artifact name.
    pub(crate) package: String,
    /// Expected RPM Provides capability, when this backend is `rpm`.
    ///
    /// This lets repo publishers document the intended package metadata while
    /// still allowing historical rows to exist before RPM specs are updated.
    #[serde(default)]
    pub(crate) provides: Option<String>,
    /// Whether repo metadata may identify a historical installed RPM that lacks
    /// the newer installed `Provides: anolisa-component(...)` declaration.
    ///
    /// Defaulting to false keeps new package mappings strict unless the repo
    /// publisher explicitly marks them as legacy-adoptable.
    #[serde(default)]
    pub(crate) legacy_adopt: bool,
}

/// Alternate input name for a component.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct ComponentAliasEntry {
    /// Alias kind, e.g. `rpm-package`.
    pub(crate) kind: String,
    /// Alias value.
    pub(crate) name: String,
}

/// Parse or validation failures for `components.toml`.
#[derive(Debug, Error)]
pub(crate) enum ComponentIndexError {
    /// TOML parse or read error.
    #[error("failed to parse component index at {path}: {reason}")]
    Parse { path: String, reason: String },
    /// Unsupported schema version.
    #[error("unsupported component index schema_version {actual} (expected {expected})")]
    UnsupportedSchema { actual: u32, expected: u32 },
    /// Invalid component row.
    #[error("invalid component index entry: {reason}")]
    Invalid { reason: String },
    /// Backend resolution or download failure.
    #[error("failed to fetch component index: {reason}")]
    Fetch { reason: String },
}

/// Backend selected for identity resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackendKind {
    /// Raw artifact backend.
    Raw,
    /// RPM package backend.
    Rpm,
    /// Any other configured backend.
    Other,
}

impl BackendKind {
    pub(crate) fn from_name(name: &str) -> Self {
        match name {
            "raw" => Self::Raw,
            "rpm" => Self::Rpm,
            _ => Self::Other,
        }
    }
}

/// Command context for a resolution request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolutionUse {
    /// `install` may install or adopt.
    Install,
    /// `adopt` only records an already-installed RPM.
    Adopt,
    /// `status` observes without writing state.
    StatusObserved,
    /// `repair` only migrates existing legacy RPM state rows.
    RepairLegacy,
}

/// Source that produced a resolved component/package pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolutionSource {
    /// Repository `components.toml`.
    ComponentIndex,
    /// Site-local `[backends.rpm.package_map]`.
    RepoPackageMap,
    /// Package identity retained by an existing managed component record.
    InstalledState,
    /// RPM metadata declares or provides `anolisa-component(...)` on host.
    InstalledRpmProvides,
    /// RPM repository metadata declares or provides `anolisa-component(...)`.
    AvailableRpmProvides,
    /// Raw distribution index fallback.
    RawDistributionIndex,
}

/// Final identity pair used by command handlers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedTarget {
    pub(crate) component: String,
    pub(crate) backend: BackendKind,
    pub(crate) package: String,
    pub(crate) source: ResolutionSource,
    pub(crate) legacy_adopt: bool,
}

impl ResolvedTarget {
    fn new(
        component: impl Into<String>,
        backend: BackendKind,
        package: impl Into<String>,
        source: ResolutionSource,
        legacy_adopt: bool,
    ) -> Self {
        Self {
            component: component.into(),
            backend,
            package: package.into(),
            source,
            legacy_adopt,
        }
    }
}

/// Cardinality result for resolving an input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolutionSet {
    /// No ANOLISA component identity could be proven.
    None,
    /// Exactly one target.
    Unique(ResolvedTarget),
    /// Several targets match and the caller must disambiguate.
    Ambiguous(Vec<ResolvedTarget>),
}

/// Options that affect resolution.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ResolveOptions<'a> {
    /// CLI package override, when supplied.
    pub(crate) package_override: Option<&'a str>,
}

/// Resolver over a repository component index plus backend-specific fallbacks.
pub(crate) struct ComponentResolver<'a> {
    component_index: Option<&'a ComponentIndex>,
    rpm_backend: Option<&'a BackendConfig>,
    rpm_query: Option<&'a dyn PackageQuery>,
}

impl<'a> ComponentResolver<'a> {
    pub(crate) fn new(
        component_index: Option<&'a ComponentIndex>,
        rpm_backend: Option<&'a BackendConfig>,
        rpm_query: Option<&'a dyn PackageQuery>,
    ) -> Self {
        Self {
            component_index,
            rpm_backend,
            rpm_query,
        }
    }

    /// Resolve `input` for `backend`.
    pub(crate) fn resolve(
        &self,
        input: &str,
        backend: BackendKind,
        use_case: ResolutionUse,
        opts: ResolveOptions<'_>,
    ) -> Result<ResolutionSet, PackageQueryError> {
        match backend {
            BackendKind::Rpm => self.resolve_rpm(input, use_case, opts),
            BackendKind::Raw => Ok(self.resolve_raw(input)),
            BackendKind::Other => Ok(ResolutionSet::None),
        }
    }

    fn resolve_raw(&self, input: &str) -> ResolutionSet {
        let targets = self
            .component_index
            .map(|idx| idx.targets_for_backend(input, BackendKind::Raw, "raw-package"))
            .unwrap_or_default();
        normalize_resolution_set(if targets.is_empty() {
            vec![ResolvedTarget::new(
                input,
                BackendKind::Raw,
                input,
                ResolutionSource::RawDistributionIndex,
                false,
            )]
        } else {
            targets
        })
    }

    fn resolve_rpm(
        &self,
        input: &str,
        use_case: ResolutionUse,
        opts: ResolveOptions<'_>,
    ) -> Result<ResolutionSet, PackageQueryError> {
        let query = self
            .rpm_query
            .expect("rpm resolution requires a PackageQuery");
        let mapped = self.rpm_backend.and_then(|b| b.package_map.get(input));

        if let Some(package) = opts.package_override {
            let mut targets = Vec::new();
            if mapped.is_some_and(|mapped| mapped == package) {
                targets.push(ResolvedTarget::new(
                    input,
                    BackendKind::Rpm,
                    package,
                    ResolutionSource::RepoPackageMap,
                    true,
                ));
            }
            if let Some(idx) = self.component_index {
                targets.extend(idx.targets_for_component_package(input, BackendKind::Rpm, package));
            }
            if let Some(target) = rpm_package_provides_component(query, package, input)? {
                targets.push(target);
            }
            return Ok(normalize_resolution_set(targets));
        }

        if let Some(idx) = self.component_index {
            let targets = idx.targets_for_backend(input, BackendKind::Rpm, "rpm-package");
            if !targets.is_empty() {
                return Ok(normalize_resolution_set(targets));
            }
        }

        if let Some(package) = mapped {
            return Ok(ResolutionSet::Unique(ResolvedTarget::new(
                input,
                BackendKind::Rpm,
                package,
                ResolutionSource::RepoPackageMap,
                true,
            )));
        }

        let provide = rpm_component_provide(input);
        let installed_providers = query.what_provides_installed(&provide)?;
        if !installed_providers.is_empty() {
            return Ok(normalize_resolution_set(
                installed_providers
                    .into_iter()
                    .map(|package| {
                        ResolvedTarget::new(
                            input,
                            BackendKind::Rpm,
                            package,
                            ResolutionSource::InstalledRpmProvides,
                            true,
                        )
                    })
                    .collect(),
            ));
        }

        let available_providers = query.what_provides_available(&provide)?;
        if !available_providers.is_empty() {
            let repo_backed_legacy = matches!(
                use_case,
                ResolutionUse::Install
                    | ResolutionUse::Adopt
                    | ResolutionUse::StatusObserved
                    | ResolutionUse::RepairLegacy
            );
            return Ok(normalize_resolution_set(
                available_providers
                    .into_iter()
                    .map(|package| {
                        ResolvedTarget::new(
                            input,
                            BackendKind::Rpm,
                            package,
                            ResolutionSource::AvailableRpmProvides,
                            repo_backed_legacy,
                        )
                    })
                    .collect(),
            ));
        }

        Ok(normalize_resolution_set(rpm_package_name_targets(
            query, input,
        )?))
    }
}

/// Resolve an RPM-oriented user input to a stable component name.
///
/// This is a read-only projection used by tests that need to exercise the full
/// RPM resolution chain (component index + package_map + rpmdb provides).
/// Production code uses [`lookup_component_alias`] which is in-memory only.
#[cfg(test)]
pub(crate) fn resolve_rpm_component_name(
    input: &str,
    rpm_backend: Option<&BackendConfig>,
    component_index: Option<&ComponentIndex>,
    query: &dyn PackageQuery,
    use_case: ResolutionUse,
) -> Option<String> {
    let resolver = ComponentResolver::new(component_index, rpm_backend, Some(query));
    match resolver.resolve(input, BackendKind::Rpm, use_case, ResolveOptions::default()) {
        Ok(ResolutionSet::Unique(target)) => Some(target.component),
        _ => None,
    }
}

/// In-memory alias resolution only — no rpmdb/dnf queries.
///
/// Used by [`common::lookup_component_name`](crate::commands::common::lookup_component_name)
/// for commands that address existing state. Checks the component index for
/// component-name, backend-package, and alias matches across both RPM and Raw
/// backends. Returns `None` when no match is found or the match is ambiguous
/// (multiple distinct components share the same alias/package name) so the
/// caller can fall back to the literal input.
pub(crate) fn lookup_component_alias(
    input: &str,
    component_index: Option<&ComponentIndex>,
) -> Option<String> {
    let idx = component_index?;

    // Collect unique component names from both backends. A single input
    // may match across RPM and Raw backends (e.g., the component name plus
    // an alias), but all matches must resolve to the same component — if two
    // distinct components share the same alias/package name, the lookup is
    // ambiguous and must not silently pick one.
    let mut components: BTreeSet<String> = BTreeSet::new();
    for target in idx.targets_for_backend(input, BackendKind::Rpm, "rpm-package") {
        components.insert(target.component);
    }
    for target in idx.targets_for_backend(input, BackendKind::Raw, "raw-package") {
        components.insert(target.component);
    }

    // Return only when the lookup resolves to exactly one unique component.
    if components.len() == 1 {
        components.into_iter().next()
    } else {
        None
    }
}

impl ComponentIndex {
    /// Parse and validate `components.toml`.
    pub(crate) fn from_toml_str(
        s: &str,
        path: impl AsRef<Path>,
    ) -> Result<Self, ComponentIndexError> {
        let path = path.as_ref().display().to_string();
        let parsed: Self = toml::from_str(s).map_err(|err| ComponentIndexError::Parse {
            path: path.clone(),
            reason: err.to_string(),
        })?;
        parsed.validate()?;
        Ok(parsed)
    }

    /// Load and validate from disk.
    pub(crate) fn load(path: impl AsRef<Path>) -> Result<Self, ComponentIndexError> {
        let path_ref = path.as_ref();
        let content =
            std::fs::read_to_string(path_ref).map_err(|err| ComponentIndexError::Parse {
                path: path_ref.display().to_string(),
                reason: err.to_string(),
            })?;
        Self::from_toml_str(&content, path_ref)
    }

    fn validate(&self) -> Result<(), ComponentIndexError> {
        if self.schema_version != COMPONENT_INDEX_SCHEMA_VERSION {
            return Err(ComponentIndexError::UnsupportedSchema {
                actual: self.schema_version,
                expected: COMPONENT_INDEX_SCHEMA_VERSION,
            });
        }
        let mut names = BTreeSet::new();
        for entry in &self.components {
            let name = entry.name.trim();
            if name.is_empty() {
                return Err(ComponentIndexError::Invalid {
                    reason: "component name must not be empty".to_string(),
                });
            }
            if !names.insert(name.to_string()) {
                return Err(ComponentIndexError::Invalid {
                    reason: format!("duplicate component '{name}'"),
                });
            }
            for backend in &entry.backends {
                if backend.kind.trim().is_empty() {
                    return Err(ComponentIndexError::Invalid {
                        reason: format!("component '{name}' has an empty backend kind"),
                    });
                }
                if backend.package.trim().is_empty() {
                    return Err(ComponentIndexError::Invalid {
                        reason: format!("component '{name}' has an empty backend package"),
                    });
                }
                if let Some(provides) = backend.provides.as_deref()
                    && BackendKind::from_name(&backend.kind) == BackendKind::Rpm
                    && provides != rpm_component_provide(name)
                {
                    return Err(ComponentIndexError::Invalid {
                        reason: format!(
                            "component '{name}' rpm provides must be '{}', got '{provides}'",
                            rpm_component_provide(name)
                        ),
                    });
                }
            }
            for alias in &entry.aliases {
                if alias.kind.trim().is_empty() || alias.name.trim().is_empty() {
                    return Err(ComponentIndexError::Invalid {
                        reason: format!("component '{name}' has an empty alias kind or name"),
                    });
                }
            }
        }
        Ok(())
    }

    fn targets_for_backend(
        &self,
        input: &str,
        backend: BackendKind,
        alias_kind: &str,
    ) -> Vec<ResolvedTarget> {
        let mut targets = Vec::new();
        for entry in &self.components {
            let matches_component = entry.name == input;
            let matches_alias = entry
                .aliases
                .iter()
                .any(|alias| alias.kind == alias_kind && alias.name == input);
            for backend_entry in entry.backends_for(backend) {
                let matches_package = backend_entry.package == input;
                if matches_component || matches_alias || matches_package {
                    targets.push(index_target(entry, backend, backend_entry));
                }
            }
        }
        targets
    }

    fn targets_for_component_package(
        &self,
        component: &str,
        backend: BackendKind,
        package: &str,
    ) -> Vec<ResolvedTarget> {
        let mut targets = Vec::new();
        for entry in &self.components {
            if entry.name != component {
                continue;
            }
            for backend_entry in entry.backends_for(backend) {
                if backend_entry.package == package {
                    targets.push(index_target(entry, backend, backend_entry));
                }
            }
        }
        targets
    }
}

impl ComponentIndexEntry {
    fn backends_for(&self, backend: BackendKind) -> impl Iterator<Item = &ComponentBackendEntry> {
        self.backends
            .iter()
            .filter(move |entry| BackendKind::from_name(&entry.kind) == backend)
    }
}

fn index_target(
    entry: &ComponentIndexEntry,
    backend: BackendKind,
    backend_entry: &ComponentBackendEntry,
) -> ResolvedTarget {
    ResolvedTarget::new(
        entry.name.clone(),
        backend,
        backend_entry.package.clone(),
        ResolutionSource::ComponentIndex,
        backend_entry.legacy_adopt,
    )
}

fn normalize_resolution_set(mut targets: Vec<ResolvedTarget>) -> ResolutionSet {
    let mut deduped = Vec::new();
    for target in targets.drain(..) {
        if !deduped.iter().any(|seen: &ResolvedTarget| {
            seen.component == target.component
                && seen.backend == target.backend
                && seen.package == target.package
        }) {
            deduped.push(target);
        }
    }
    match deduped.len() {
        0 => ResolutionSet::None,
        1 => ResolutionSet::Unique(deduped.remove(0)),
        _ => ResolutionSet::Ambiguous(deduped),
    }
}

pub(crate) fn rpm_component_provide(component: &str) -> String {
    format!("anolisa-component({component})")
}

fn rpm_package_provides_component(
    query: &dyn PackageQuery,
    package: &str,
    component: &str,
) -> Result<Option<ResolvedTarget>, PackageQueryError> {
    let capability = rpm_component_provide(component);
    let (providers, source) = match query.query_installed(package)? {
        Some(_) => (
            query.what_provides_installed(&capability)?,
            ResolutionSource::InstalledRpmProvides,
        ),
        None => (
            query.what_provides_available(&capability)?,
            ResolutionSource::AvailableRpmProvides,
        ),
    };
    Ok(providers
        .iter()
        .any(|provider| provider == package)
        .then(|| ResolvedTarget::new(component, BackendKind::Rpm, package, source, true)))
}

fn rpm_package_name_targets(
    query: &dyn PackageQuery,
    package: &str,
) -> Result<Vec<ResolvedTarget>, PackageQueryError> {
    let installed = query.query_installed(package)?.is_some();
    let capabilities = if installed {
        query.provided_capabilities_installed(package)?
    } else {
        query.provided_capabilities_available(package)?
    };
    Ok(rpm_components_from_capabilities(&capabilities)
        .into_iter()
        .map(|component| {
            ResolvedTarget::new(
                component,
                BackendKind::Rpm,
                package,
                if installed {
                    ResolutionSource::InstalledRpmProvides
                } else {
                    ResolutionSource::AvailableRpmProvides
                },
                true,
            )
        })
        .collect())
}

pub(crate) fn rpm_components_from_capabilities(capabilities: &[String]) -> Vec<String> {
    let mut components = Vec::new();
    for capability in capabilities {
        let Some(rest) = capability.trim().strip_prefix("anolisa-component(") else {
            continue;
        };
        let Some(end) = rest.find(')') else {
            continue;
        };
        let component = rest[..end].trim();
        if component.is_empty() || components.iter().any(|c| c == component) {
            continue;
        }
        components.push(component.to_string());
    }
    components
}

/// Load repo-side `components.toml`, returning a structured error on failure.
///
/// Used by commands (`ls`, `install --all`) that require the component index
/// to function. For best-effort usage where a missing index is acceptable,
/// use [`load_optional_component_index`] instead.
pub(crate) fn load_component_index(
    layout: &FsLayout,
    env: &anolisa_env::EnvFacts,
    repo_config: &RepoConfig,
) -> Result<ComponentIndex, ComponentIndexError> {
    let host = HostVars {
        os: env.os.clone(),
        arch: env.arch.clone(),
    };
    let (name, backend) =
        repo_config
            .select_backend(Some("raw"))
            .map_err(|err| ComponentIndexError::Fetch {
                reason: format!("cannot resolve raw backend in repo.toml: {err}"),
            })?;
    let base_url = repo_config
        .resolved_base_url(name, backend, &host)
        .map_err(|err| ComponentIndexError::Fetch {
            reason: format!("cannot resolve base_url for raw backend: {err}"),
        })?;
    let url = component_index_url(&base_url);

    let cache = DownloadCache::new(layout.cache_dir.clone());
    #[cfg(test)]
    if !url.starts_with("file://") {
        return Err(ComponentIndexError::Fetch {
            reason: format!("test mode: refusing non-file URL {url}"),
        });
    }
    let downloaded = cache
        .fetch(&url, None)
        .map_err(|err| ComponentIndexError::Fetch {
            reason: format!("failed to fetch {url}: {err}"),
        })?;
    ComponentIndex::load(&downloaded.cached_path)
}

/// Best-effort load of repo-side `components.toml`.
pub(crate) fn load_optional_component_index(
    layout: &FsLayout,
    env: &anolisa_env::EnvFacts,
    repo_config: &RepoConfig,
) -> Option<ComponentIndex> {
    load_component_index(layout, env, repo_config).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use anolisa_platform::pkg_query::{PackageInfo, PackageVersion};

    #[derive(Default)]
    struct FakeQuery {
        installed: Vec<(String, PackageInfo)>,
        component_providers: Vec<(String, Vec<String>)>,
        available_component_providers: Vec<(String, Vec<String>)>,
        package_provides: Vec<(String, Vec<String>)>,
        available_package_provides: Vec<(String, Vec<String>)>,
    }

    impl PackageQuery for FakeQuery {
        fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
            Ok(self
                .installed
                .iter()
                .find(|(name, _)| name == package)
                .map(|(_, info)| info.clone()))
        }

        fn query_available(&self, _package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
            Ok(Vec::new())
        }

        fn what_provides_installed(
            &self,
            capability: &str,
        ) -> Result<Vec<String>, PackageQueryError> {
            Ok(self
                .component_providers
                .iter()
                .find(|(cap, _)| cap == capability)
                .map(|(_, providers)| providers.clone())
                .unwrap_or_default())
        }

        fn what_provides_available(
            &self,
            capability: &str,
        ) -> Result<Vec<String>, PackageQueryError> {
            Ok(self
                .available_component_providers
                .iter()
                .find(|(cap, _)| cap == capability)
                .map(|(_, providers)| providers.clone())
                .unwrap_or_default())
        }

        fn provided_capabilities_installed(
            &self,
            package: &str,
        ) -> Result<Vec<String>, PackageQueryError> {
            Ok(self
                .package_provides
                .iter()
                .find(|(pkg, _)| pkg == package)
                .map(|(_, caps)| caps.clone())
                .unwrap_or_default())
        }

        fn provided_capabilities_available(
            &self,
            package: &str,
        ) -> Result<Vec<String>, PackageQueryError> {
            Ok(self
                .available_package_provides
                .iter()
                .find(|(pkg, _)| pkg == package)
                .map(|(_, caps)| caps.clone())
                .unwrap_or_default())
        }
    }

    fn pkg_info(name: &str) -> PackageInfo {
        PackageInfo {
            name: name.to_string(),
            version: PackageVersion {
                epoch: None,
                version: "1.0.0".to_string(),
                release: Some("1.al8".to_string()),
            },
            arch: "x86_64".to_string(),
            origin: None,
        }
    }

    fn index() -> ComponentIndex {
        ComponentIndex::from_toml_str(
            r#"
schema_version = 1
publisher = "anolisa"

[[components]]
name = "cosh"
display_name = "Copilot Shell"
summary = "shell"

[[components.backends]]
kind = "raw"
package = "cosh"

[[components.backends]]
kind = "rpm"
package = "copilot-shell"
provides = "anolisa-component(cosh)"
legacy_adopt = true

[[components.aliases]]
kind = "rpm-package"
name = "copilot-shell"
"#,
            "components.toml",
        )
        .expect("parse index")
    }

    #[test]
    fn repository_component_index_template_is_valid() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let index_path = manifest_dir.join("../../manifests/components.toml");
        ComponentIndex::load(&index_path).expect("component index template must parse");
    }

    #[test]
    fn repository_component_index_uses_sec_core_as_canonical_name() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let index_path = manifest_dir.join("../../manifests/components.toml");
        let idx = ComponentIndex::load(&index_path).expect("component index template must parse");
        let query = FakeQuery::default();
        let resolver = ComponentResolver::new(Some(&idx), None, Some(&query));

        let got = resolver
            .resolve(
                "sec-core",
                BackendKind::Rpm,
                ResolutionUse::Install,
                ResolveOptions::default(),
            )
            .expect("resolve sec-core");
        match got {
            ResolutionSet::Unique(target) => {
                assert_eq!(target.component, "sec-core");
                assert_eq!(target.package, "agent-sec-core");
                assert_eq!(target.source, ResolutionSource::ComponentIndex);
            }
            other => panic!("expected unique, got {other:?}"),
        }

        let package_name = resolver
            .resolve(
                "agent-sec-core",
                BackendKind::Rpm,
                ResolutionUse::Install,
                ResolveOptions::default(),
            )
            .expect("resolve package name");
        match package_name {
            ResolutionSet::Unique(target) => {
                assert_eq!(target.component, "sec-core");
                assert_eq!(target.package, "agent-sec-core");
                assert_eq!(target.source, ResolutionSource::ComponentIndex);
            }
            other => panic!("expected unique, got {other:?}"),
        }
    }

    #[test]
    fn load_optional_component_index_uses_raw_index_for_rpm_resolution() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let layout =
            anolisa_platform::fs_layout::FsLayout::system(Some(tmp.path().join("install-root")));
        let raw_parent = tmp.path().join("a-raw");
        let raw_v1 = raw_parent.join("v1");
        let rpm_root = tmp.path().join("z-rpm");
        std::fs::create_dir_all(&raw_v1).expect("mkdir raw repo");
        std::fs::create_dir_all(&rpm_root).expect("mkdir rpm repo");
        std::fs::write(
            raw_v1.join("components.toml"),
            r#"
schema_version = 1

[[components]]
name = "raw-index"
"#,
        )
        .expect("write raw components.toml");
        std::fs::write(
            rpm_root.join("components.toml"),
            r#"
schema_version = 1

[[components]]
name = "rpm-ignored"
"#,
        )
        .expect("write rpm components.toml");
        let repo_config = RepoConfig::from_toml_str(&format!(
            r#"
schema_version = 1
default_backend = "rpm"

[backends.raw]
base_url = "file://{}"

[backends.rpm]
base_url = "file://{}"
"#,
            raw_parent.display(),
            rpm_root.display()
        ))
        .expect("repo config");
        let env = anolisa_env::EnvFacts {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            libc: None,
            kernel: None,
            pkg_base: None,
            os_id: None,
            os_version: None,
            btf: None,
            cap_bpf: None,
            container: None,
            user: "tester".to_string(),
            uid: 1000,
            home: tmp.path().join("home"),
        };

        let idx =
            load_optional_component_index(&layout, &env, &repo_config).expect("load raw index");

        assert_eq!(idx.components.len(), 1);
        assert_eq!(idx.components[0].name, "raw-index");
    }

    #[test]
    fn component_index_resolves_component_name_to_rpm_package() {
        let idx = index();
        let query = FakeQuery::default();
        let resolver = ComponentResolver::new(Some(&idx), None, Some(&query));
        let got = resolver
            .resolve(
                "cosh",
                BackendKind::Rpm,
                ResolutionUse::Install,
                ResolveOptions::default(),
            )
            .expect("resolve");
        match got {
            ResolutionSet::Unique(target) => {
                assert_eq!(target.component, "cosh");
                assert_eq!(target.package, "copilot-shell");
                assert_eq!(target.source, ResolutionSource::ComponentIndex);
                assert!(target.legacy_adopt);
            }
            other => panic!("expected unique, got {other:?}"),
        }
    }

    #[test]
    fn component_index_precedes_package_map() {
        let idx = index();
        let repo = RepoConfig::from_toml_str(
            r#"
schema_version = 1
default_backend = "rpm"

[backends.rpm]
base_url = "https://example.invalid/rpm"

[backends.rpm.package_map]
cosh = "site-copilot"
"#,
        )
        .expect("repo config");
        let query = FakeQuery::default();
        let resolver = ComponentResolver::new(Some(&idx), repo.backends.get("rpm"), Some(&query));
        let got = resolver
            .resolve(
                "cosh",
                BackendKind::Rpm,
                ResolutionUse::Install,
                ResolveOptions::default(),
            )
            .expect("resolve");
        match got {
            ResolutionSet::Unique(target) => {
                assert_eq!(target.package, "copilot-shell");
                assert_eq!(target.source, ResolutionSource::ComponentIndex);
            }
            other => panic!("expected unique, got {other:?}"),
        }
    }

    #[test]
    fn component_index_resolves_rpm_package_alias_to_component() {
        let idx = index();
        let query = FakeQuery::default();
        let resolver = ComponentResolver::new(Some(&idx), None, Some(&query));
        let got = resolver
            .resolve(
                "copilot-shell",
                BackendKind::Rpm,
                ResolutionUse::Install,
                ResolveOptions::default(),
            )
            .expect("resolve");
        match got {
            ResolutionSet::Unique(target) => {
                assert_eq!(target.component, "cosh");
                assert_eq!(target.package, "copilot-shell");
            }
            other => panic!("expected unique, got {other:?}"),
        }
    }

    #[test]
    fn component_index_resolves_raw_component() {
        let idx = index();
        let resolver = ComponentResolver::new(Some(&idx), None, None);
        let got = resolver
            .resolve(
                "cosh",
                BackendKind::Raw,
                ResolutionUse::Install,
                ResolveOptions::default(),
            )
            .expect("resolve");
        match got {
            ResolutionSet::Unique(target) => {
                assert_eq!(target.component, "cosh");
                assert_eq!(target.package, "cosh");
                assert_eq!(target.source, ResolutionSource::ComponentIndex);
            }
            other => panic!("expected unique, got {other:?}"),
        }
    }

    #[test]
    fn rpm_package_name_falls_back_to_package_own_provides() {
        let query = FakeQuery {
            installed: vec![("copilot-shell".to_string(), pkg_info("copilot-shell"))],
            package_provides: vec![(
                "copilot-shell".to_string(),
                vec!["anolisa-component(cosh) = 1.0.0".to_string()],
            )],
            ..Default::default()
        };
        let resolver = ComponentResolver::new(None, None, Some(&query));
        let got = resolver
            .resolve(
                "copilot-shell",
                BackendKind::Rpm,
                ResolutionUse::Install,
                ResolveOptions::default(),
            )
            .expect("resolve");
        match got {
            ResolutionSet::Unique(target) => {
                assert_eq!(target.component, "cosh");
                assert_eq!(target.package, "copilot-shell");
                assert_eq!(target.source, ResolutionSource::InstalledRpmProvides);
            }
            other => panic!("expected unique, got {other:?}"),
        }
    }

    #[test]
    fn available_component_provider_identifies_absent_package() {
        let query = FakeQuery {
            available_component_providers: vec![(
                "anolisa-component(cosh)".to_string(),
                vec!["copilot-shell".to_string()],
            )],
            ..Default::default()
        };
        let resolver = ComponentResolver::new(None, None, Some(&query));
        let got = resolver
            .resolve(
                "cosh",
                BackendKind::Rpm,
                ResolutionUse::Install,
                ResolveOptions::default(),
            )
            .expect("resolve");
        match got {
            ResolutionSet::Unique(target) => {
                assert_eq!(target.component, "cosh");
                assert_eq!(target.package, "copilot-shell");
                assert_eq!(target.source, ResolutionSource::AvailableRpmProvides);
            }
            other => panic!("expected unique, got {other:?}"),
        }
    }

    #[test]
    fn plain_rpm_package_without_metadata_is_none() {
        let query = FakeQuery {
            installed: vec![("bash".to_string(), pkg_info("bash"))],
            ..Default::default()
        };
        let resolver = ComponentResolver::new(None, None, Some(&query));
        let got = resolver
            .resolve(
                "bash",
                BackendKind::Rpm,
                ResolutionUse::Install,
                ResolveOptions::default(),
            )
            .expect("resolve");
        assert_eq!(got, ResolutionSet::None);
    }

    #[test]
    fn unsupported_schema_is_rejected() {
        let err = ComponentIndex::from_toml_str("schema_version = 99", "components.toml")
            .expect_err("unsupported schema");
        assert!(matches!(
            err,
            ComponentIndexError::UnsupportedSchema { actual: 99, .. }
        ));
    }
}
