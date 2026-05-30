//! Feature flag storage and resolution.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// In-memory representation of feature flags for all components.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct FeatureStore {
    #[serde(default)]
    pub component: HashMap<String, ComponentConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub features: HashMap<String, bool>,
}

fn default_true() -> bool {
    true
}

impl FeatureStore {
    /// Load feature store from a TOML file.
    pub fn load(path: &Path) -> Result<Self, FeatureStoreError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content =
            std::fs::read_to_string(path).map_err(|e| FeatureStoreError::Io(e.to_string()))?;
        toml::from_str(&content).map_err(|e| FeatureStoreError::Parse(e.to_string()))
    }

    /// Save feature store to a TOML file.
    pub fn save(&self, path: &Path) -> Result<(), FeatureStoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| FeatureStoreError::Io(e.to_string()))?;
        }
        let content =
            toml::to_string_pretty(self).map_err(|e| FeatureStoreError::Parse(e.to_string()))?;
        std::fs::write(path, content).map_err(|e| FeatureStoreError::Io(e.to_string()))
    }

    /// Check if a feature is enabled for a component.
    pub fn is_enabled(&self, component: &str, feature: &str) -> bool {
        self.component
            .get(component)
            .and_then(|c| c.features.get(feature))
            .copied()
            .unwrap_or(false)
    }

    /// Enable a feature.
    pub fn enable(&mut self, component: &str, feature: &str) {
        self.component
            .entry(component.to_string())
            .or_insert_with(|| ComponentConfig {
                enabled: true,
                features: HashMap::new(),
            })
            .features
            .insert(feature.to_string(), true);
    }

    /// Disable a feature.
    pub fn disable(&mut self, component: &str, feature: &str) {
        if let Some(comp) = self.component.get_mut(component) {
            comp.features.insert(feature.to_string(), false);
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FeatureStoreError {
    #[error("I/O error: {0}")]
    Io(String),
    #[error("parse error: {0}")]
    Parse(String),
}
