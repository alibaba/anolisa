pub mod config;
pub mod manager;
pub mod variables;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use config::{ExtensionConfig, ExtensionHooks};
pub use manager::ExtensionManager;

use crate::skill::COPILOT_CONFIG_DIR;

/// Sub-directory under `~/.copilot-shell/` containing installed extensions.
pub const USER_EXTENSIONS_DIR: &str = "extensions";

/// System-wide extensions directory (installed via RPM/package manager).
pub const SYSTEM_EXTENSIONS_DIR: &str = "/usr/share/anolisa/extensions";

/// The config file name to look for in each extension directory.
pub const EXTENSION_CONFIG_FILENAME: &str = "cosh-extension.json";

/// Optional install metadata file produced by programmatic installation.
pub const INSTALL_METADATA_FILENAME: &str = "cosh-extension-install.json";

/// A fully loaded extension at runtime.
#[derive(Debug, Clone)]
pub struct Extension {
    /// Extension name (from cosh-extension.json `name` field).
    pub name: String,
    /// Extension version string.
    pub version: String,
    /// Absolute path to the extension directory on disk.
    pub path: PathBuf,
    /// Whether this extension is active (loaded successfully).
    pub is_active: bool,
    /// Parsed and hydrated configuration.
    pub config: ExtensionConfig,
    /// Optional install metadata (from cosh-extension-install.json).
    pub install_metadata: Option<InstallMetadata>,
}

/// Metadata about how an extension was installed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallMetadata {
    /// Source path or URL from which the extension was installed.
    pub source: String,
    /// Installation type: "local" (copied) or "link" (symlinked).
    #[serde(rename = "type")]
    pub install_type: String,
    /// ISO 8601 timestamp when the extension was installed.
    pub installed_at: String,
}

/// Returns the user-level extensions directory: `~/.copilot-shell/extensions/`.
pub fn user_extensions_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(COPILOT_CONFIG_DIR).join(USER_EXTENSIONS_DIR))
}

/// Returns the system-level extensions directory.
pub fn system_extensions_dir() -> PathBuf {
    PathBuf::from(SYSTEM_EXTENSIONS_DIR)
}
