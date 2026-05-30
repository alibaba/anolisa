//! Installed state tracking (installed.toml).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Tracks what components are installed and their file lists.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct InstalledState {
    #[serde(default)]
    pub components: HashMap<String, InstalledComponent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledComponent {
    pub version: String,
    pub installed_at: String,
    pub install_mode: String,
    pub files: Vec<String>,
    pub build_hash: Option<String>,
}

impl InstalledState {
    pub fn load(path: &Path) -> Result<Self, std::io::Error> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        toml::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
    }

    pub fn save(&self, path: &Path) -> Result<(), std::io::Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        std::fs::write(path, content)
    }

    pub fn is_installed(&self, component: &str) -> bool {
        self.components.contains_key(component)
    }
}
