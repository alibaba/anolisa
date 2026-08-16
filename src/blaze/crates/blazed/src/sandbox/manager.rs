// SPDX-License-Identifier: Apache-2.0
//! Recoverable sandbox create, destroy, and startup cleanup.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use blaze_core::BlazeError;
use blaze_core::backend::{BackendKind, SpawnRequest};
use blaze_core::lifecycle::{BackendOwnership, OperationKind, SandboxInstance, SandboxState};
use blaze_core::policy::RuntimeDecision;
use blaze_core::storage::{AcquireOpts, StorageProvider, StorageSlot};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::{BlazeDaemonError, Result};
use crate::guest::{GuestClient, GuestExecResult, MAX_GUEST_FILE_BYTES};
use crate::metrics::Metrics;
use crate::sandbox::template::TemplateCatalog;
use crate::spawner::{
    BackendSpawnRequest, DynBackendInstance, SpawnerRegistry, spawn_with_runtime_directory,
};
use crate::state_store::StateStore;

const GUEST_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Inputs already parsed and policy-evaluated by the API.
#[derive(Debug, Clone)]
pub struct CreateSandbox {
    /// Policy decision for this request.
    pub decision: RuntimeDecision,
    /// Image identity used by storage allocation.
    pub image_digest: String,
    /// Concrete backend selected from the policy and daemon availability.
    pub runtime_backend: BackendKind,
    /// Executable selected during daemon startup.
    pub binary_path: PathBuf,
}

/// Result of one managed create request.
#[derive(Debug, Clone)]
pub struct CreateSandboxResult {
    /// Persisted sandbox metadata.
    pub instance: SandboxInstance,
    /// Backend implementation that owns the runtime.
    pub selected_backend: BackendKind,
}

/// One startup cleanup failure. Other records continue to be reconciled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileFailure {
    /// Sandbox whose cleanup remains incomplete.
    pub instance_id: Uuid,
    /// Actionable failure description.
    pub error: String,
}

/// Aggregate startup cleanup outcome.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Number of non-terminal records examined.
    pub attempted: usize,
    /// Number moved to the terminal state.
    pub completed: usize,
    /// Records that remain recoverable.
    pub failures: Vec<ReconcileFailure>,
}

/// Owns durable lifecycle metadata and non-serializable runtime handles.
///
/// The maps are shared with read-only and non-lifecycle API paths. All
/// Create, destroy, and restart cleanup mutations enter
/// through this type and are serialized by a per-sandbox async lock.
pub struct SandboxManager {
    instances: Arc<Mutex<HashMap<Uuid, SandboxInstance>>>,
    backend_instances: Arc<Mutex<HashMap<Uuid, DynBackendInstance>>>,
    operation_locks: Mutex<HashMap<Uuid, Arc<AsyncMutex<()>>>>,
    pub(super) storage_sync_inflight: Arc<Mutex<HashSet<Uuid>>>,
    pub(super) storage_sync_permits: Arc<Semaphore>,
    spawners: Arc<SpawnerRegistry>,
    active_backend: BackendKind,
    storage: Arc<dyn StorageProvider>,
    state_store: StateStore,
    rootfs_size: u64,
    mem_size: u64,
    metrics: Arc<Metrics>,
    pub(super) template_catalog: TemplateCatalog,
}

/// Construction inputs grouped to keep daemon wiring explicit.
pub struct SandboxManagerInit {
    pub instances: HashMap<Uuid, SandboxInstance>,
    pub spawners: SpawnerRegistry,
    pub active_backend: BackendKind,
    pub storage: Arc<dyn StorageProvider>,
    pub state_store: StateStore,
    pub rootfs_size: u64,
    pub mem_size: u64,
    pub template_catalog: TemplateCatalog,
}

/// Shared resources returned to the daemon wiring and test harness.
pub struct SandboxManagerResources {
    #[cfg(test)]
    pub instances: Arc<Mutex<HashMap<Uuid, SandboxInstance>>>,
    pub metrics: Arc<Metrics>,
}

impl SandboxManager {
    /// Build a manager around state loaded from the durable state directory.
    pub fn new(init: SandboxManagerInit) -> (Self, SandboxManagerResources) {
        let SandboxManagerInit {
            instances,
            spawners,
            active_backend,
            storage,
            state_store,
            rootfs_size,
            mem_size,
            template_catalog,
        } = init;
        let operation_locks = instances
            .keys()
            .copied()
            .map(|id| (id, Arc::new(AsyncMutex::new(()))))
            .collect();
        let instances = Arc::new(Mutex::new(instances));
        let backend_instances = Arc::new(Mutex::new(HashMap::new()));
        let metrics = Arc::new(Metrics::new());
        let resources = SandboxManagerResources {
            #[cfg(test)]
            instances: instances.clone(),
            metrics: metrics.clone(),
        };
        (
            Self {
                instances,
                backend_instances,
                operation_locks: Mutex::new(operation_locks),
                storage_sync_inflight: Arc::new(Mutex::new(HashSet::new())),
                // The periodic worker is sequential. Retain that bound when a
                // timed-out provider operation has to finish in the background.
                storage_sync_permits: Arc::new(Semaphore::new(1)),
                spawners: Arc::new(spawners),
                active_backend,
                storage,
                state_store,
                rootfs_size,
                mem_size,
                metrics,
                template_catalog,
            },
            resources,
        )
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

    pub(crate) fn backend_owner(&self, id: Uuid) -> Option<DynBackendInstance> {
        match self.backend_instances.lock() {
            Ok(instances) => instances.get(&id).cloned(),
            Err(poisoned) => poisoned.into_inner().get(&id).cloned(),
        }
    }

    #[cfg(test)]
    pub(crate) fn insert_backend_owner(&self, id: Uuid, owner: DynBackendInstance) -> Result<()> {
        self.backend_instances
            .lock()
            .map_err(|_| poisoned("backend_instances"))?
            .insert(id, owner);
        Ok(())
    }

    pub(super) async fn reconstruct_storage(&self, id: Uuid) -> Result<StorageSlot> {
        self.storage
            .reconstruct(&id.to_string())
            .await
            .map_err(Into::into)
    }

    pub(super) async fn sync_storage(&self, slot: &StorageSlot) -> Result<()> {
        self.storage.sync_artifacts(slot).await.map_err(Into::into)
    }

    /// Return all persisted sandbox metadata.
    pub fn list(&self) -> Result<Vec<SandboxInstance>> {
        Ok(self
            .instances
            .lock()
            .map_err(|_| poisoned("instances"))?
            .values()
            .cloned()
            .collect())
    }

    /// Return one persisted sandbox.
    pub fn get(&self, id: Uuid) -> Result<SandboxInstance> {
        self.instances
            .lock()
            .map_err(|_| poisoned("instances"))?
            .get(&id)
            .cloned()
            .ok_or_else(|| BlazeDaemonError::NotFound(format!("instance {id}")))
    }

    /// Execute one command through the running sandbox guest.
    pub async fn exec(
        &self,
        id: Uuid,
        command: String,
        cwd: Option<String>,
        env: Option<HashMap<String, String>>,
        timeout_secs: u32,
    ) -> Result<GuestExecResult> {
        let _operation = self.lock_running(id).await?;
        self.guest_client(id)?
            .exec(command, cwd, env, timeout_secs)
            .await
            .map_err(BlazeDaemonError::from)
    }

    /// Read one file through the running sandbox guest.
    pub async fn read_file(&self, id: Uuid, path: String) -> Result<Vec<u8>> {
        let _operation = self.lock_running(id).await?;
        self.guest_client(id)?
            .read_file(path)
            .await
            .map_err(BlazeDaemonError::from)
    }

    /// Replace one file through the running sandbox guest.
    pub async fn write_file(&self, id: Uuid, path: String, data: &[u8]) -> Result<()> {
        let _operation = self.lock_running(id).await?;
        self.guest_client(id)?
            .write_file(path, data)
            .await
            .map_err(BlazeDaemonError::from)
    }

    /// Create a sandbox from a fresh runtime allocation.
    pub async fn create(&self, request: CreateSandbox) -> Result<CreateSandboxResult> {
        let mut instance = SandboxInstance::new(
            request.runtime_backend,
            request.decision.workload_class,
            request.image_digest,
            request.decision.policy_name.clone(),
        );
        let operation_lock = self.operation_lock(instance.id);
        let _operation = operation_lock.lock().await;
        instance.transition(SandboxState::Creating)?;
        instance.begin_operation(OperationKind::Create);

        // Publish the stable identity and create intent before allocation.
        if let Err(error) = self.state_store.persist(&instance) {
            match self.state_store.has_run_dir_residual(instance.id) {
                Ok(true) => {}
                Ok(false) => return Err(error),
                Err(residual_error) => {
                    return Err(BlazeDaemonError::RecoveryRequired(format!(
                        "create {}: initial state publication failed: {error}; could not inspect \
                         publication residual: {residual_error}",
                        instance.id
                    )));
                }
            }
            let rollback_errors = self.commit_create_rollback(&mut instance);
            if rollback_errors.is_empty() {
                return Err(error);
            }
            return Err(BlazeDaemonError::RecoveryRequired(format!(
                "create {}: initial state publication failed: {error}; {}",
                instance.id,
                rollback_errors.join("; ")
            )));
        }
        if let Some(error) = self.retain_instance(instance.clone()) {
            return Err(BlazeDaemonError::RecoveryRequired(format!(
                "create {}: {error}",
                instance.id
            )));
        }

        let storage = match self
            .storage
            .acquire(&AcquireOpts {
                instance_id: instance.id.to_string(),
                rootfs_size: self.rootfs_size,
                mem_size: self.mem_size,
            })
            .await
        {
            Ok(storage) => storage,
            Err(error) => {
                let (source, residual) = error.into_parts();
                return Err(self.retain_failed_acquire(&mut instance, residual, source.into()));
            }
        };
        crate::failpoint::pause("create-after-storage-acquire").await;

        let work_dir = match self.state_store.run_dir(instance.id) {
            Ok(work_dir) => work_dir,
            Err(error) => {
                return Err(self
                    .cleanup_failed_create(&mut instance, storage, None, false, error)
                    .await);
            }
        };
        let spawner = match self.spawners.get(self.active_backend) {
            Some(spawner) => spawner,
            None => {
                return Err(self
                    .cleanup_failed_create(
                        &mut instance,
                        storage,
                        None,
                        false,
                        BlazeDaemonError::Internal(format!(
                            "active backend {} has no registered spawner",
                            self.active_backend
                        )),
                    )
                    .await);
            }
        };
        if let Err(error) = spawner.prepare_spawn(&work_dir).await {
            return Err(self
                .cleanup_failed_create(&mut instance, storage, None, false, error.into())
                .await);
        }

        instance.backend_ownership = BackendOwnership::Starting;
        if let Err(error) = self.state_store.persist(&instance) {
            instance.backend_ownership = BackendOwnership::NotStarted;
            return Err(self
                .cleanup_failed_create(&mut instance, storage, None, false, error)
                .await);
        }
        if let Some(error) = self.retain_instance(instance.clone()) {
            instance.backend_ownership = BackendOwnership::NotStarted;
            return Err(self
                .cleanup_failed_create(
                    &mut instance,
                    storage,
                    None,
                    false,
                    BlazeDaemonError::Internal(error),
                )
                .await);
        }

        let backend_request = match BackendSpawnRequest::new(
            SpawnRequest {
                instance_id: instance.id,
                binary_path: request.binary_path,
                storage: storage.clone(),
                backend: request.decision.backend,
                vm: request.decision.vm,
            },
            work_dir.clone(),
        ) {
            Ok(request) => request,
            Err(error) => {
                instance.backend_ownership = BackendOwnership::NotStarted;
                return Err(self
                    .cleanup_failed_create(&mut instance, storage, None, false, error.into())
                    .await);
            }
        };
        let spawn = match crate::failpoint::backend("create-spawn") {
            Ok(()) => spawn_with_runtime_directory(spawner.as_ref(), backend_request).await,
            Err(error) => Err(crate::spawner::SpawnFailure::clean(error)),
        };
        let actual_backend = match spawn {
            Ok(backend_instance) => {
                instance.backend_ownership = BackendOwnership::Running;
                let actual_backend = backend_instance.backend();
                if let Err(error) = self
                    .wait_for_guest_ready(&backend_instance, "create-guest-ready")
                    .await
                {
                    return Err(self
                        .cleanup_failed_create(
                            &mut instance,
                            storage,
                            Some(backend_instance),
                            false,
                            error.into(),
                        )
                        .await);
                }
                let mut backend_instance = Some(backend_instance);
                let registered = match self.backend_instances.lock() {
                    Ok(mut instances) => {
                        instances.insert(
                            instance.id,
                            backend_instance
                                .take()
                                .expect("backend instance is present"),
                        );
                        true
                    }
                    Err(_) => false,
                };
                if !registered {
                    return Err(self
                        .cleanup_failed_create(
                            &mut instance,
                            storage,
                            backend_instance,
                            false,
                            BlazeDaemonError::Internal(
                                "backend_instances lock poisoned".to_string(),
                            ),
                        )
                        .await);
                }
                actual_backend
            }
            Err(error) => {
                let (source, backend) = error.into_parts();
                instance.backend_ownership = if backend.is_some() {
                    BackendOwnership::Running
                } else {
                    BackendOwnership::Stopped
                };
                return Err(self
                    .cleanup_failed_create(&mut instance, storage, backend, false, source.into())
                    .await);
            }
        };

        if let Err(error) = instance.transition(SandboxState::Running) {
            return Err(self
                .cleanup_failed_create(&mut instance, storage, None, true, error.into())
                .await);
        }
        instance.finish_operation();
        if let Err(error) = crate::failpoint::state("create-state-commit")
            .and_then(|_| self.state_store.persist(&instance))
        {
            return Err(self
                .cleanup_failed_create(&mut instance, storage, None, true, error)
                .await);
        }
        if let Some(error) = self.retain_instance(instance.clone()) {
            return Err(self
                .cleanup_failed_create(
                    &mut instance,
                    storage,
                    None,
                    true,
                    BlazeDaemonError::Internal(error),
                )
                .await);
        }
        self.metrics.inc(&self.metrics.instances_created);
        Ok(CreateSandboxResult {
            instance,
            selected_backend: actual_backend,
        })
    }

    /// Idempotently destroy one sandbox and its owned runtime resources.
    pub async fn destroy(&self, id: Uuid) -> Result<bool> {
        let operation_lock = self.operation_lock(id);
        let _operation = operation_lock.lock().await;
        self.destroy_locked(id).await
    }

    async fn destroy_locked(&self, id: Uuid) -> Result<bool> {
        let mut original = self.get(id)?;
        if original.state == SandboxState::Destroyed {
            return Ok(false);
        }

        if original.operation.as_ref().map(|operation| operation.kind)
            != Some(OperationKind::Destroy)
        {
            original.begin_operation(OperationKind::Destroy);
        }
        if let Err(error) = crate::failpoint::state("destroy-intent-state-commit")
            .and_then(|_| self.state_store.persist(&original))
        {
            let _ = self.mark_recovery(id);
            return Err(BlazeDaemonError::RecoveryRequired(format!(
                "destroy {id}: intent persistence failed: {error}; resources retained"
            )));
        }
        if let Some(error) = self.retain_instance(original.clone()) {
            let _ = self.mark_recovery(id);
            return Err(BlazeDaemonError::RecoveryRequired(format!(
                "destroy {id}: {error}; resources retained"
            )));
        }

        let backend = self
            .backend_instances
            .lock()
            .map_err(|_| poisoned("backend_instances"))?
            .get(&id)
            .cloned();
        let stop_result = match crate::failpoint::backend("destroy-kill") {
            Ok(()) => {
                if let Some(backend) = backend.as_ref() {
                    backend.kill().await
                } else if matches!(
                    original.backend_ownership,
                    BackendOwnership::NotStarted | BackendOwnership::Stopped
                ) {
                    Ok(())
                } else {
                    match self.spawners.get(original.backend) {
                        Some(spawner) => match self.state_store.run_dir(id) {
                            Ok(run_dir) => spawner.cleanup_orphan(id, &run_dir).await,
                            Err(error) => Err(BlazeError::BackendError {
                                msg: format!(
                                    "open owned run directory for persisted instance {id}: {error}"
                                ),
                            }),
                        },
                        None => Err(BlazeError::BackendError {
                            msg: format!(
                                "no recovery spawner registered for persisted backend {}",
                                original.backend
                            ),
                        }),
                    }
                }
            }
            Err(error) => Err(error),
        };
        if let Err(error) = stop_result {
            let recovery = self.mark_recovery(id).err();
            return Err(BlazeDaemonError::RecoveryRequired(format!(
                "destroy {id}: backend termination failed: {error}; owner and storage retained{}",
                recovery
                    .map(|error| format!("; recovery state persistence failed: {error}"))
                    .unwrap_or_default()
            )));
        }

        original.backend_ownership = BackendOwnership::Stopped;
        if let Err(error) = crate::failpoint::state("destroy-stop-state-commit")
            .and_then(|_| self.state_store.persist(&original))
        {
            let recovery = self.mark_instance_recovery(original.clone()).err();
            return Err(BlazeDaemonError::RecoveryRequired(format!(
                "destroy {id}: backend stopped but stop state persistence failed: {error}; \
                 storage retained{}",
                recovery
                    .map(|error| format!("; recovery state persistence failed: {error}"))
                    .unwrap_or_default()
            )));
        }
        if let Some(error) = self.retain_instance(original.clone()) {
            let _ = self.mark_recovery(id);
            return Err(BlazeDaemonError::RecoveryRequired(format!(
                "destroy {id}: backend stopped but lifecycle retention failed: {error}; \
                 storage retained"
            )));
        }

        if let Err(error) = self.storage.release_by_id(&id.to_string()).await {
            let recovery = self.mark_recovery(id).err();
            return Err(BlazeDaemonError::RecoveryRequired(format!(
                "destroy {id}: backend stopped but storage release failed: {error}; \
                 lifecycle retained for retry{}",
                recovery
                    .map(|error| format!("; recovery state persistence failed: {error}"))
                    .unwrap_or_default()
            )));
        }

        let mut destroyed = original;
        if destroyed.state != SandboxState::Destroyed {
            destroyed.transition(SandboxState::Destroyed)?;
        }
        destroyed.finish_operation();
        if let Err(error) = crate::failpoint::state("destroy-final-state-commit")
            .and_then(|_| self.state_store.persist(&destroyed))
        {
            let recovery = self.mark_recovery(id).err();
            return Err(BlazeDaemonError::RecoveryRequired(format!(
                "destroy {id}: resources released but final state persistence failed: {error}{}",
                recovery
                    .map(|error| format!("; recovery state persistence failed: {error}"))
                    .unwrap_or_default()
            )));
        }
        let retention_error = self.retain_instance(destroyed);
        match self.backend_instances.lock() {
            Ok(mut instances) => {
                instances.remove(&id);
            }
            Err(poisoned) => {
                poisoned.into_inner().remove(&id);
            }
        }
        if let Some(error) = retention_error {
            return Err(BlazeDaemonError::RecoveryRequired(format!(
                "destroy {id}: resources released but {error}"
            )));
        }
        self.metrics.inc(&self.metrics.instances_destroyed);
        Ok(true)
    }

    /// Reconcile every non-terminal record without aborting on one failure.
    pub async fn reconcile_startup(&self) -> ReconcileReport {
        let ids = match self.instances.lock() {
            Ok(instances) => instances
                .values()
                .filter(|instance| instance.state != SandboxState::Destroyed)
                .map(|instance| instance.id)
                .collect::<Vec<_>>(),
            Err(poisoned) => poisoned
                .into_inner()
                .values()
                .filter(|instance| instance.state != SandboxState::Destroyed)
                .map(|instance| instance.id)
                .collect::<Vec<_>>(),
        };
        let mut report = ReconcileReport {
            attempted: ids.len(),
            ..ReconcileReport::default()
        };
        for id in ids {
            let operation_lock = self.operation_lock(id);
            let _operation = operation_lock.lock().await;
            match self.destroy_locked(id).await {
                Ok(_) => report.completed += 1,
                Err(error) => {
                    let recovery = self.mark_recovery(id).err();
                    report.failures.push(ReconcileFailure {
                        instance_id: id,
                        error: match recovery {
                            Some(recovery) => {
                                format!("{error}; recovery state persistence failed: {recovery}")
                            }
                            None => error.to_string(),
                        },
                    });
                }
            }
        }
        report
    }

    async fn lock_running(&self, id: Uuid) -> Result<OwnedMutexGuard<()>> {
        let operation = self.operation_lock(id).lock_owned().await;
        let instance = self.get(id)?;
        if instance.state != SandboxState::Running || instance.operation.is_some() {
            return Err(BlazeDaemonError::Conflict(format!(
                "instance {id} is not available for guest operations"
            )));
        }
        Ok(operation)
    }

    fn guest_client(&self, id: Uuid) -> Result<GuestClient> {
        let backend = self
            .backend_instances
            .lock()
            .map_err(|_| poisoned("backend_instances"))?
            .get(&id)
            .cloned()
            .ok_or_else(|| {
                BlazeDaemonError::Conflict(format!("instance {id} has no backend owner"))
            })?;
        let socket = backend.guest_socket_path();
        if socket.as_os_str().is_empty() {
            return Err(BlazeDaemonError::Conflict(format!(
                "instance {id} has no guest transport"
            )));
        }
        Ok(GuestClient::new(
            socket.to_path_buf(),
            GUEST_REQUEST_TIMEOUT,
            MAX_GUEST_FILE_BYTES,
        ))
    }

    async fn wait_for_guest_ready(
        &self,
        backend: &DynBackendInstance,
        failpoint: &str,
    ) -> crate::guest::Result<()> {
        let socket = backend.guest_socket_path();
        if socket.as_os_str().is_empty() {
            return Ok(());
        }
        crate::failpoint::guest(failpoint)?;
        GuestClient::new(
            socket.to_path_buf(),
            GUEST_REQUEST_TIMEOUT,
            MAX_GUEST_FILE_BYTES,
        )
        .wait_ready(GUEST_REQUEST_TIMEOUT, &CancellationToken::new())
        .await
    }

    async fn cleanup_failed_create(
        &self,
        instance: &mut SandboxInstance,
        storage: StorageSlot,
        backend: Option<DynBackendInstance>,
        registered: bool,
        original: BlazeDaemonError,
    ) -> BlazeDaemonError {
        if instance.operation.is_none() {
            instance.begin_operation(OperationKind::Create);
        }
        let mut cleanup_errors = Vec::new();
        let backend = if registered {
            match self.backend_instances.lock() {
                Ok(mut instances) => instances.remove(&instance.id),
                Err(poisoned) => poisoned.into_inner().remove(&instance.id),
            }
        } else {
            backend
        };
        let mut backend_stopped = matches!(
            instance.backend_ownership,
            BackendOwnership::NotStarted | BackendOwnership::Stopped
        );
        if registered && backend.is_none() {
            backend_stopped = false;
            cleanup_errors.push("registered backend owner is missing".to_string());
        }
        if let Some(backend) = backend.as_ref() {
            match backend.kill().await {
                Ok(()) => {
                    backend_stopped = true;
                    instance.backend_ownership = BackendOwnership::Stopped;
                }
                Err(error) => {
                    backend_stopped = false;
                    cleanup_errors.push(format!("backend termination failed: {error}"));
                }
            }
        }

        let mut storage_released = false;
        if backend_stopped {
            match self.storage.release(storage).await {
                Ok(()) => storage_released = true,
                Err(error) => cleanup_errors.push(format!("storage release failed: {error}")),
            }
        } else {
            cleanup_errors.push("storage retained until backend termination succeeds".to_string());
        }

        if backend_stopped && storage_released {
            cleanup_errors.extend(self.commit_create_rollback(instance));
            if cleanup_errors.is_empty() {
                self.metrics.inc(&self.metrics.instances_destroyed);
                return original;
            }
            return BlazeDaemonError::RecoveryRequired(format!(
                "{original}; cleanup completed but {}",
                cleanup_errors.join("; ")
            ));
        }

        if let Some(backend) = backend
            && let Some(error) = self.retain_backend(instance.id, backend)
        {
            cleanup_errors.push(error);
        }
        if instance.state != SandboxState::RecoveryRequired
            && let Err(error) = instance.transition(SandboxState::RecoveryRequired)
        {
            cleanup_errors.push(format!("recovery state update failed: {error}"));
        }
        if let Err(error) = self.state_store.persist(instance) {
            cleanup_errors.push(format!("state persistence failed: {error}"));
        }
        if let Some(error) = self.retain_instance(instance.clone()) {
            cleanup_errors.push(error);
        }
        BlazeDaemonError::RecoveryRequired(format!(
            "{original}; cleanup incomplete: {}",
            cleanup_errors.join("; ")
        ))
    }

    fn retain_failed_acquire(
        &self,
        instance: &mut SandboxInstance,
        residual: Option<StorageSlot>,
        original: BlazeDaemonError,
    ) -> BlazeDaemonError {
        if residual.is_some() {
            let mut errors = Vec::new();
            if instance.state != SandboxState::RecoveryRequired
                && let Err(error) = instance.transition(SandboxState::RecoveryRequired)
            {
                errors.push(format!("recovery state update failed: {error}"));
            }
            if let Err(error) = self.state_store.persist(instance) {
                errors.push(format!("state persistence failed: {error}"));
            }
            if let Some(error) = self.retain_instance(instance.clone()) {
                errors.push(error);
            }
            let suffix = if errors.is_empty() {
                "residual storage retained for destroy retry".to_string()
            } else {
                format!(
                    "residual storage retained with recovery errors: {}",
                    errors.join("; ")
                )
            };
            return BlazeDaemonError::RecoveryRequired(format!(
                "{original}; instance {}: {suffix}",
                instance.id
            ));
        }

        let errors = self.commit_create_rollback(instance);
        if errors.is_empty() {
            original
        } else {
            BlazeDaemonError::RecoveryRequired(format!(
                "{original}; acquire rollback completed but {}",
                errors.join("; ")
            ))
        }
    }

    /// Commit a fully compensated create as terminal without losing the
    /// operation record when that terminal commit itself fails.
    fn commit_create_rollback(&self, instance: &mut SandboxInstance) -> Vec<String> {
        let recoverable = instance.clone();
        let mut terminal = recoverable.clone();
        terminal.backend_ownership = BackendOwnership::Stopped;
        let terminal_result = (|| -> Result<()> {
            if terminal.state != SandboxState::Destroyed {
                terminal.transition(SandboxState::Destroyed)?;
            }
            terminal.finish_operation();
            crate::failpoint::state("create-rollback-final-state-commit")?;
            self.state_store.persist(&terminal)
        })();

        match terminal_result {
            Ok(()) => {
                *instance = terminal.clone();
                self.retain_instance(terminal).into_iter().collect()
            }
            Err(error) => {
                let mut errors = vec![format!("final state persistence failed: {error}")];
                let mut recovery = recoverable;
                recovery.backend_ownership = BackendOwnership::Stopped;
                if recovery.state != SandboxState::RecoveryRequired
                    && let Err(error) = recovery.transition(SandboxState::RecoveryRequired)
                {
                    errors.push(format!("recovery state update failed: {error}"));
                }
                if let Err(error) = self.state_store.persist(&recovery) {
                    errors.push(format!("recovery state persistence failed: {error}"));
                }
                if let Some(error) = self.retain_instance(recovery.clone()) {
                    errors.push(error);
                }
                *instance = recovery;
                errors
            }
        }
    }

    fn mark_recovery(&self, id: Uuid) -> Result<()> {
        self.mark_instance_recovery(self.get(id)?)
    }

    fn mark_instance_recovery(&self, mut instance: SandboxInstance) -> Result<()> {
        if instance.state != SandboxState::RecoveryRequired {
            instance.transition(SandboxState::RecoveryRequired)?;
        }
        let persist = self.state_store.persist(&instance);
        let retained = self.retain_instance(instance);
        match (persist, retained) {
            (Ok(()), None) => Ok(()),
            (Err(error), None) => Err(error),
            (Ok(()), Some(error)) => Err(BlazeDaemonError::Internal(error)),
            (Err(persist), Some(retain)) => Err(BlazeDaemonError::RecoveryRequired(format!(
                "recovery state persistence failed: {persist}; {retain}"
            ))),
        }
    }

    fn retain_backend(&self, id: Uuid, backend: DynBackendInstance) -> Option<String> {
        match self.backend_instances.lock() {
            Ok(mut instances) => {
                instances.insert(id, backend);
                None
            }
            Err(poisoned) => {
                poisoned.into_inner().insert(id, backend);
                Some("backend owner retained in poisoned runtime map".to_string())
            }
        }
    }

    fn retain_instance(&self, instance: SandboxInstance) -> Option<String> {
        match self.instances.lock() {
            Ok(mut instances) => {
                instances.insert(instance.id, instance);
                None
            }
            Err(poisoned) => {
                poisoned.into_inner().insert(instance.id, instance);
                Some("instance state retained in poisoned lifecycle map".to_string())
            }
        }
    }
}

fn poisoned(name: &str) -> BlazeDaemonError {
    BlazeDaemonError::Internal(format!("{name} lock poisoned"))
}
