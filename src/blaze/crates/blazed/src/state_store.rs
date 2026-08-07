// SPDX-License-Identifier: Apache-2.0
//! Daemon-owned access to persisted sandbox state and runtime directories.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use blaze_core::lifecycle::SandboxInstance;
use uuid::Uuid;

use crate::error::Result;

/// Central access point for the daemon state directory.
///
/// Keeping path derivation and lifecycle-record I/O behind this type lets the
/// daemon strengthen object ownership without changing every caller again.
#[derive(Clone, Debug)]
pub struct StateStore {
    root: Arc<PathBuf>,
}

impl StateStore {
    /// Create a state-store view rooted at the configured daemon directory.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root: Arc::new(root),
        }
    }

    /// Return the configured state directory.
    pub fn root(&self) -> &Path {
        self.root.as_path()
    }

    /// Return the directory assigned to one sandbox.
    pub fn run_dir(&self, id: Uuid) -> PathBuf {
        self.root.join(id.to_string())
    }

    /// Persist one lifecycle record below this store.
    pub fn persist(&self, instance: &SandboxInstance) -> Result<()> {
        instance.persist(self.root())?;
        Ok(())
    }

    /// Load one lifecycle record from this store.
    pub fn load(&self, id: Uuid) -> Result<SandboxInstance> {
        Ok(SandboxInstance::load(self.root(), id)?)
    }

    /// Best-effort scan of persisted lifecycle records.
    pub fn scan(&self) -> Result<HashMap<Uuid, SandboxInstance>> {
        let mut instances = HashMap::new();
        if !self.root().exists() {
            return Ok(instances);
        }

        for entry in std::fs::read_dir(self.root())? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Ok(id) = Uuid::parse_str(name) else {
                continue;
            };
            match self.load(id) {
                Ok(instance) => {
                    instances.insert(id, instance);
                }
                Err(error) => {
                    tracing::warn!(instance = %id, error = %error, "skipping corrupt instance state");
                }
            }
        }
        tracing::info!(
            instances = instances.len(),
            "rehydrated instances from state_dir"
        );
        Ok(instances)
    }
}

#[cfg(test)]
mod tests {
    use blaze_core::backend::BackendKind;
    use blaze_core::lifecycle::StartPath;
    use blaze_core::policy::WorkloadClass;

    use super::*;

    #[test]
    fn store_centralizes_record_io_scan_and_run_directory_derivation() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("state");
        std::fs::create_dir(&root).expect("state directory");
        let store = StateStore::new(root.clone());
        let instance = SandboxInstance::new(
            BackendKind::Mock,
            WorkloadClass::AgentTool,
            "sha256:test".into(),
            StartPath::Cold,
            "default".into(),
        );

        store.persist(&instance).expect("persist instance");

        let loaded = store.load(instance.id).expect("load instance");
        assert_eq!(loaded.id, instance.id);
        assert_eq!(
            store.run_dir(instance.id),
            root.join(instance.id.to_string())
        );
        let scanned = store.scan().expect("scan state store");
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[&instance.id].id, instance.id);
    }
}
