//! Component-contract lint.
//!
//! The framework promises "add a capability without touching CLI code".
//! That only holds if every capability's manifest + DistributionIndex
//! entry are structurally complete *before* `enable --dry-run` runs the
//! planner. This module is the single place that walks the contract and
//! reports structural breakage — missing component references, version
//! drift, missing sha256, install destinations outside ANOLISA-owned
//! roots, unparseable env requirements, malformed health/adapter blocks.
//!
//! Two severities:
//!
//! * [`LintSeverity::Error`] — would have produced a broken plan
//!   (planner cannot make sense of the input, or executor would refuse
//!   to install). Forces [`crate::PlanStatus::Blocked`].
//! * [`LintSeverity::Warning`] — non-fatal contract gaps (missing
//!   descriptions, no health checks declared, etc.). Degrades the
//!   plan but does not block execution.
//!
//! The planner consumes [`lint_capability`] and folds findings into the
//! existing `warnings` / `blocked_reason` channels — there is no new
//! wire surface beyond the per-finding `{capability, component,
//! severity, code, message}` row.

use serde::Serialize;

use anolisa_platform::fs_layout::FsLayout;

use crate::catalog::Catalog;
use crate::distribution::{ArtifactType, DistributionEntry, DistributionIndex};
use crate::install_runner::SUPPORTED_ARTIFACT_TYPES;
use crate::manifest::{ArtifactSpec, ComponentManifest, ContractSpec, EnvRequirements, HealthSpec};
use crate::path_safety::{PathBoundaryError, validate_owned_path};

/// Severity of a [`LintFinding`]. Wire form is lowercase to match the
/// rest of the plan's `--json` payload.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LintSeverity {
    /// Finding blocks plan execution.
    Error,
    /// Finding is non-fatal but should be surfaced to the user.
    Warning,
}

impl LintSeverity {
    /// Stable lowercase wire label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

/// Structured lint result. One row per problem so the JSON consumer
/// can group/filter by `code` without reparsing free-form messages.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LintFinding {
    /// Capability being linted.
    pub capability: String,
    /// Component the finding is anchored to, when applicable. Capability-
    /// level findings leave this `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    /// Whether the finding blocks execution.
    pub severity: LintSeverity,
    /// Stable machine-readable identifier (e.g. `"E_MISSING_COMPONENT"`).
    /// Codes are part of the wire contract; do not rename without a
    /// migration note.
    pub code: &'static str,
    /// Human-readable explanation with enough context for manifest authors.
    pub message: String,
}

impl LintFinding {
    fn error(
        capability: &str,
        component: Option<&str>,
        code: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            capability: capability.to_string(),
            component: component.map(|s| s.to_string()),
            severity: LintSeverity::Error,
            code,
            message: message.into(),
        }
    }

    fn warning(
        capability: &str,
        component: Option<&str>,
        code: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            capability: capability.to_string(),
            component: component.map(|s| s.to_string()),
            severity: LintSeverity::Warning,
            code,
            message: message.into(),
        }
    }
}

/// Lint `capability_name` against the loaded `catalog`, the
/// `dist_index`, and the resolved `layout`.
///
/// Returns one [`LintFinding`] per problem, in source order (capability-
/// level first, then per-component). Returns an empty vector when the
/// contract is clean.
///
/// If the capability is absent from the catalog, the only finding is
/// `E_UNKNOWN_CAPABILITY` — the planner already routes unknown
/// capabilities to INVALID_ARGUMENT, but emitting a lint row keeps the
/// `--json` payload uniform and lets tooling distinguish "you typo'd"
/// from "the manifest is broken".
pub fn lint_capability(
    catalog: &Catalog,
    dist_index: &DistributionIndex,
    layout: &FsLayout,
    capability_name: &str,
) -> Vec<LintFinding> {
    let mut findings: Vec<LintFinding> = Vec::new();

    let Some(cap) = catalog.capability(capability_name) else {
        findings.push(LintFinding::error(
            capability_name,
            None,
            "E_UNKNOWN_CAPABILITY",
            format!("capability '{capability_name}' is not in the catalog"),
        ));
        return findings;
    };

    if cap.components.is_empty() {
        findings.push(LintFinding::error(
            capability_name,
            None,
            "E_NO_COMPONENTS",
            format!(
                "capability '{capability_name}' lists no components — nothing for enable to install"
            ),
        ));
    }

    for comp_name in &cap.components {
        let Some(comp) = catalog.component(comp_name) else {
            findings.push(LintFinding::error(
                capability_name,
                Some(comp_name),
                "E_MISSING_COMPONENT",
                format!(
                    "capability '{capability_name}' references component '{comp_name}' which is not in the catalog"
                ),
            ));
            continue;
        };

        lint_component(capability_name, comp, dist_index, layout, &mut findings);
    }

    findings
}

fn lint_component(
    capability: &str,
    comp: &ComponentManifest,
    dist_index: &DistributionIndex,
    layout: &FsLayout,
    findings: &mut Vec<LintFinding>,
) {
    let name = comp.component.name.as_str();

    // ---- component metadata -------------------------------------------------

    if comp.component.version.trim().is_empty() {
        findings.push(LintFinding::error(
            capability,
            Some(name),
            "E_MISSING_VERSION",
            format!("component '{name}' has no version declared"),
        ));
    }

    if comp.install.modes.is_empty() {
        findings.push(LintFinding::error(
            capability,
            Some(name),
            "E_NO_INSTALL_MODES",
            format!(
                "component '{name}' declares no install modes — enable cannot decide where to install"
            ),
        ));
    } else {
        for mode in &comp.install.modes {
            if !matches!(mode.as_str(), "system" | "user") {
                findings.push(LintFinding::error(
                    capability,
                    Some(name),
                    "E_UNSUPPORTED_INSTALL_MODE",
                    format!(
                        "component '{name}' declares unsupported install mode '{mode}' (allowed: system, user)"
                    ),
                ));
            }
        }
    }

    // ---- minimal-schema required fields ------------------------------------

    lint_minimal_schema(capability, comp, findings);

    // ---- install files: paths must land under owned roots ------------------

    lint_install_files(capability, comp, layout, findings);

    // ---- env requirements ---------------------------------------------------

    lint_env_requirements(capability, name, &comp.env_requirements, findings);

    // ---- health: structural shape only -------------------------------------

    lint_health_checks(capability, name, &comp.health_checks, findings);

    // ---- distribution index entries ----------------------------------------

    lint_distribution(capability, comp, dist_index, findings);
}

/// A manifest opts into the minimal schema (phase1-2-dev §2.1) the moment it
/// carries any of its distinguishing markers: a `[component].owner`, a
/// non-empty `[component.contract]`, or a non-empty `[component.artifact]`.
///
/// Detection is additive on purpose. Legacy manifests — `[component]` + flat
/// `[install]`/`[environment]` with none of these markers — never enter the
/// stricter required-field pass, so [`lint_minimal_schema`] cannot newly block
/// an artifact that was previously `Ready`. The destructive flip to
/// minimal-only lives in a later batch.
fn uses_minimal_schema(comp: &ComponentManifest) -> bool {
    comp.component.owner.is_some()
        || comp.contract != ContractSpec::default()
        || comp.artifact != ArtifactSpec::default()
}

/// Required-field pass for minimal-schema manifests only (see
/// [`uses_minimal_schema`]). Each missing field is an error because the
/// minimal schema promises a self-describing artifact: identity (`name`,
/// `version`, `owner`), a compatibility envelope (`contract.schema_version`),
/// a target (`platform.os`/`platform.arch`), an artifact shape
/// (`artifact.type`), and at least one install file (`layout.files`).
/// `health_check` stays optional — `enable` synthesizes a probe from the first
/// executable when it is absent.
fn lint_minimal_schema(
    capability: &str,
    comp: &ComponentManifest,
    findings: &mut Vec<LintFinding>,
) {
    if !uses_minimal_schema(comp) {
        return;
    }

    let name = comp.component.name.as_str();

    // `name` anchors every other finding, so flag an empty one capability-level.
    if name.trim().is_empty() {
        findings.push(LintFinding::error(
            capability,
            None,
            "E_MISSING_COMPONENT_NAME",
            "minimal-schema component has an empty [component].name",
        ));
    }

    if blank(comp.component.owner.as_deref()) {
        findings.push(LintFinding::error(
            capability,
            Some(name),
            "E_MISSING_OWNER",
            format!(
                "minimal-schema component '{name}' has no [component].owner — required so an installed artifact is attributable"
            ),
        ));
    }

    if blank(comp.contract.schema_version.as_deref()) {
        findings.push(LintFinding::error(
            capability,
            Some(name),
            "E_MISSING_CONTRACT_SCHEMA_VERSION",
            format!(
                "minimal-schema component '{name}' has no [component.contract].schema_version — required to gate compatibility"
            ),
        ));
    }

    if comp.env_requirements.os.iter().all(|o| o.trim().is_empty()) {
        findings.push(LintFinding::error(
            capability,
            Some(name),
            "E_MISSING_PLATFORM_OS",
            format!(
                "minimal-schema component '{name}' declares no [component.platform].os — resolver cannot target a host"
            ),
        ));
    }

    if comp
        .env_requirements
        .arch
        .iter()
        .all(|a| a.trim().is_empty())
    {
        findings.push(LintFinding::error(
            capability,
            Some(name),
            "E_MISSING_PLATFORM_ARCH",
            format!(
                "minimal-schema component '{name}' declares no [component.platform].arch — resolver cannot target a host"
            ),
        ));
    }

    if blank(comp.artifact.artifact_type.as_deref()) {
        findings.push(LintFinding::error(
            capability,
            Some(name),
            "E_MISSING_ARTIFACT_TYPE",
            format!(
                "minimal-schema component '{name}' has no [component.artifact].type (binary | archive | script-only | mixed)"
            ),
        ));
    }

    if comp.install.files.is_empty() {
        findings.push(LintFinding::error(
            capability,
            Some(name),
            "E_NO_LAYOUT_FILES",
            format!(
                "minimal-schema component '{name}' declares no [component.layout.files] — nothing for enable to install"
            ),
        ));
    }
}

/// `true` when the optional string is absent or only whitespace.
fn blank(value: Option<&str>) -> bool {
    value.map(str::trim).unwrap_or("").is_empty()
}

fn lint_install_files(
    capability: &str,
    comp: &ComponentManifest,
    layout: &FsLayout,
    findings: &mut Vec<LintFinding>,
) {
    let name = comp.component.name.as_str();
    for (idx, file) in comp.install.files.iter().enumerate() {
        let Some(template) = file.install_path() else {
            findings.push(LintFinding::error(
                capability,
                Some(name),
                "E_INSTALL_FILE_EMPTY",
                format!("component '{name}' install.files[{idx}] has neither source nor dest set"),
            ));
            continue;
        };

        let rendered = render_install_path(template, layout);
        let path = std::path::PathBuf::from(&rendered);

        // Distinguish two failure modes so users can tell "you wrote a
        // path under /etc directly" from "you used `..` in the dest".
        match validate_owned_path(layout, &path) {
            Ok(()) => {}
            Err(PathBoundaryError::External { .. }) => {
                findings.push(LintFinding::error(
                    capability,
                    Some(name),
                    "E_INSTALL_PATH_OUT_OF_BOUNDS",
                    format!(
                        "component '{name}' install file '{template}' resolves to '{rendered}' which is not under an ANOLISA-owned root (use {{bindir}} / {{libexecdir}} / {{datadir}} / {{etcdir}} / {{statedir}} / {{logdir}} / {{cachedir}})"
                    ),
                ));
            }
            Err(PathBoundaryError::Traversal { .. }) => {
                findings.push(LintFinding::error(
                    capability,
                    Some(name),
                    "E_INSTALL_PATH_TRAVERSAL",
                    format!(
                        "component '{name}' install file '{template}' contains a '.' or '..' segment after layout substitution"
                    ),
                ));
            }
        }

        if file.kind == crate::manifest::FileKind::Symlink {
            lint_symlink_entry(capability, name, idx, file, layout, findings);
        }
    }
}

/// Symlink-specific checks: both ends must be declared, and the referent
/// (`source`) is a layout-template path that must land under an
/// ANOLISA-owned root — a link may not point outside any more than a
/// regular file may be written there.
fn lint_symlink_entry(
    capability: &str,
    name: &str,
    idx: usize,
    file: &crate::manifest::InstallFileSpec,
    layout: &FsLayout,
    findings: &mut Vec<LintFinding>,
) {
    if file.dest.is_none() {
        findings.push(LintFinding::error(
            capability,
            Some(name),
            "E_SYMLINK_MISSING_DEST",
            format!("component '{name}' install.files[{idx}] is a symlink without a dest (link location)"),
        ));
    }
    let Some(referent) = file.source.as_deref() else {
        findings.push(LintFinding::error(
            capability,
            Some(name),
            "E_SYMLINK_MISSING_SOURCE",
            format!("component '{name}' install.files[{idx}] is a symlink without a source (link referent)"),
        ));
        return;
    };
    let rendered = render_install_path(referent, layout);
    match validate_owned_path(layout, std::path::Path::new(&rendered)) {
        Ok(()) => {}
        Err(PathBoundaryError::External { .. }) => {
            findings.push(LintFinding::error(
                capability,
                Some(name),
                "E_INSTALL_PATH_OUT_OF_BOUNDS",
                format!(
                    "component '{name}' symlink referent '{referent}' resolves to '{rendered}' which is not under an ANOLISA-owned root"
                ),
            ));
        }
        Err(PathBoundaryError::Traversal { .. }) => {
            findings.push(LintFinding::error(
                capability,
                Some(name),
                "E_INSTALL_PATH_TRAVERSAL",
                format!(
                    "component '{name}' symlink referent '{referent}' contains a '.' or '..' segment after layout substitution"
                ),
            ));
        }
    }
}

/// Mirror of [`crate::enable_plan`]'s template substitution. Kept private
/// to the lint module to avoid coupling lint output to planner internals;
/// the substitution map is small and any drift between the two is itself
/// a contract bug worth catching in review.
fn render_install_path(template: &str, layout: &FsLayout) -> String {
    let bin = layout.bin_dir.to_string_lossy().into_owned();
    let etc = layout.etc_dir.to_string_lossy().into_owned();
    let state = layout.state_dir.to_string_lossy().into_owned();
    let log = layout.log_dir.to_string_lossy().into_owned();
    let data = layout.datadir.to_string_lossy().into_owned();
    let libexec = layout.libexec_dir.to_string_lossy().into_owned();
    let cache = layout.cache_dir.to_string_lossy().into_owned();
    let lib = layout.lib_dir.to_string_lossy().into_owned();
    template
        .replace("{bindir}", &bin)
        .replace("{sysconfdir}", &etc)
        .replace("{etcdir}", &etc)
        .replace("{etc_dir}", &etc)
        .replace("{statedir}", &state)
        .replace("{state_dir}", &state)
        .replace("{logdir}", &log)
        .replace("{log_dir}", &log)
        .replace("{sharedir}", &data)
        .replace("{datadir}", &data)
        .replace("{libexecdir}", &libexec)
        .replace("{libexec_dir}", &libexec)
        .replace("{cachedir}", &cache)
        .replace("{cache_dir}", &cache)
        .replace("{libdir}", &lib)
        .replace("{lib_dir}", &lib)
}

fn lint_env_requirements(
    capability: &str,
    component: &str,
    reqs: &EnvRequirements,
    findings: &mut Vec<LintFinding>,
) {
    if let Some(kernel_min) = reqs.kernel_min.as_deref() {
        let head: String = kernel_min
            .trim_start_matches(">=")
            .trim_start_matches('>')
            .trim()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if head.is_empty() || head.split('.').any(|p| p.is_empty()) {
            findings.push(LintFinding::error(
                capability,
                Some(component),
                "E_KERNEL_MIN_UNPARSEABLE",
                format!(
                    "component '{component}' env_requirements.kernel_min = '{kernel_min}' is not a parseable version constraint"
                ),
            ));
        }
    }

    for os in &reqs.os {
        if os.trim().is_empty() {
            findings.push(LintFinding::error(
                capability,
                Some(component),
                "E_EMPTY_ENV_OS",
                format!("component '{component}' env_requirements.os contains an empty entry"),
            ));
        }
    }

    for arch in &reqs.arch {
        if arch.trim().is_empty() {
            findings.push(LintFinding::error(
                capability,
                Some(component),
                "E_EMPTY_ENV_ARCH",
                format!("component '{component}' env_requirements.arch contains an empty entry"),
            ));
        }
    }
}

fn lint_health_checks(
    capability: &str,
    component: &str,
    checks: &[HealthSpec],
    findings: &mut Vec<LintFinding>,
) {
    // Absence of `[[health_checks]]` is intentional for many components
    // (the planner cannot meaningfully run them yet anyway) — we only
    // lint the *shape* of declared entries, not their presence.
    for (idx, h) in checks.iter().enumerate() {
        if h.kind.trim().is_empty() {
            findings.push(LintFinding::error(
                capability,
                Some(component),
                "E_HEALTH_CHECK_NO_KIND",
                format!("component '{component}' health_checks[{idx}] has no kind set"),
            ));
        }
        let has_target = h.command.is_some() || h.probe.is_some() || h.unit.is_some();
        if !has_target {
            findings.push(LintFinding::warning(
                capability,
                Some(component),
                "W_HEALTH_CHECK_NO_TARGET",
                format!(
                    "component '{component}' health_checks[{idx}] has no command/probe/unit — nothing to run"
                ),
            ));
        }
    }
}

fn lint_distribution(
    capability: &str,
    comp: &ComponentManifest,
    dist_index: &DistributionIndex,
    findings: &mut Vec<LintFinding>,
) {
    let name = comp.component.name.as_str();
    let entries: Vec<&DistributionEntry> = dist_index
        .entries
        .iter()
        .filter(|e| e.component == name)
        .collect();

    if entries.is_empty() {
        // The planner already surfaces this as a per-component blocker
        // via the resolver, but emitting it as a lint warning lets tools
        // distinguish "the index doesn't know about you yet" (release
        // pipeline gap) from "the index disagrees with your manifest"
        // (catalog gap). Keep this a warning so demo overlays that
        // attach an index at run time are not blocked at lint time.
        findings.push(LintFinding::warning(
            capability,
            Some(name),
            "W_NO_DISTRIBUTION_ENTRY",
            format!(
                "component '{name}' has no entry in the distribution index — enable will only resolve via an overlay"
            ),
        ));
        return;
    }

    // Pre-scan: does this component have at least one entry the install
    // runner can actually handle? If yes, unsupported sibling entries
    // (a future `rpm` / `deb` / `oci` published alongside today's
    // `tar_gz`) are demoted to a per-entry warning so the resolver-
    // selectable path keeps the plan Ready. If no, every unsupported
    // entry is an error so the planner refuses a component that has
    // literally no install path the runner can dispatch.
    let has_supported_entry = entries
        .iter()
        .any(|e| SUPPORTED_ARTIFACT_TYPES.contains(&artifact_type_label(e.artifact_type)));

    let mut saw_version_match = false;
    for entry in entries.iter() {
        if entry.version == comp.component.version {
            saw_version_match = true;
        }

        if entry
            .sha256
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
        {
            findings.push(LintFinding::error(
                capability,
                Some(name),
                "E_ARTIFACT_NO_SHA256",
                format!(
                    "component '{name}' artifact ({} v{}) has no sha256 — refuse to install without verification",
                    backend_label(entry),
                    entry.version,
                ),
            ));
        }

        if entry.backend.trim().is_empty() {
            findings.push(LintFinding::error(
                capability,
                Some(name),
                "E_ARTIFACT_NO_BACKEND",
                format!(
                    "component '{name}' artifact v{} has no backend declared",
                    entry.version
                ),
            ));
        }

        // Block unimplemented artifact types at lint time so a `rpm` /
        // `deb` / `oci` / `zip` / `file` entry never makes it into a
        // `Ready` plan when there's no fallback. Today the install runner
        // only dispatches `binary` and `tar_gz` — anything else would
        // dry-run "fine" and then fail at the runner with
        // `InstallError::UnsupportedArtifactType`. The supported set
        // lives in install_runner so the two cannot drift.
        //
        // Scope rule: if the component publishes at least one supported
        // entry, a parallel unsupported entry is informational only
        // (W_UNSUPPORTED_ARTIFACT_TYPE), since the resolver will pick
        // the supported one. If every entry is unsupported, escalate so
        // the component cannot reach a Ready plan.
        let wire_type = artifact_type_label(entry.artifact_type);
        if !SUPPORTED_ARTIFACT_TYPES.contains(&wire_type) {
            let message = format!(
                "component '{name}' artifact v{} declares artifact_type '{}' which the install runner cannot handle (supported: {})",
                entry.version,
                wire_type,
                SUPPORTED_ARTIFACT_TYPES.join(", "),
            );
            if has_supported_entry {
                findings.push(LintFinding::warning(
                    capability,
                    Some(name),
                    "W_UNSUPPORTED_ARTIFACT_TYPE",
                    message,
                ));
            } else {
                findings.push(LintFinding::error(
                    capability,
                    Some(name),
                    "E_UNSUPPORTED_ARTIFACT_TYPE",
                    message,
                ));
            }
        }

        if entry.os.trim().is_empty() {
            findings.push(LintFinding::error(
                capability,
                Some(name),
                "E_ARTIFACT_NO_OS",
                format!(
                    "component '{name}' artifact v{} has no 'os' selector",
                    entry.version
                ),
            ));
        }
        if entry.arch.trim().is_empty() {
            findings.push(LintFinding::error(
                capability,
                Some(name),
                "E_ARTIFACT_NO_ARCH",
                format!(
                    "component '{name}' artifact v{} has no 'arch' selector",
                    entry.version
                ),
            ));
        }

        if entry.install_modes.is_empty() {
            findings.push(LintFinding::warning(
                capability,
                Some(name),
                "W_ARTIFACT_NO_INSTALL_MODES",
                format!(
                    "component '{name}' artifact v{} declares no install_modes — resolver will accept it for any mode",
                    entry.version
                ),
            ));
        }
    }

    if !saw_version_match {
        findings.push(LintFinding::warning(
            capability,
            Some(name),
            "W_DISTRIBUTION_VERSION_DRIFT",
            format!(
                "component '{name}' manifest version '{}' has no exact match in the distribution index ({})",
                comp.component.version,
                entries
                    .iter()
                    .map(|e| e.version.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        ));
    }
}

fn artifact_type_label(t: ArtifactType) -> &'static str {
    match t {
        ArtifactType::Rpm => "rpm",
        ArtifactType::Deb => "deb",
        ArtifactType::TarGz => "tar_gz",
        ArtifactType::Zip => "zip",
        ArtifactType::Oci => "oci",
        ArtifactType::File => "file",
        ArtifactType::Binary => "binary",
    }
}

fn backend_label(entry: &DistributionEntry) -> String {
    if entry.backend.is_empty() {
        artifact_type_label(entry.artifact_type).to_string()
    } else {
        entry.backend.clone()
    }
}

/// `true` if any finding is an error. Convenience helper for the planner
/// so it does not have to know about [`LintSeverity`] directly.
pub fn has_errors(findings: &[LintFinding]) -> bool {
    findings.iter().any(|f| f.severity == LintSeverity::Error)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::distribution::{ArtifactType, DistributionEntry, DistributionIndex};
    use crate::manifest::{
        BuildSpec, CapabilityManifest, CapabilityMeta, ComponentManifest, ComponentMeta,
        DependenciesSpec, FeatureSpec, FileKind, InstallFileSpec, InstallSpec, SourceSpec,
    };
    use crate::{Catalog, CatalogLayers};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn user_layout() -> FsLayout {
        // Use FsLayout::user (the public ctor); XDG_* env may be set in
        // the test process, but path-boundary validation only needs the
        // layout's own roots to be self-consistent, which they always
        // are regardless of XDG selection.
        FsLayout::user(PathBuf::from("/tmp/h"))
    }

    fn cap_with(name: &str, components: Vec<String>, description: &str) -> CapabilityManifest {
        CapabilityManifest {
            schema_version: 2,
            capability: CapabilityMeta {
                name: name.to_string(),
                description: description.to_string(),
                layer: "tier1-capability".to_string(),
                stability: "stable".to_string(),
            },
            components,
            default_features: Vec::new(),
            env_requirements: EnvRequirements::default(),
        }
    }

    fn comp_with(
        name: &str,
        version: &str,
        install_modes: Vec<String>,
        files: Vec<InstallFileSpec>,
    ) -> ComponentManifest {
        ComponentManifest {
            schema_version: 2,
            component: ComponentMeta {
                name: name.to_string(),
                version: version.to_string(),
                layer: "runtime".to_string(),
                domain: None,
                display_name: None,
                owner: None,
                license: None,
                repository: None,
            },
            contract: Default::default(),
            artifact: Default::default(),
            source: SourceSpec::default(),
            distribution_selectors: Vec::new(),
            build: BuildSpec::default(),
            install: InstallSpec {
                modes: install_modes,
                files,
                services: Vec::new(),
                capabilities: Vec::new(),
            },
            env_requirements: EnvRequirements::default(),
            dependencies: DependenciesSpec::default(),
            features: Vec::<FeatureSpec>::new(),
            adapters: Vec::new(),
            health_check: None,
            health_checks: Vec::new(),
        }
    }

    fn make_catalog(caps: Vec<CapabilityManifest>, comps: Vec<ComponentManifest>) -> Catalog {
        let mut capabilities = BTreeMap::new();
        for c in caps {
            capabilities.insert(c.capability.name.clone(), c);
        }
        let mut components = BTreeMap::new();
        for c in comps {
            components.insert(c.component.name.clone(), c);
        }
        Catalog {
            capabilities,
            components,
            layers: CatalogLayers::bundled_only(PathBuf::from("/dev/null")),
        }
    }

    fn empty_index() -> DistributionIndex {
        DistributionIndex {
            schema_version: 1,
            channel: None,
            generated_at: None,
            expires_at: None,
            publisher: None,
            signature: None,
            entries: Vec::new(),
        }
    }

    fn index_with(entries: Vec<DistributionEntry>) -> DistributionIndex {
        DistributionIndex {
            schema_version: 1,
            channel: None,
            generated_at: None,
            expires_at: None,
            publisher: None,
            signature: None,
            entries,
        }
    }

    fn entry(
        comp: &str,
        version: &str,
        backend: &str,
        artifact_type: ArtifactType,
        sha256: Option<&str>,
    ) -> DistributionEntry {
        DistributionEntry {
            component: comp.to_string(),
            version: version.to_string(),
            channel: "stable".to_string(),
            artifact_type,
            backend: backend.to_string(),
            url: format!("file:///tmp/{comp}-{version}"),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            libc: None,
            pkg_base: None,
            install_modes: vec!["user".to_string()],
            sha256: sha256.map(|s| s.to_string()),
            signature: None,
            artifact_id: None,
            manifest_digest: None,
            size: None,
            signature_url: None,
            os_version: None,
            dependencies: Vec::new(),
        }
    }

    #[test]
    fn unknown_capability_yields_single_error() {
        let catalog = make_catalog(Vec::new(), Vec::new());
        let findings = lint_capability(&catalog, &empty_index(), &user_layout(), "nope");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, "E_UNKNOWN_CAPABILITY");
        assert!(has_errors(&findings));
    }

    #[test]
    fn missing_component_reference_is_an_error() {
        let cap = cap_with("c", vec!["ghost".to_string()], "");
        let catalog = make_catalog(vec![cap], Vec::new());
        let findings = lint_capability(&catalog, &empty_index(), &user_layout(), "c");
        let codes: Vec<_> = findings.iter().map(|f| f.code).collect();
        assert!(codes.contains(&"E_MISSING_COMPONENT"));
        assert!(has_errors(&findings));
    }

    #[test]
    fn install_path_outside_owned_root_is_an_error() {
        let cap = cap_with("c", vec!["x".to_string()], "");
        let comp = comp_with(
            "x",
            "1.0.0",
            vec!["user".to_string()],
            vec![InstallFileSpec {
                kind: FileKind::Data,
                source: None,
                dest: Some("/etc/passwd".to_string()),
                mode: None,
            }],
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let findings = lint_capability(&catalog, &empty_index(), &user_layout(), "c");
        let codes: Vec<_> = findings.iter().map(|f| f.code).collect();
        assert!(codes.contains(&"E_INSTALL_PATH_OUT_OF_BOUNDS"));
        assert!(has_errors(&findings));
    }

    #[test]
    fn install_path_with_traversal_is_an_error() {
        let cap = cap_with("c", vec!["x".to_string()], "");
        let comp = comp_with(
            "x",
            "1.0.0",
            vec!["user".to_string()],
            vec![InstallFileSpec {
                kind: FileKind::Data,
                source: None,
                dest: Some("{bindir}/../escape".to_string()),
                mode: None,
            }],
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let findings = lint_capability(&catalog, &empty_index(), &user_layout(), "c");
        let codes: Vec<_> = findings.iter().map(|f| f.code).collect();
        assert!(codes.contains(&"E_INSTALL_PATH_TRAVERSAL"));
        assert!(has_errors(&findings));
    }

    #[test]
    fn symlink_without_source_or_dest_is_an_error() {
        let cap = cap_with("c", vec!["x".to_string()], "");
        let comp = comp_with(
            "x",
            "1.0.0",
            vec!["user".to_string()],
            vec![
                InstallFileSpec {
                    kind: FileKind::Symlink,
                    source: None,
                    dest: Some("{bindir}/rtk".to_string()),
                    mode: None,
                },
                InstallFileSpec {
                    kind: FileKind::Symlink,
                    source: Some("{libexecdir}/tokenless/rtk".to_string()),
                    dest: None,
                    mode: None,
                },
            ],
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let findings = lint_capability(&catalog, &empty_index(), &user_layout(), "c");
        let codes: Vec<_> = findings.iter().map(|f| f.code).collect();
        assert!(codes.contains(&"E_SYMLINK_MISSING_SOURCE"));
        assert!(codes.contains(&"E_SYMLINK_MISSING_DEST"));
        assert!(has_errors(&findings));
    }

    #[test]
    fn symlink_referent_outside_owned_root_is_an_error() {
        let cap = cap_with("c", vec!["x".to_string()], "");
        let comp = comp_with(
            "x",
            "1.0.0",
            vec!["user".to_string()],
            vec![InstallFileSpec {
                kind: FileKind::Symlink,
                source: Some("/usr/bin/python3".to_string()),
                dest: Some("{bindir}/rtk".to_string()),
                mode: None,
            }],
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let findings = lint_capability(&catalog, &empty_index(), &user_layout(), "c");
        let codes: Vec<_> = findings.iter().map(|f| f.code).collect();
        assert!(codes.contains(&"E_INSTALL_PATH_OUT_OF_BOUNDS"));
        assert!(has_errors(&findings));
    }

    #[test]
    fn symlink_with_owned_referent_passes() {
        let cap = cap_with("c", vec!["x".to_string()], "");
        let comp = comp_with(
            "x",
            "1.0.0",
            vec!["user".to_string()],
            vec![
                InstallFileSpec {
                    kind: FileKind::Executable,
                    source: None,
                    dest: Some("{libexecdir}/tokenless/rtk".to_string()),
                    mode: Some("0755".to_string()),
                },
                InstallFileSpec {
                    kind: FileKind::Symlink,
                    source: Some("{libexecdir}/tokenless/rtk".to_string()),
                    dest: Some("{bindir}/rtk".to_string()),
                    mode: None,
                },
            ],
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let findings = lint_capability(&catalog, &empty_index(), &user_layout(), "c");
        assert!(
            !has_errors(&findings),
            "expected no errors, got: {findings:?}"
        );
    }

    #[test]
    fn unsupported_install_mode_is_an_error() {
        let cap = cap_with("c", vec!["x".to_string()], "");
        let comp = comp_with(
            "x",
            "1.0.0",
            vec!["weird".to_string()],
            vec![InstallFileSpec {
                kind: FileKind::Data,
                source: None,
                dest: Some("{bindir}/x".to_string()),
                mode: None,
            }],
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let findings = lint_capability(&catalog, &empty_index(), &user_layout(), "c");
        let codes: Vec<_> = findings.iter().map(|f| f.code).collect();
        assert!(codes.contains(&"E_UNSUPPORTED_INSTALL_MODE"));
    }

    /// A fully-populated minimal-schema component: owner + contract +
    /// platform + artifact + one executable layout file. Tests clone this and
    /// null out fields to assert the required-field pass fires only where it
    /// should.
    fn minimal_comp(name: &str) -> ComponentManifest {
        let mut comp = comp_with(
            name,
            "1.0.0",
            vec!["user".to_string()],
            vec![InstallFileSpec {
                kind: FileKind::Executable,
                source: None,
                dest: Some(format!("{{bindir}}/{name}")),
                mode: None,
            }],
        );
        comp.component.owner = Some("team".to_string());
        comp.contract = ContractSpec {
            schema_version: Some("1.0".to_string()),
            min_anolisa_version: None,
        };
        comp.artifact = ArtifactSpec {
            artifact_type: Some("binary".to_string()),
            archive_format: None,
            naming_pattern: None,
        };
        comp.env_requirements = EnvRequirements {
            os: vec!["linux".to_string()],
            arch: vec!["x86_64".to_string()],
            ..EnvRequirements::default()
        };
        comp
    }

    #[test]
    fn minimal_schema_complete_manifest_has_no_required_field_errors() {
        let cap = cap_with("c", vec!["m".to_string()], "");
        let comp = minimal_comp("m");
        let catalog = make_catalog(vec![cap], vec![comp]);
        let findings = lint_capability(&catalog, &empty_index(), &user_layout(), "c");
        // Empty index yields a W_NO_DISTRIBUTION_ENTRY warning, never an error.
        assert!(!has_errors(&findings), "unexpected errors: {findings:?}");
    }

    #[test]
    fn minimal_schema_missing_required_fields_block() {
        let cap = cap_with("c", vec!["m".to_string()], "");
        let mut comp = minimal_comp("m");
        // Keep the `owner` marker so minimal mode stays on, then strip the rest.
        comp.contract = ContractSpec::default();
        comp.artifact = ArtifactSpec::default();
        comp.env_requirements = EnvRequirements::default();
        comp.install.files.clear();
        let catalog = make_catalog(vec![cap], vec![comp]);
        let findings = lint_capability(&catalog, &empty_index(), &user_layout(), "c");
        let codes: Vec<_> = findings.iter().map(|f| f.code).collect();
        assert!(codes.contains(&"E_MISSING_CONTRACT_SCHEMA_VERSION"));
        assert!(codes.contains(&"E_MISSING_PLATFORM_OS"));
        assert!(codes.contains(&"E_MISSING_PLATFORM_ARCH"));
        assert!(codes.contains(&"E_MISSING_ARTIFACT_TYPE"));
        assert!(codes.contains(&"E_NO_LAYOUT_FILES"));
        assert!(has_errors(&findings));
    }

    #[test]
    fn minimal_schema_missing_owner_is_flagged_via_contract_marker() {
        // The `owner` itself is absent, so the contract sub-table is what opts
        // this manifest into minimal mode — proving detection is not owner-only.
        let cap = cap_with("c", vec!["m".to_string()], "");
        let mut comp = minimal_comp("m");
        comp.component.owner = None;
        let catalog = make_catalog(vec![cap], vec![comp]);
        let findings = lint_capability(&catalog, &empty_index(), &user_layout(), "c");
        let codes: Vec<_> = findings.iter().map(|f| f.code).collect();
        assert!(codes.contains(&"E_MISSING_OWNER"));
        assert!(has_errors(&findings));
    }

    #[test]
    fn legacy_schema_skips_minimal_required_fields() {
        // No owner/contract/artifact markers → legacy. Even though it declares
        // no platform and no artifact.type, none of the minimal required-field
        // codes may appear — the additive guard must not newly block it.
        let cap = cap_with("c", vec!["x".to_string()], "");
        let comp = comp_with(
            "x",
            "1.0.0",
            vec!["user".to_string()],
            vec![InstallFileSpec {
                kind: FileKind::Data,
                source: None,
                dest: Some("{bindir}/x".to_string()),
                mode: None,
            }],
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let findings = lint_capability(&catalog, &empty_index(), &user_layout(), "c");
        let codes: Vec<_> = findings.iter().map(|f| f.code).collect();
        for c in [
            "E_MISSING_OWNER",
            "E_MISSING_CONTRACT_SCHEMA_VERSION",
            "E_MISSING_PLATFORM_OS",
            "E_MISSING_PLATFORM_ARCH",
            "E_MISSING_ARTIFACT_TYPE",
            "E_NO_LAYOUT_FILES",
        ] {
            assert!(!codes.contains(&c), "legacy manifest newly blocked by {c}");
        }
    }

    #[test]
    fn missing_sha256_in_distribution_is_an_error() {
        let cap = cap_with("c", vec!["x".to_string()], "");
        let comp = comp_with(
            "x",
            "1.0.0",
            vec!["user".to_string()],
            vec![InstallFileSpec {
                kind: FileKind::Data,
                source: None,
                dest: Some("{bindir}/x".to_string()),
                mode: None,
            }],
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = index_with(vec![entry(
            "x",
            "1.0.0",
            "binary",
            ArtifactType::Binary,
            None,
        )]);
        let findings = lint_capability(&catalog, &index, &user_layout(), "c");
        let codes: Vec<_> = findings.iter().map(|f| f.code).collect();
        assert!(codes.contains(&"E_ARTIFACT_NO_SHA256"));
        assert!(has_errors(&findings));
    }

    #[test]
    fn version_drift_is_a_warning_not_an_error() {
        let cap = cap_with("c", vec!["x".to_string()], "described");
        let comp = comp_with(
            "x",
            "1.0.0",
            vec!["user".to_string()],
            vec![InstallFileSpec {
                kind: FileKind::Data,
                source: None,
                dest: Some("{bindir}/x".to_string()),
                mode: None,
            }],
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = index_with(vec![entry(
            "x",
            "0.9.0",
            "binary",
            ArtifactType::Binary,
            Some(&"0".repeat(64)),
        )]);
        let findings = lint_capability(&catalog, &index, &user_layout(), "c");
        let codes: Vec<_> = findings.iter().map(|f| f.code).collect();
        assert!(codes.contains(&"W_DISTRIBUTION_VERSION_DRIFT"));
        assert!(!has_errors(&findings));
    }

    #[test]
    fn empty_distribution_index_yields_warning_only() {
        let cap = cap_with("c", vec!["x".to_string()], "described");
        let comp = comp_with(
            "x",
            "1.0.0",
            vec!["user".to_string()],
            vec![InstallFileSpec {
                kind: FileKind::Data,
                source: None,
                dest: Some("{bindir}/x".to_string()),
                mode: None,
            }],
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let findings = lint_capability(&catalog, &empty_index(), &user_layout(), "c");
        let codes: Vec<_> = findings.iter().map(|f| f.code).collect();
        assert!(codes.contains(&"W_NO_DISTRIBUTION_ENTRY"));
        assert!(!has_errors(&findings));
    }

    #[test]
    fn unsupported_artifact_type_is_a_blocking_error_when_no_supported_fallback() {
        // rpm is a perfectly valid ArtifactType wire value, but the
        // install runner does not understand it yet — lint must refuse
        // it so a `Ready` dry-run plan cannot be followed by a runner
        // failure. When every published entry for this component is
        // unsupported, the resolver has no usable path so escalate to
        // error. The supported set lives in install_runner.
        let cap = cap_with("c", vec!["x".to_string()], "described");
        let comp = comp_with(
            "x",
            "1.0.0",
            vec!["user".to_string()],
            vec![InstallFileSpec {
                kind: FileKind::Data,
                source: None,
                dest: Some("{bindir}/x".to_string()),
                mode: None,
            }],
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = index_with(vec![entry(
            "x",
            "1.0.0",
            "rpm",
            ArtifactType::Rpm,
            Some(&"0".repeat(64)),
        )]);
        let findings = lint_capability(&catalog, &index, &user_layout(), "c");
        let codes: Vec<_> = findings.iter().map(|f| f.code).collect();
        assert!(
            codes.contains(&"E_UNSUPPORTED_ARTIFACT_TYPE"),
            "expected E_UNSUPPORTED_ARTIFACT_TYPE, got: {codes:?}",
        );
        assert!(has_errors(&findings));
        // The supported set is the single source of truth — lint must
        // accept anything in it without escalation.
        for supported in SUPPORTED_ARTIFACT_TYPES {
            assert!(
                supported == &"binary" || supported == &"tar_gz",
                "supported set drifted from the lint test fixture: {supported}",
            );
        }
    }

    #[test]
    fn unsupported_artifact_type_is_demoted_to_warning_when_supported_sibling_exists() {
        // If the component publishes both a supported (tar_gz) and an
        // unsupported (rpm) entry, the resolver will pick the supported
        // one — lint must NOT block the plan over a parallel entry the
        // runner will never touch. Emit it as W_UNSUPPORTED_ARTIFACT_TYPE
        // so tools can still surface the gap without escalating.
        let cap = cap_with("c", vec!["x".to_string()], "described");
        let comp = comp_with(
            "x",
            "1.0.0",
            vec!["user".to_string()],
            vec![InstallFileSpec {
                kind: FileKind::Data,
                source: None,
                dest: Some("{bindir}/x".to_string()),
                mode: None,
            }],
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = index_with(vec![
            entry(
                "x",
                "1.0.0",
                "tar_gz",
                ArtifactType::TarGz,
                Some(&"0".repeat(64)),
            ),
            entry(
                "x",
                "1.0.0",
                "rpm",
                ArtifactType::Rpm,
                Some(&"0".repeat(64)),
            ),
        ]);
        let findings = lint_capability(&catalog, &index, &user_layout(), "c");
        let codes: Vec<_> = findings.iter().map(|f| f.code).collect();
        assert!(
            codes.contains(&"W_UNSUPPORTED_ARTIFACT_TYPE"),
            "expected W_UNSUPPORTED_ARTIFACT_TYPE warning, got: {codes:?}",
        );
        assert!(
            !codes.contains(&"E_UNSUPPORTED_ARTIFACT_TYPE"),
            "rpm sibling must not escalate to error when a supported entry exists: {codes:?}",
        );
        assert!(
            !has_errors(&findings),
            "plan must not be blocked by a parallel unsupported entry: {findings:?}",
        );
    }

    #[test]
    fn happy_path_yields_no_errors() {
        let cap = cap_with("c", vec!["x".to_string()], "described");
        let comp = comp_with(
            "x",
            "1.0.0",
            vec!["user".to_string()],
            vec![InstallFileSpec {
                kind: FileKind::Data,
                source: None,
                dest: Some("{bindir}/x".to_string()),
                mode: None,
            }],
        );
        let catalog = make_catalog(vec![cap], vec![comp]);
        let index = index_with(vec![entry(
            "x",
            "1.0.0",
            "binary",
            ArtifactType::Binary,
            Some(&"0".repeat(64)),
        )]);
        let findings = lint_capability(&catalog, &index, &user_layout(), "c");
        assert!(
            !has_errors(&findings),
            "expected no errors, got: {findings:?}"
        );
    }
}
