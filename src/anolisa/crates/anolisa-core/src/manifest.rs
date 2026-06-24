//! Manifest v2 schema.
//!
//! This module hosts the canonical typed representation of the TOML manifests
//! shipped under `src/anolisa/manifests/`. The single top-level shape is
//! `ComponentManifest` — a concrete component (runtime or osbase substrate).
//!
//! All deserialization is *tolerant*: missing optional fields default and we
//! accept both the new canonical TOML layout (per `templates/*.toml`) and the
//! current bundled fixture layout. Unknown keys are silently ignored so that
//! schema growth in either direction does not break existing artifacts.

use crate::distribution::ArtifactType;
use crate::health::CheckSpec;
use crate::hooks::HookPhase;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Default schema version applied when the TOML omits it.
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

// ---------------------------------------------------------------------------
// ComponentManifest
// ---------------------------------------------------------------------------

/// Canonical runtime / osbase component manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "ComponentManifestRaw")]
pub struct ComponentManifest {
    /// Schema version after tolerant parsing.
    pub schema_version: u32,
    /// Component identity and layer metadata.
    pub component: ComponentMeta,
    /// `[component.contract]` schema/version envelope (minimal schema).
    pub contract: ContractSpec,
    /// `[component.artifact]` artifact shape description (minimal schema).
    pub artifact: ArtifactSpec,
    /// Source tree or release source declaration.
    pub source: SourceSpec,
    /// Structured selector list. Each entry says "for this install_mode + OS
    /// family + arch (+optional libc/pkg_base), prefer these artifact types
    /// in order". The resolver feeds `preferred_artifact_types` into
    /// `ResolveQuery` as the tiebreaker.
    pub distribution_selectors: Vec<DistributionSelector>,
    /// Build backend declaration for source builds.
    pub build: BuildSpec,
    /// Files, services, and capabilities installed by this component.
    pub install: InstallSpec,
    /// `[backends]` — backend-specific packaging metadata. Empty for components
    /// that only ship raw artifacts; the RPM backend reads
    /// [`ManifestBackends::rpm`] to resolve the package name during adopt.
    #[serde(default, skip_serializing_if = "ManifestBackends::is_empty")]
    pub backends: ManifestBackends,
    /// Component-level host requirements.
    pub env_requirements: EnvRequirements,
    /// Structured dependency lists. `build`, `runtime`, and `components`
    /// stay separate so downstream consumers can reason about each kind
    /// (e.g. resolver only follows `components`, doctor only checks
    /// `runtime`).
    pub dependencies: DependenciesSpec,
    /// Feature toggles exposed by this component manifest.
    pub features: Vec<FeatureSpec>,
    /// `[[adapters]]` declarations preserved verbatim. The component schema
    /// itself does not install these — the Adapter layer (scan + framework
    /// detect + safe install/remove) consumes them — but the manifest keeps
    /// every parsed field so that tooling need not re-read the TOML.
    pub adapters: Vec<AdapterSpec>,
    /// Minimal-schema `[component.health_check]`. `None` falls back to a
    /// synthesized `binary_version` over the first executable layout file —
    /// see [`ComponentManifest::health_spec`].
    pub health_check: Option<CheckSpec>,
    /// Legacy `[[health_checks]]` entries in source order, retained for the
    /// existing `status` probe path during the additive-compat window.
    pub health_checks: Vec<HealthSpec>,
}

/// Structured distribution selector, surfaced for downstream consumers
/// (resolver, planner, doctor).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DistributionSelector {
    /// Optional install-mode selector.
    #[serde(default)]
    pub install_mode: Option<String>,
    /// Accepted OS names.
    #[serde(default)]
    pub os: Vec<String>,
    /// Accepted CPU architectures.
    #[serde(default)]
    pub arch: Vec<String>,
    /// Optional libc selector.
    #[serde(default)]
    pub libc: Option<String>,
    /// Optional package-base selector such as `rpm` or `deb`.
    #[serde(default)]
    pub pkg_base: Option<String>,
    /// Artifact-type preference used only after target filtering.
    #[serde(default)]
    pub preferred_artifact_types: Vec<ArtifactType>,
}

/// `[backends]` — backend-specific packaging metadata.
///
/// Orthogonal to [`DistributionSelector::pkg_base`] (which says *which package
/// format a distribution uses*): this says *what the component is named under a
/// given backend*. Only populated where a backend needs an explicit name; an
/// absent table leaves package-name resolution to the lower mapping tiers
/// (repo.toml `package_map` / RPM provides / default `anolisa-<component>`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManifestBackends {
    /// RPM backend packaging info; `None` falls through to the lower tiers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm: Option<RpmBackendSpec>,
}

impl ManifestBackends {
    /// Whether no backend metadata is present (used by `skip_serializing_if` so
    /// raw-only manifests round-trip without an empty `[backends]` table).
    pub fn is_empty(&self) -> bool {
        self.rpm.is_none()
    }
}

/// `[backends.rpm]` — RPM-backend packaging info for a component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpmBackendSpec {
    /// RPM package name for this component (e.g. `anolisa-copilot-shell`),
    /// the highest-precedence package-name source after a CLI `--package`
    /// override.
    pub package: String,
}

/// Component identity and placement metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentMeta {
    /// Stable component name.
    pub name: String,
    /// Component version expected by planners.
    pub version: String,
    /// Architecture layer label (`runtime`, `osbase`, ...).
    pub layer: String,
    /// Optional domain label for catalog grouping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Human-facing component name (minimal schema `display_name`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Owning team/maintainer (minimal schema `owner`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// SPDX license identifier (minimal schema `license`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Upstream repository URL (minimal schema `repository`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
}

/// `[component.contract]` — schema/version compatibility envelope (minimal
/// schema). Both fields are optional during the tokenless-first migration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContractSpec {
    /// Component-manifest schema version, e.g. `"1.0"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<String>,
    /// Minimum ANOLISA CLI version that can install this component.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_anolisa_version: Option<String>,
}

/// `[component.artifact]` — single-artifact shape description (minimal schema).
/// Complements [`DistributionSelector`], which selects among multiple targets.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactSpec {
    /// Artifact form: `binary` | `archive` | `script-only` | `mixed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_type: Option<String>,
    /// Archive format when `artifact_type = "archive"`, e.g. `"tar.gz"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_format: Option<String>,
    /// Filename template, e.g. `"{name}-{version}-{os}-{arch}.tar.gz"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub naming_pattern: Option<String>,
}

/// Source declaration for source-build capable components.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceSpec {
    /// Source kind (`git`, `path`, `archive`, ...).
    pub kind: String,
    /// Local source path, when the source is already staged.
    pub path: Option<String>,
    /// Remote source URL, when source must be fetched.
    pub url: Option<String>,
}

/// Build backend declaration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildSpec {
    /// Backend name such as `cargo`.
    pub backend: String,
    /// Expected output paths or target names.
    pub outputs: Vec<String>,
}

/// Install contract emitted by a component manifest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstallSpec {
    /// Supported install modes.
    pub modes: Vec<String>,
    /// Files copied or extracted during install.
    pub files: Vec<InstallFileSpec>,
    /// Service units managed by lifecycle operations (enable/start on
    /// install, stop/disable on uninstall).
    pub services: Vec<ServiceSpec>,
    /// Linux capability assignments requested for installed files.
    pub capabilities: Vec<InstallCapabilitySpec>,
    /// Lifecycle hook scripts run around install/uninstall phases.
    pub hooks: Vec<ManifestHookSpec>,
}

/// Role of an installed file (minimal-schema `type` key).
///
/// Drives default permissions (an [`FileKind::Executable`] without an explicit
/// mode installs 0755) and default health-check synthesis — the first
/// executable layout file becomes a `binary_version` probe when no
/// `[component.health_check]` is declared. Legacy `[install]` files carry no
/// `type` and default to [`FileKind::Data`].
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    /// Opaque data file (default).
    #[default]
    Data,
    /// Executable binary.
    Executable,
    /// Configuration file (purge-scoped on uninstall).
    Config,
    /// Shared library.
    Library,
    /// Symbolic link created at install time. `source` is the link's
    /// referent (a layout-template path like `{libexecdir}/tokenless/rtk`,
    /// expanded at resolve time — NOT an archive member), `target`/`dest`
    /// is where the link is created. `mode` is ignored: symlink
    /// permissions are meaningless on Linux.
    Symlink,
}

/// One file mapping in an install contract.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallFileSpec {
    /// Source path inside an artifact or source tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Destination path after layout placeholder expansion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest: Option<String>,
    /// Unix file mode requested by the manifest, e.g. `"0755"` or `"0644"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// File role from the minimal-schema `type` key. Defaults to
    /// [`FileKind::Data`]; legacy `[install]` files have no `type`.
    #[serde(default, skip_serializing_if = "is_default_file_kind")]
    pub kind: FileKind,
}

/// Skip serializing the default [`FileKind`] so round-tripped legacy manifests
/// stay byte-stable.
fn is_default_file_kind(kind: &FileKind) -> bool {
    *kind == FileKind::Data
}

impl InstallFileSpec {
    /// Destination if present, otherwise legacy single-path source.
    pub fn install_path(&self) -> Option<&str> {
        self.dest.as_deref().or(self.source.as_deref())
    }

    /// Human-readable mapping for plans and warnings.
    pub fn display(&self) -> String {
        match (self.source.as_deref(), self.dest.as_deref()) {
            (Some(source), Some(dest)) => format!("{source} -> {dest}"),
            (Some(source), None) => source.to_string(),
            (None, Some(dest)) => dest.to_string(),
            (None, None) => "<empty>".to_string(),
        }
    }
}

/// Manager scope for a declared service unit.
///
/// `system` units live under `{unitdir}` and are driven by `systemctl`
/// (needs root); `user` units live under `{userunitdir}` and are driven by
/// `systemctl --user` (per-user, no root). System services are only
/// actionable in system install mode.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceScope {
    /// System-level unit (`systemctl`, needs root). Default.
    #[default]
    System,
    /// User-level unit (`systemctl --user`, per-user, no root).
    User,
}

impl ServiceScope {
    /// systemd manager namespace recorded as `ServiceRef.manager` and shown
    /// in diagnostics: `systemd` for system units, `systemd-user` for user
    /// units (driven by `systemctl --user`). Single source of truth so the
    /// persisted label can never disagree with a unit's scope.
    pub fn manager_label(self) -> &'static str {
        match self {
            Self::System => "systemd",
            Self::User => "systemd-user",
        }
    }
}

/// systemd unit lifecycle declaration from `[[component.services]]`.
///
/// Drives `enable`/`start` on install and `stop`/`disable` on uninstall.
/// The unit *file* is an ordinary layout entry; this spec references it by
/// name and declares how its lifecycle is managed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceSpec {
    /// Native unit name, e.g. `agentsight.service`, or a template unit
    /// `anolisa-memory@.service` instantiated via [`instance`](Self::instance).
    pub unit: String,
    /// Manager scope. Defaults to [`ServiceScope::System`].
    #[serde(default)]
    pub scope: ServiceScope,
    /// `systemctl enable` on install for boot persistence. Defaults true.
    #[serde(default = "default_true")]
    pub enable: bool,
    /// Ensure the unit is active on install, restarting it when already
    /// running so upgrades pick up the new binary. Defaults true.
    #[serde(default = "default_true")]
    pub start: bool,
    /// Instance token for template (`@.service`) units — e.g. `%u` expands to
    /// the caller's username (`anolisa-memory@<user>`). `None` for plain units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
}

impl ServiceSpec {
    /// Build a spec from a legacy `[install] services = [...]` unit name:
    /// system scope, enable + start, no template instance.
    fn from_legacy_unit(unit: String) -> Self {
        Self {
            unit,
            scope: ServiceScope::System,
            enable: true,
            start: true,
            instance: None,
        }
    }
}

/// Serde default for boolean fields whose absent value is `true`.
fn default_true() -> bool {
    true
}

/// Linux capability assignment for an installed file.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallCapabilitySpec {
    /// Path receiving capabilities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Capability names, e.g. `CAP_BPF`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub caps: Vec<String>,
    /// Best-effort flag. When true, a `setcap` failure (no xattr support,
    /// non-root, unsupported filesystem) degrades to a warning instead of
    /// aborting the install. Defaults to false — a strict capability whose
    /// failure must abort. Skipped on serialize when false so legacy
    /// manifests round-trip byte-stable.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

impl InstallCapabilitySpec {
    /// Human-readable capability assignment for plans and warnings.
    pub fn display(&self) -> String {
        match (self.path.as_deref(), self.caps.is_empty()) {
            (Some(path), false) => format!("{path}: {}", self.caps.join("+")),
            (Some(path), true) => path.to_string(),
            (None, false) => self.caps.join("+"),
            (None, true) => "<empty>".to_string(),
        }
    }
}

/// Lifecycle hook declaration from `[[component.hooks]]`.
///
/// Distinct from the executor input [`HookSpec`](crate::hooks::HookSpec):
/// this is the *contract-level declaration*. `script` is an unexpanded
/// layout-template path (e.g. `{datadir}/hooks/ws-ckpt/pre-uninstall.sh`),
/// not a resolved absolute path, and the owning component comes from the
/// surrounding manifest. The executor input is derived from this at install
/// time (placeholder expansion + path-safety validation).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestHookSpec {
    /// Lifecycle phase that triggers the script.
    pub phase: HookPhase,
    /// Layout-template path to the script, shipped in the package payload as
    /// an ordinary layout file.
    pub script: String,
    /// Maximum wall-clock seconds before the script is killed. Defaults to 30.
    #[serde(default = "default_hook_timeout")]
    pub timeout_secs: u32,
    /// Failure handling. When false (default), a failure warns and the verb
    /// continues; when true, it aborts and the installer rolls back. ws-ckpt's
    /// pre-uninstall recover stays false to match the RPM `recover || warn`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub strict: bool,
}

/// Serde default for [`ManifestHookSpec::timeout_secs`] — matches the
/// executor's alpha 30-second default.
fn default_hook_timeout() -> u32 {
    30
}

/// Optional feature declared by a component manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureSpec {
    /// Stable feature name.
    pub name: String,
    /// Human-readable summary.
    pub description: String,
    /// Whether this feature is enabled by default.
    pub default: bool,
}

/// Structured dependency lists, preserving the original `[dependencies]`
/// kind for each entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DependenciesSpec {
    /// Build-time dependencies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build: Vec<String>,
    /// Runtime dependencies checked by doctor/status flows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtime: Vec<String>,
    /// Component dependencies followed by planners.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<String>,
}

impl DependenciesSpec {
    /// True when every kind list is empty.
    pub fn is_empty(&self) -> bool {
        self.build.is_empty() && self.runtime.is_empty() && self.components.is_empty()
    }
}

/// One `[[adapters]]` entry. We keep every field the loader can parse so
/// later tooling (adapter registry, doctor, build planner) does not have
/// to re-read the TOML. `detect` is captured as a free-form map because
/// each framework defines its own probe shape.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AdapterSpec {
    /// Adapter display or manifest name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Human-facing adapter label for CLI output and documentation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Framework this adapter targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework: Option<String>,
    /// Adapter kind (`first-party`, `third-party`, `protocol`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Adapter type within the framework (`plugin`, `extension`,
    /// `service`, `skill_bundle`, ...). The adapter manager gates on this
    /// value: only `"plugin"` (or absent/`None`, defaulting to plugin) is
    /// supported; all other values are rejected at enable time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_type: Option<String>,
    /// Trust level (`first-party`, `third-party`, `protocol`). Separate
    /// from `kind` so both dimensions remain usable independently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust: Option<String>,
    /// Framework-native plugin identifier, when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    /// Source path inside the component artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Destination path after layout placeholder expansion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest: Option<String>,
    /// Bundle description: how the driver should read the adapter's
    /// resource directory.
    #[serde(default, skip_serializing_if = "AdapterBundleSpec::is_empty")]
    pub bundle: AdapterBundleSpec,
    /// Compatibility metadata: driver schema and framework version
    /// constraints.
    #[serde(default, skip_serializing_if = "AdapterCompatSpec::is_empty")]
    pub compat: AdapterCompatSpec,
    /// Framework-specific detection hints preserved as TOML values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub detect: BTreeMap<String, toml::Value>,
    /// Skills this adapter delivers into the framework.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "deserialize_skills",
        serialize_with = "serialize_skills"
    )]
    pub skills: Vec<AdapterSkillSpec>,
    /// Post-install config key/value pairs the driver should apply.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config: Vec<AdapterConfigSetSpec>,
    /// Semver constraint on the target framework version, e.g. `">=1.2"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework_version_req: Option<String>,
    /// OpenClaw-specific adapter configuration, when this adapter targets
    /// the `openclaw` framework.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openclaw: Option<OpenClawAdapterSpec>,
    /// Hermes-specific adapter configuration, when this adapter targets
    /// the `hermes` framework.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hermes: Option<HermesAdapterSpec>,
}

/// Bundle description for an adapter resource directory. The driver uses
/// `entry` as a hint to locate the framework-native manifest inside the
/// bundle; it is NOT an executable path.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AdapterBundleSpec {
    /// Bundle schema version, for future evolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<u32>,
    /// Entry-point file inside the bundle directory (e.g.
    /// `"plugin.json"`). The driver reads this file to understand the
    /// bundle; it is never executed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
}

impl AdapterBundleSpec {
    fn is_empty(&self) -> bool {
        self.schema.is_none() && self.entry.is_none()
    }
}

/// Compatibility metadata for an adapter entry. Lets packaging and the
/// driver gate on schema evolution and framework version constraints.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AdapterCompatSpec {
    /// Driver-side schema version this adapter targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver_schema: Option<u32>,
    /// Semver constraint on the target framework, e.g. `">=0.1.0"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework_version: Option<String>,
}

impl AdapterCompatSpec {
    fn is_empty(&self) -> bool {
        self.driver_schema.is_none() && self.framework_version.is_none()
    }
}

/// A post-install config key/value pair that the driver should apply to
/// the framework's configuration. The driver interprets `key` as a
/// framework-specific config path; `value` is the TOML value to set.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdapterConfigSetSpec {
    /// Framework-specific config key path.
    pub key: String,
    /// Value to set.
    pub value: toml::Value,
}

/// One skill entry in an `[[adapters]]` or framework-specific section.
///
/// Two TOML forms are accepted (via `deserialize_skills`):
///
/// * **String array** (legacy): `skills = ["code-scanner", "prompt-scanner"]`
///   — normalised to `AdapterSkillSpec { name, source: None }`.
/// * **Table array** (recommended):
///   ```toml
///   [[adapters.openclaw.skills]]
///   name   = "code-scanner"
///   source = "{datadir}/skills/code-scanner/"
///   ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdapterSkillSpec {
    /// Skill name.
    pub name: String,
    /// Source directory for the skill bundle. When absent, the driver
    /// falls back to `<resource_root>/skills/<name>/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Deserialise `skills` from either a string array or a table array.
fn deserialize_skills<'de, D>(deserializer: D) -> Result<Vec<AdapterSkillSpec>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Item {
        Simple(String),
        Full(AdapterSkillSpec),
    }

    let items: Vec<Item> = Vec::deserialize(deserializer)?;
    Ok(items
        .into_iter()
        .map(|item| match item {
            Item::Simple(name) => AdapterSkillSpec { name, source: None },
            Item::Full(spec) => spec,
        })
        .collect())
}

/// Serialise `skills` as a string array when all sources are `None`,
/// preserving backward-compatible TOML output.
fn serialize_skills<S>(skills: &[AdapterSkillSpec], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    if skills.iter().all(|s| s.source.is_none()) {
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        names.serialize(serializer)
    } else {
        skills.serialize(serializer)
    }
}

/// OpenClaw-specific adapter configuration. When present on an
/// `[[adapters]]` entry whose `framework = "openclaw"`, the driver uses
/// these fields instead of the generic adapter-level ones.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OpenClawAdapterSpec {
    /// OpenClaw-specific bundle description (overrides generic
    /// `[adapters.bundle]` when present).
    #[serde(default, skip_serializing_if = "AdapterBundleSpec::is_empty")]
    pub bundle: AdapterBundleSpec,
    /// Skills delivered into OpenClaw's skill directory.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "deserialize_skills",
        serialize_with = "serialize_skills"
    )]
    pub skills: Vec<AdapterSkillSpec>,
    /// Post-install config key/value pairs for OpenClaw.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config: Vec<AdapterConfigSetSpec>,
}

impl OpenClawAdapterSpec {
    /// Whether no framework-specific data is present.
    pub fn is_empty(&self) -> bool {
        self.bundle.is_empty() && self.skills.is_empty() && self.config.is_empty()
    }
}

/// Hermes-specific adapter configuration. When present on an
/// `[[adapters]]` entry whose `framework = "hermes"`, the driver uses
/// these fields instead of the generic adapter-level ones.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HermesAdapterSpec {
    /// Hermes-specific bundle description.
    #[serde(default, skip_serializing_if = "AdapterBundleSpec::is_empty")]
    pub bundle: AdapterBundleSpec,
    /// Skills delivered into Hermes's skill directory.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "deserialize_skills",
        serialize_with = "serialize_skills"
    )]
    pub skills: Vec<AdapterSkillSpec>,
}

impl HermesAdapterSpec {
    /// Whether no framework-specific data is present.
    pub fn is_empty(&self) -> bool {
        self.bundle.is_empty() && self.skills.is_empty()
    }
}

/// One `[[health_checks]]` entry. Multiple checks per component are
/// expected (binary probe + systemd unit + http endpoint, etc.) so we
/// keep the entire list rather than only the first.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HealthSpec {
    /// Optional stable health-check name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Health-check kind (`file`, `command`, `systemd`, ...).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// Command line for command-style checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Probe path for binary/file checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<String>,
    /// Service unit for service-manager checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Optional checks degrade instead of failing when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optional: Option<bool>,
}

#[derive(Deserialize)]
struct ComponentManifestRaw {
    #[serde(default = "current_schema_version", alias = "manifest_version")]
    schema_version: u32,
    component: ComponentMetaRaw,
    #[serde(default)]
    source: Option<SourceRaw>,
    #[serde(default)]
    distribution: Option<DistributionRaw>,
    #[serde(default)]
    build: Option<BuildRaw>,
    #[serde(default)]
    install: Option<InstallRaw>,
    // `[backends]` deserializes directly into the typed shape: the structs are
    // simple and tolerant (`default` table, optional `rpm` sub-table), so no
    // separate Raw mirror is warranted.
    #[serde(default)]
    backends: ManifestBackends,
    #[serde(default, alias = "env_requirements")]
    environment: EnvRequirementsRaw,
    #[serde(default)]
    dependencies: DependenciesRaw,
    #[serde(default)]
    features: Vec<FeatureRaw>,
    #[serde(default)]
    adapters: Vec<AdapterRaw>,
    #[serde(default, alias = "health")]
    health_checks: Vec<HealthCheckRaw>,
}

#[derive(Deserialize)]
struct ComponentMetaRaw {
    name: String,
    version: String,
    #[serde(default = "default_runtime_layer")]
    layer: String,
    #[serde(default)]
    domain: Option<String>,
    // Minimal-schema additions on the `[component]` table.
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    repository: Option<String>,
    // Minimal-schema nested sub-tables (`[component.contract]`, etc.). When
    // present they take precedence over the legacy top-level sections.
    #[serde(default)]
    contract: Option<ContractRaw>,
    #[serde(default)]
    platform: Option<EnvRequirementsRaw>,
    #[serde(default)]
    artifact: Option<ArtifactRaw>,
    #[serde(default)]
    layout: Option<LayoutRaw>,
    // `[[component.services]]` — systemd unit lifecycle declarations, a
    // sibling of `[component.layout]`. ServiceSpec is simple and tolerant
    // (every field but `unit` defaults), so it deserializes directly without
    // a separate Raw mirror — same rationale as `[backends]`.
    #[serde(default)]
    services: Vec<ServiceSpec>,
    // `[[component.capabilities]]` — install-related setcap declarations,
    // a sibling of `[component.layout]` in the minimal schema (the legacy
    // schema declares them under `[install.capabilities]` instead).
    #[serde(default)]
    capabilities: Vec<InstallCapabilityRaw>,
    // `[[component.hooks]]` — lifecycle hook declarations, minimal-schema only
    // (legacy `[install]` predates hooks). ManifestHookSpec deserializes
    // directly: phase + script are required, the rest defaults.
    #[serde(default)]
    hooks: Vec<ManifestHookSpec>,
    // `[component.health_check]` — minimal-schema structured check. Parses
    // directly into the internally-tagged `CheckSpec` (`type = "..."`).
    #[serde(default)]
    health_check: Option<CheckSpec>,
}

#[derive(Deserialize, Default)]
struct ContractRaw {
    #[serde(default)]
    schema_version: Option<String>,
    #[serde(default)]
    min_anolisa_version: Option<String>,
}

#[derive(Deserialize, Default)]
struct ArtifactRaw {
    #[serde(default, rename = "type")]
    artifact_type: Option<String>,
    #[serde(default)]
    archive_format: Option<String>,
    #[serde(default)]
    naming_pattern: Option<String>,
}

/// `[component.layout]` — minimal-schema install layout. Maps onto the same
/// internal [`InstallSpec`] as the legacy `[install]` section.
#[derive(Deserialize, Default)]
struct LayoutRaw {
    #[serde(default)]
    modes: Vec<String>,
    #[serde(default)]
    files: Vec<LayoutFileRaw>,
}

#[derive(Deserialize, Default)]
struct LayoutFileRaw {
    #[serde(default)]
    source: Option<String>,
    /// Minimal schema uses `target`; `dest` tolerated for symmetry.
    #[serde(default, alias = "dest")]
    target: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    /// Minimal-schema `type` → [`FileKind`]. Absent defaults to `Data`.
    #[serde(default, rename = "type")]
    kind: FileKind,
}

#[derive(Deserialize, Default)]
struct SourceRaw {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    upstream: Option<String>,
}

#[derive(Deserialize, Default)]
struct DistributionRaw {
    #[serde(default)]
    selectors: Vec<DistributionSelectorRaw>,
}

#[derive(Deserialize, Default)]
struct DistributionSelectorRaw {
    #[serde(default)]
    install_mode: Option<String>,
    #[serde(default)]
    os: Vec<String>,
    #[serde(default)]
    arch: Vec<String>,
    #[serde(default)]
    libc: Option<String>,
    #[serde(default)]
    pkg_base: Option<String>,
    #[serde(default)]
    preferred_artifact_types: Vec<ArtifactType>,
}

impl From<DistributionSelectorRaw> for DistributionSelector {
    fn from(raw: DistributionSelectorRaw) -> Self {
        Self {
            install_mode: raw.install_mode,
            os: raw.os,
            arch: raw.arch,
            libc: raw.libc,
            pkg_base: raw.pkg_base,
            preferred_artifact_types: raw.preferred_artifact_types,
        }
    }
}

#[derive(Deserialize, Default)]
struct BuildRaw {
    #[serde(default, alias = "backend")]
    system: Option<String>,
    #[serde(default, alias = "outputs")]
    targets: Vec<String>,
    #[serde(default)]
    outputs_named: Vec<BuildOutputRaw>,
}

#[derive(Deserialize)]
struct BuildOutputRaw {
    name: String,
}

#[derive(Deserialize, Default)]
struct InstallRaw {
    #[serde(default)]
    modes: Vec<String>,
    #[serde(default)]
    files: Vec<InstallFileRaw>,
    #[serde(default)]
    services: Vec<String>,
    #[serde(default)]
    capabilities: Vec<InstallCapabilityRaw>,
}

#[derive(Deserialize, Default)]
struct InstallFileRaw {
    #[serde(default)]
    dest: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    /// Optional `type` so legacy `[install]` manifests can declare
    /// symlinks; absent defaults to `Data`, keeping old files byte-stable.
    #[serde(default, rename = "type")]
    kind: FileKind,
}

#[derive(Deserialize, Default)]
struct InstallCapabilityRaw {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    caps: Vec<String>,
    #[serde(default)]
    optional: bool,
}

#[derive(Deserialize, Default)]
struct DependenciesRaw {
    #[serde(default)]
    build: Vec<String>,
    #[serde(default)]
    runtime: Vec<String>,
    #[serde(default)]
    components: Vec<String>,
}

#[derive(Deserialize)]
struct FeatureRaw {
    name: String,
    #[serde(default, alias = "label")]
    description: String,
    #[serde(default)]
    default: bool,
}

#[derive(Deserialize, Default)]
struct AdapterRaw {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    framework: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    adapter_type: Option<String>,
    #[serde(default)]
    trust: Option<String>,
    #[serde(default)]
    plugin_id: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    dest: Option<String>,
    #[serde(default)]
    bundle: AdapterBundleRaw,
    #[serde(default)]
    compat: AdapterCompatRaw,
    #[serde(default)]
    detect: BTreeMap<String, toml::Value>,
    #[serde(default, deserialize_with = "deserialize_skills")]
    skills: Vec<AdapterSkillSpec>,
    #[serde(default)]
    config: Vec<AdapterConfigSetSpec>,
    #[serde(default)]
    framework_version_req: Option<String>,
    #[serde(default)]
    openclaw: Option<OpenClawAdapterSpec>,
    #[serde(default)]
    hermes: Option<HermesAdapterSpec>,
}

#[derive(Deserialize, Default)]
struct AdapterBundleRaw {
    #[serde(default)]
    schema: Option<u32>,
    #[serde(default)]
    entry: Option<String>,
}

#[derive(Deserialize, Default)]
struct AdapterCompatRaw {
    #[serde(default)]
    driver_schema: Option<u32>,
    #[serde(default)]
    framework_version: Option<String>,
}

#[derive(Deserialize, Default)]
struct HealthCheckRaw {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    probe: Option<String>,
    #[serde(default)]
    unit: Option<String>,
    #[serde(default)]
    optional: Option<bool>,
}

impl From<ComponentManifestRaw> for ComponentManifest {
    fn from(raw: ComponentManifestRaw) -> Self {
        // Destructure the `[component]` table once so the nested minimal-schema
        // sub-tables (contract/platform/artifact/layout) can be consumed
        // alongside the identity fields without partial-move friction.
        let ComponentMetaRaw {
            name,
            version,
            layer,
            domain,
            display_name,
            owner,
            license,
            repository,
            contract: contract_raw,
            platform: platform_raw,
            artifact: artifact_raw,
            layout: layout_raw,
            services: component_services,
            capabilities: component_capabilities,
            hooks: component_hooks,
            health_check,
        } = raw.component;

        let component = ComponentMeta {
            name,
            version,
            layer,
            domain,
            display_name,
            owner,
            license,
            repository,
        };

        let contract = contract_raw
            .map(|c| ContractSpec {
                schema_version: c.schema_version,
                min_anolisa_version: c.min_anolisa_version,
            })
            .unwrap_or_default();

        let artifact = artifact_raw
            .map(|a| ArtifactSpec {
                artifact_type: a.artifact_type,
                archive_format: a.archive_format,
                naming_pattern: a.naming_pattern,
            })
            .unwrap_or_default();

        let source = raw.source.map(source_from_raw).unwrap_or_default();

        let distribution_selectors = raw
            .distribution
            .map(|d| {
                d.selectors
                    .into_iter()
                    .map(DistributionSelector::from)
                    .collect()
            })
            .unwrap_or_default();

        let build = raw
            .build
            .map(|b| {
                let mut outputs = b.targets;
                outputs.extend(b.outputs_named.into_iter().map(|o| o.name));
                BuildSpec {
                    backend: b.system.unwrap_or_default(),
                    outputs,
                }
            })
            .unwrap_or_default();

        // Prefer the minimal-schema `[component.layout]`; fall back to the
        // legacy top-level `[install]` for not-yet-migrated manifests. The
        // minimal `target` key maps onto the internal `dest`. Services,
        // capabilities, and hooks are `[component.*]` siblings of
        // `[component.layout]`, parsed only on this minimal path; legacy
        // `[install]` carries services/capabilities inline and predates hooks.
        // Legacy files may carry `type` (e.g. symlinks); absent defaults `Data`.
        let install = if let Some(layout) = layout_raw {
            let files = layout
                .files
                .into_iter()
                .map(|f| InstallFileSpec {
                    source: f.source,
                    dest: f.target,
                    mode: f.mode,
                    kind: f.kind,
                })
                .filter(|f| f.install_path().is_some())
                .collect();
            let capabilities = capabilities_from_raw(component_capabilities);
            let services = component_services
                .into_iter()
                .filter(|s| !s.unit.is_empty())
                .collect();
            let hooks = component_hooks
                .into_iter()
                .filter(|h| !h.script.is_empty())
                .collect();
            InstallSpec {
                modes: layout.modes,
                files,
                services,
                capabilities,
                hooks,
            }
        } else {
            raw.install
                .map(|i| {
                    let files = i
                        .files
                        .into_iter()
                        .map(|f| InstallFileSpec {
                            source: f.source,
                            dest: f.dest,
                            mode: f.mode,
                            kind: f.kind,
                        })
                        .filter(|f| f.install_path().is_some())
                        .collect();
                    let capabilities = capabilities_from_raw(i.capabilities);
                    let services = i
                        .services
                        .into_iter()
                        .filter(|u| !u.is_empty())
                        .map(ServiceSpec::from_legacy_unit)
                        .collect();
                    InstallSpec {
                        modes: i.modes,
                        files,
                        services,
                        capabilities,
                        // Hooks are minimal-schema only; legacy `[install]`
                        // predates them.
                        hooks: Vec::new(),
                    }
                })
                .unwrap_or_default()
        };

        let dependencies = DependenciesSpec {
            build: raw.dependencies.build,
            runtime: raw.dependencies.runtime,
            components: raw.dependencies.components,
        };

        let features = raw
            .features
            .into_iter()
            .map(|f| FeatureSpec {
                name: f.name,
                description: f.description,
                default: f.default,
            })
            .collect();

        let adapters: Vec<AdapterSpec> = raw
            .adapters
            .into_iter()
            .map(|a| AdapterSpec {
                name: a.name,
                display_name: a.display_name,
                framework: a.framework,
                kind: a.kind,
                adapter_type: a.adapter_type,
                trust: a.trust,
                plugin_id: a.plugin_id,
                source: a.source,
                dest: a.dest,
                bundle: AdapterBundleSpec {
                    schema: a.bundle.schema,
                    entry: a.bundle.entry,
                },
                compat: AdapterCompatSpec {
                    driver_schema: a.compat.driver_schema,
                    framework_version: a.compat.framework_version,
                },
                detect: a.detect,
                skills: a.skills,
                config: a.config,
                framework_version_req: a.framework_version_req,
                openclaw: a.openclaw,
                hermes: a.hermes,
            })
            .collect();

        let health_checks: Vec<HealthSpec> = raw
            .health_checks
            .into_iter()
            .map(|h| HealthSpec {
                name: h.name,
                kind: h.kind.unwrap_or_default(),
                command: h.command,
                probe: h.probe,
                unit: h.unit,
                optional: h.optional,
            })
            .collect();

        // Prefer the minimal-schema `[component.platform]`; fall back to the
        // legacy `[environment]` / `requires_env`.
        let env_requirements = platform_raw
            .map(EnvRequirements::from)
            .unwrap_or_else(|| raw.environment.into());

        Self {
            schema_version: raw.schema_version,
            component,
            contract,
            artifact,
            source,
            distribution_selectors,
            build,
            install,
            backends: raw.backends,
            env_requirements,
            dependencies,
            features,
            adapters,
            health_check,
            health_checks,
        }
    }
}

/// Map raw capability entries to specs, dropping fully-empty rows. Shared by
/// the minimal-schema (`[[component.capabilities]]`) and legacy
/// (`[install.capabilities]`) parse paths so the two cannot drift.
fn capabilities_from_raw(raw: Vec<InstallCapabilityRaw>) -> Vec<InstallCapabilitySpec> {
    raw.into_iter()
        .map(|c| InstallCapabilitySpec {
            path: c.path,
            caps: c.caps,
            optional: c.optional,
        })
        .filter(|c| c.path.is_some() || !c.caps.is_empty())
        .collect()
}

fn source_from_raw(raw: SourceRaw) -> SourceSpec {
    let kind = raw
        .kind
        .or(raw.upstream)
        .unwrap_or_else(|| "workspace".to_string());
    SourceSpec {
        kind,
        path: raw.path,
        url: raw.url,
    }
}

// ---------------------------------------------------------------------------
// EnvRequirements
// ---------------------------------------------------------------------------

/// Host requirements normalized from the component TOML styles.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(from = "EnvRequirementsRaw")]
pub struct EnvRequirements {
    /// Accepted OS names.
    pub os: Vec<String>,
    /// Accepted CPU architectures.
    pub arch: Vec<String>,
    /// Accepted libc families.
    pub libc: Vec<String>,
    /// Minimum kernel version.
    pub kernel_min: Option<String>,
    /// Whether BTF debug data must be available.
    pub btf: Option<bool>,
    /// Whether the process/host must expose CAP_BPF.
    pub cap_bpf: Option<bool>,
    /// Accepted package bases.
    pub pkg_base: Vec<String>,
}

#[derive(Deserialize, Default)]
struct EnvRequirementsRaw {
    // Bare keys, also accepted via `[component.platform]`.
    #[serde(default)]
    os: Option<StringOrList>,
    #[serde(default)]
    arch: Option<StringOrList>,
    #[serde(default)]
    libc: Option<StringOrList>,
    #[serde(default, alias = "min_kernel")]
    kernel: Option<String>,
    #[serde(default)]
    pkg_base: Option<StringOrList>,

    // Component-style keys.
    #[serde(default)]
    requires_os: Option<StringOrList>,
    #[serde(default)]
    requires_arch: Option<StringOrList>,
    #[serde(default)]
    requires_libc: Option<StringOrList>,
    #[serde(default)]
    requires_kernel: Option<String>,
    #[serde(default)]
    requires_pkg_base: Option<StringOrList>,

    // Either prefix accepts the free-form map.
    #[serde(default)]
    requires_env: BTreeMap<String, toml::Value>,

    #[serde(default)]
    btf: Option<bool>,
    #[serde(default)]
    cap_bpf: Option<bool>,
}

impl From<EnvRequirementsRaw> for EnvRequirements {
    fn from(r: EnvRequirementsRaw) -> Self {
        let merge = |a: Option<StringOrList>, b: Option<StringOrList>| -> Vec<String> {
            a.or(b).map(|v| v.into_vec()).unwrap_or_default()
        };
        let btf = r
            .btf
            .or_else(|| lookup_bool(&r.requires_env, "btf_available"))
            .or_else(|| lookup_bool(&r.requires_env, "btf"));
        let cap_bpf = r
            .cap_bpf
            .or_else(|| lookup_cap_bpf(r.requires_env.get("linux_capabilities")))
            .or_else(|| lookup_cap_bpf(r.requires_env.get("capability")));
        Self {
            os: merge(r.os, r.requires_os),
            arch: merge(r.arch, r.requires_arch),
            libc: merge(r.libc, r.requires_libc),
            kernel_min: r.kernel.or(r.requires_kernel),
            btf,
            cap_bpf,
            pkg_base: merge(r.pkg_base, r.requires_pkg_base),
        }
    }
}

fn lookup_bool(map: &BTreeMap<String, toml::Value>, key: &str) -> Option<bool> {
    match map.get(key)? {
        toml::Value::Boolean(b) => Some(*b),
        toml::Value::String(s) => match s.as_str() {
            "true" | "yes" | "1" => Some(true),
            "false" | "no" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn lookup_cap_bpf(value: Option<&toml::Value>) -> Option<bool> {
    let v = value?;
    match v {
        toml::Value::String(s) => Some(s.eq_ignore_ascii_case("CAP_BPF")),
        toml::Value::Array(items) => Some(items.iter().any(|item| match item {
            toml::Value::String(s) => s.eq_ignore_ascii_case("CAP_BPF"),
            _ => false,
        })),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum StringOrList {
    One(String),
    Many(Vec<String>),
}

impl StringOrList {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(s) => vec![s],
            Self::Many(v) => v,
        }
    }
}

impl<'de> Deserialize<'de> for StringOrList {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Helper {
            One(String),
            Many(Vec<String>),
        }
        Ok(match Helper::deserialize(deserializer)? {
            Helper::One(s) => Self::One(s),
            Helper::Many(v) => Self::Many(v),
        })
    }
}

fn current_schema_version() -> u32 {
    CURRENT_SCHEMA_VERSION
}
fn default_runtime_layer() -> String {
    "runtime".to_string()
}

// ---------------------------------------------------------------------------
// File-loading entry points
// ---------------------------------------------------------------------------

impl ComponentManifest {
    /// Load a component manifest from TOML on disk.
    pub fn from_file(path: &Path) -> Result<Self, ManifestError> {
        let content = read_to_string(path)?;
        toml::from_str(&content)
            .map_err(|e| ManifestError::Parse(path.display().to_string(), e.to_string()))
    }

    /// Parse a component manifest from TOML text.
    pub fn from_toml_str(s: &str) -> Result<Self, ManifestError> {
        toml::from_str(s).map_err(|e| ManifestError::Parse("<string>".into(), e.to_string()))
    }

    /// Declared RPM package name from `[backends.rpm].package`, if any.
    ///
    /// This is the highest-precedence package-name source after a CLI
    /// `--package` override during RPM adopt; `None` falls through to the
    /// repo.toml `package_map` / provides / default-naming tiers.
    pub fn rpm_package(&self) -> Option<&str> {
        self.backends.rpm.as_ref().map(|s| s.package.as_str())
    }

    /// Health check to run after install: the declared
    /// `[component.health_check]`, or a synthesized `binary_version` over the
    /// first [`FileKind::Executable`] layout file. Returns `None` when neither
    /// is available (no declared check and no executable to probe).
    ///
    /// The synthesized probe targets the file's install destination
    /// (post-placeholder template), so the engine expands `{bindir}` against
    /// the active layout — matching how the file itself was installed.
    pub fn health_spec(&self) -> Option<CheckSpec> {
        if let Some(spec) = &self.health_check {
            return Some(spec.clone());
        }
        self.install
            .files
            .iter()
            .find(|f| f.kind == FileKind::Executable)
            .and_then(|f| f.install_path())
            .map(|target| CheckSpec::BinaryVersion {
                binary: target.to_string(),
                expect_pattern: None,
                timeout_secs: None,
            })
    }
}

fn read_to_string(path: &Path) -> Result<String, ManifestError> {
    std::fs::read_to_string(path).map_err(|e| ManifestError::Io(path.display().to_string(), e))
}

/// Helper used by [`Catalog`] when scanning layer directories.
pub(crate) fn manifest_paths(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if !dir.exists() {
        return files;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("toml") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Errors raised while loading manifests.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// Manifest file could not be read.
    #[error("cannot read manifest '{0}': {1}")]
    Io(String, std::io::Error),
    /// Manifest TOML could not be parsed.
    #[error("cannot parse manifest '{0}': {1}")]
    Parse(String, String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill_names(names: &[&str]) -> Vec<AdapterSkillSpec> {
        names
            .iter()
            .map(|n| AdapterSkillSpec {
                name: n.to_string(),
                source: None,
            })
            .collect()
    }

    #[test]
    fn component_manifest_parses_existing_fixture() {
        let toml_text = r#"
            [component]
            name = "agentsight"
            version = "0.2.0"
            layer = "runtime"
            domain = "observability"

            [build]
            system = "cargo"
            targets = ["agentsight"]

            [install]
            modes = ["system"]
            services = ["agentsight.service"]

            [[install.files]]
            source = "target/release/agentsight"
            dest = "{bindir}/agentsight"

            [environment]
            requires_os = "linux"
            requires_arch = ["x86_64"]
            requires_kernel = ">=5.8"

            [environment.requires_env]
            btf_available = "true"
            capability = "CAP_BPF"

            [dependencies]
            build = ["rust>=1.91"]
            runtime = ["kernel-headers"]

            [[features]]
            name = "token_counting"
            label = "LLM Token metering"
            default = true
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        assert_eq!(m.component.name, "agentsight");
        assert_eq!(m.component.domain.as_deref(), Some("observability"));
        assert_eq!(m.build.backend, "cargo");
        assert_eq!(m.install.modes, vec!["system"]);
        assert_eq!(m.install.files.len(), 1);
        assert_eq!(
            m.install.files[0].source.as_deref(),
            Some("target/release/agentsight")
        );
        assert_eq!(
            m.install.files[0].dest.as_deref(),
            Some("{bindir}/agentsight")
        );
        assert_eq!(m.env_requirements.kernel_min.as_deref(), Some(">=5.8"));
        assert_eq!(m.env_requirements.btf, Some(true));
        assert_eq!(m.env_requirements.cap_bpf, Some(true));
        assert_eq!(m.features.len(), 1);
        assert_eq!(m.features[0].description, "LLM Token metering");
        assert_eq!(m.dependencies.build, vec!["rust>=1.91"]);
        assert_eq!(m.dependencies.runtime, vec!["kernel-headers"]);
        assert!(m.dependencies.components.is_empty());
        // No `[backends]` table → empty, and the rpm-package accessor is None.
        assert!(m.backends.is_empty());
        assert_eq!(m.rpm_package(), None);
    }

    #[test]
    fn component_manifest_parses_rpm_backend_package() {
        let toml_text = r#"
            [component]
            name = "copilot-shell"
            version = "0.1.0"
            layer = "runtime"

            [backends.rpm]
            package = "anolisa-copilot-shell"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        assert!(!m.backends.is_empty());
        assert_eq!(m.rpm_package(), Some("anolisa-copilot-shell"));
    }

    #[test]
    fn component_manifest_rpm_backend_round_trips() {
        // `[backends]` must survive a serialize→deserialize cycle, and a
        // manifest without it must serialize *without* an empty table (the
        // `skip_serializing_if` contract that keeps raw-only manifests clean).
        let with_rpm = ComponentManifest::from_toml_str(
            r#"
            [component]
            name = "copilot-shell"
            version = "0.1.0"
            layer = "runtime"

            [backends.rpm]
            package = "anolisa-copilot-shell"
        "#,
        )
        .expect("parse");
        let dumped = toml::to_string(&with_rpm).expect("serialize");
        assert!(
            dumped.contains("anolisa-copilot-shell"),
            "rpm package must round-trip: {dumped}"
        );

        let without = ComponentManifest::from_toml_str(
            r#"
            [component]
            name = "agentsight"
            version = "0.2.0"
            layer = "runtime"
        "#,
        )
        .expect("parse");
        let dumped = toml::to_string(&without).expect("serialize");
        assert!(
            !dumped.contains("[backends]"),
            "empty backends must be skipped on serialize: {dumped}"
        );
    }

    #[test]
    fn component_manifest_parses_minimal_schema() {
        // Minimal schema (phase1-2-dev §2.1): namespaced [component.*] sections.
        let toml_text = r#"
            [component]
            name = "tokenless"
            version = "0.5.0"
            display_name = "Tokenless"
            owner = "tokenless-team"
            license = "MIT"
            repository = "https://github.com/alibaba/anolisa"

            [component.contract]
            schema_version = "1.0"
            min_anolisa_version = "0.2.0"

            [component.platform]
            os = ["linux"]
            arch = ["x86_64", "aarch64"]
            min_kernel = "5.4"

            [component.artifact]
            type = "archive"
            archive_format = "tar.gz"
            naming_pattern = "{name}-{version}-{os}-{arch}.tar.gz"

            [component.layout]
            modes = ["user", "system"]

            [[component.layout.files]]
            source = "bin/tokenless"
            target = "{bindir}/tokenless"
            mode = "0755"
            type = "executable"

            [component.health_check]
            type = "binary_version"
            binary = "{bindir}/tokenless"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse minimal");
        // Identity + new [component] metadata.
        assert_eq!(m.component.name, "tokenless");
        assert_eq!(m.component.display_name.as_deref(), Some("Tokenless"));
        assert_eq!(m.component.owner.as_deref(), Some("tokenless-team"));
        assert_eq!(m.component.license.as_deref(), Some("MIT"));
        // [component.contract] / [component.artifact].
        assert_eq!(m.contract.schema_version.as_deref(), Some("1.0"));
        assert_eq!(m.contract.min_anolisa_version.as_deref(), Some("0.2.0"));
        assert_eq!(m.artifact.artifact_type.as_deref(), Some("archive"));
        assert_eq!(m.artifact.archive_format.as_deref(), Some("tar.gz"));
        // [component.platform] → env_requirements (min_kernel → kernel_min).
        assert_eq!(m.env_requirements.os, vec!["linux"]);
        assert_eq!(m.env_requirements.arch, vec!["x86_64", "aarch64"]);
        assert_eq!(m.env_requirements.kernel_min.as_deref(), Some("5.4"));
        // [component.layout] → install (minimal `target` mapped to dest).
        assert_eq!(m.install.modes, vec!["user", "system"]);
        assert_eq!(m.install.files.len(), 1);
        assert_eq!(m.install.files[0].source.as_deref(), Some("bin/tokenless"));
        assert_eq!(
            m.install.files[0].dest.as_deref(),
            Some("{bindir}/tokenless")
        );
    }

    #[test]
    fn minimal_layout_takes_precedence_over_legacy_install() {
        // When both are present, [component.layout] wins (migration guard).
        let toml_text = r#"
            [component]
            name = "dual"
            version = "1.0.0"

            [component.layout]
            [[component.layout.files]]
            source = "bin/new"
            target = "{bindir}/new"

            [install]
            modes = ["system"]
            [[install.files]]
            source = "bin/old"
            dest = "{bindir}/old"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        assert_eq!(m.install.files.len(), 1);
        assert_eq!(m.install.files[0].source.as_deref(), Some("bin/new"));
    }

    #[test]
    fn install_files_preserve_source_dest_and_fallbacks() {
        let toml_text = r#"
            [component]
            name = "tool"
            version = "1.0.0"

            [install]
            modes = ["user"]

            [[install.files]]
            source = "target/release/tool"
            dest = "{bindir}/tool"

            [[install.files]]
            source = "{datadir}/source-only"

            [[install.files]]
            dest = "{etcdir}/dest-only"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");

        assert_eq!(m.install.files.len(), 3);
        assert_eq!(
            m.install.files[0].source.as_deref(),
            Some("target/release/tool")
        );
        assert_eq!(m.install.files[0].dest.as_deref(), Some("{bindir}/tool"));
        assert_eq!(m.install.files[0].install_path(), Some("{bindir}/tool"));
        assert_eq!(
            m.install.files[1].source.as_deref(),
            Some("{datadir}/source-only")
        );
        assert_eq!(m.install.files[1].dest, None);
        assert_eq!(
            m.install.files[1].install_path(),
            Some("{datadir}/source-only")
        );
        assert_eq!(m.install.files[2].source, None);
        assert_eq!(
            m.install.files[2].dest.as_deref(),
            Some("{etcdir}/dest-only")
        );
        assert_eq!(
            m.install.files[2].install_path(),
            Some("{etcdir}/dest-only")
        );
    }

    #[test]
    fn install_capabilities_preserve_path_and_caps() {
        let toml_text = r#"
            [component]
            name = "agentsight"
            version = "0.2.0"

            [install]
            modes = ["system"]

            [[install.capabilities]]
            path = "{bindir}/agentsight"
            caps = ["cap_bpf", "cap_perfmon"]
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");

        assert_eq!(m.install.capabilities.len(), 1);
        assert_eq!(
            m.install.capabilities[0].path.as_deref(),
            Some("{bindir}/agentsight")
        );
        assert_eq!(
            m.install.capabilities[0].caps,
            vec!["cap_bpf", "cap_perfmon"]
        );
    }

    #[test]
    fn install_capability_optional_parses_and_defaults_false() {
        // `optional = true` marks a setcap as best-effort (warn on failure
        // rather than aborting the install); absent, it defaults to a strict
        // capability whose failure must abort.
        let toml_text = r#"
            [component]
            name = "agentsight"
            version = "0.2.0"

            [install]
            modes = ["system"]

            [[install.capabilities]]
            path = "{bindir}/agentsight"
            caps = ["cap_bpf", "cap_perfmon"]
            optional = true

            [[install.capabilities]]
            path = "{bindir}/strict-tool"
            caps = ["cap_net_admin"]
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");

        assert_eq!(m.install.capabilities.len(), 2);
        assert!(m.install.capabilities[0].optional);
        assert!(!m.install.capabilities[1].optional);
    }

    #[test]
    fn minimal_schema_parses_component_capabilities() {
        // `[[component.capabilities]]` lives in the [component] namespace,
        // a sibling of [component.layout]; the layout branch must read it
        // (rather than dropping it on the floor as it did before).
        let toml_text = r#"
            [component]
            name = "agentsight"
            version = "0.2.0"

            [component.layout]
            modes = ["system"]

            [[component.layout.files]]
            source = "bin/agentsight"
            target = "{bindir}/agentsight"
            type = "executable"

            [[component.capabilities]]
            path = "{bindir}/agentsight"
            caps = ["cap_bpf", "cap_perfmon"]
            optional = true
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse minimal caps");

        assert_eq!(m.install.capabilities.len(), 1);
        assert_eq!(
            m.install.capabilities[0].path.as_deref(),
            Some("{bindir}/agentsight")
        );
        assert_eq!(
            m.install.capabilities[0].caps,
            vec!["cap_bpf", "cap_perfmon"]
        );
        assert!(m.install.capabilities[0].optional);
    }

    #[test]
    fn minimal_schema_parses_component_services_with_defaults() {
        // `[[component.services]]` parses into ServiceSpec. scope defaults to
        // system; enable/start default to true; instance is template-only.
        let toml_text = r#"
            [component]
            name = "agentsight"
            version = "0.2.0"

            [component.layout]
            modes = ["system"]

            [[component.layout.files]]
            source = "bin/agentsight"
            target = "{bindir}/agentsight"
            type = "executable"

            [[component.services]]
            unit = "agentsight.service"

            [[component.services]]
            unit = "anolisa-memory@.service"
            scope = "user"
            enable = false
            start = false
            instance = "%u"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse services");

        assert_eq!(m.install.services.len(), 2);
        // First service: every optional field defaulted.
        assert_eq!(m.install.services[0].unit, "agentsight.service");
        assert_eq!(m.install.services[0].scope, ServiceScope::System);
        assert!(m.install.services[0].enable);
        assert!(m.install.services[0].start);
        assert_eq!(m.install.services[0].instance, None);
        // Second service: explicit user-scope opt-in template unit.
        assert_eq!(m.install.services[1].unit, "anolisa-memory@.service");
        assert_eq!(m.install.services[1].scope, ServiceScope::User);
        assert!(!m.install.services[1].enable);
        assert!(!m.install.services[1].start);
        assert_eq!(m.install.services[1].instance.as_deref(), Some("%u"));
    }

    #[test]
    fn legacy_install_services_map_to_specs_with_defaults() {
        // Legacy `[install] services = [...]` is a bare unit-name list; each
        // name becomes a system-scoped ServiceSpec with enable/start true.
        let toml_text = r#"
            [component]
            name = "ws-ckpt"
            version = "0.1.0"

            [install]
            modes = ["system"]
            services = ["ws-ckpt.service"]
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse legacy services");

        assert_eq!(m.install.services.len(), 1);
        assert_eq!(m.install.services[0].unit, "ws-ckpt.service");
        assert_eq!(m.install.services[0].scope, ServiceScope::System);
        assert!(m.install.services[0].enable);
        assert!(m.install.services[0].start);
    }

    #[test]
    fn minimal_schema_parses_component_hooks_with_defaults() {
        // `[[component.hooks]]` declares lifecycle scripts. strict defaults to
        // false (warn-and-continue) and timeout_secs to 30; phase reuses the
        // executor's HookPhase vocabulary. `script` stays an unexpanded
        // layout-template path — expansion happens at install time.
        let toml_text = r#"
            [component]
            name = "ws-ckpt"
            version = "0.1.0"

            [component.layout]
            modes = ["system"]

            [[component.layout.files]]
            source = "bin/ws-ckpt"
            target = "{bindir}/ws-ckpt"
            type = "executable"

            [[component.hooks]]
            phase = "pre_uninstall"
            script = "{datadir}/hooks/ws-ckpt/pre-uninstall.sh"

            [[component.hooks]]
            phase = "post_uninstall"
            script = "{datadir}/hooks/ws-ckpt/post-uninstall.sh"
            timeout_secs = 120
            strict = true
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse hooks");

        assert_eq!(m.install.hooks.len(), 2);
        // First hook: defaulted timeout + non-strict.
        assert_eq!(m.install.hooks[0].phase, HookPhase::PreUninstall);
        assert_eq!(
            m.install.hooks[0].script,
            "{datadir}/hooks/ws-ckpt/pre-uninstall.sh"
        );
        assert_eq!(m.install.hooks[0].timeout_secs, 30);
        assert!(!m.install.hooks[0].strict);
        // Second hook: explicit timeout + strict abort.
        assert_eq!(m.install.hooks[1].phase, HookPhase::PostUninstall);
        assert_eq!(m.install.hooks[1].timeout_secs, 120);
        assert!(m.install.hooks[1].strict);
    }

    #[test]
    fn dependencies_preserve_build_runtime_components_kind() {
        let toml_text = r#"
            [component]
            name = "agentsight"
            version = "0.2.0"

            [dependencies]
            build = ["rust>=1.91", "clang>=14"]
            runtime = ["kernel-headers"]
            components = ["sec-core"]
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        assert_eq!(m.dependencies.build, vec!["rust>=1.91", "clang>=14"]);
        assert_eq!(m.dependencies.runtime, vec!["kernel-headers"]);
        assert_eq!(m.dependencies.components, vec!["sec-core"]);
    }

    #[test]
    fn adapters_preserve_framework_kind_plugin_source_dest_detect() {
        let toml_text = r#"
            [component]
            name = "agentsight"
            version = "0.2.0"

            [[adapters]]
            framework = "openclaw"
            kind = "third-party"
            plugin_id = "agentsight-openclaw"
            source = "adapters/agentsight/openclaw"
            dest = "{datadir}/adapters/{component}/openclaw/"

            [adapters.detect]
            config_path = "~/.openclaw/config.toml"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        assert_eq!(m.adapters.len(), 1);
        let a = &m.adapters[0];
        assert_eq!(a.framework.as_deref(), Some("openclaw"));
        assert_eq!(a.kind.as_deref(), Some("third-party"));
        assert_eq!(a.plugin_id.as_deref(), Some("agentsight-openclaw"));
        assert_eq!(a.source.as_deref(), Some("adapters/agentsight/openclaw"));
        assert_eq!(
            a.dest.as_deref(),
            Some("{datadir}/adapters/{component}/openclaw/")
        );
        assert_eq!(
            a.detect.get("config_path").and_then(|v| v.as_str()),
            Some("~/.openclaw/config.toml")
        );
    }

    #[test]
    fn layout_file_kind_parses_and_defaults_to_data() {
        let toml_text = r#"
            [component]
            name = "tokenless"
            version = "0.5.0"

            [component.layout]
            [[component.layout.files]]
            source = "bin/tokenless"
            target = "{bindir}/tokenless"
            type = "executable"

            [[component.layout.files]]
            source = "share/data.bin"
            target = "{sharedir}/tokenless/data.bin"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        assert_eq!(m.install.files.len(), 2);
        assert_eq!(m.install.files[0].kind, FileKind::Executable);
        // No `type` key → defaults to Data.
        assert_eq!(m.install.files[1].kind, FileKind::Data);
    }

    /// `type = "symlink"` parses in both schemas: minimal layout files and
    /// legacy `[[install.files]]` (catalog manifests).
    #[test]
    fn symlink_file_kind_parses_in_both_schemas() {
        let minimal = ComponentManifest::from_toml_str(
            r#"
            [component]
            name = "tokenless"
            version = "0.5.0"

            [component.layout]
            [[component.layout.files]]
            source = "{libexecdir}/tokenless/rtk"
            target = "{bindir}/rtk"
            type = "symlink"
        "#,
        )
        .expect("parse minimal");
        assert_eq!(minimal.install.files[0].kind, FileKind::Symlink);

        let legacy = ComponentManifest::from_toml_str(
            r#"
            [component]
            name = "tokenless"
            version = "0.5.0"

            [install]
            modes = ["user"]

            [[install.files]]
            source = "{libexecdir}/tokenless/rtk"
            dest = "{bindir}/rtk"
            type = "symlink"
        "#,
        )
        .expect("parse legacy");
        assert_eq!(legacy.install.files[0].kind, FileKind::Symlink);
    }

    #[test]
    fn layout_files_target_aliases_dest() {
        // `target` (minimal) and `dest` (legacy) land in the same internal
        // field, so both spellings install to the same place.
        let m = ComponentManifest::from_toml_str(
            r#"
            [component]
            name = "t"
            version = "1.0.0"
            [component.layout]
            [[component.layout.files]]
            source = "bin/t"
            dest = "{bindir}/t"
        "#,
        )
        .expect("parse");
        assert_eq!(m.install.files[0].dest.as_deref(), Some("{bindir}/t"));
    }

    #[test]
    fn health_spec_uses_declared_check_when_present() {
        let m = ComponentManifest::from_toml_str(
            r#"
            [component]
            name = "tokenless"
            version = "0.5.0"
            [component.health_check]
            type = "binary_version"
            binary = "{bindir}/tokenless"
        "#,
        )
        .expect("parse");
        match m.health_spec() {
            Some(CheckSpec::BinaryVersion { binary, .. }) => {
                assert_eq!(binary, "{bindir}/tokenless");
            }
            other => panic!("expected declared binary_version, got {other:?}"),
        }
    }

    #[test]
    fn health_spec_synthesizes_binary_version_from_first_executable() {
        let m = ComponentManifest::from_toml_str(
            r#"
            [component]
            name = "tokenless"
            version = "0.5.0"
            [component.layout]
            [[component.layout.files]]
            source = "share/x"
            target = "{sharedir}/x"
            type = "data"
            [[component.layout.files]]
            source = "bin/tokenless"
            target = "{bindir}/tokenless"
            type = "executable"
        "#,
        )
        .expect("parse");
        match m.health_spec() {
            Some(CheckSpec::BinaryVersion { binary, .. }) => {
                assert_eq!(binary, "{bindir}/tokenless", "first executable wins");
            }
            other => panic!("expected synthesized binary_version, got {other:?}"),
        }
    }

    #[test]
    fn health_spec_is_none_without_check_or_executable() {
        let m = ComponentManifest::from_toml_str(
            r#"
            [component]
            name = "t"
            version = "1.0.0"
            [component.layout]
            [[component.layout.files]]
            source = "share/x"
            target = "{sharedir}/x"
            type = "data"
        "#,
        )
        .expect("parse");
        assert!(m.health_spec().is_none());
    }

    #[test]
    fn multiple_health_checks_are_preserved_in_order() {
        let toml_text = r#"
            [component]
            name = "agentsight"
            version = "0.2.0"

            [[health_checks]]
            name = "binary"
            kind = "command"
            command = "{bindir}/agentsight --help"

            [[health_checks]]
            name = "service"
            kind = "systemd"
            unit = "agentsight.service"
            optional = true
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        assert_eq!(m.health_checks.len(), 2);
        assert_eq!(m.health_checks[0].name.as_deref(), Some("binary"));
        assert_eq!(m.health_checks[0].kind, "command");
        assert_eq!(
            m.health_checks[0].command.as_deref(),
            Some("{bindir}/agentsight --help")
        );
        assert_eq!(m.health_checks[1].name.as_deref(), Some("service"));
        assert_eq!(m.health_checks[1].kind, "systemd");
        assert_eq!(
            m.health_checks[1].unit.as_deref(),
            Some("agentsight.service")
        );
        assert_eq!(m.health_checks[1].optional, Some(true));
    }

    #[test]
    fn adapter_new_fields_parse_full_example() {
        let toml_text = r#"
            [component]
            name = "sec-core"
            version = "0.1.0"

            [[adapters]]
            framework = "openclaw"
            name = "sec-core-openclaw"
            display_name = "Sec Core for OpenClaw"
            adapter_type = "plugin"
            trust = "first-party"
            plugin_id = "sec-core"
            source = "adapters/openclaw"
            dest = "{datadir}/adapters/{component}/openclaw/"

            [adapters.bundle]
            schema = 1
            entry = "plugin.json"

            [adapters.compat]
            driver_schema = 1
            framework_version = ">=0.1.0"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        assert_eq!(m.adapters.len(), 1);
        let a = &m.adapters[0];
        assert_eq!(a.name.as_deref(), Some("sec-core-openclaw"));
        assert_eq!(a.display_name.as_deref(), Some("Sec Core for OpenClaw"));
        assert_eq!(a.framework.as_deref(), Some("openclaw"));
        assert_eq!(a.adapter_type.as_deref(), Some("plugin"));
        assert_eq!(a.trust.as_deref(), Some("first-party"));
        assert_eq!(a.plugin_id.as_deref(), Some("sec-core"));
        assert_eq!(a.source.as_deref(), Some("adapters/openclaw"));
        assert_eq!(
            a.dest.as_deref(),
            Some("{datadir}/adapters/{component}/openclaw/")
        );
        assert_eq!(a.bundle.schema, Some(1));
        assert_eq!(a.bundle.entry.as_deref(), Some("plugin.json"));
        assert_eq!(a.compat.driver_schema, Some(1));
        assert_eq!(a.compat.framework_version.as_deref(), Some(">=0.1.0"));
    }

    #[test]
    fn adapter_new_fields_round_trip() {
        let toml_text = r#"
            [component]
            name = "roundtrip"
            version = "1.0.0"

            [[adapters]]
            framework = "openclaw"
            name = "rt-adapter"
            display_name = "RT Adapter"
            adapter_type = "extension"
            trust = "third-party"
            plugin_id = "rt"
            source = "adapters/openclaw"
            dest = "{datadir}/adapters/{component}/openclaw/"

            [adapters.bundle]
            schema = 2
            entry = "manifest.json"

            [adapters.compat]
            driver_schema = 3
            framework_version = ">=1.0.0"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        let serialized = toml::to_string_pretty(&m).expect("serialize");
        let m2 = ComponentManifest::from_toml_str(&serialized).expect("re-parse");
        assert_eq!(
            m.adapters, m2.adapters,
            "round-trip must preserve all adapter fields"
        );
    }

    #[test]
    fn adapter_minimal_fields_still_parse() {
        let toml_text = r#"
            [component]
            name = "minimal"
            version = "1.0.0"

            [[adapters]]
            framework = "openclaw"
            source = "adapters/openclaw"
            dest = "{datadir}/adapters/{component}/openclaw/"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        assert_eq!(m.adapters.len(), 1);
        let a = &m.adapters[0];
        assert_eq!(a.framework.as_deref(), Some("openclaw"));
        assert!(a.display_name.is_none());
        assert!(a.adapter_type.is_none());
        assert!(a.trust.is_none());
        assert!(a.bundle.is_empty());
        assert!(a.compat.is_empty());
    }

    #[test]
    fn adapter_empty_adapters_array_still_parses() {
        let toml_text = r#"
            [component]
            name = "no-adapters"
            version = "1.0.0"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        assert!(m.adapters.is_empty());
    }

    #[test]
    fn adapter_new_fields_default_to_none_when_absent() {
        let toml_text = r#"
            [component]
            name = "agentsight"
            version = "0.2.0"

            [[adapters]]
            framework = "openclaw"
            kind = "third-party"
            plugin_id = "agentsight-openclaw"
            source = "adapters/agentsight/openclaw"
            dest = "{datadir}/adapters/{component}/openclaw/"

            [adapters.detect]
            config_path = "~/.openclaw/config.toml"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        let a = &m.adapters[0];
        assert!(a.display_name.is_none());
        assert!(a.adapter_type.is_none());
        assert!(a.trust.is_none());
        assert!(a.bundle.schema.is_none());
        assert!(a.bundle.entry.is_none());
        assert!(a.compat.driver_schema.is_none());
        assert!(a.compat.framework_version.is_none());
        // Existing fields preserved.
        assert_eq!(a.kind.as_deref(), Some("third-party"));
        assert_eq!(
            a.detect.get("config_path").and_then(|v| v.as_str()),
            Some("~/.openclaw/config.toml")
        );
    }

    #[test]
    fn adapter_bundle_and_compat_skip_serializing_when_empty() {
        let toml_text = r#"
            [component]
            name = "skiptest"
            version = "1.0.0"

            [[adapters]]
            framework = "cosh"
            source = "adapters/cosh"
            dest = "{datadir}/adapters/{component}/cosh/"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        let serialized = toml::to_string_pretty(&m).expect("serialize");
        assert!(
            !serialized.contains("[bundle]"),
            "empty bundle must be skipped in serialization"
        );
        assert!(
            !serialized.contains("[compat]"),
            "empty compat must be skipped in serialization"
        );
    }

    #[test]
    fn component_domain_distinguishes_unset_from_explicit_empty() {
        let unset = ComponentManifest::from_toml_str(
            r#"
            [component]
            name = "unset-domain"
            version = "1.0.0"
        "#,
        )
        .expect("parse unset");
        assert_eq!(unset.component.domain, None);

        let explicit_empty = ComponentManifest::from_toml_str(
            r#"
            [component]
            name = "empty-domain"
            version = "1.0.0"
            domain = ""
        "#,
        )
        .expect("parse explicit empty");
        assert_eq!(explicit_empty.component.domain.as_deref(), Some(""));
    }

    #[test]
    fn adapter_skills_and_config_parse() {
        let toml_text = r#"
            [component]
            name = "agent-sec-core"
            version = "2.1.0"

            [[adapters]]
            framework = "openclaw"
            adapter_type = "plugin"
            plugin_id = "agent-sec"
            skills = ["sec-audit", "cred-scan"]
            framework_version_req = ">=1.2"

            [adapters.bundle]
            entry = "openclaw.plugin.json"

            [[adapters.config]]
            key = "plugins.entries.agent-sec.hooks.allowConversationAccess"
            value = true
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        assert_eq!(m.adapters.len(), 1);
        let a = &m.adapters[0];
        assert_eq!(a.skills, skill_names(&["sec-audit", "cred-scan"]));
        assert_eq!(a.framework_version_req.as_deref(), Some(">=1.2"));
        assert_eq!(a.config.len(), 1);
        assert_eq!(
            a.config[0].key,
            "plugins.entries.agent-sec.hooks.allowConversationAccess"
        );
        assert_eq!(a.config[0].value, toml::Value::Boolean(true));
    }

    #[test]
    fn adapter_openclaw_specific_section_parses() {
        let toml_text = r#"
            [component]
            name = "sec-core"
            version = "0.1.0"

            [[adapters]]
            framework = "openclaw"
            plugin_id = "sec-core"

            [adapters.openclaw]
            skills = ["sec-audit"]

            [adapters.openclaw.bundle]
            entry = "openclaw.plugin.json"

            [[adapters.openclaw.config]]
            key = "plugins.entries.sec-core.enabled"
            value = true
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        let a = &m.adapters[0];
        let oc = a.openclaw.as_ref().expect("openclaw section");
        assert_eq!(oc.skills, skill_names(&["sec-audit"]));
        assert_eq!(oc.bundle.entry.as_deref(), Some("openclaw.plugin.json"));
        assert_eq!(oc.config.len(), 1);
        assert_eq!(oc.config[0].key, "plugins.entries.sec-core.enabled");
    }

    #[test]
    fn adapter_hermes_specific_section_parses() {
        let toml_text = r#"
            [component]
            name = "sec-core"
            version = "0.1.0"

            [[adapters]]
            framework = "hermes"
            skills = ["sec-audit"]

            [adapters.hermes]
            skills = ["sec-audit"]

            [adapters.hermes.bundle]
            entry = "hermes.manifest.yaml"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        let a = &m.adapters[0];
        let h = a.hermes.as_ref().expect("hermes section");
        assert_eq!(h.skills, skill_names(&["sec-audit"]));
        assert_eq!(h.bundle.entry.as_deref(), Some("hermes.manifest.yaml"));
    }

    #[test]
    fn adapter_skills_config_round_trip() {
        let toml_text = r#"
            [component]
            name = "roundtrip"
            version = "1.0.0"

            [[adapters]]
            framework = "openclaw"
            plugin_id = "rt"
            skills = ["skill-a", "skill-b"]
            framework_version_req = ">=1.0"

            [[adapters.config]]
            key = "some.key"
            value = "hello"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        let serialized = toml::to_string_pretty(&m).expect("serialize");
        let m2 = ComponentManifest::from_toml_str(&serialized).expect("re-parse");
        assert_eq!(m.adapters[0].skills, m2.adapters[0].skills);
        assert_eq!(m.adapters[0].config, m2.adapters[0].config);
        assert_eq!(
            m.adapters[0].framework_version_req,
            m2.adapters[0].framework_version_req
        );
    }

    #[test]
    fn adapter_empty_skills_config_skip_serializing() {
        let toml_text = r#"
            [component]
            name = "skiptest"
            version = "1.0.0"

            [[adapters]]
            framework = "openclaw"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        let serialized = toml::to_string_pretty(&m).expect("serialize");
        assert!(
            !serialized.contains("skills"),
            "empty skills must be skipped: {serialized}"
        );
        assert!(
            !serialized.contains("config"),
            "empty config must be skipped: {serialized}"
        );
        assert!(
            !serialized.contains("[openclaw]"),
            "absent openclaw section must be skipped: {serialized}"
        );
        assert!(
            !serialized.contains("[hermes]"),
            "absent hermes section must be skipped: {serialized}"
        );
    }

    #[test]
    fn component_md_example_parses() {
        let toml_text = r#"
            schema_version = 1

            [component]
            name         = "agent-sec-core"
            version      = "2.1.0"
            display_name = "Agent Security Core"
            owner        = "security-team"
            license      = "Apache-2.0"
            repository   = "https://github.com/example/agent-sec-core"
            layer        = "runtime"
            domain       = "security"

            [component.contract]
            schema_version      = "1"
            min_anolisa_version = "0.6.0"

            [[adapters]]
            name         = "agent-sec-openclaw"
            display_name = "Agent Sec (OpenClaw)"
            framework    = "openclaw"
            kind         = "first-party"
            adapter_type = "plugin"
            plugin_id    = "agent-sec"
            skills       = ["sec-audit", "cred-scan"]
            framework_version_req = ">=1.2"

            [adapters.bundle]
            entry = "openclaw.plugin.json"

            [[adapters.config]]
            key   = "plugins.entries.agent-sec.hooks.allowConversationAccess"
            value = true

            [[adapters]]
            name         = "agent-sec-hermes"
            display_name = "Agent Sec (hermes)"
            framework    = "hermes"
            kind         = "first-party"
            skills       = ["sec-audit"]
            framework_version_req = ">=0.4"

            [adapters.bundle]
            entry = "hermes.manifest.yaml"

            [[adapters.config]]
            key   = "security.conversation_access"
            value = true
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse component.md example");
        assert_eq!(m.adapters.len(), 2);

        let oc = &m.adapters[0];
        assert_eq!(oc.framework.as_deref(), Some("openclaw"));
        assert_eq!(oc.plugin_id.as_deref(), Some("agent-sec"));
        assert_eq!(oc.skills, skill_names(&["sec-audit", "cred-scan"]));
        assert_eq!(oc.bundle.entry.as_deref(), Some("openclaw.plugin.json"));
        assert_eq!(oc.config.len(), 1);

        let hm = &m.adapters[1];
        assert_eq!(hm.framework.as_deref(), Some("hermes"));
        assert_eq!(hm.skills, skill_names(&["sec-audit"]));
        assert_eq!(hm.bundle.entry.as_deref(), Some("hermes.manifest.yaml"));
        assert_eq!(hm.config.len(), 1);
    }

    #[test]
    fn existing_manifests_still_parse_after_schema_extension() {
        let toml_text = r#"
            [component]
            name = "agentsight"
            version = "0.2.0"
            layer = "runtime"

            [[adapters]]
            framework = "openclaw"
            kind = "third-party"
            plugin_id = "agentsight-openclaw"
            source = "adapters/agentsight/openclaw"
            dest = "{datadir}/adapters/{component}/openclaw/"

            [adapters.detect]
            config_path = "~/.openclaw/config.toml"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        let a = &m.adapters[0];
        assert!(a.skills.is_empty());
        assert!(a.config.is_empty());
        assert!(a.framework_version_req.is_none());
        assert!(a.openclaw.is_none());
        assert!(a.hermes.is_none());
        assert_eq!(a.plugin_id.as_deref(), Some("agentsight-openclaw"));
    }

    // -- AdapterSkillSpec dual-format ------------------------------------------

    #[test]
    fn adapter_skills_table_array_parses() {
        let toml_text = r#"
            [component]
            name = "sec-core"
            version = "0.1.0"
            layer = "runtime"

            [[adapters]]
            framework = "openclaw"

            [[adapters.openclaw.skills]]
            name = "code-scanner"
            source = "{datadir}/skills/code-scanner/"

            [[adapters.openclaw.skills]]
            name = "prompt-scanner"
            source = "{datadir}/skills/prompt-scanner/"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        let oc = m.adapters[0].openclaw.as_ref().expect("openclaw section");
        assert_eq!(oc.skills.len(), 2);
        assert_eq!(oc.skills[0].name, "code-scanner");
        assert_eq!(
            oc.skills[0].source.as_deref(),
            Some("{datadir}/skills/code-scanner/")
        );
        assert_eq!(oc.skills[1].name, "prompt-scanner");
    }

    #[test]
    fn adapter_skills_string_array_still_parses() {
        let toml_text = r#"
            [component]
            name = "sec-core"
            version = "0.1.0"
            layer = "runtime"

            [[adapters]]
            framework = "openclaw"

            [adapters.openclaw]
            skills = ["code-scanner", "prompt-scanner"]
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        let oc = m.adapters[0].openclaw.as_ref().expect("openclaw section");
        assert_eq!(oc.skills, skill_names(&["code-scanner", "prompt-scanner"]));
        assert!(
            oc.skills.iter().all(|s| s.source.is_none()),
            "string-form skills must have no source"
        );
    }

    #[test]
    fn adapter_skills_table_array_hermes_parses() {
        let toml_text = r#"
            [component]
            name = "sec-core"
            version = "0.1.0"
            layer = "runtime"

            [[adapters]]
            framework = "hermes"

            [[adapters.hermes.skills]]
            name = "code-scanner"
            source = "{datadir}/skills/code-scanner/"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        let h = m.adapters[0].hermes.as_ref().expect("hermes section");
        assert_eq!(h.skills.len(), 1);
        assert_eq!(h.skills[0].name, "code-scanner");
        assert_eq!(
            h.skills[0].source.as_deref(),
            Some("{datadir}/skills/code-scanner/")
        );
    }

    #[test]
    fn adapter_skills_generic_table_array_parses() {
        let toml_text = r#"
            [component]
            name = "sec-core"
            version = "0.1.0"
            layer = "runtime"

            [[adapters]]
            framework = "openclaw"

            [[adapters.skills]]
            name = "code-scanner"
            source = "{datadir}/skills/code-scanner/"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        let a = &m.adapters[0];
        assert_eq!(a.skills.len(), 1);
        assert_eq!(a.skills[0].name, "code-scanner");
        assert_eq!(
            a.skills[0].source.as_deref(),
            Some("{datadir}/skills/code-scanner/")
        );
    }

    #[test]
    fn adapter_skills_string_array_round_trips() {
        let toml_text = r#"
            [component]
            name = "roundtrip"
            version = "1.0.0"
            layer = "runtime"

            [[adapters]]
            framework = "openclaw"
            skills = ["a", "b"]
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        let serialized = toml::to_string_pretty(&m).expect("serialize");
        // The serializer uses a multi-line format; just check the values
        // appear and no `source` or `name` key is emitted (i.e. it
        // serialized as a string array, not a table array).
        assert!(
            !serialized.contains("source ="),
            "string-form skills must not produce 'source =' keys: {serialized}"
        );
        let m2 = ComponentManifest::from_toml_str(&serialized).expect("re-parse");
        assert_eq!(m.adapters[0].skills, m2.adapters[0].skills);
    }

    #[test]
    fn adapter_skills_table_array_serialises_with_source() {
        let toml_text = r#"
            [component]
            name = "sec-core"
            version = "0.1.0"
            layer = "runtime"

            [[adapters]]
            framework = "openclaw"

            [[adapters.skills]]
            name = "code-scanner"
            source = "{datadir}/skills/code-scanner/"
        "#;
        let m = ComponentManifest::from_toml_str(toml_text).expect("parse");
        let serialized = toml::to_string_pretty(&m).expect("serialize");
        assert!(
            serialized.contains("code-scanner"),
            "must contain skill name: {serialized}"
        );
        let m2 = ComponentManifest::from_toml_str(&serialized).expect("re-parse");
        assert_eq!(m.adapters[0].skills, m2.adapters[0].skills);
    }
}
