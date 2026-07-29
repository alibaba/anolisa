// SPDX-License-Identifier: Apache-2.0
//! Daemon-wide shared state: configuration, policy engine, pool, template
//! and hook registries, and the in-memory instance map. All API handlers
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
use blaze_core::template::TemplateRegistry;
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::error::Result;
use crate::metrics::Metrics;
use crate::spawner::{DynBackendInstance, DynSpawner, SpawnerRegistry};

/// All daemon mutable state. Cloning is via `Arc` (see the `state.clone()`
/// idiom in `daemon.rs`); the struct itself is never `Clone`.
pub struct ServerState {
    pub config: Mutex<DaemonConfig>,
    pub policy: Mutex<PolicyEngine>,
    pub pool: Mutex<PoolManager>,
    pub template: Mutex<TemplateRegistry>,
    pub hook: Mutex<HookRegistry>,
    pub instances: Mutex<HashMap<Uuid, SandboxInstance>>,
    pub backend_instances: Mutex<HashMap<Uuid, DynBackendInstance>>,
    operation_locks: Mutex<HashMap<Uuid, Arc<AsyncMutex<()>>>>,
    pub spawners: SpawnerRegistry,
    /// The backend kind that `build_spawner` actually probed and selected.
    /// API handlers use this to constrain availability to the single active
    /// backend rather than reporting all configured binaries.
    pub active_backend: BackendKind,
    pub storage: Arc<dyn StorageProvider>,
    pub state_dir: PathBuf,
    pub metrics: Metrics,
}

impl ServerState {
    /// Build a server state, scanning `state_dir` to repopulate the
    /// `instances` map from previous runs (best-effort; corrupt entries
    /// are skipped with a warning).
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        config: DaemonConfig,
        policy: PolicyEngine,
        pool: PoolManager,
        template: TemplateRegistry,
        hook: HookRegistry,
        spawners: SpawnerRegistry,
        active_backend: BackendKind,
        storage: Arc<dyn StorageProvider>,
    ) -> Self {
        let state_dir = config.daemon.state_dir.clone();
        let instances = scan_state_dir(&state_dir).unwrap_or_else(|err| {
            tracing::warn!(error = %err, "failed to scan state_dir, starting empty");
            HashMap::new()
        });
        let operation_locks = instances
            .keys()
            .copied()
            .map(|id| (id, Arc::new(AsyncMutex::new(()))))
            .collect();

        Self {
            config: Mutex::new(config),
            policy: Mutex::new(policy),
            pool: Mutex::new(pool),
            template: Mutex::new(template),
            hook: Mutex::new(hook),
            instances: Mutex::new(instances),
            backend_instances: Mutex::new(HashMap::new()),
            operation_locks: Mutex::new(operation_locks),
            spawners,
            active_backend,
            storage,
            state_dir,
            metrics: Metrics::new(),
        }
    }

    /// Return the async operation lock that serializes one sandbox mutation.
    pub fn operation_lock(&self, id: Uuid) -> Arc<AsyncMutex<()>> {
        match self.operation_locks.lock() {
            Ok(mut locks) => locks
                .entry(id)
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone(),
            Err(poisoned) => poisoned
                .into_inner()
                .entry(id)
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone(),
        }
    }

    /// Return the implementation responsible for a persisted backend kind.
    pub fn spawner_for(&self, kind: BackendKind) -> Option<DynSpawner> {
        self.spawners.get(kind)
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
