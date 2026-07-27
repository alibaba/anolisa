//! RPM candidate resolution shared by `install` and `adopt`: mapping a
//! component name (or `--package` override) to the RPM package(s) that could
//! back it, via the repo-side component index, repo.toml `package_map`, and
//! rpmdb `Provides: anolisa-component(...)` metadata.
//!
//! Presence probing and adoption both moved to the planner-driven pipelines
//! (`dispatch.rs`, `adopt.rs`); only the identity resolution lives here.

use anolisa_platform::pkg_query::{PackageQuery, PackageQueryError};
use anolisa_platform::rpm_select::{PinnedSelection, nevra, select_pinned_candidate};

use crate::repo_config::BackendConfig;
use crate::resolution::{
    BackendKind, ComponentIndex, ComponentResolver, ResolutionSet, ResolutionUse, ResolveOptions,
    ResolvedTarget,
};

/// Resolved RPM component/package pair.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RpmTarget {
    pub(crate) component: String,
    pub(crate) package: String,
}

impl RpmTarget {
    pub(crate) fn from_resolved(target: ResolvedTarget) -> Self {
        Self {
            component: target.component,
            package: target.package,
        }
    }

    pub(crate) fn label(&self) -> String {
        if self.component == self.package {
            self.package.clone()
        } else {
            format!("{} -> {}", self.component, self.package)
        }
    }
}

/// Resolve candidate RPM component/package pairs for `input`.
///
/// Precedence, in order: CLI `--package`, repo-side component index,
/// repo.toml `package_map`, installed/available
/// `anolisa-component(<name>)` providers, then the input package's own
/// `Provides: anolisa-component(<component>)` metadata.
///
/// Ordinary RPM packages without ANOLISA metadata return an empty vector:
/// `install --backend rpm <arg>` installs ANOLISA components, not arbitrary
/// `dnf install <arg>` targets.
///
/// # Errors
/// Propagates a hard [`PackageQueryError`] from the package query; empty
/// query results are the normal "no explicit component identity" branch.
#[cfg(test)]
pub(crate) fn rpm_package_candidates(
    cli_override: Option<&str>,
    rpm_backend: Option<&BackendConfig>,
    query: &dyn PackageQuery,
    input: &str,
) -> Result<Vec<RpmTarget>, PackageQueryError> {
    rpm_package_candidates_with_index(
        cli_override,
        rpm_backend,
        None,
        query,
        input,
        ResolutionUse::Install,
    )
}

/// A repository candidate a `--version` pin resolved to.
///
/// Carries both the exact transaction spec (`artifact`) and the reporting
/// fields the dry-run/JSON surface exposes; the bare package identity stays
/// with the caller for observation and persisted state.
#[derive(Debug, Clone)]
pub(crate) struct PinnedRpm {
    /// Exact NEVRA handed to the native transaction.
    pub(crate) artifact: String,
    /// Upstream VERSION field the pin matched (the `--version` value).
    pub(crate) version: String,
    /// Full resolved EVR (`[epoch:]version-release`) of the candidate.
    pub(crate) evr: String,
    /// Architecture of the selected candidate (used to verify the installed
    /// build matches the pin).
    pub(crate) arch: String,
    /// Source repository the candidate came from, when reported.
    pub(crate) source_repo: Option<String>,
}

/// Why a `--version` pin could not resolve to a host-compatible candidate.
///
/// Kept distinct from [`crate::response::CliError`] so the caller can attach
/// the component/package/arch context it owns when rendering the message.
pub(crate) enum PinError {
    /// The repository query itself failed.
    Query(PackageQueryError),
    /// No candidate carried the requested version for any architecture.
    VersionAbsent,
    /// The version exists, but only for architectures this host cannot run.
    ArchUnsupported {
        /// Architectures the requested version is published for.
        offered: Vec<String>,
    },
}

/// Resolve `requested_version` of `package` to an exact repository candidate
/// for `host_arch`, querying the configured ANOLISA RPM repository.
///
/// Delegates candidate selection to
/// [`anolisa_platform::rpm_select::select_pinned_candidate`] (VERSION-field
/// match, host-arch/`noarch` filter, highest-EVR pick) and renders the winner
/// to a NEVRA. Never falls back to another version.
///
/// # Errors
/// See [`PinError`]: a hard query failure, an absent version, or a version
/// published only for other architectures.
pub(crate) fn resolve_pinned_candidate(
    query: &dyn PackageQuery,
    package: &str,
    requested_version: &str,
    host_arch: &str,
) -> Result<PinnedRpm, PinError> {
    let candidates = query.query_available(package).map_err(PinError::Query)?;
    match select_pinned_candidate(&candidates, requested_version, host_arch) {
        PinnedSelection::Selected(info) => Ok(PinnedRpm {
            artifact: nevra(&info),
            version: info.version.version.clone(),
            evr: info.version.to_string(),
            arch: info.arch.clone(),
            source_repo: info.origin.clone(),
        }),
        PinnedSelection::VersionAbsent => Err(PinError::VersionAbsent),
        PinnedSelection::ArchUnsupported { offered } => Err(PinError::ArchUnsupported { offered }),
    }
}

pub(crate) fn rpm_package_candidates_with_index(
    cli_override: Option<&str>,
    rpm_backend: Option<&BackendConfig>,
    component_index: Option<&ComponentIndex>,
    query: &dyn PackageQuery,
    input: &str,
    use_case: ResolutionUse,
) -> Result<Vec<RpmTarget>, PackageQueryError> {
    let resolver = ComponentResolver::new(component_index, rpm_backend, Some(query));
    let resolved = resolver.resolve(
        input,
        BackendKind::Rpm,
        use_case,
        ResolveOptions {
            package_override: cli_override,
        },
    )?;
    Ok(match resolved {
        ResolutionSet::None => Vec::new(),
        ResolutionSet::Unique(target) => vec![RpmTarget::from_resolved(target)],
        ResolutionSet::Ambiguous(targets) => {
            targets.into_iter().map(RpmTarget::from_resolved).collect()
        }
    })
}
