// SPDX-License-Identifier: Apache-2.0
//! Daemon-wide shared state: configuration, policy engine, pool, hook registry,
//! and the in-memory instance map. All API handlers
//! receive an [`Arc<ServerState>`] and acquire the relevant `Mutex<...>`
//! lock just long enough to read or mutate the piece they need — locks
//! are never held across `.await` boundaries.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use blaze_core::backend::BackendKind;
use blaze_core::config::DaemonConfig;
use blaze_core::kernel::HookRegistry;
use blaze_core::lifecycle::SandboxInstance;
use blaze_core::policy::PolicyEngine;
use blaze_core::pool::PoolManager;
use blaze_core::storage::StorageProvider;
use uuid::Uuid;

use crate::error::Result;
use crate::metrics::Metrics;
use crate::sandbox::template::TemplateCatalog;
#[cfg(test)]
use crate::sandbox::template::validate_template_roots;
use crate::sandbox::{SandboxManager, SandboxManagerInit};
use crate::spawner::SpawnerRegistry;
use crate::state_store::StateStore;

/// All daemon mutable state. Cloning is via `Arc` (see the `state.clone()`
/// idiom in `daemon.rs`); the struct itself is never `Clone`.
pub struct ServerState {
    pub config: Mutex<DaemonConfig>,
    pub policy: Mutex<PolicyEngine>,
    pub pool: Arc<Mutex<PoolManager>>,
    pub hook: Mutex<HookRegistry>,
    pub instances: Arc<Mutex<HashMap<Uuid, SandboxInstance>>>,
    pub manager: Arc<SandboxManager>,
    /// The backend kind that `build_spawner` actually probed and selected.
    /// API handlers use this to constrain availability to the single active
    /// backend rather than reporting all configured binaries.
    pub active_backend: BackendKind,
    pub storage: Arc<dyn StorageProvider>,
    pub state_store: StateStore,
    pub metrics: Arc<Metrics>,
}

impl ServerState {
    /// Build a server state, scanning `state_dir` to repopulate the
    /// `instances` map from previous runs (best-effort; corrupt entries
    /// are skipped with a warning).
    #[allow(clippy::too_many_arguments)]
    pub fn build_with_store(
        config: DaemonConfig,
        policy: PolicyEngine,
        pool: PoolManager,
        hook: HookRegistry,
        spawners: SpawnerRegistry,
        active_backend: BackendKind,
        storage: Arc<dyn StorageProvider>,
        template_catalog: TemplateCatalog,
        state_store: StateStore,
    ) -> Result<Self> {
        Self::assemble(
            config,
            policy,
            pool,
            hook,
            spawners,
            active_backend,
            storage,
            template_catalog,
            state_store,
        )
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        config: DaemonConfig,
        policy: PolicyEngine,
        pool: PoolManager,
        hook: HookRegistry,
        spawners: SpawnerRegistry,
        active_backend: BackendKind,
        storage: Arc<dyn StorageProvider>,
    ) -> Result<Self> {
        let template_roots = validate_template_roots(
            &config.template,
            &config.storage.images_dir,
            &config.storage.instances_dir,
            &config.policy.dir,
            &config.backends,
            &config.daemon.state_dir,
            &config.daemon.socket,
            None,
        )?;
        let template_catalog = TemplateCatalog::open_validated(&config.template, template_roots)?;
        let state_store = StateStore::new(config.daemon.state_dir.clone());
        Self::assemble(
            config,
            policy,
            pool,
            hook,
            spawners,
            active_backend,
            storage,
            template_catalog,
            state_store,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn assemble(
        config: DaemonConfig,
        policy: PolicyEngine,
        pool: PoolManager,
        hook: HookRegistry,
        spawners: SpawnerRegistry,
        active_backend: BackendKind,
        storage: Arc<dyn StorageProvider>,
        template_catalog: TemplateCatalog,
        state_store: StateStore,
    ) -> Result<Self> {
        let instances = state_store.scan().unwrap_or_else(|err| {
            tracing::warn!(error = %err, "failed to scan state_dir, starting empty");
            HashMap::new()
        });
        let (manager, resources) = SandboxManager::new(SandboxManagerInit {
            instances,
            pool,
            spawners,
            active_backend,
            storage: storage.clone(),
            state_store: state_store.clone(),
            rootfs_size: config.storage.rootfs_size,
            mem_size: config.storage.mem_size,
            template_catalog,
        });

        Ok(Self {
            config: Mutex::new(config),
            policy: Mutex::new(policy),
            pool: resources.pool,
            hook: Mutex::new(hook),
            instances: resources.instances,
            manager: Arc::new(manager),
            active_backend,
            storage,
            state_store,
            metrics: resources.metrics,
        })
    }

    /// Return the async operation lock that serializes one sandbox mutation.
    pub fn operation_lock(&self, id: Uuid) -> Arc<tokio::sync::Mutex<()>> {
        self.manager.operation_lock(id)
    }
}

#[cfg(test)]
mod tests {
    use blaze_core::lifecycle::StartPath;
    use blaze_core::policy::WorkloadClass;

    use crate::file_provider::FileStorageProvider;
    use crate::spawner::{MockSpawner, SpawnerRegistry};

    use super::*;

    #[test]
    fn builder_uses_the_preopened_state_store_after_path_replacement() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let configured_root = temporary.path().join("state");
        let retained_root = temporary.path().join("retained-state");
        let images = temporary.path().join("images");
        let instances = temporary.path().join("instances");
        for directory in [&configured_root, &images, &instances] {
            std::fs::create_dir(directory).expect("test directory");
        }

        let mut config = DaemonConfig::default();
        config.daemon.state_dir = configured_root.clone();
        config.storage.images_dir = images.clone();
        config.storage.instances_dir = instances.clone();
        config.template.dir = temporary.path().join("templates");
        config.template.import_root = Some(temporary.path().join("template-imports"));
        std::fs::create_dir(
            config
                .template
                .import_root
                .as_ref()
                .expect("template import root"),
        )
        .expect("template import directory");
        let template_catalog = TemplateCatalog::open(&config.template).expect("template catalog");
        let existing = SandboxInstance::new(
            BackendKind::Mock,
            WorkloadClass::AgentTool,
            "sha256:existing".into(),
            StartPath::Cold,
            "default".into(),
        );
        existing
            .persist(&configured_root)
            .expect("persist existing fixture");
        let state_store = StateStore::new(configured_root.clone());

        std::fs::rename(&configured_root, &retained_root).expect("move opened state root");
        std::fs::create_dir(&configured_root).expect("replacement state root");

        let mut spawners = SpawnerRegistry::new();
        spawners.insert(BackendKind::Mock, Arc::new(MockSpawner));
        let storage: Arc<dyn StorageProvider> =
            Arc::new(FileStorageProvider::with_images(images, instances));
        let state = ServerState::build_with_store(
            config,
            PolicyEngine::new(),
            PoolManager::new(),
            HookRegistry::new(),
            spawners,
            BackendKind::Mock,
            storage,
            template_catalog,
            state_store,
        )
        .expect("server state");

        assert!(
            state
                .instances
                .lock()
                .expect("instances")
                .contains_key(&existing.id)
        );
        let new_instance = SandboxInstance::new(
            BackendKind::Mock,
            WorkloadClass::AgentTool,
            "sha256:new".into(),
            StartPath::Cold,
            "default".into(),
        );
        state
            .state_store
            .persist(&new_instance)
            .expect("persist through retained store");

        assert!(
            retained_root
                .join(new_instance.id.to_string())
                .join("state.json")
                .is_file()
        );
        assert!(!configured_root.join(new_instance.id.to_string()).exists());
    }
}
