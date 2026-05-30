//! Component manifest TOML parsing.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// Top-level manifest structure parsed from `component.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub component: ComponentSection,
    pub build: Option<BuildSection>,
    pub install: Option<InstallSection>,
    pub environment: Option<EnvironmentSection>,
    pub dependencies: Option<DependenciesSection>,
    #[serde(default)]
    pub features: Vec<FeatureSection>,
    #[serde(default)]
    pub adapters: Vec<AdapterSection>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AdapterSection {
    /// Target framework name: `cosh`, `openclaw`, `hermes`, `mcp`, ...
    pub framework: String,
    /// Adapter classification — drives default install behavior.
    pub kind: AdapterKind,
    /// Build-output source path (relative to component build root).
    pub source: String,
    /// Install destination, supports placeholders (`{datadir}`, `{component}`).
    pub dest: String,
    /// Optional detection hint (binary name, paths). Authoritative rules live
    /// in the framework probe; this is just a manifest-side annotation.
    #[serde(default)]
    pub detect: std::collections::HashMap<String, toml::Value>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AdapterKind {
    /// Bundled with ANOLISA (e.g. `cosh`); installed by default with the capability.
    FirstParty,
    /// External framework (e.g. `openclaw`, `hermes`); opt-in via `--with-adapter`.
    ThirdParty,
    /// Open protocol (e.g. `mcp`); installed to a standard advertise location.
    Protocol,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ComponentSection {
    pub name: String,
    pub version: String,
    pub layer: String,
    pub domain: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BuildSection {
    pub system: String,
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub toolchain: HashMap<String, String>,
    #[serde(default)]
    pub pre_build: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InstallSection {
    #[serde(default)]
    pub modes: Vec<String>,
    #[serde(default)]
    pub files: Vec<InstallFile>,
    #[serde(default)]
    pub services: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InstallFile {
    pub source: String,
    pub dest: String,
    #[serde(default = "default_mode")]
    pub mode: String,
    pub symlink: Option<String>,
}

fn default_mode() -> String {
    "0755".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct EnvironmentSection {
    pub requires_os: Option<String>,
    #[serde(default)]
    pub requires_arch: Vec<String>,
    pub requires_kernel: Option<String>,
    #[serde(default)]
    pub requires_env: HashMap<String, String>,
    #[serde(default)]
    pub incompatible_env: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DependenciesSection {
    #[serde(default)]
    pub build: Vec<String>,
    #[serde(default)]
    pub runtime: Vec<String>,
    #[serde(default)]
    pub components: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FeatureSection {
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub requires_env: HashMap<String, String>,
    #[serde(default)]
    pub conflicts_with: Vec<String>,
}

impl Manifest {
    /// Parse a manifest from a TOML file path.
    pub fn from_file(path: &Path) -> Result<Self, ManifestError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| ManifestError::Io(path.display().to_string(), e))?;
        toml::from_str(&content)
            .map_err(|e| ManifestError::Parse(path.display().to_string(), e.to_string()))
    }

    /// Parse a manifest from a TOML string.
    pub fn from_str(s: &str) -> Result<Self, ManifestError> {
        toml::from_str(s).map_err(|e| ManifestError::Parse("<string>".into(), e.to_string()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("cannot read manifest '{0}': {1}")]
    Io(String, std::io::Error),
    #[error("cannot parse manifest '{0}': {1}")]
    Parse(String, String),
}
