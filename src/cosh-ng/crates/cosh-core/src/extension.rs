//! Extension package discovery, validation, state, and runtime contributions.

pub mod agent;
pub mod config;
pub mod generation;
pub mod git;
pub mod identity;
pub mod installer;
pub mod manager;
pub mod manifest;
pub mod mcp;
pub mod runtime;
pub mod runtime_context;
pub mod scaffold;
pub mod settings;
pub mod source;
pub mod state;
pub mod variables;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use agent::AgentRegistry;
pub use config::{ExtensionConfig, ExtensionHooks};
pub use generation::{GenerationController, RuntimeGeneration, RuntimeSnapshot};
pub use manager::ExtensionManager;
pub use manifest::ManifestSchemaVersion;
pub use manifest::{ContextContribution, McpServerContribution, SettingDefinition, SettingType};
pub use mcp::McpRuntime;
pub use runtime::RuntimeSnapshotBuilder;
pub use runtime_context::ExtensionContextSnapshot;

use crate::skill::COPILOT_CONFIG_DIR;

/// Sub-directory under `~/.copilot-shell/` containing installed extensions.
pub const USER_EXTENSIONS_DIR: &str = "extensions";

/// System-wide extensions directory installed by the host package manager.
pub const SYSTEM_EXTENSIONS_DIR: &str = "/usr/share/anolisa/extensions";

/// System-wide extensions directory used by raw installations.
pub(crate) const LOCAL_SYSTEM_EXTENSIONS_DIR: &str = "/usr/local/share/anolisa/extensions";

/// Canonical extension manifest file name.
pub const EXTENSION_CONFIG_FILENAME: &str = "cosh-extension.json";

/// Legacy install metadata file name used by existing direct-layout packages.
pub const INSTALL_METADATA_FILENAME: &str = "cosh-extension-install.json";

/// Managed installation metadata stored beside, never inside, package payload.
pub const MANAGED_INSTALL_METADATA_FILENAME: &str = "installation.json";

/// Persisted user intent for an extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DesiredState {
    /// The user wants the extension loaded when healthy.
    Enabled,
    /// The user does not want the extension loaded.
    Disabled,
}

/// Whether an extension contributes to the current catalog snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveState {
    /// Contributions are present in this snapshot.
    Enabled,
    /// Contributions are absent from this snapshot.
    Disabled,
    /// The management transport has not observed a live runtime snapshot.
    NotLoaded,
}

/// When a desired-state change can become effective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Activation {
    /// The catalog snapshot already reflects the desired state.
    Immediate,
    /// A safe runtime boundary is required before switching generations.
    PendingSafeReload,
    /// A new cosh session is required.
    NextSession,
}

/// Health of the selected extension installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionHealth {
    /// The selected package is valid and all implemented contributions loaded.
    Healthy,
    /// The package is usable, but has warnings or deferred contribution kinds.
    Degraded,
    /// More than one source exists and no valid selection resolves it.
    Conflict,
    /// State or package validation failed closed.
    Broken,
}

/// Source class for a discovered extension installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExtensionSourceKind {
    /// Validated local directory copied into the managed store.
    PathCopy,
    /// Development installation linked to an external canonical directory.
    Link,
    /// HTTPS Git installation pinned to a resolved revision.
    GitHttps,
    /// Existing direct-layout package under the user extension directory.
    Legacy,
    /// Read-only package under the system extension directory.
    System,
    /// Multiple installations exist but no source is selected.
    Conflict,
}

/// Stable machine-readable catalog diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionDiagnostic {
    /// Stable diagnostic code for protocol consumers.
    pub code: String,
    /// Human-readable detail suitable for logs and the slash UI.
    pub message: String,
}

impl ExtensionDiagnostic {
    /// Builds a diagnostic from a stable code and contextual message.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// A selected extension installation and its current catalog projection.
#[derive(Debug, Clone)]
pub struct Extension {
    /// Package identity from the validated manifest.
    pub name: String,
    /// Installed package version.
    pub version: String,
    /// Selected package root.
    pub path: PathBuf,
    /// Compatibility field retained for existing runtime consumers.
    pub is_active: bool,
    /// Runtime configuration consumed by skill and hook owners.
    pub config: ExtensionConfig,
    /// Optional legacy install metadata.
    pub install_metadata: Option<InstallMetadata>,
    /// Optional versioned metadata for a managed installation.
    pub managed_install_metadata: Option<source::ManagedInstallationMetadata>,
    /// Parsed manifest schema generation.
    pub schema_version: ManifestSchemaVersion,
    /// Selected source class.
    pub source: ExtensionSourceKind,
    /// Canonical source identity used by selection and diagnostics.
    pub source_identity: String,
    /// Sources currently available for this package identity.
    pub available_sources: Vec<ExtensionSourceKind>,
    /// Canonical identities keyed by the user-facing source selector.
    pub available_source_identities: BTreeMap<String, String>,
    /// Persisted user intent.
    pub desired_state: DesiredState,
    /// State represented by this catalog snapshot.
    pub effective_state: EffectiveState,
    /// Activation boundary for the current projection.
    pub activation: Activation,
    /// Package/catalog health.
    pub health: ExtensionHealth,
    /// Stable capability security fingerprint.
    pub capability_fingerprint: String,
    /// Validated canonical capability IDs.
    pub capabilities: Vec<String>,
    /// Diagnostics associated with this package.
    pub diagnostics: Vec<ExtensionDiagnostic>,
    /// Typed setting schema declared by the selected package.
    pub settings: Vec<SettingDefinition>,
    /// Validated context files contributed by the selected package.
    pub contexts: Vec<ContextContribution>,
    /// Validated stdio MCP server declarations.
    pub mcp_servers: Vec<McpServerContribution>,
    /// Explicit package directories containing agent definitions.
    pub agent_directories: Vec<PathBuf>,
}

/// Legacy metadata describing how a direct-layout extension was installed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallMetadata {
    /// Source path or URL from which the extension was installed.
    pub source: String,
    /// Legacy installation type, such as `local` or `link`.
    #[serde(rename = "type")]
    pub install_type: String,
    /// ISO 8601 installation timestamp.
    pub installed_at: String,
}

/// Returns the user-level extension directory.
pub fn user_extensions_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(COPILOT_CONFIG_DIR).join(USER_EXTENSIONS_DIR))
}

/// Returns system extension directories for the running installation.
///
/// Installed binaries derive both raw and package-managed roots from their
/// FHS libexec location. Development binaries fall back to the host defaults.
pub(crate) fn system_extensions_dirs() -> Vec<PathBuf> {
    system_extensions_dirs_for_executable(std::env::current_exe().ok().as_deref())
}

fn system_extensions_dirs_for_executable(executable: Option<&Path>) -> Vec<PathBuf> {
    let Some(prefix) = executable.and_then(anolisa_prefix_from_executable) else {
        return vec![
            PathBuf::from(LOCAL_SYSTEM_EXTENSIONS_DIR),
            PathBuf::from(SYSTEM_EXTENSIONS_DIR),
        ];
    };
    vec![
        prefix.join("usr/local/share/anolisa/extensions"),
        prefix.join("usr/share/anolisa/extensions"),
    ]
}

fn anolisa_prefix_from_executable(executable: &Path) -> Option<&Path> {
    let cosh_ng_dir = executable.parent()?;
    let anolisa_dir = cosh_ng_dir.parent()?;
    let libexec_dir = anolisa_dir.parent()?;
    if executable.file_name()? != "cosh-core"
        || cosh_ng_dir.file_name()? != "cosh-ng"
        || anolisa_dir.file_name()? != "anolisa"
        || libexec_dir.file_name()? != "libexec"
    {
        return None;
    }

    let fhs_tree = libexec_dir.parent()?;
    if fhs_tree.ends_with("usr/local") {
        fhs_tree.parent()?.parent()
    } else if fhs_tree.ends_with("usr") {
        fhs_tree.parent()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_extension_roots_prefer_raw_installations() {
        assert_eq!(
            system_extensions_dirs_for_executable(None),
            vec![
                PathBuf::from("/usr/local/share/anolisa/extensions"),
                PathBuf::from("/usr/share/anolisa/extensions"),
            ]
        );
    }

    #[test]
    fn system_extension_roots_follow_custom_prefix() {
        let executable = Path::new("/opt/anolisa/usr/local/libexec/anolisa/cosh-ng/cosh-core");

        assert_eq!(
            system_extensions_dirs_for_executable(Some(executable)),
            vec![
                PathBuf::from("/opt/anolisa/usr/local/share/anolisa/extensions"),
                PathBuf::from("/opt/anolisa/usr/share/anolisa/extensions"),
            ]
        );
    }
}
