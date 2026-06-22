//! Adapter receipt schema (`AdapterClaim`) and its security-boundary
//! [`ClaimResource`] model.
//!
//! A receipt is **pure data**: it records what a framework driver took
//! over on behalf of one component, so [`status`](super::manager) and
//! [`disable`](super::manager) can run later without re-reading the
//! resource directory and without trusting any executable instruction
//! from disk. Receipts never carry argv, shell strings, script paths, or
//! reverse commands — the framework CLI invocation is constructed by the
//! built-in driver, not read back from the receipt.
//!
//! Every value that `status`/`disable` would interpret as a path, a
//! symlink, or a framework-registry entry must live in [`ClaimResource`],
//! the closed set the Manager re-validates before handing the claim to a
//! driver. The framework-specific [`DriverPayload`] may only hold typed
//! data the driver needs to *understand* the receipt; it is never a path
//! safety boundary and must reference paths by [`ClaimResource::id`]
//! rather than duplicating them.
//!
//! Wire format note: the enums here are **externally tagged** (serde
//! default, no `#[serde(flatten)]`). `toml` 0.8 mis-serializes
//! internally-tagged enums combined with `flatten`; externally-tagged
//! variants round-trip cleanly as long as scalar fields are declared
//! before nested tables/arrays. The round-trip is pinned by the
//! `adapter_claim_toml_round_trip` test.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::path_safety::{PathBoundaryError, canonicalize_nearest_existing, validate_owned_path};
use anolisa_platform::fs_layout::FsLayout;

/// Schema version for the generic claim shape and [`ClaimResource`].
/// Persisted in every receipt so a future on-disk migration can branch.
pub const CLAIM_SCHEMA_VERSION: u32 = 1;

/// Schema version for [`DriverPayload`]. Bumped independently of
/// [`CLAIM_SCHEMA_VERSION`] when a driver's typed payload changes shape.
pub const DRIVER_SCHEMA_VERSION: u32 = 1;

/// A single adapter receipt: "the current user's `component` has, through
/// `framework`'s driver, taken over the framework-side state described by
/// `resources`".
///
/// Persisted in the user-level `installed.toml` as `[[adapter_claims]]`,
/// alongside `[[objects]]`. Scalar fields are declared first so the TOML
/// serializer emits them before the `resources` array and the
/// `driver_payload` table (TOML requires scalars to precede sub-tables
/// within a table).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdapterClaim {
    /// Generic claim + [`ClaimResource`] schema version
    /// ([`CLAIM_SCHEMA_VERSION`] at write time).
    pub claim_schema: u32,
    /// ANOLISA component this receipt belongs to.
    pub component: String,
    /// Framework name; must resolve to a built-in driver.
    pub framework: String,
    /// Framework-native plugin id, when the framework has one. Sanitized
    /// before it ever enters an argv (see [`validate_plugin_id`]). The
    /// authoritative copy for CLI use lives in the
    /// [`ClaimResourceKind::FrameworkPlugin`] resource; this top-level
    /// field is a convenience for listing/scan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    /// RFC3339 UTC timestamp when enable last wrote this receipt.
    pub enabled_at: String,
    /// Resource directory read at enable time. Kept for status display and
    /// upgrade detection; `disable` must NOT depend on it still existing.
    pub resource_root: PathBuf,
    /// Digest of the resource tree at enable time, for drift/upgrade
    /// detection. Optional: a driver may decline to compute one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_digest: Option<String>,
    /// [`DriverPayload`] schema version ([`DRIVER_SCHEMA_VERSION`] at write
    /// time).
    pub driver_schema: u32,
    /// Lifecycle status of the receipt itself.
    pub status: ClaimStatus,
    /// Manager-validatable resource declarations — the receipt's security
    /// boundary. Re-validated before every `status`/`disable`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<ClaimResource>,
    /// Framework-specific typed payload. Closed enum, no free-form map.
    pub driver_payload: DriverPayload,
}

impl AdapterClaim {
    /// Find a resource by its stable `id`.
    pub fn resource(&self, id: &str) -> Option<&ClaimResource> {
        self.resources.iter().find(|r| r.id == id)
    }

    /// Re-validate every [`ClaimResource`] against the current layout and
    /// the driver's static external roots, plus any embedded `plugin_id`.
    ///
    /// The Manager calls this before writing a receipt, after reading one
    /// back, and before handing the claim to a driver's `status`/`disable`
    /// — so a forged `installed.toml` cannot widen ANOLISA's authority to
    /// an arbitrary path or smuggle a shell metacharacter into an argv.
    ///
    /// # Errors
    ///
    /// Returns the first [`ClaimValidationError`] encountered: an owned
    /// path outside ANOLISA roots, an external path outside every
    /// `allowed_external_roots` entry, a traversal/symlink escape, or an
    /// invalid plugin id.
    pub fn validate(
        &self,
        layout: &FsLayout,
        allowed_external_roots: &[PathBuf],
    ) -> Result<(), ClaimValidationError> {
        if let Some(pid) = &self.plugin_id {
            validate_plugin_id(pid)?;
        }
        for resource in &self.resources {
            resource.validate(layout, allowed_external_roots)?;
            match &resource.kind {
                ClaimResourceKind::FrameworkPlugin { framework, .. }
                | ClaimResourceKind::FrameworkConfig { framework, .. }
                    if framework != &self.framework =>
                {
                    return Err(ClaimValidationError::FrameworkMismatch {
                        id: resource.id.clone(),
                        resource_framework: framework.clone(),
                        claim_framework: self.framework.clone(),
                    });
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Lifecycle status of a receipt.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    /// Adapter is enabled and the receipt is authoritative.
    Enabled,
    /// A prior `disable` could not fully clean up; the receipt is kept so
    /// the cleanup can be retried.
    CleanupFailed,
}

/// One entry in a receipt's `resources` list — the unit the Manager
/// validates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaimResource {
    /// Stable id referenced from [`DriverPayload`] and condition reports.
    pub id: String,
    /// Human-facing role, e.g. `openclaw_state_dir`.
    pub purpose: String,
    /// The typed, validatable resource.
    pub kind: ClaimResourceKind,
}

impl ClaimResource {
    /// Validate this resource against ANOLISA-owned roots (for owned
    /// paths) or the driver's static external roots (for external paths),
    /// and sanitize any embedded plugin id.
    ///
    /// # Errors
    ///
    /// See [`AdapterClaim::validate`].
    pub fn validate(
        &self,
        layout: &FsLayout,
        allowed_external_roots: &[PathBuf],
    ) -> Result<(), ClaimValidationError> {
        match &self.kind {
            ClaimResourceKind::OwnedPath { path } => {
                validate_owned_path(layout, path).map_err(|source| {
                    ClaimValidationError::OwnedPath {
                        id: self.id.clone(),
                        source,
                    }
                })
            }
            ClaimResourceKind::ExternalPath { path } => {
                validate_external_path(path, allowed_external_roots).map_err(|source| {
                    ClaimValidationError::ExternalPath {
                        id: self.id.clone(),
                        source,
                    }
                })
            }
            ClaimResourceKind::FrameworkPlugin { plugin_id, .. } => validate_plugin_id(plugin_id),
            ClaimResourceKind::FrameworkConfig { key, .. } => {
                if key.is_empty() {
                    return Err(ClaimValidationError::ConfigKey {
                        id: self.id.clone(),
                        reason: "config key must not be empty".to_string(),
                    });
                }
                validate_config_key(key).map_err(|_| ClaimValidationError::ConfigKey {
                    id: self.id.clone(),
                    reason: format!("config key '{key}' contains unsafe characters"),
                })
            }
        }
    }
}

/// The closed set of resource kinds a receipt may declare.
///
/// MVP implements only the three kinds OpenClaw needs. Additional kinds
/// (`Tree`, `JsonKeys`, `Symlink`, `FrameworkMarketplace`) are introduced
/// when their first driver lands — adding a variant here is a deliberate,
/// reviewed extension of the security boundary, never an open map.
///
/// Externally tagged with snake_case variant keys (`owned_path`,
/// `external_path`, `framework_plugin`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaimResourceKind {
    /// A path inside an ANOLISA-owned root; validated by
    /// [`validate_owned_path`].
    OwnedPath {
        /// Absolute owned path.
        path: PathBuf,
    },
    /// A path in a framework/user directory. Validated against the
    /// driver's static `allowed_external_roots` only — the receipt does
    /// **not** get to declare its own allowed root (that would let a
    /// forged receipt authorize itself).
    ExternalPath {
        /// Absolute external path.
        path: PathBuf,
    },
    /// A record in a framework's plugin registry. `plugin_id` is
    /// whitelist-sanitized before it enters any argv.
    FrameworkPlugin {
        /// Framework that owns the registry (e.g. `openclaw`).
        framework: String,
        /// Native plugin id.
        plugin_id: String,
    },
    /// A framework configuration key/value pair that ANOLISA applied.
    /// The key path is framework-specific; the value is the TOML
    /// representation of what was set.
    FrameworkConfig {
        /// Framework that owns the config (e.g. `openclaw`).
        framework: String,
        /// Config key path.
        key: String,
    },
}

/// Framework-specific typed payload. Closed enum — there is no runtime
/// custom-type escape hatch. The variant key doubles as the
/// `driver_payload_kind` discriminator.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DriverPayload {
    /// OpenClaw driver payload.
    #[serde(rename = "openclaw")]
    OpenClaw(OpenClawClaim),
    /// Hermes driver payload.
    #[serde(rename = "hermes")]
    Hermes(HermesClaim),
}

/// OpenClaw driver payload. Holds only [`ClaimResource::id`] references —
/// never the paths themselves — so the validated `resources` list stays
/// the single source of truth for path data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenClawClaim {
    /// Resource id of the OpenClaw state/home directory
    /// ([`ClaimResourceKind::ExternalPath`]).
    pub state_dir_resource: String,
    /// Resource id of the registered plugin
    /// ([`ClaimResourceKind::FrameworkPlugin`]).
    pub plugin_resource: String,
    /// Resource ids of delivered skill directories.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skill_resources: Vec<String>,
    /// Resource ids of applied config key/value pairs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config_resources: Vec<String>,
}

/// Hermes driver payload. Holds only [`ClaimResource::id`] references.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HermesClaim {
    /// Resource id of the Hermes home directory
    /// ([`ClaimResourceKind::ExternalPath`]).
    pub home_resource: String,
    /// Resource id of the installed plugin directory
    /// ([`ClaimResourceKind::ExternalPath`]).
    pub plugin_resource: String,
    /// Resource ids of delivered skill directories.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skill_resources: Vec<String>,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Reasons a receipt's resources or plugin id fail validation.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ClaimValidationError {
    /// An [`ClaimResourceKind::OwnedPath`] is outside ANOLISA-owned roots.
    #[error("owned-path resource '{id}' failed boundary check: {source}")]
    OwnedPath {
        /// Offending resource id.
        id: String,
        /// Underlying boundary error.
        #[source]
        source: PathBoundaryError,
    },
    /// An [`ClaimResourceKind::ExternalPath`] is outside every allowed
    /// external root, or contains a traversal/symlink escape.
    #[error("external-path resource '{id}' failed boundary check: {source}")]
    ExternalPath {
        /// Offending resource id.
        id: String,
        /// Underlying boundary error.
        #[source]
        source: ExternalPathError,
    },
    /// A `plugin_id` is empty or contains characters outside the
    /// argv-safe whitelist.
    #[error("invalid plugin id '{plugin_id}': {reason}")]
    PluginId {
        /// The rejected id.
        plugin_id: String,
        /// Why it was rejected.
        reason: String,
    },
    /// A config key in a [`ClaimResourceKind::FrameworkConfig`] resource
    /// is empty or contains unsafe characters.
    #[error("invalid config key in resource '{id}': {reason}")]
    ConfigKey {
        /// Offending resource id.
        id: String,
        /// Why it was rejected.
        reason: String,
    },
    /// A resource declares a framework that differs from the claim's.
    #[error(
        "resource '{id}' declares framework '{resource_framework}' but claim targets '{claim_framework}'"
    )]
    FrameworkMismatch {
        /// Offending resource id.
        id: String,
        /// Framework in the resource.
        resource_framework: String,
        /// Framework in the claim.
        claim_framework: String,
    },
}

/// Reasons an external path is rejected.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ExternalPathError {
    /// Path contains a `.` or `..` segment.
    #[error("path '{path}' contains a '.' or '..' segment")]
    Traversal {
        /// Rejected path.
        path: PathBuf,
    },
    /// Path is not under any allowed external root (lexically or after
    /// canonicalizing the deepest existing ancestor).
    #[error("path '{path}' is not under any allowed external root for this driver")]
    OutsideAllowedRoots {
        /// Rejected path.
        path: PathBuf,
    },
}

/// Validate an external path: reject traversal, require containment under
/// one of `allowed_roots` both lexically and after canonicalizing the
/// deepest existing ancestor (defeats a symlinked ancestor that escapes
/// the root). Mirrors [`validate_owned_path`] but against driver-declared
/// roots instead of the layout's owned roots.
///
/// # Errors
///
/// [`ExternalPathError::Traversal`] for `.`/`..` segments;
/// [`ExternalPathError::OutsideAllowedRoots`] when no allowed root
/// contains the path.
pub fn validate_external_path(
    path: &Path,
    allowed_roots: &[PathBuf],
) -> Result<(), ExternalPathError> {
    use std::path::Component;
    for component in path.components() {
        if matches!(component, Component::ParentDir | Component::CurDir) {
            return Err(ExternalPathError::Traversal {
                path: path.to_path_buf(),
            });
        }
    }
    if !allowed_roots.iter().any(|root| path.starts_with(root)) {
        return Err(ExternalPathError::OutsideAllowedRoots {
            path: path.to_path_buf(),
        });
    }
    if let Some(canonical) = canonicalize_nearest_existing(path) {
        let canonical_roots: Vec<PathBuf> = allowed_roots
            .iter()
            .filter_map(|r| canonicalize_nearest_existing(r))
            .collect();
        if !canonical_roots.is_empty() && !canonical_roots.iter().any(|r| canonical.starts_with(r))
        {
            return Err(ExternalPathError::OutsideAllowedRoots {
                path: path.to_path_buf(),
            });
        }
    }
    Ok(())
}

/// Reject a plugin id unless it is a non-empty string of argv-safe
/// characters (`[A-Za-z0-9._-]`) that is neither `.`/`..` nor leading
/// with `-` (which an argv parser could mistake for a flag).
///
/// # Errors
///
/// [`ClaimValidationError::PluginId`] with a specific reason.
pub fn validate_plugin_id(plugin_id: &str) -> Result<(), ClaimValidationError> {
    let reject = |reason: &str| {
        Err(ClaimValidationError::PluginId {
            plugin_id: plugin_id.to_string(),
            reason: reason.to_string(),
        })
    };
    if plugin_id.is_empty() {
        return reject("must not be empty");
    }
    if plugin_id == "." || plugin_id == ".." {
        return reject("must not be '.' or '..'");
    }
    if plugin_id.starts_with('-') {
        return reject("must not start with '-' (would be parsed as a flag)");
    }
    if let Some(bad) = plugin_id
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
    {
        return Err(ClaimValidationError::PluginId {
            plugin_id: plugin_id.to_string(),
            reason: format!("contains disallowed character '{bad}'"),
        });
    }
    Ok(())
}

/// Reject a skill name that is empty, `.`/`..`, starts with `-`, or
/// contains characters outside `[A-Za-z0-9._-]`. Same whitelist as
/// [`validate_plugin_id`] — a skill name becomes a directory name under
/// the framework's skill root, so it must be path-component-safe.
pub fn validate_skill_name(name: &str) -> Result<(), super::AdapterError> {
    let reject = |reason: String| {
        Err(super::AdapterError::InvalidAdapterInput {
            component: String::new(),
            framework: String::new(),
            reason: format!("invalid skill name '{name}': {reason}"),
        })
    };
    if name.is_empty() {
        return reject("must not be empty".to_string());
    }
    if name == "." || name == ".." {
        return reject("must not be '.' or '..'".to_string());
    }
    if name.starts_with('-') {
        return reject("must not start with '-'".to_string());
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
    {
        return reject(format!("contains disallowed character '{bad}'"));
    }
    Ok(())
}

/// Reject a config key that is empty or contains shell metacharacters.
/// Allowed: printable ASCII minus `` ` `` `$` `;` `|` `&` `(` `)` `{`
/// `}` `[` `]` `<` `>` `\` `!` `#` `~`. This prevents injection when
/// the key is passed as a CLI argument to `config set`.
pub fn validate_config_key(key: &str) -> Result<(), super::AdapterError> {
    let reject = |reason: String| {
        Err(super::AdapterError::InvalidAdapterInput {
            component: String::new(),
            framework: String::new(),
            reason: format!("invalid config key '{key}': {reason}"),
        })
    };
    if key.is_empty() {
        return reject("must not be empty".to_string());
    }
    const BANNED: &[char] = &[
        '`', '$', ';', '|', '&', '(', ')', '{', '}', '[', ']', '<', '>', '\\', '!', '#', '~', '\'',
        '"', ' ', '\t', '\n', '\r',
    ];
    if let Some(bad) = key.chars().find(|c| BANNED.contains(c) || !c.is_ascii()) {
        return reject(format!("contains disallowed character '{bad}'"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_claim() -> AdapterClaim {
        AdapterClaim {
            claim_schema: CLAIM_SCHEMA_VERSION,
            component: "tokenless".to_string(),
            framework: "openclaw".to_string(),
            plugin_id: Some("tokenless".to_string()),
            enabled_at: "2026-06-12T10:30:45Z".to_string(),
            resource_root: PathBuf::from("/usr/local/share/anolisa/adapters/tokenless/openclaw"),
            bundle_digest: Some("sha256:abc".to_string()),
            driver_schema: DRIVER_SCHEMA_VERSION,
            status: ClaimStatus::Enabled,
            resources: vec![
                ClaimResource {
                    id: "openclaw_state_dir".to_string(),
                    purpose: "openclaw_state_dir".to_string(),
                    kind: ClaimResourceKind::ExternalPath {
                        path: PathBuf::from("/home/alice/.openclaw"),
                    },
                },
                ClaimResource {
                    id: "openclaw_plugin".to_string(),
                    purpose: "openclaw_plugin".to_string(),
                    kind: ClaimResourceKind::FrameworkPlugin {
                        framework: "openclaw".to_string(),
                        plugin_id: "tokenless".to_string(),
                    },
                },
            ],
            driver_payload: DriverPayload::OpenClaw(OpenClawClaim {
                state_dir_resource: "openclaw_state_dir".to_string(),
                plugin_resource: "openclaw_plugin".to_string(),
                skill_resources: Vec::new(),
                config_resources: Vec::new(),
            }),
        }
    }

    /// The receipt must round-trip through TOML losslessly. This is the
    /// pin against the `toml` 0.8 enum-serialization footgun: if a future
    /// edit reaches for `#[serde(flatten)]` or an internally-tagged enum,
    /// this test fails.
    #[test]
    fn adapter_claim_toml_round_trip() {
        // Wrap in a table so the array-of-tables nesting matches how the
        // claim is stored inside `InstalledState`.
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Wrapper {
            adapter_claims: Vec<AdapterClaim>,
        }
        let wrapper = Wrapper {
            adapter_claims: vec![sample_claim()],
        };
        let text = toml::to_string_pretty(&wrapper).expect("serialize to TOML");
        let parsed: Wrapper = toml::from_str(&text).expect("parse from TOML");
        assert_eq!(wrapper, parsed, "round-trip mismatch; TOML was:\n{text}");
    }

    #[test]
    fn adapter_claim_json_round_trip() {
        let claim = sample_claim();
        let json = serde_json::to_string(&claim).expect("serialize JSON");
        let parsed: AdapterClaim = serde_json::from_str(&json).expect("parse JSON");
        assert_eq!(claim, parsed);
    }

    #[test]
    fn validate_plugin_id_accepts_safe_ids() {
        validate_plugin_id("tokenless").expect("plain");
        validate_plugin_id("ws-ckpt").expect("dash");
        validate_plugin_id("a.b_c-1").expect("mixed");
    }

    #[test]
    fn validate_plugin_id_rejects_unsafe_ids() {
        assert!(validate_plugin_id("").is_err(), "empty");
        assert!(validate_plugin_id("..").is_err(), "dotdot");
        assert!(validate_plugin_id("-rf").is_err(), "leading dash");
        assert!(validate_plugin_id("a/b").is_err(), "slash");
        assert!(validate_plugin_id("a b").is_err(), "space");
        assert!(validate_plugin_id("a;b").is_err(), "semicolon");
        assert!(validate_plugin_id("a$b").is_err(), "dollar");
    }

    #[test]
    fn validate_external_path_rejects_traversal() {
        let roots = vec![PathBuf::from("/home/alice/.openclaw")];
        let err = validate_external_path(Path::new("/home/alice/.openclaw/../.ssh"), &roots)
            .expect_err("must reject");
        assert!(matches!(err, ExternalPathError::Traversal { .. }));
    }

    #[test]
    fn validate_external_path_rejects_outside_root() {
        let roots = vec![PathBuf::from("/home/alice/.openclaw")];
        let err =
            validate_external_path(Path::new("/etc/passwd"), &roots).expect_err("must reject");
        assert!(matches!(err, ExternalPathError::OutsideAllowedRoots { .. }));
    }

    #[test]
    fn validate_external_path_accepts_under_root() {
        let roots = vec![PathBuf::from("/home/alice/.openclaw")];
        validate_external_path(
            Path::new("/home/alice/.openclaw/extensions/tokenless"),
            &roots,
        )
        .expect("under root must pass");
    }

    /// A forged receipt pointing an "external" path at `/etc` must be
    /// rejected by the full claim validation, using the driver's allowed
    /// roots — not any root the receipt names for itself.
    #[test]
    fn forged_external_path_rejected_by_claim_validate() {
        let layout = FsLayout::system(None);
        let allowed = vec![PathBuf::from("/home/alice/.openclaw")];
        let mut claim = sample_claim();
        claim.resources[0].kind = ClaimResourceKind::ExternalPath {
            path: PathBuf::from("/etc/cron.d/evil"),
        };
        let err = claim.validate(&layout, &allowed).expect_err("must reject");
        assert!(matches!(err, ClaimValidationError::ExternalPath { .. }));
    }

    fn sample_hermes_claim() -> AdapterClaim {
        AdapterClaim {
            claim_schema: CLAIM_SCHEMA_VERSION,
            component: "agent-sec".to_string(),
            framework: "hermes".to_string(),
            plugin_id: Some("agent-sec".to_string()),
            enabled_at: "2026-06-22T10:30:45Z".to_string(),
            resource_root: PathBuf::from("/usr/local/share/anolisa/adapters/agent-sec/hermes"),
            bundle_digest: Some("sha256:def".to_string()),
            driver_schema: DRIVER_SCHEMA_VERSION,
            status: ClaimStatus::Enabled,
            resources: vec![
                ClaimResource {
                    id: "hermes_home".to_string(),
                    purpose: "hermes_home".to_string(),
                    kind: ClaimResourceKind::ExternalPath {
                        path: PathBuf::from("/home/alice/.hermes"),
                    },
                },
                ClaimResource {
                    id: "hermes_plugin".to_string(),
                    purpose: "hermes_plugin_dir".to_string(),
                    kind: ClaimResourceKind::ExternalPath {
                        path: PathBuf::from("/home/alice/.hermes/plugins/agent-sec"),
                    },
                },
            ],
            driver_payload: DriverPayload::Hermes(HermesClaim {
                home_resource: "hermes_home".to_string(),
                plugin_resource: "hermes_plugin".to_string(),
                skill_resources: Vec::new(),
            }),
        }
    }

    #[test]
    fn hermes_claim_toml_round_trip() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Wrapper {
            adapter_claims: Vec<AdapterClaim>,
        }
        let wrapper = Wrapper {
            adapter_claims: vec![sample_hermes_claim()],
        };
        let text = toml::to_string_pretty(&wrapper).expect("serialize Hermes to TOML");
        let parsed: Wrapper = toml::from_str(&text).expect("parse Hermes from TOML");
        assert_eq!(wrapper, parsed, "Hermes round-trip mismatch; TOML:\n{text}");
    }

    #[test]
    fn hermes_claim_json_round_trip() {
        let claim = sample_hermes_claim();
        let json = serde_json::to_string(&claim).expect("serialize Hermes JSON");
        let parsed: AdapterClaim = serde_json::from_str(&json).expect("parse Hermes JSON");
        assert_eq!(claim, parsed);
    }

    #[test]
    fn framework_config_resource_validates() {
        let layout = FsLayout::system(None);
        let allowed = vec![PathBuf::from("/home/alice/.openclaw")];
        let resource = ClaimResource {
            id: "config_touch".to_string(),
            purpose: "openclaw_config".to_string(),
            kind: ClaimResourceKind::FrameworkConfig {
                framework: "openclaw".to_string(),
                key: "plugins.entries.sec.enabled".to_string(),
            },
        };
        resource
            .validate(&layout, &allowed)
            .expect("config resource should pass");
    }

    #[test]
    fn openclaw_claim_with_skills_and_config_round_trips() {
        let claim = AdapterClaim {
            claim_schema: CLAIM_SCHEMA_VERSION,
            component: "sec-core".to_string(),
            framework: "openclaw".to_string(),
            plugin_id: Some("sec-core".to_string()),
            enabled_at: "2026-06-22T12:00:00Z".to_string(),
            resource_root: PathBuf::from("/data/adapters/sec-core/openclaw"),
            bundle_digest: None,
            driver_schema: DRIVER_SCHEMA_VERSION,
            status: ClaimStatus::Enabled,
            resources: vec![
                ClaimResource {
                    id: "state_dir".to_string(),
                    purpose: "openclaw_state_dir".to_string(),
                    kind: ClaimResourceKind::ExternalPath {
                        path: PathBuf::from("/home/alice/.openclaw"),
                    },
                },
                ClaimResource {
                    id: "plugin".to_string(),
                    purpose: "openclaw_plugin".to_string(),
                    kind: ClaimResourceKind::FrameworkPlugin {
                        framework: "openclaw".to_string(),
                        plugin_id: "sec-core".to_string(),
                    },
                },
                ClaimResource {
                    id: "skill_sec_audit".to_string(),
                    purpose: "openclaw_skill".to_string(),
                    kind: ClaimResourceKind::ExternalPath {
                        path: PathBuf::from("/home/alice/.openclaw/skills/sec-audit"),
                    },
                },
                ClaimResource {
                    id: "config_enabled".to_string(),
                    purpose: "openclaw_config".to_string(),
                    kind: ClaimResourceKind::FrameworkConfig {
                        framework: "openclaw".to_string(),
                        key: "plugins.entries.sec-core.enabled".to_string(),
                    },
                },
            ],
            driver_payload: DriverPayload::OpenClaw(OpenClawClaim {
                state_dir_resource: "state_dir".to_string(),
                plugin_resource: "plugin".to_string(),
                skill_resources: vec!["skill_sec_audit".to_string()],
                config_resources: vec!["config_enabled".to_string()],
            }),
        };
        let json = serde_json::to_string(&claim).expect("serialize");
        let parsed: AdapterClaim = serde_json::from_str(&json).expect("parse");
        assert_eq!(claim, parsed);
    }

    #[test]
    fn validate_skill_name_accepts_safe_names() {
        validate_skill_name("sec-audit").expect("dash");
        validate_skill_name("cred_scan").expect("underscore");
        validate_skill_name("skill.v2").expect("dot");
        validate_skill_name("a1").expect("short");
    }

    #[test]
    fn validate_skill_name_rejects_unsafe_names() {
        assert!(validate_skill_name("").is_err(), "empty");
        assert!(validate_skill_name("..").is_err(), "dotdot");
        assert!(validate_skill_name(".").is_err(), "dot");
        assert!(validate_skill_name("-rf").is_err(), "leading dash");
        assert!(validate_skill_name("a/b").is_err(), "slash");
        assert!(validate_skill_name("a b").is_err(), "space");
        assert!(validate_skill_name("../x").is_err(), "traversal");
    }

    #[test]
    fn validate_config_key_accepts_safe_keys() {
        validate_config_key("plugins.entries.sec.enabled").expect("dotted path");
        validate_config_key("foo.bar_baz-1").expect("mixed");
    }

    #[test]
    fn validate_config_key_rejects_unsafe_keys() {
        assert!(validate_config_key("").is_err(), "empty");
        assert!(validate_config_key("a;b").is_err(), "semicolon");
        assert!(validate_config_key("a$b").is_err(), "dollar");
        assert!(validate_config_key("a`b").is_err(), "backtick");
        assert!(validate_config_key("a b").is_err(), "space");
        assert!(validate_config_key("a|b").is_err(), "pipe");
    }

    #[test]
    fn framework_mismatch_rejected_by_claim_validate() {
        let layout = FsLayout::system(None);
        let allowed = vec![PathBuf::from("/home/alice/.openclaw")];
        let mut claim = sample_claim();
        claim.resources.push(ClaimResource {
            id: "wrong_framework".to_string(),
            purpose: "test".to_string(),
            kind: ClaimResourceKind::FrameworkPlugin {
                framework: "hermes".to_string(),
                plugin_id: "tokenless".to_string(),
            },
        });
        let err = claim.validate(&layout, &allowed).expect_err("must reject");
        assert!(
            matches!(err, ClaimValidationError::FrameworkMismatch { .. }),
            "got {err:?}"
        );
    }
}
