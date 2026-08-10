// SPDX-License-Identifier: Apache-2.0
//! Daemon-wide shared state: configuration, policy engine, pool, hook registry,
//! and the in-memory instance map. All API handlers
//! receive an [`Arc<ServerState>`] and acquire the relevant `Mutex<...>`
//! lock just long enough to read or mutate the piece they need — locks
//! are never held across `.await` boundaries.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
    pub state_dir: PathBuf,
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
        let state_dir = config.daemon.state_dir.clone();
        let instances = scan_state_dir(&state_dir).unwrap_or_else(|err| {
            tracing::warn!(error = %err, "failed to scan state_dir, starting empty");
            HashMap::new()
        });
        let (manager, resources) = SandboxManager::new(SandboxManagerInit {
            instances,
            pool,
            spawners,
            active_backend,
            storage: storage.clone(),
            state_dir: state_dir.clone(),
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
            state_dir,
            metrics: resources.metrics,
        })
    }

    /// Return the async operation lock that serializes one sandbox mutation.
    pub fn operation_lock(&self, id: Uuid) -> Arc<tokio::sync::Mutex<()>> {
        self.manager.operation_lock(id)
    }
}

/// Best-effort: walk `{state_dir}/<uuid>/state.json` and rebuild the
/// instance map. Used both at boot and (in the future) by `daemon doctor`.
fn scan_state_dir(state_dir: &Path) -> Result<HashMap<Uuid, SandboxInstance>> {
    let mut out = HashMap::new();
    if !state_dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(state_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        let Ok(id) = Uuid::parse_str(name_str) else {
            continue;
        };
        match SandboxInstance::load(state_dir, id) {
            Ok(inst) => {
                out.insert(id, inst);
            }
            Err(err) => {
                tracing::warn!(instance = %id, error = %err, "skipping corrupt instance state");
            }
        }
    }
    tracing::info!(instances = out.len(), "rehydrated instances from state_dir");
    Ok(out)
}
