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
}
