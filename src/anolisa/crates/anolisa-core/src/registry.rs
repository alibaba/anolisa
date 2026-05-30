//! Component registry: discovers and loads manifests.

use crate::manifest::{Manifest, ManifestError};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Registry of known components loaded from manifest files.
#[derive(Debug, Default)]
pub struct Registry {
    pub manifests: HashMap<String, Manifest>,
}

impl Registry {
    /// Load all manifests from a directory (recursively).
    pub fn load_from_dir(dir: &Path) -> Result<Self, ManifestError> {
        let mut manifests = HashMap::new();

        if !dir.exists() {
            return Ok(Self { manifests });
        }

        for entry in walkdir(dir) {
            if entry.extension().is_some_and(|e| e == "toml") {
                match Manifest::from_file(&entry) {
                    Ok(m) => {
                        manifests.insert(m.component.name.clone(), m);
                    }
                    Err(e) => {
                        eprintln!("Warning: skipping {}: {e}", entry.display());
                    }
                }
            }
        }

        Ok(Self { manifests })
    }

    /// Get a manifest by component name.
    pub fn get(&self, name: &str) -> Option<&Manifest> {
        self.manifests.get(name)
    }

    /// List all registered component names.
    pub fn names(&self) -> Vec<&str> {
        self.manifests.keys().map(|s| s.as_str()).collect()
    }
}

/// Simple recursive file listing (avoids external walkdir dep for skeleton).
fn walkdir(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(walkdir(&path));
            } else {
                files.push(path);
            }
        }
    }
    files
}
