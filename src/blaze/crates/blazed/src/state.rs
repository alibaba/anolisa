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
    #[cfg(test)]
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
        Self::build_with_template_catalog(
            config,
            policy,
            pool,
            hook,
            spawners,
            active_backend,
            storage,
            template_catalog,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_with_template_catalog(
        config: DaemonConfig,
        policy: PolicyEngine,
        pool: PoolManager,
        hook: HookRegistry,
        spawners: SpawnerRegistry,
        active_backend: BackendKind,
        storage: Arc<dyn StorageProvider>,
        template_catalog: TemplateCatalog,
    ) -> Result<Self> {
        let state_store = StateStore::new(config.daemon.state_dir.clone());
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
