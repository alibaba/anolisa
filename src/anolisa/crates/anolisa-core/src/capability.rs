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
    pub capability: CapabilityHeader,
    pub implementation: CapabilityImpl,
    #[serde(default)]
    pub requires_env: HashMap<String, toml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CapabilityHeader {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CapabilityImpl {
    /// Component(s) backing this capability.
    pub components: Vec<String>,
    /// Per-component feature lists. Keyed by component name.
    #[serde(default)]
    pub features: HashMap<String, Vec<String>>,
}

impl CapabilityManifest {
    pub fn from_file(path: &Path) -> Result<Self, CapabilityError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| CapabilityError::Io(path.display().to_string(), e))?;
        toml::from_str(&content)
            .map_err(|e| CapabilityError::Parse(path.display().to_string(), e.to_string()))
    }

    pub fn from_str(s: &str) -> Result<Self, CapabilityError> {
        toml::from_str(s).map_err(|e| CapabilityError::Parse("<string>".into(), e.to_string()))
    }
}

/// Translation result: capability → operations to perform.
#[derive(Debug, Clone)]
pub struct ResolvedPlan {
    pub capability: String,
    pub components: Vec<String>,
    pub features: HashMap<String, Vec<String>>,
}

/// Capability Resolver — translates capability names into execution plans.
pub struct CapabilityResolver {
    manifests: HashMap<String, CapabilityManifest>,
}

impl CapabilityResolver {
    pub fn new() -> Self {
        Self {
            manifests: HashMap::new(),
        }
    }

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

#[derive(Debug, thiserror::Error)]
pub enum CapabilityError {
    #[error("cannot read capability manifest '{0}': {1}")]
    Io(String, std::io::Error),
    #[error("cannot parse capability manifest '{0}': {1}")]
    Parse(String, String),
    #[error("capability '{0}' not found")]
    NotFound(String),
    #[error("environment does not satisfy capability '{0}': {1}")]
    EnvNotSatisfied(String, String),
}
