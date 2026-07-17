//! RPM candidate resolution shared by `install` and `adopt`: mapping a
//! component name (or `--package` override) to the RPM package(s) that could
//! back it, via the repo-side component index, repo.toml `package_map`, and
//! rpmdb `Provides: anolisa-component(...)` metadata.
//!
//! Presence probing and adoption both moved to the planner-driven pipelines
//! (`dispatch.rs`, `adopt.rs`); only the identity resolution lives here.

use anolisa_platform::pkg_query::{PackageQuery, PackageQueryError};

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
