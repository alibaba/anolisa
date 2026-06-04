//! Capability layer.
//!
//! `Capability` is the customer-facing noun (`token-optimization`,
//! `workspace-checkpoint`, ...). The `CapabilityResolver` translates a capability
//! request into the underlying component + feature operations.
//!
//! Layer Discipline (see design doc): Tier 1 command handlers must go through
//! the resolver and never import component-level types directly. Errors raised
//! out of this module are intentionally capability-vocabulary; component-level
//! errors must be translated here before bubbling up to Tier 1 output.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// Capability manifest parsed from `manifests/capabilities/<name>.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct CapabilityManifest {
    /// User-facing identity and description.
    pub capability: CapabilityHeader,
    /// Component and feature mapping that backs this capability.
    pub implementation: CapabilityImpl,
    /// Capability-level environment requirements retained from TOML so the
    /// planner can evaluate them against host facts.
    #[serde(default)]
    pub requires_env: HashMap<String, toml::Value>,
}

/// User-facing capability metadata from a capability manifest.
#[derive(Debug, Clone, Deserialize)]
pub struct CapabilityHeader {
    /// Stable capability name used by CLI commands.
    pub name: String,
    /// Human-readable summary shown in list/status surfaces.
    pub description: String,
}

/// Mapping from a capability to its backing components/features.
#[derive(Debug, Clone, Deserialize)]
pub struct CapabilityImpl {
    /// Component(s) backing this capability.
    pub components: Vec<String>,
    /// Per-component feature lists. Keyed by component name.
    #[serde(default)]
    pub features: HashMap<String, Vec<String>>,
}

impl CapabilityManifest {
    /// Load and parse a capability manifest from TOML on disk.
    pub fn from_file(path: &Path) -> Result<Self, CapabilityError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| CapabilityError::Io(path.display().to_string(), e))?;
        toml::from_str(&content)
            .map_err(|e| CapabilityError::Parse(path.display().to_string(), e.to_string()))
    }

    /// Parse a capability manifest from TOML text.
    pub fn from_toml_str(s: &str) -> Result<Self, CapabilityError> {
        toml::from_str(s).map_err(|e| CapabilityError::Parse("<string>".into(), e.to_string()))
    }
}

/// Translation result: capability → operations to perform.
#[derive(Debug, Clone)]
pub struct ResolvedPlan {
    /// Capability name resolved by the manifest.
    pub capability: String,
    /// Backing components to install or operate on.
    pub components: Vec<String>,
    /// Per-component feature selections inherited from the manifest.
    pub features: HashMap<String, Vec<String>>,
}

/// Capability Resolver — translates capability names into execution plans.
pub struct CapabilityResolver {
    manifests: HashMap<String, CapabilityManifest>,
}

impl CapabilityResolver {
    /// Build an empty resolver. Register manifests before resolving names.
    pub fn new() -> Self {
        Self {
            manifests: HashMap::new(),
        }
    }

    /// Register or replace a capability manifest by its stable name.
    pub fn register(&mut self, manifest: CapabilityManifest) {
        self.manifests
            .insert(manifest.capability.name.clone(), manifest);
    }

    /// Translate a capability name into a component + feature plan.
    /// Environment gating is performed by the caller against `EnvFacts`.
    pub fn resolve(&self, name: &str) -> Result<ResolvedPlan, CapabilityError> {
        let m = self
            .manifests
            .get(name)
            .ok_or_else(|| CapabilityError::NotFound(name.into()))?;
        Ok(ResolvedPlan {
            capability: m.capability.name.clone(),
            components: m.implementation.components.clone(),
            features: m.implementation.features.clone(),
        })
    }

    /// All registered capability names.
    pub fn list(&self) -> Vec<&str> {
        self.manifests.keys().map(|s| s.as_str()).collect()
    }

    /// Lookup a registered capability manifest.
    pub fn get(&self, name: &str) -> Option<&CapabilityManifest> {
        self.manifests.get(name)
    }
}

impl Default for CapabilityResolver {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors raised while parsing or resolving capabilities.
#[derive(Debug, thiserror::Error)]
pub enum CapabilityError {
    /// Capability manifest could not be read.
    #[error("cannot read capability manifest '{0}': {1}")]
    Io(String, std::io::Error),
    /// Capability manifest TOML is invalid.
    #[error("cannot parse capability manifest '{0}': {1}")]
    Parse(String, String),
    /// Requested capability name is not registered.
    #[error("capability '{0}' not found")]
    NotFound(String),
    /// Host facts do not satisfy capability-level requirements.
    #[error("environment does not satisfy capability '{0}': {1}")]
    EnvNotSatisfied(String, String),
}
