// SPDX-License-Identifier: Apache-2.0
//! UDS HTTP API server.
//!
//! Routing is a hand-rolled `match` on `(method, path-segments)` rather
//! than a router framework — the surface is small (~17 endpoints) and
//! the cost of a fresh dependency outweighs the readability win.

use std::collections::HashMap;
use std::convert::Infallible;
use std::str::FromStr;
use std::sync::Arc;

use blaze_core::BlazeError;
use blaze_core::backend::{BackendKind, BackendStatus, SpawnRequest, select_backend};
use blaze_core::kernel::HookKind;
use blaze_core::lifecycle::{BackendOwnership, SandboxInstance, SandboxState, StartPath};
use blaze_core::policy::{ImageMetadata, RuntimeDecision, WorkloadClass, parse_duration};
use blaze_core::pool::{PoolConfig, PoolKey};
use blaze_core::storage::AcquireOpts;
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::CONTENT_TYPE;
use hyper::{Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::error::{BlazeDaemonError, Result};
use crate::state::ServerState;

/// Top-level request handler. Always returns `Ok(Response)`; internal
/// errors are turned into JSON error bodies so hyper never sees a panic.
pub async fn handle(
    req: Request<Incoming>,
    state: Arc<ServerState>,
) -> std::result::Result<Response<Full<Bytes>>, Infallible> {
    state.metrics.inc(&state.metrics.requests_total);

    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();

    let response = match collect_body(req).await {
        Ok(body) => dispatch(&method, &path, &query, body, &state).await,
        Err(e) => Err(e),
    };

    let resp = match response {
        Ok(r) => r,
        Err(e) => error_response(&e),
    };
    Ok(resp)
}

async fn collect_body(req: Request<Incoming>) -> Result<Vec<u8>> {
    let collected = req.into_body().collect().await?;
    Ok(collected.to_bytes().to_vec())
}

async fn dispatch(
    method: &Method,
    path: &str,
    _query: &str,
    body: Vec<u8>,
    state: &Arc<ServerState>,
) -> Result<Response<Full<Bytes>>> {
    let parts: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let m = method.as_str();

    match (m, parts.as_slice()) {
        ("GET", ["v1", "health"]) => health(state),
        ("GET", ["v1", "instances"]) => list_instances(state),
        ("POST", ["v1", "instances"]) => create_instance(state, &body).await,
        ("GET", ["v1", "instances", id]) => get_instance(state, id),
        ("POST", ["v1", "instances", id, "checkpoint"]) => checkpoint(state, id).await,
        ("POST", ["v1", "instances", id, "reset"]) => reset_instance(state, id).await,
        ("POST", ["v1", "instances", id, "destroy"]) => destroy_instance(state, id).await,
        ("GET", ["v1", "pools"]) => list_pools(state),
        ("GET", ["v1", "pools", backend, class]) => pool_status(state, backend, class),
        ("POST", ["v1", "pools", backend, class, "drain"]) => drain_pool(state, backend, class),
        ("PUT", ["v1", "pools", backend, class, "sizing"]) => {
            resize_pool(state, backend, class, &body)
        }
        ("POST", ["v1", "templates", "gc"]) => gc_templates(state),
        ("GET", ["v1", "templates"]) => list_templates(state),
        ("GET", ["v1", "templates", id]) => inspect_template(state, id),
        ("GET", ["v1", "policies"]) => list_policies(state),
        ("GET", ["v1", "hooks"]) => list_hooks(state),
        ("GET", ["v1", "metrics"]) => metrics(state),
        ("POST", ["v1", "admin", "reload"]) => admin_reload(state),
        _ => Err(BlazeDaemonError::NotFound(format!("{method} {path}"))),
    }
}

// ---------------------------------------------------------------------------
// Health / metrics / admin
// ---------------------------------------------------------------------------

fn health(state: &Arc<ServerState>) -> Result<Response<Full<Bytes>>> {
    let pool_status = state.storage.pool_status();
    json_ok(&json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "storage_pool": pool_status,
    }))
}

fn metrics(state: &Arc<ServerState>) -> Result<Response<Full<Bytes>>> {
    let body = state.metrics.render();
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/plain; version=0.0.4")
        .body(Full::new(Bytes::from(body)))?)
}

fn admin_reload(state: &Arc<ServerState>) -> Result<Response<Full<Bytes>>> {
    let policy_dir = {
        let cfg = state
            .config
            .lock()
            .map_err(|_| BlazeDaemonError::Internal("config lock poisoned".into()))?;
        cfg.policy.dir.clone()
    };
    let new_engine = blaze_core::policy::PolicyEngine::load_dir(&policy_dir)?;
    let count = new_engine.policies().len();
    {
        let mut engine = state
            .policy
            .lock()
            .map_err(|_| BlazeDaemonError::Internal("policy lock poisoned".into()))?;
        *engine = new_engine;
    }
    tracing::info!(policies = count, "policy engine reloaded");
    json_ok(&json!({ "reloaded": true, "policies": count }))
}

// ---------------------------------------------------------------------------
// Instances
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateInstanceReq {
    workload_class: WorkloadClass,
    image_digest: String,
    #[serde(default)]
    labels: HashMap<String, String>,
    #[serde(default)]
    kernel_version: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateInstanceResp {
    instance: SandboxInstance,
    decision: RuntimeDecision,
    start_path: StartPath,
    selected_backend: BackendKind,
}

fn list_instances(state: &Arc<ServerState>) -> Result<Response<Full<Bytes>>> {
    let map = state
        .instances
        .lock()
        .map_err(|_| BlazeDaemonError::Internal("instances lock poisoned".into()))?;
    let list: Vec<&SandboxInstance> = map.values().collect();
    json_ok(&list)
}

fn get_instance(state: &Arc<ServerState>, id: &str) -> Result<Response<Full<Bytes>>> {
    let uuid = parse_uuid(id)?;
    let map = state
        .instances
        .lock()
        .map_err(|_| BlazeDaemonError::Internal("instances lock poisoned".into()))?;
    let inst = map
        .get(&uuid)
        .ok_or_else(|| BlazeDaemonError::NotFound(format!("instance {uuid}")))?;
    json_ok(inst)
}

async fn create_instance(state: &Arc<ServerState>, body: &[u8]) -> Result<Response<Full<Bytes>>> {
    let req: CreateInstanceReq = serde_json::from_slice(body)
        .map_err(|e| BlazeDaemonError::BadRequest(format!("invalid create body: {e}")))?;

    let img = ImageMetadata {
        digest: req.image_digest.clone(),
        workload_class: Some(req.workload_class),
        kernel_version: req.kernel_version.clone(),
    };

    // 1. Policy evaluation.
    let decision = {
        let engine = state
            .policy
            .lock()
            .map_err(|_| BlazeDaemonError::Internal("policy lock poisoned".into()))?;
        match engine.evaluate(&req.labels, &img) {
            Ok(d) => d,
            Err(e) => {
                state.metrics.inc(&state.metrics.policy_eval_failures);
                return Err(e.into());
            }
        }
    };

    // 2. Backend selection. Constrain availability to the daemon's active
    // spawner — only the backend that was actually probed at boot can execute.
    let availability: Vec<BackendStatus> = {
        let cfg = state
            .config
            .lock()
            .map_err(|_| BlazeDaemonError::Internal("config lock poisoned".into()))?;
        decision
            .backend_priority
            .iter()
            .map(|kind| {
                let available = *kind == state.active_backend
                    && (state.active_backend == BackendKind::Mock
                        || cfg
                            .backends
                            .get(kind.as_str())
                            .map(|p| p.exists())
                            .unwrap_or(false));
                BackendStatus {
                    kind: *kind,
                    available,
                    version: None,
                }
            })
            .collect()
    };
    // Select backend from available options. If no match is found:
    // - Mock mode: fall back to the first policy entry (dev convenience)
    // - Real backend: propagate BackendUnavailable (policy does not permit
    //   the active backend, refusing to silently bypass policy)
    let policy_backend = match select_backend(&decision.backend_priority, &availability) {
        Ok(b) => b,
        Err(e) => {
            if state.active_backend == BackendKind::Mock {
                *decision.backend_priority.first().ok_or_else(|| {
                    BlazeDaemonError::Internal("policy has empty backend_priority".into())
                })?
            } else {
                return Err(e.into());
            }
        }
    };
    // Policy chooses an allowed backend preference. Runtime ownership must
    // record the spawner that actually serves the request. In portable Mock
    // mode those can differ because Mock is an explicit local fallback.
    let runtime_backend = if state.active_backend == BackendKind::Mock {
        BackendKind::Mock
    } else {
        policy_backend
    };

    let pool_key = PoolKey::new(
        runtime_backend,
        decision.workload_class,
        req.image_digest.clone(),
    );
    if decision.pool_eligible {
        if let Some((instance, selected_backend)) = activate_warm_instance(state, &pool_key).await?
        {
            state.metrics.inc(&state.metrics.pool_hits);
            return json_created(&CreateInstanceResp {
                start_path: instance.start_path,
                instance,
                decision,
                selected_backend,
            });
        }
        state.metrics.inc(&state.metrics.pool_misses);
    }

    let start_path = StartPath::Cold;
    let mut instance = SandboxInstance::new(
        runtime_backend,
        decision.workload_class,
        req.image_digest.clone(),
        start_path,
        decision.policy_name.clone(),
    );
    instance.transition(SandboxState::Creating)?;
    let operation_lock = state.operation_lock(instance.id);
    let _operation = operation_lock.lock().await;

    let (binary_path, rootfs_size, mem_size) = {
        let cfg = state
            .config
            .lock()
            .map_err(|_| BlazeDaemonError::Internal("config lock poisoned".into()))?;
        (
            cfg.backends
                .get(state.active_backend.as_str())
                .cloned()
                .unwrap_or_default(),
            cfg.storage.rootfs_size,
            cfg.storage.mem_size,
        )
    };
    // Publish ownership before allocation. A restart can now discover this
    // stable ID and release either an absent slot or a completed allocation.
    instance.persist(&state.state_dir)?;
    if let Some(error) = retain_instance_state(state, instance.clone()) {
        return Err(BlazeDaemonError::RecoveryRequired(format!(
            "create {}: {error}",
            instance.id
        )));
    }
    let storage = match state
        .storage
        .acquire(&AcquireOpts {
            instance_id: instance.id.to_string(),
            rootfs_size,
            mem_size,
        })
        .await
    {
        Ok(storage) => storage,
        Err(error) => {
            let (source, residual) = error.into_parts();
            return Err(retain_failed_acquire(
                state,
                &mut instance,
                residual,
                source.into(),
            ));
        }
    };
    crate::failpoint::pause("create-after-storage-acquire").await;

    instance.backend_ownership = BackendOwnership::Starting;
    if let Err(error) = instance.persist(&state.state_dir) {
        instance.backend_ownership = BackendOwnership::NotStarted;
        return Err(cleanup_failed_create(
            state,
            &mut instance,
            storage,
            None,
            false,
            error.into(),
        )
        .await);
    }
    if let Some(error) = retain_instance_state(state, instance.clone()) {
        instance.backend_ownership = BackendOwnership::NotStarted;
        return Err(cleanup_failed_create(
            state,
            &mut instance,
            storage,
            None,
            false,
            BlazeDaemonError::Internal(error),
        )
        .await);
    }

    let work_dir = state.state_dir.join(instance.id.to_string());
    let spawner = match state.spawner_for(state.active_backend) {
        Some(spawner) => spawner,
        None => {
            instance.backend_ownership = BackendOwnership::NotStarted;
            return Err(cleanup_failed_create(
                state,
                &mut instance,
                storage,
                None,
                false,
                BlazeDaemonError::Internal(format!(
                    "active backend {} has no registered spawner",
                    state.active_backend
                )),
            )
            .await);
        }
    };
    let spawn = match crate::failpoint::backend("create-spawn") {
        Ok(()) => {
            spawner
                .spawn(SpawnRequest {
                    instance_id: instance.id,
                    run_dir: work_dir,
                    binary_path,
                    storage: storage.clone(),
                    backend: decision.backend.clone(),
                    vm: decision.vm.clone(),
                })
                .await
        }
        Err(error) => Err(crate::spawner::SpawnFailure::clean(error)),
    };
    let actual_backend = match spawn {
        Ok(backend_instance) => {
            instance.backend_ownership = BackendOwnership::Running;
            let real_backend = backend_instance.backend();
            let mut backend_instance = Some(backend_instance);
            let registered = match state.backend_instances.lock() {
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
                return Err(cleanup_failed_create(
                    state,
                    &mut instance,
                    storage,
                    backend_instance,
                    false,
                    BlazeDaemonError::Internal("backend_instances lock poisoned".to_string()),
                )
                .await);
            }
            real_backend
        }
        Err(error) => {
            let (source, backend) = error.into_parts();
            instance.backend_ownership = if backend.is_some() {
                BackendOwnership::Running
            } else {
                BackendOwnership::Stopped
            };
            return Err(cleanup_failed_create(
                state,
                &mut instance,
                storage,
                backend,
                false,
                source.into(),
            )
            .await);
        }
    };
    if let Err(error) = instance.transition(SandboxState::Running) {
        return Err(
            cleanup_failed_create(state, &mut instance, storage, None, true, error.into()).await,
        );
    }
    if let Err(error) = crate::failpoint::state("create-state-commit")
        .and_then(|_| instance.persist(&state.state_dir).map_err(Into::into))
    {
        return Err(cleanup_failed_create(state, &mut instance, storage, None, true, error).await);
    }

    let inserted = match state.instances.lock() {
        Ok(mut instances) => {
            instances.insert(instance.id, instance.clone());
            true
        }
        Err(_) => false,
    };
    if !inserted {
        return Err(cleanup_failed_create(
            state,
            &mut instance,
            storage,
            None,
            true,
            BlazeDaemonError::Internal("instances lock poisoned".to_string()),
        )
        .await);
    }
    state.metrics.inc(&state.metrics.instances_created);

    json_created(&CreateInstanceResp {
        instance,
        decision,
        start_path,
        selected_backend: actual_backend,
    })
}

async fn activate_warm_instance(
    state: &Arc<ServerState>,
    key: &PoolKey,
) -> Result<Option<(SandboxInstance, BackendKind)>> {
    let candidate = state
        .pool
        .lock()
        .map_err(|_| BlazeDaemonError::Internal("pool lock poisoned".into()))?
        .lookup(key);
    let Some(id) = candidate else {
        return Ok(None);
    };
    let operation_lock = state.operation_lock(id);
    let _operation = operation_lock.lock().await;

    let instance = state
        .instances
        .lock()
        .map_err(|_| BlazeDaemonError::Internal("instances lock poisoned".into()))?
        .get(&id)
        .cloned();
    let backend = state
        .backend_instances
        .lock()
        .map_err(|_| BlazeDaemonError::Internal("backend_instances lock poisoned".into()))?
        .get(&id)
        .cloned();

    let invalid_reason = match (&instance, &backend) {
        (None, _) => Some("lifecycle metadata is missing".to_string()),
        (_, None) => Some("backend owner is missing".to_string()),
        (Some(instance), Some(_)) if instance.state != SandboxState::Warm => {
            Some(format!("lifecycle state is {}", instance.state))
        }
        (Some(instance), Some(backend)) if backend.backend() != instance.backend => Some(format!(
            "backend owner is {}, metadata is {}",
            backend.backend(),
            instance.backend
        )),
        (_, Some(backend)) => match backend.try_wait().await {
            Ok(None) => None,
            Ok(Some(status)) => Some(format!("backend exited: {status:?}")),
            Err(error) => Some(format!("backend liveness check failed: {error}")),
        },
    };

    if let Some(reason) = invalid_reason {
        quarantine_warm_instance(state, key, id, instance, backend, &reason).await;
        return Ok(None);
    }

    let original = instance.expect("validated warm metadata");
    match state.storage.reconstruct(&id.to_string()).await {
        Ok(_) => {}
        Err(error @ BlazeError::StorageIncomplete { .. }) => {
            quarantine_warm_instance(
                state,
                key,
                id,
                Some(original),
                backend,
                &format!("storage validation failed: {error}"),
            )
            .await;
            return Ok(None);
        }
        Err(error) => {
            return Err(restore_warm_claim(state, key, original, error.into()));
        }
    }

    crate::failpoint::pause("warm-before-state-commit").await;
    let mut instance = original.clone();
    let selected_backend = backend.expect("validated warm backend").backend();
    if let Err(error) = instance
        .transition(SandboxState::Creating)
        .and_then(|_| instance.transition(SandboxState::Running))
        .and_then(|_| instance.persist(&state.state_dir))
    {
        return Err(restore_warm_claim(state, key, original, error.into()));
    }
    state
        .instances
        .lock()
        .map_err(|_| BlazeDaemonError::Internal("instances lock poisoned".into()))?
        .insert(id, instance.clone());
    Ok(Some((instance, selected_backend)))
}

fn restore_warm_claim(
    state: &Arc<ServerState>,
    key: &PoolKey,
    instance: SandboxInstance,
    cause: BlazeDaemonError,
) -> BlazeDaemonError {
    let id = instance.id;
    let mut errors = Vec::new();
    if let Err(error) = instance.persist(&state.state_dir) {
        errors.push(format!("restore warm state persistence failed: {error}"));
    }
    if let Some(error) = retain_instance_state(state, instance) {
        errors.push(error);
    }
    match state.pool.lock() {
        Ok(mut pool) => pool.restore_lookup(key.clone(), id),
        Err(poisoned) => {
            poisoned.into_inner().restore_lookup(key.clone(), id);
            errors.push("pool lock poisoned while restoring warm claim".to_string());
        }
    }
    let details = if errors.is_empty() {
        "warm claim restored for retry".to_string()
    } else {
        format!("warm claim restored with errors: {}", errors.join("; "))
    };
    BlazeDaemonError::RecoveryRequired(format!("{cause}; instance {id}: {details}"))
}

async fn quarantine_warm_instance(
    state: &Arc<ServerState>,
    key: &PoolKey,
    id: Uuid,
    mut instance: Option<SandboxInstance>,
    backend: Option<crate::spawner::DynBackendInstance>,
    reason: &str,
) {
    match state.pool.lock() {
        Ok(mut pool) => pool.quarantine(key, id),
        Err(poisoned) => poisoned.into_inner().quarantine(key, id),
    }
    tracing::warn!(instance = %id, reason, "warm instance validation failed");

    let backend_stopped = match backend.as_ref() {
        Some(backend) => match backend.kill().await {
            Ok(()) => true,
            Err(error) => {
                tracing::error!(instance = %id, %error, "quarantined backend cleanup failed");
                false
            }
        },
        None => match instance.as_ref() {
            Some(instance)
                if matches!(
                    instance.backend_ownership,
                    BackendOwnership::NotStarted | BackendOwnership::Stopped
                ) =>
            {
                true
            }
            Some(instance) => match state.spawner_for(instance.backend) {
                Some(spawner) => match spawner
                    .cleanup_orphan(id, &state.state_dir.join(id.to_string()))
                    .await
                {
                    Ok(()) => true,
                    Err(error) => {
                        tracing::error!(
                            instance = %id,
                            %error,
                            "quarantined orphan cleanup failed"
                        );
                        false
                    }
                },
                None => {
                    tracing::error!(
                        instance = %id,
                        backend = %instance.backend,
                        "quarantined backend has no recovery spawner"
                    );
                    false
                }
            },
            None => false,
        },
    };
    if !backend_stopped {
        return;
    }
    let Some(instance) = instance.as_mut() else {
        tracing::error!(
            instance = %id,
            "quarantined lifecycle metadata missing; retaining storage"
        );
        return;
    };
    instance.backend_ownership = BackendOwnership::Stopped;
    if let Err(error) = instance.persist(&state.state_dir) {
        tracing::error!(instance = %id, %error, "quarantined stop state commit failed");
        return;
    }
    if let Some(error) = retain_instance_state(state, instance.clone()) {
        tracing::error!(instance = %id, %error, "quarantined stop state retention failed");
        return;
    }
    if let Err(error) = state.storage.release_by_id(&id.to_string()).await {
        tracing::error!(instance = %id, %error, "quarantined storage cleanup failed");
        return;
    }

    if instance.state != SandboxState::Destroyed
        && let Err(error) = instance.transition(SandboxState::Destroyed)
    {
        tracing::error!(instance = %id, %error, "quarantined lifecycle cleanup failed");
        return;
    }
    if let Err(error) = instance.persist(&state.state_dir) {
        tracing::error!(instance = %id, %error, "quarantined state commit failed");
        return;
    }
    match state.instances.lock() {
        Ok(mut instances) => {
            instances.insert(id, instance.clone());
        }
        Err(poisoned) => {
            poisoned.into_inner().insert(id, instance.clone());
        }
    }
    match state.backend_instances.lock() {
        Ok(mut instances) => {
            instances.remove(&id);
        }
        Err(poisoned) => {
            poisoned.into_inner().remove(&id);
        }
    }
}

async fn cleanup_failed_create(
    state: &Arc<ServerState>,
    instance: &mut SandboxInstance,
    storage: blaze_core::storage::StorageSlot,
    backend: Option<crate::spawner::DynBackendInstance>,
    registered: bool,
    original: BlazeDaemonError,
) -> BlazeDaemonError {
    let mut cleanup_errors = Vec::new();
    let backend = if registered {
        match state.backend_instances.lock() {
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
        match state.storage.release(storage).await {
            Ok(()) => storage_released = true,
            Err(error) => cleanup_errors.push(format!("storage release failed: {error}")),
        }
    } else {
        cleanup_errors.push("storage retained until backend termination succeeds".to_string());
    }

    if backend_stopped && storage_released {
        instance.backend_ownership = BackendOwnership::Stopped;
        if let Err(error) = instance.transition(SandboxState::Destroyed) {
            let mut recovery_errors = vec![format!("lifecycle update failed: {error}")];
            if let Err(persist_error) = instance.persist(&state.state_dir) {
                recovery_errors.push(format!("state persistence failed: {persist_error}"));
            }
            if let Some(retain_error) = retain_instance_state(state, instance.clone()) {
                recovery_errors.push(retain_error);
            }
            return BlazeDaemonError::RecoveryRequired(format!(
                "{original}; cleanup completed but {}",
                recovery_errors.join("; ")
            ));
        }
        if let Err(error) = instance.persist(&state.state_dir) {
            let mut recovery_errors = vec![format!("state persistence failed: {error}")];
            if let Some(retain_error) = retain_instance_state(state, instance.clone()) {
                recovery_errors.push(retain_error);
            }
            return BlazeDaemonError::RecoveryRequired(format!(
                "{original}; cleanup completed but {}",
                recovery_errors.join("; ")
            ));
        }
        if let Some(error) = retain_instance_state(state, instance.clone()) {
            return BlazeDaemonError::RecoveryRequired(format!(
                "{original}; cleanup completed but {error}"
            ));
        }
        state.metrics.inc(&state.metrics.instances_destroyed);
        return original;
    }

    if let Some(backend) = backend
        && let Some(error) = retain_backend_owner(state, instance.id, backend)
    {
        cleanup_errors.push(error);
    }
    if let Err(error) = instance.persist(&state.state_dir) {
        cleanup_errors.push(format!("state persistence failed: {error}"));
    }
    if let Some(error) = retain_instance_state(state, instance.clone()) {
        cleanup_errors.push(error);
    }
    BlazeDaemonError::RecoveryRequired(format!(
        "{original}; cleanup incomplete: {}",
        cleanup_errors.join("; ")
    ))
}

fn retain_failed_acquire(
    state: &Arc<ServerState>,
    instance: &mut SandboxInstance,
    residual: Option<blaze_core::storage::StorageSlot>,
    original: BlazeDaemonError,
) -> BlazeDaemonError {
    if residual.is_some() {
        let mut errors = Vec::new();
        if let Err(error) = instance.persist(&state.state_dir) {
            errors.push(format!("state persistence failed: {error}"));
        }
        if let Some(error) = retain_instance_state(state, instance.clone()) {
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

    let mut errors = Vec::new();
    instance.backend_ownership = BackendOwnership::Stopped;
    if let Err(error) = instance.transition(SandboxState::Destroyed) {
        errors.push(format!("lifecycle update failed: {error}"));
    }
    if let Err(error) = instance.persist(&state.state_dir) {
        errors.push(format!("state persistence failed: {error}"));
    }
    if let Some(error) = retain_instance_state(state, instance.clone()) {
        errors.push(error);
    }
    if errors.is_empty() {
        original
    } else {
        BlazeDaemonError::RecoveryRequired(format!(
            "{original}; acquire rollback completed but {}",
            errors.join("; ")
        ))
    }
}

fn retain_backend_owner(
    state: &Arc<ServerState>,
    id: Uuid,
    backend: crate::spawner::DynBackendInstance,
) -> Option<String> {
    match state.backend_instances.lock() {
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

fn retain_instance_state(state: &Arc<ServerState>, instance: SandboxInstance) -> Option<String> {
    match state.instances.lock() {
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

async fn checkpoint(state: &Arc<ServerState>, id: &str) -> Result<Response<Full<Bytes>>> {
    let uuid = parse_uuid(id)?;
    let operation_lock = state.operation_lock(uuid);
    let _operation = operation_lock.lock().await;
    let mut map = state
        .instances
        .lock()
        .map_err(|_| BlazeDaemonError::Internal("instances lock poisoned".into()))?;
    let inst = map
        .get_mut(&uuid)
        .ok_or_else(|| BlazeDaemonError::NotFound(format!("instance {uuid}")))?;

    if inst.state == SandboxState::Running {
        inst.transition(SandboxState::Paused)?;
    }
    inst.transition(SandboxState::Checkpointed)?;
    inst.persist(&state.state_dir)?;

    let checkpoint_id = format!("ckpt-{}-{}", inst.id, chrono::Utc::now().timestamp());
    json_ok(&json!({
        "checkpoint_id": checkpoint_id,
        "instance_id": inst.id,
    }))
}

async fn reset_instance(state: &Arc<ServerState>, id: &str) -> Result<Response<Full<Bytes>>> {
    let uuid = parse_uuid(id)?;
    let operation_lock = state.operation_lock(uuid);
    let _operation = operation_lock.lock().await;
    let mut map = state
        .instances
        .lock()
        .map_err(|_| BlazeDaemonError::Internal("instances lock poisoned".into()))?;
    let inst = map
        .get_mut(&uuid)
        .ok_or_else(|| BlazeDaemonError::NotFound(format!("instance {uuid}")))?;
    // TODO(v0.2): perform actual data-plane reset (full-recreate or
    // mm-template rollback per policy reset_mode) before returning to
    // pool. Current implementation is control-plane state only.
    inst.transition(SandboxState::Reset)?;
    inst.transition(SandboxState::Warm)?;
    inst.persist(&state.state_dir)?;

    // return to pool keyed on (backend, class, image_digest)
    let key = PoolKey::new(inst.backend, inst.workload_class, inst.image_digest.clone());
    let inst_id = inst.id;
    let snapshot = inst.clone();
    drop(map);
    {
        let mut pool = state
            .pool
            .lock()
            .map_err(|_| BlazeDaemonError::Internal("pool lock poisoned".into()))?;
        pool.return_to_pool(key, inst_id);
    }
    state.metrics.inc(&state.metrics.instances_resets);
    json_ok(&snapshot)
}

async fn destroy_instance(state: &Arc<ServerState>, id: &str) -> Result<Response<Full<Bytes>>> {
    let uuid = parse_uuid(id)?;
    let operation_lock = state.operation_lock(uuid);
    let _operation = operation_lock.lock().await;
    let mut original = state
        .instances
        .lock()
        .map_err(|_| BlazeDaemonError::Internal("instances lock poisoned".into()))?
        .get(&uuid)
        .cloned()
        .ok_or_else(|| BlazeDaemonError::NotFound(format!("instance {uuid}")))?;
    let backend = state
        .backend_instances
        .lock()
        .map_err(|_| BlazeDaemonError::Internal("backend_instances lock poisoned".into()))?
        .get(&uuid)
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
                match state.spawner_for(original.backend) {
                    Some(spawner) => {
                        spawner
                            .cleanup_orphan(uuid, &state.state_dir.join(uuid.to_string()))
                            .await
                    }
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
        return Err(BlazeDaemonError::RecoveryRequired(format!(
            "destroy {uuid}: backend termination failed: {error}; owner and storage retained"
        )));
    }

    original.backend_ownership = BackendOwnership::Stopped;
    if let Err(error) = original.persist(&state.state_dir) {
        return Err(BlazeDaemonError::RecoveryRequired(format!(
            "destroy {uuid}: backend stopped but stop state persistence failed: {error}; storage retained"
        )));
    }
    if let Some(error) = retain_instance_state(state, original.clone()) {
        return Err(BlazeDaemonError::RecoveryRequired(format!(
            "destroy {uuid}: backend stopped but lifecycle retention failed: {error}; storage retained"
        )));
    }

    if let Err(error) = state.storage.release_by_id(&uuid.to_string()).await {
        return Err(BlazeDaemonError::RecoveryRequired(format!(
            "destroy {uuid}: backend stopped but storage release failed: {error}; lifecycle retained for retry"
        )));
    }

    let mut destroyed = original;
    if destroyed.state != SandboxState::Destroyed {
        destroyed.transition(SandboxState::Destroyed)?;
    }
    destroyed.persist(&state.state_dir)?;
    state
        .instances
        .lock()
        .map_err(|_| BlazeDaemonError::Internal("instances lock poisoned".into()))?
        .insert(uuid, destroyed.clone());
    state
        .backend_instances
        .lock()
        .map_err(|_| BlazeDaemonError::Internal("backend_instances lock poisoned".into()))?
        .remove(&uuid);

    state.metrics.inc(&state.metrics.instances_destroyed);
    json_ok(&json!({
        "destroyed": true,
        "instance_id": destroyed.id,
    }))
}

// ---------------------------------------------------------------------------
// Pools
// ---------------------------------------------------------------------------

fn list_pools(state: &Arc<ServerState>) -> Result<Response<Full<Bytes>>> {
    let pool = state
        .pool
        .lock()
        .map_err(|_| BlazeDaemonError::Internal("pool lock poisoned".into()))?;
    let listed: Vec<_> = pool
        .list_pools()
        .into_iter()
        .map(|(k, s)| {
            json!({
                "key": {
                    "backend": k.backend.as_str(),
                    "workload_class": k.workload_class.as_str(),
                    "image_digest": k.image_digest,
                },
                "stats": s,
            })
        })
        .collect();
    json_ok(&listed)
}

fn pool_status(
    state: &Arc<ServerState>,
    backend: &str,
    class: &str,
) -> Result<Response<Full<Bytes>>> {
    let pool = state
        .pool
        .lock()
        .map_err(|_| BlazeDaemonError::Internal("pool lock poisoned".into()))?;
    let backend_kind = BackendKind::from_str(backend)
        .map_err(|e| BlazeDaemonError::BadRequest(format!("backend: {e}")))?;
    let class_kind = WorkloadClass::from_str(class)
        .map_err(|e| BlazeDaemonError::BadRequest(format!("class: {e}")))?;

    let listed: Vec<_> = pool
        .list_pools()
        .into_iter()
        .filter(|(k, _)| k.backend == backend_kind && k.workload_class == class_kind)
        .map(|(k, s)| {
            json!({
                "key": {
                    "backend": k.backend.as_str(),
                    "workload_class": k.workload_class.as_str(),
                    "image_digest": k.image_digest,
                },
                "stats": s,
            })
        })
        .collect();
    json_ok(&listed)
}

fn drain_pool(
    state: &Arc<ServerState>,
    backend: &str,
    class: &str,
) -> Result<Response<Full<Bytes>>> {
    let backend_kind = BackendKind::from_str(backend)
        .map_err(|e| BlazeDaemonError::BadRequest(format!("backend: {e}")))?;
    let class_kind = WorkloadClass::from_str(class)
        .map_err(|e| BlazeDaemonError::BadRequest(format!("class: {e}")))?;
    // TODO(v0.2): after removing instance IDs from the pool, walk
    // spawn_handles and kill the underlying processes so that drain
    // actually frees host resources.
    let drained = {
        let mut pool = state
            .pool
            .lock()
            .map_err(|_| BlazeDaemonError::Internal("pool lock poisoned".into()))?;
        pool.drain(backend_kind, class_kind)
    };
    json_ok(&json!({
        "drained": drained,
        "count": drained.len(),
    }))
}

#[derive(Debug, Deserialize)]
struct ResizeReq {
    #[serde(default)]
    enabled: Option<bool>,
    min: u32,
    target: u32,
    max: u32,
    #[serde(default)]
    image_digest: Option<String>,
    #[serde(default)]
    warm_ttl_secs: Option<u64>,
}

fn resize_pool(
    state: &Arc<ServerState>,
    backend: &str,
    class: &str,
    body: &[u8],
) -> Result<Response<Full<Bytes>>> {
    let req: ResizeReq = serde_json::from_slice(body)
        .map_err(|e| BlazeDaemonError::BadRequest(format!("invalid resize body: {e}")))?;
    let backend_kind = BackendKind::from_str(backend)
        .map_err(|e| BlazeDaemonError::BadRequest(format!("backend: {e}")))?;
    let class_kind = WorkloadClass::from_str(class)
        .map_err(|e| BlazeDaemonError::BadRequest(format!("class: {e}")))?;
    let key = PoolKey::new(
        backend_kind,
        class_kind,
        req.image_digest.clone().unwrap_or_default(),
    );
    let cfg = PoolConfig {
        enabled: req.enabled.unwrap_or(true),
        min: req.min,
        target: req.target,
        max: req.max,
        warm_ttl: std::time::Duration::from_secs(req.warm_ttl_secs.unwrap_or(30 * 60)),
        reset_mode: blaze_core::policy::ResetMode::default(),
    };
    {
        let mut pool = state
            .pool
            .lock()
            .map_err(|_| BlazeDaemonError::Internal("pool lock poisoned".into()))?;
        pool.resize(&key, cfg);
    }
    json_ok(&json!({
        "resized": true,
        "backend": backend,
        "class": class,
    }))
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

fn list_templates(state: &Arc<ServerState>) -> Result<Response<Full<Bytes>>> {
    let reg = state
        .template
        .lock()
        .map_err(|_| BlazeDaemonError::Internal("template lock poisoned".into()))?;
    json_ok(&reg.list())
}

fn inspect_template(state: &Arc<ServerState>, id: &str) -> Result<Response<Full<Bytes>>> {
    let uuid = parse_uuid(id)?;
    let reg = state
        .template
        .lock()
        .map_err(|_| BlazeDaemonError::Internal("template lock poisoned".into()))?;
    let view = reg
        .inspect(uuid)
        .ok_or_else(|| BlazeDaemonError::NotFound(format!("template {uuid}")))?;
    json_ok(&view)
}

fn gc_templates(state: &Arc<ServerState>) -> Result<Response<Full<Bytes>>> {
    let idle_ttl = {
        let cfg = state
            .config
            .lock()
            .map_err(|_| BlazeDaemonError::Internal("config lock poisoned".into()))?;
        parse_duration(&cfg.template.idle_ttl).unwrap_or(std::time::Duration::from_secs(3600))
    };
    let collected = {
        let mut reg = state
            .template
            .lock()
            .map_err(|_| BlazeDaemonError::Internal("template lock poisoned".into()))?;
        reg.gc_unused(idle_ttl)
    };
    json_ok(&json!({
        "collected": collected,
        "count": collected.len(),
    }))
}

// ---------------------------------------------------------------------------
// Policies / hooks
// ---------------------------------------------------------------------------

fn list_policies(state: &Arc<ServerState>) -> Result<Response<Full<Bytes>>> {
    let engine = state
        .policy
        .lock()
        .map_err(|_| BlazeDaemonError::Internal("policy lock poisoned".into()))?;
    let names: Vec<_> = engine
        .policies()
        .iter()
        .map(|p| {
            json!({
                "name": p.policy_name,
                "priority": p.priority,
                "workload_class": p.match_.workload_class.as_str(),
            })
        })
        .collect();
    json_ok(&names)
}

fn list_hooks(state: &Arc<ServerState>) -> Result<Response<Full<Bytes>>> {
    let reg = state
        .hook
        .lock()
        .map_err(|_| BlazeDaemonError::Internal("hook lock poisoned".into()))?;
    json_ok(&reg.list())
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn parse_uuid(s: &str) -> Result<Uuid> {
    Uuid::parse_str(s).map_err(|e| BlazeDaemonError::BadRequest(format!("invalid uuid: {e}")))
}

fn json_ok<T: Serialize>(value: &T) -> Result<Response<Full<Bytes>>> {
    json_response(StatusCode::OK, value)
}

fn json_created<T: Serialize>(value: &T) -> Result<Response<Full<Bytes>>> {
    json_response(StatusCode::CREATED, value)
}

fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Result<Response<Full<Bytes>>> {
    let body = serde_json::to_vec_pretty(value)?;
    Ok(Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(body)))?)
}

fn error_response(err: &BlazeDaemonError) -> Response<Full<Bytes>> {
    let status =
        StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = json!({
        "error": err.to_string(),
        "status": status.as_u16(),
    });
    let bytes = serde_json::to_vec_pretty(&body)
        .unwrap_or_else(|_| br#"{"error":"serialize_failed"}"#.to_vec());
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(bytes)))
        .unwrap_or_else(|_| {
            // Hyper's builder can fail on invalid header values; this branch
            // should be unreachable. Fall back to a status-only response.
            Response::new(Full::new(Bytes::from_static(b"{}")))
        })
}

// Keep the unused-import lint quiet when `HookKind` is gated behind
// future-only hook registration paths.
#[allow(dead_code)]
fn _hookkind_marker(_k: HookKind) {}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use async_trait::async_trait;
    use blaze_core::backend::BackendKind;
    use blaze_core::config::DaemonConfig;
    use blaze_core::kernel::HookRegistry;
    use blaze_core::policy::{
        BackendConfigs, FallbackOnMissingHook, PolicyEngine, PolicyFile, PolicyHooks, PolicyMatch,
        PolicyPool, PolicySelect, ResetMode, WorkloadClass,
    };
    use blaze_core::pool::PoolManager;
    use blaze_core::storage::{
        AcquireOpts, PoolStatus, StorageAcquireError, StorageProvider, StorageSlot,
    };
    use blaze_core::template::TemplateRegistry;

    use crate::file_provider::FileStorageProvider;
    use crate::spawner::{
        BackendInstance, BackendSpawner, DynBackendInstance, DynSpawner, MockSpawner, SpawnFailure,
        SpawnResult, SpawnerRegistry,
    };
    use crate::state::ServerState;

    use super::*;

    fn spawners(kind: BackendKind, spawner: DynSpawner) -> SpawnerRegistry {
        let mut registry = SpawnerRegistry::new();
        registry.insert(kind, spawner);
        registry
    }

    fn test_config(temp: &tempfile::TempDir) -> DaemonConfig {
        let mut config = DaemonConfig::default();
        config.daemon.state_dir = temp.path().join("state");
        config.storage.images_dir = temp.path().join("images");
        config.storage.instances_dir = temp.path().join("instances");
        std::fs::create_dir_all(&config.daemon.state_dir).expect("state");
        std::fs::create_dir_all(&config.storage.images_dir).expect("images");
        std::fs::create_dir_all(&config.storage.instances_dir).expect("instances");
        config
    }

    fn test_policy(kind: BackendKind, pooled: bool) -> PolicyFile {
        PolicyFile {
            manifest_version: 1,
            policy_name: "ownership-test".into(),
            priority: 100,
            match_: PolicyMatch {
                workload_class: WorkloadClass::AgentTool,
                image_labels: HashMap::new(),
            },
            select: PolicySelect {
                backend_priority: vec![kind],
                kernel_hooks: vec![],
                templates: vec![],
                fallback_on_missing_hook: FallbackOnMissingHook::default(),
            },
            pool: pooled.then_some(PolicyPool {
                enabled: true,
                min: 0,
                target: 0,
                max: 1,
                warm_ttl: "30m".into(),
                reset_mode: ResetMode::FullRecreate,
            }),
            checkpoint: None,
            quota: None,
            hooks: PolicyHooks::default(),
            backend: BackendConfigs::default(),
            vm: None,
        }
    }

    fn test_request() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "workload_class": "agent-tool",
            "image_digest": "sha256:ownership-test"
        }))
        .expect("request")
    }

    fn build_test_state(
        config: DaemonConfig,
        policy: PolicyFile,
        registry: SpawnerRegistry,
        active_backend: BackendKind,
        storage: Arc<dyn StorageProvider>,
    ) -> Arc<ServerState> {
        Arc::new(ServerState::build(
            config,
            PolicyEngine::with_policies(vec![policy]),
            PoolManager::new(),
            TemplateRegistry::new(),
            HookRegistry::new(),
            registry,
            active_backend,
            storage,
        ))
    }

    #[cfg(feature = "test-failpoints")]
    fn mock_state(temp: &tempfile::TempDir, pooled: bool) -> Arc<ServerState> {
        let config = test_config(temp);
        let storage: Arc<dyn StorageProvider> = Arc::new(FileStorageProvider::with_images(
            config.storage.images_dir.clone(),
            config.storage.instances_dir.clone(),
        ));
        build_test_state(
            config,
            test_policy(BackendKind::Mock, pooled),
            spawners(BackendKind::Mock, Arc::new(MockSpawner)),
            BackendKind::Mock,
            storage,
        )
    }

    async fn created_json(state: &Arc<ServerState>, request: &[u8]) -> serde_json::Value {
        let response = create_instance(state, request).await.expect("create");
        serde_json::from_slice(
            &response
                .into_body()
                .collect()
                .await
                .expect("body")
                .to_bytes(),
        )
        .expect("created json")
    }

    struct TransientReconstructStorage {
        inner: FileStorageProvider,
        fail_reconstruct: AtomicBool,
    }

    impl TransientReconstructStorage {
        fn new(images_dir: std::path::PathBuf, instances_dir: std::path::PathBuf) -> Self {
            Self {
                inner: FileStorageProvider::with_images(images_dir, instances_dir),
                fail_reconstruct: AtomicBool::new(false),
            }
        }
    }

    #[async_trait]
    impl StorageProvider for TransientReconstructStorage {
        async fn probe(&self) -> blaze_core::Result<bool> {
            self.inner.probe().await
        }

        async fn acquire(
            &self,
            opts: &AcquireOpts,
        ) -> std::result::Result<StorageSlot, StorageAcquireError> {
            self.inner.acquire(opts).await
        }

        async fn release(&self, slot: StorageSlot) -> blaze_core::Result<()> {
            self.inner.release(slot).await
        }

        async fn release_by_id(&self, instance_id: &str) -> blaze_core::Result<()> {
            self.inner.release_by_id(instance_id).await
        }

        async fn reconstruct(&self, instance_id: &str) -> blaze_core::Result<StorageSlot> {
            if self.fail_reconstruct.load(Ordering::Acquire) {
                return Err(BlazeError::StorageError {
                    msg: "transient reconstruct failure".into(),
                });
            }
            self.inner.reconstruct(instance_id).await
        }

        async fn flush_dirty(&self, slot: &StorageSlot) -> blaze_core::Result<()> {
            self.inner.flush_dirty(slot).await
        }

        fn pool_status(&self) -> PoolStatus {
            self.inner.pool_status()
        }

        async fn drain_pool(&self) -> blaze_core::Result<usize> {
            self.inner.drain_pool().await
        }
    }

    struct OwnershipObservingStorage {
        inner: FileStorageProvider,
        state_dir: PathBuf,
        observed: Arc<AtomicBool>,
    }

    #[async_trait]
    impl StorageProvider for OwnershipObservingStorage {
        async fn probe(&self) -> blaze_core::Result<bool> {
            self.inner.probe().await
        }

        async fn acquire(
            &self,
            opts: &AcquireOpts,
        ) -> std::result::Result<StorageSlot, StorageAcquireError> {
            let id = Uuid::parse_str(&opts.instance_id).expect("stable instance ID");
            let instance = SandboxInstance::load(&self.state_dir, id).expect("ownership published");
            assert_eq!(instance.state, SandboxState::Creating);
            assert_eq!(instance.backend_ownership, BackendOwnership::NotStarted);
            self.observed.store(true, Ordering::Release);
            self.inner.acquire(opts).await
        }

        async fn release(&self, slot: StorageSlot) -> blaze_core::Result<()> {
            self.inner.release(slot).await
        }

        async fn release_by_id(&self, instance_id: &str) -> blaze_core::Result<()> {
            self.inner.release_by_id(instance_id).await
        }

        async fn reconstruct(&self, instance_id: &str) -> blaze_core::Result<StorageSlot> {
            self.inner.reconstruct(instance_id).await
        }

        async fn flush_dirty(&self, slot: &StorageSlot) -> blaze_core::Result<()> {
            self.inner.flush_dirty(slot).await
        }

        fn pool_status(&self) -> PoolStatus {
            self.inner.pool_status()
        }

        async fn drain_pool(&self) -> blaze_core::Result<usize> {
            self.inner.drain_pool().await
        }
    }

    struct FailOnceOwner {
        instance_id: Uuid,
        attempts: AtomicUsize,
    }

    #[async_trait]
    impl BackendInstance for FailOnceOwner {
        fn backend(&self) -> BackendKind {
            BackendKind::Mock
        }

        async fn try_wait(&self) -> blaze_core::Result<Option<SpawnResult>> {
            Ok(None)
        }

        async fn kill(&self) -> blaze_core::Result<()> {
            if self.attempts.fetch_add(1, Ordering::AcqRel) == 0 {
                return Err(BlazeError::BackendError {
                    msg: format!("instance {} termination deferred", self.instance_id),
                });
            }
            Ok(())
        }
    }

    struct PartialSpawnSpawner;

    #[async_trait]
    impl BackendSpawner for PartialSpawnSpawner {
        async fn spawn(
            &self,
            request: SpawnRequest,
        ) -> std::result::Result<DynBackendInstance, SpawnFailure> {
            let owner: DynBackendInstance = Arc::new(FailOnceOwner {
                instance_id: request.instance_id,
                attempts: AtomicUsize::new(0),
            });
            Err(SpawnFailure::with_owner(
                BlazeError::BackendError {
                    msg: "backend readiness failed".into(),
                },
                owner,
            ))
        }

        async fn probe(&self, _binary_path: &Path) -> blaze_core::Result<bool> {
            Ok(true)
        }

        async fn cleanup_orphan(
            &self,
            _instance_id: Uuid,
            _run_dir: &Path,
        ) -> blaze_core::Result<()> {
            Err(BlazeError::BackendError {
                msg: "partial owner must remain registered".into(),
            })
        }
    }

    struct RecordingSpawner {
        cleanup_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl BackendSpawner for RecordingSpawner {
        async fn spawn(
            &self,
            _request: SpawnRequest,
        ) -> std::result::Result<DynBackendInstance, SpawnFailure> {
            Err(SpawnFailure::clean(BlazeError::BackendError {
                msg: "spawn not used".into(),
            }))
        }

        async fn probe(&self, _binary_path: &Path) -> blaze_core::Result<bool> {
            Ok(true)
        }

        async fn cleanup_orphan(
            &self,
            _instance_id: Uuid,
            _run_dir: &Path,
        ) -> blaze_core::Result<()> {
            self.cleanup_count.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    /// When multiple backend binaries exist on disk but the daemon probed
    /// Firecracker at boot, only Firecracker should be reported available
    /// and selected — even if policy prioritizes bubblewrap higher.
    #[tokio::test]
    async fn availability_constrained_to_active_backend() {
        // Create temp files to simulate both binaries existing.
        let tmp = std::env::temp_dir().join("blaze-test-active-backend");
        let _ = std::fs::create_dir_all(&tmp);
        let fc_bin = tmp.join("firecracker");
        let bwrap_bin = tmp.join("bwrap");
        std::fs::write(&fc_bin, b"fake-fc").unwrap();
        std::fs::write(&bwrap_bin, b"fake-bwrap").unwrap();

        // Minimal config with both backends present.
        let mut config = DaemonConfig::default();
        config.daemon.state_dir = tmp.join("state");
        let _ = std::fs::create_dir_all(&config.daemon.state_dir);
        config.backends.insert("firecracker".into(), fc_bin.clone());
        config
            .backends
            .insert("bubblewrap".into(), bwrap_bin.clone());

        // Policy that prioritizes bubblewrap over firecracker.
        let policy_file = PolicyFile {
            manifest_version: 1,
            policy_name: "test-multi-backend".into(),
            priority: 100,
            match_: PolicyMatch {
                workload_class: WorkloadClass::AgentRl,
                image_labels: HashMap::new(),
            },
            select: PolicySelect {
                backend_priority: vec![BackendKind::Bubblewrap, BackendKind::Firecracker],
                kernel_hooks: vec![],
                templates: vec![],
                fallback_on_missing_hook: FallbackOnMissingHook::default(),
            },
            pool: None,
            checkpoint: None,
            quota: None,
            hooks: PolicyHooks::default(),
            backend: BackendConfigs::default(),
            vm: None,
        };
        let engine = PolicyEngine::with_policies(vec![policy_file]);

        // Build state with active_backend = Firecracker (simulating probe
        // selected FC at boot) but using MockSpawner for test portability.
        let spawner: DynSpawner = Arc::new(MockSpawner);
        let storage_dir = tmp.join("storage");
        let _ = std::fs::create_dir_all(&storage_dir);
        let storage: Arc<dyn blaze_core::storage::StorageProvider> =
            Arc::new(FileStorageProvider::new(storage_dir));
        let state = Arc::new(ServerState::build(
            config,
            engine,
            PoolManager::new(),
            TemplateRegistry::new(),
            HookRegistry::new(),
            spawners(BackendKind::Firecracker, spawner),
            BackendKind::Firecracker,
            storage,
        ));

        // Create instance request for AgentRl workload.
        let req_body = serde_json::to_vec(&serde_json::json!({
            "workload_class": "agent-rl",
            "image_digest": "sha256:abc123",
        }))
        .unwrap();

        let resp = create_instance(&state, &req_body).await.unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let resp_json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // The instance should be created with backend = firecracker,
        // NOT bubblewrap (even though bwrap was higher priority in policy)
        // because only the active backend is reported as available.
        assert_eq!(
            resp_json["instance"]["backend"].as_str().unwrap(),
            "firecracker",
            "instance backend should be the active backend (firecracker), \
             not the higher-priority bubblewrap"
        );

        // Cleanup.
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn warm_claim_validates_runtime_and_quarantines_dead_owner() {
        let temp = tempfile::tempdir().expect("temp");
        let mut config = DaemonConfig::default();
        config.daemon.state_dir = temp.path().join("state");
        config.storage.images_dir = temp.path().join("images");
        config.storage.instances_dir = temp.path().join("instances");
        std::fs::create_dir_all(&config.daemon.state_dir).expect("state");
        std::fs::create_dir_all(&config.storage.images_dir).expect("images");
        std::fs::create_dir_all(&config.storage.instances_dir).expect("instances");

        let policy = PolicyFile {
            manifest_version: 1,
            policy_name: "warm-validation".into(),
            priority: 100,
            match_: PolicyMatch {
                workload_class: WorkloadClass::AgentTool,
                image_labels: HashMap::new(),
            },
            select: PolicySelect {
                backend_priority: vec![BackendKind::Mock],
                kernel_hooks: vec![],
                templates: vec![],
                fallback_on_missing_hook: FallbackOnMissingHook::default(),
            },
            pool: Some(PolicyPool {
                enabled: true,
                min: 0,
                target: 0,
                max: 1,
                warm_ttl: "30m".into(),
                reset_mode: ResetMode::FullRecreate,
            }),
            checkpoint: None,
            quota: None,
            hooks: PolicyHooks::default(),
            backend: BackendConfigs::default(),
            vm: None,
        };
        let storage: Arc<dyn blaze_core::storage::StorageProvider> =
            Arc::new(FileStorageProvider::with_images(
                config.storage.images_dir.clone(),
                config.storage.instances_dir.clone(),
            ));
        let state = Arc::new(ServerState::build(
            config,
            PolicyEngine::with_policies(vec![policy]),
            PoolManager::new(),
            TemplateRegistry::new(),
            HookRegistry::new(),
            spawners(BackendKind::Mock, Arc::new(MockSpawner)),
            BackendKind::Mock,
            storage,
        ));
        let request = serde_json::to_vec(&json!({
            "workload_class": "agent-tool",
            "image_digest": "sha256:warm-validation"
        }))
        .expect("request");

        let cold = create_instance(&state, &request)
            .await
            .expect("cold create");
        let cold: serde_json::Value =
            serde_json::from_slice(&cold.into_body().collect().await.expect("body").to_bytes())
                .expect("cold json");
        let id = cold["instance"]["id"].as_str().expect("id").to_string();
        reset_instance(&state, &id).await.expect("return to pool");

        let warm = create_instance(&state, &request)
            .await
            .expect("warm create");
        let warm: serde_json::Value =
            serde_json::from_slice(&warm.into_body().collect().await.expect("body").to_bytes())
                .expect("warm json");
        assert_eq!(warm["instance"]["id"], id);
        assert_eq!(warm["start_path"], "warm");

        reset_instance(&state, &id)
            .await
            .expect("return live owner");
        let owner = state
            .backend_instances
            .lock()
            .expect("owners")
            .get(&Uuid::parse_str(&id).expect("uuid"))
            .cloned()
            .expect("owner");
        owner.kill().await.expect("simulate backend exit");

        let replacement = create_instance(&state, &request)
            .await
            .expect("cold fallback");
        let replacement: serde_json::Value = serde_json::from_slice(
            &replacement
                .into_body()
                .collect()
                .await
                .expect("body")
                .to_bytes(),
        )
        .expect("replacement json");
        assert_ne!(replacement["instance"]["id"], id);
        assert_eq!(replacement["start_path"], "cold");
        let key = PoolKey::new(
            BackendKind::Mock,
            WorkloadClass::AgentTool,
            "sha256:warm-validation".into(),
        );
        assert_eq!(
            state
                .pool
                .lock()
                .expect("pool")
                .stats(&key)
                .quarantine_count,
            1
        );
    }

    #[tokio::test]
    async fn create_publishes_ownership_before_provider_acquire() {
        let temp = tempfile::tempdir().expect("temp");
        let config = test_config(&temp);
        let observed = Arc::new(AtomicBool::new(false));
        let storage: Arc<dyn StorageProvider> = Arc::new(OwnershipObservingStorage {
            inner: FileStorageProvider::with_images(
                config.storage.images_dir.clone(),
                config.storage.instances_dir.clone(),
            ),
            state_dir: config.daemon.state_dir.clone(),
            observed: observed.clone(),
        });
        let state = build_test_state(
            config,
            test_policy(BackendKind::Mock, false),
            spawners(BackendKind::Mock, Arc::new(MockSpawner)),
            BackendKind::Mock,
            storage,
        );

        created_json(&state, &test_request()).await;
        assert!(observed.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn mock_fallback_uses_runtime_backend_for_warm_reuse() {
        let temp = tempfile::tempdir().expect("temp");
        let config = test_config(&temp);
        let storage: Arc<dyn StorageProvider> = Arc::new(FileStorageProvider::with_images(
            config.storage.images_dir.clone(),
            config.storage.instances_dir.clone(),
        ));
        let state = build_test_state(
            config,
            test_policy(BackendKind::Firecracker, true),
            spawners(BackendKind::Mock, Arc::new(MockSpawner)),
            BackendKind::Mock,
            storage,
        );
        let request = test_request();

        let cold = created_json(&state, &request).await;
        let id = cold["instance"]["id"].as_str().expect("id").to_string();
        assert_eq!(cold["instance"]["backend"], "mock");
        assert_eq!(cold["selected_backend"], "mock");

        reset_instance(&state, &id).await.expect("return to pool");
        let warm = created_json(&state, &request).await;
        assert_eq!(warm["instance"]["id"], id);
        assert_eq!(warm["instance"]["backend"], "mock");
        assert_eq!(warm["selected_backend"], "mock");
        assert_eq!(warm["start_path"], "warm");
    }

    #[tokio::test]
    async fn partial_spawn_failure_retains_owner_and_storage_for_destroy() {
        let temp = tempfile::tempdir().expect("temp");
        let config = test_config(&temp);
        let instances_dir = config.storage.instances_dir.clone();
        let storage: Arc<dyn StorageProvider> = Arc::new(FileStorageProvider::with_images(
            config.storage.images_dir.clone(),
            instances_dir.clone(),
        ));
        let state = build_test_state(
            config,
            test_policy(BackendKind::Mock, false),
            spawners(BackendKind::Mock, Arc::new(PartialSpawnSpawner)),
            BackendKind::Mock,
            storage,
        );

        let error = create_instance(&state, &test_request())
            .await
            .expect_err("partial spawn must require recovery");
        assert!(matches!(error, BlazeDaemonError::RecoveryRequired(_)));
        let instance = state
            .instances
            .lock()
            .expect("instances")
            .values()
            .next()
            .cloned()
            .expect("retained lifecycle");
        assert_eq!(instance.backend_ownership, BackendOwnership::Running);
        assert!(instances_dir.join(instance.id.to_string()).is_dir());
        assert!(
            state
                .backend_instances
                .lock()
                .expect("owners")
                .contains_key(&instance.id)
        );

        destroy_instance(&state, &instance.id.to_string())
            .await
            .expect("retry destroy");
        assert!(!instances_dir.join(instance.id.to_string()).exists());
        assert_eq!(
            state.instances.lock().expect("instances")[&instance.id].state,
            SandboxState::Destroyed
        );
    }

    #[tokio::test]
    async fn restart_destroy_uses_the_persisted_backend_spawner() {
        let temp = tempfile::tempdir().expect("temp");
        let config = test_config(&temp);
        let storage = Arc::new(FileStorageProvider::with_images(
            config.storage.images_dir.clone(),
            config.storage.instances_dir.clone(),
        ));
        let mut instance = SandboxInstance::new(
            BackendKind::Bubblewrap,
            WorkloadClass::AgentTool,
            "sha256:recovery".into(),
            StartPath::Cold,
            "recovery-test".into(),
        );
        instance
            .transition(SandboxState::Creating)
            .expect("creating");
        instance.transition(SandboxState::Running).expect("running");
        instance.backend_ownership = BackendOwnership::Running;
        instance.persist(&config.daemon.state_dir).expect("persist");
        storage
            .acquire(&AcquireOpts {
                instance_id: instance.id.to_string(),
                rootfs_size: 4096,
                mem_size: 4096,
            })
            .await
            .expect("storage");

        let active_cleanups = Arc::new(AtomicUsize::new(0));
        let persisted_cleanups = Arc::new(AtomicUsize::new(0));
        let mut registry = SpawnerRegistry::new();
        registry.insert(
            BackendKind::Mock,
            Arc::new(RecordingSpawner {
                cleanup_count: active_cleanups.clone(),
            }),
        );
        registry.insert(
            BackendKind::Bubblewrap,
            Arc::new(RecordingSpawner {
                cleanup_count: persisted_cleanups.clone(),
            }),
        );
        let state = build_test_state(
            config,
            test_policy(BackendKind::Mock, false),
            registry,
            BackendKind::Mock,
            storage,
        );

        destroy_instance(&state, &instance.id.to_string())
            .await
            .expect("destroy recovered instance");
        assert_eq!(persisted_cleanups.load(Ordering::Acquire), 1);
        assert_eq!(active_cleanups.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn mock_fallback_restart_destroy_uses_mock_spawner() {
        let temp = tempfile::tempdir().expect("temp");
        let config = test_config(&temp);
        let instances_dir = config.storage.instances_dir.clone();
        let initial_storage: Arc<dyn StorageProvider> = Arc::new(FileStorageProvider::with_images(
            config.storage.images_dir.clone(),
            instances_dir.clone(),
        ));
        let initial_state = build_test_state(
            config.clone(),
            test_policy(BackendKind::Firecracker, false),
            spawners(BackendKind::Mock, Arc::new(MockSpawner)),
            BackendKind::Mock,
            initial_storage,
        );
        let created = created_json(&initial_state, &test_request()).await;
        let id = created["instance"]["id"].as_str().expect("id").to_string();
        assert_eq!(created["instance"]["backend"], "mock");
        drop(initial_state);

        let mock_cleanups = Arc::new(AtomicUsize::new(0));
        let policy_cleanups = Arc::new(AtomicUsize::new(0));
        let mut registry = SpawnerRegistry::new();
        registry.insert(
            BackendKind::Mock,
            Arc::new(RecordingSpawner {
                cleanup_count: mock_cleanups.clone(),
            }),
        );
        registry.insert(
            BackendKind::Firecracker,
            Arc::new(RecordingSpawner {
                cleanup_count: policy_cleanups.clone(),
            }),
        );
        let restarted_storage: Arc<dyn StorageProvider> =
            Arc::new(FileStorageProvider::with_images(
                config.storage.images_dir.clone(),
                instances_dir.clone(),
            ));
        let restarted = build_test_state(
            config,
            test_policy(BackendKind::Firecracker, false),
            registry,
            BackendKind::Mock,
            restarted_storage,
        );

        destroy_instance(&restarted, &id)
            .await
            .expect("destroy recovered mock instance");
        assert_eq!(mock_cleanups.load(Ordering::Acquire), 1);
        assert_eq!(policy_cleanups.load(Ordering::Acquire), 0);
        assert!(!instances_dir.join(id).exists());
    }

    #[tokio::test]
    async fn write_ahead_create_without_slot_is_destroyable_after_restart() {
        let temp = tempfile::tempdir().expect("temp");
        let config = test_config(&temp);
        let instances_dir = config.storage.instances_dir.clone();
        let mut instance = SandboxInstance::new(
            BackendKind::Mock,
            WorkloadClass::AgentTool,
            "sha256:write-ahead".into(),
            StartPath::Cold,
            "write-ahead-test".into(),
        );
        instance
            .transition(SandboxState::Creating)
            .expect("creating");
        instance
            .persist(&config.daemon.state_dir)
            .expect("write-ahead state");
        let id = instance.id;
        assert!(!instances_dir.join(id.to_string()).exists());

        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let storage: Arc<dyn StorageProvider> = Arc::new(FileStorageProvider::with_images(
            config.storage.images_dir.clone(),
            instances_dir.clone(),
        ));
        let restarted = build_test_state(
            config,
            test_policy(BackendKind::Mock, false),
            spawners(
                BackendKind::Mock,
                Arc::new(RecordingSpawner {
                    cleanup_count: cleanup_count.clone(),
                }),
            ),
            BackendKind::Mock,
            storage,
        );

        destroy_instance(&restarted, &id.to_string())
            .await
            .expect("destroy state without slot");
        assert_eq!(cleanup_count.load(Ordering::Acquire), 0);
        assert_eq!(
            restarted.instances.lock().expect("instances")[&id].state,
            SandboxState::Destroyed
        );
        assert!(!instances_dir.join(id.to_string()).exists());
    }

    #[tokio::test]
    async fn warm_reconstruct_restores_transient_failure_for_retry() {
        let temp = tempfile::tempdir().expect("temp");
        let config = test_config(&temp);
        let storage = Arc::new(TransientReconstructStorage::new(
            config.storage.images_dir.clone(),
            config.storage.instances_dir.clone(),
        ));
        let state = build_test_state(
            config,
            test_policy(BackendKind::Mock, true),
            spawners(BackendKind::Mock, Arc::new(MockSpawner)),
            BackendKind::Mock,
            storage.clone(),
        );
        let request = test_request();
        let cold = created_json(&state, &request).await;
        let id = cold["instance"]["id"].as_str().expect("id").to_string();
        reset_instance(&state, &id).await.expect("warm");

        storage.fail_reconstruct.store(true, Ordering::Release);
        let error = create_instance(&state, &request)
            .await
            .expect_err("transient error must preserve claim");
        assert!(matches!(error, BlazeDaemonError::RecoveryRequired(_)));
        let uuid = Uuid::parse_str(&id).expect("uuid");
        assert_eq!(
            state.instances.lock().expect("instances")[&uuid].state,
            SandboxState::Warm
        );
        let key = PoolKey::new(
            BackendKind::Mock,
            WorkloadClass::AgentTool,
            "sha256:ownership-test".into(),
        );
        assert_eq!(state.pool.lock().expect("pool").stats(&key).warm_count, 1);

        storage.fail_reconstruct.store(false, Ordering::Release);
        let retried = created_json(&state, &request).await;
        assert_eq!(retried["instance"]["id"], id);
        assert_eq!(retried["start_path"], "warm");
    }

    #[tokio::test]
    async fn warm_reconstruct_quarantines_an_incomplete_slot() {
        let temp = tempfile::tempdir().expect("temp");
        let config = test_config(&temp);
        let instances_dir = config.storage.instances_dir.clone();
        let storage: Arc<dyn StorageProvider> = Arc::new(FileStorageProvider::with_images(
            config.storage.images_dir.clone(),
            instances_dir.clone(),
        ));
        let state = build_test_state(
            config,
            test_policy(BackendKind::Mock, true),
            spawners(BackendKind::Mock, Arc::new(MockSpawner)),
            BackendKind::Mock,
            storage,
        );
        let request = test_request();
        let cold = created_json(&state, &request).await;
        let id = cold["instance"]["id"].as_str().expect("id").to_string();
        reset_instance(&state, &id).await.expect("warm");
        std::fs::remove_file(instances_dir.join(&id).join("mem.bin")).expect("remove artifact");

        let replacement = created_json(&state, &request).await;
        assert_ne!(replacement["instance"]["id"], id);
        assert_eq!(replacement["start_path"], "cold");
        let uuid = Uuid::parse_str(&id).expect("uuid");
        assert_eq!(
            state.instances.lock().expect("instances")[&uuid].state,
            SandboxState::Destroyed
        );
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn failure_hooks_drive_create_and_destroy_compensation() {
        let request = test_request();

        let spawn_temp = tempfile::tempdir().expect("temp");
        let spawn_state = mock_state(&spawn_temp, false);
        let spawn_hook = crate::failpoint::TestFailpoint::new(&["create-spawn"]);
        spawn_hook
            .run(create_instance(&spawn_state, &request))
            .await
            .expect_err("spawn failure");
        let spawn_instance = spawn_state
            .instances
            .lock()
            .expect("instances")
            .values()
            .next()
            .cloned()
            .expect("destroyed create");
        assert_eq!(spawn_instance.state, SandboxState::Destroyed);

        let commit_temp = tempfile::tempdir().expect("temp");
        let commit_state = mock_state(&commit_temp, false);
        let commit_hook = crate::failpoint::TestFailpoint::new(&["create-state-commit"]);
        commit_hook
            .run(create_instance(&commit_state, &request))
            .await
            .expect_err("state commit failure");
        let commit_instance = commit_state
            .instances
            .lock()
            .expect("instances")
            .values()
            .next()
            .cloned()
            .expect("destroyed create");
        assert_eq!(commit_instance.state, SandboxState::Destroyed);
        assert!(
            commit_state
                .backend_instances
                .lock()
                .expect("owners")
                .is_empty()
        );

        let destroy_temp = tempfile::tempdir().expect("temp");
        let destroy_state = mock_state(&destroy_temp, false);
        let created = created_json(&destroy_state, &request).await;
        let id = created["instance"]["id"].as_str().expect("id").to_string();
        let kill_hook = crate::failpoint::TestFailpoint::new(&["destroy-kill"]);
        kill_hook
            .run(destroy_instance(&destroy_state, &id))
            .await
            .expect_err("kill boundary");
        let uuid = Uuid::parse_str(&id).expect("uuid");
        assert!(
            destroy_state
                .backend_instances
                .lock()
                .expect("owners")
                .contains_key(&uuid)
        );
        destroy_instance(&destroy_state, &id)
            .await
            .expect("destroy retry");

        let release_temp = tempfile::tempdir().expect("temp");
        let release_state = mock_state(&release_temp, false);
        let created = created_json(&release_state, &request).await;
        let id = created["instance"]["id"].as_str().expect("id").to_string();
        let release_hook = crate::failpoint::TestFailpoint::new(&["storage-release"]);
        release_hook
            .run(destroy_instance(&release_state, &id))
            .await
            .expect_err("release boundary");
        let uuid = Uuid::parse_str(&id).expect("uuid");
        assert_eq!(
            release_state.instances.lock().expect("instances")[&uuid].backend_ownership,
            BackendOwnership::Stopped
        );
        destroy_instance(&release_state, &id)
            .await
            .expect("release retry");
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn acquire_rollback_failure_retains_a_destroyable_record() {
        let temp = tempfile::tempdir().expect("temp");
        let state = mock_state(&temp, false);
        let acquire_hook = crate::failpoint::TestFailpoint::new(&[
            "storage-acquire-artifacts",
            "storage-acquire-rollback",
        ]);
        let error = acquire_hook
            .run(create_instance(&state, &test_request()))
            .await
            .expect_err("residual slot must require recovery");
        assert!(matches!(error, BlazeDaemonError::RecoveryRequired(_)));

        let instance = state
            .instances
            .lock()
            .expect("instances")
            .values()
            .next()
            .cloned()
            .expect("recovery record");
        assert_eq!(instance.state, SandboxState::Creating);
        assert_eq!(instance.backend_ownership, BackendOwnership::NotStarted);
        assert!(
            temp.path()
                .join("instances")
                .join(instance.id.to_string())
                .is_dir()
        );
        destroy_instance(&state, &instance.id.to_string())
            .await
            .expect("destroy residual slot");
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn acquired_slot_is_destroyable_after_restart_before_start_commit() {
        let temp = tempfile::tempdir().expect("temp");
        let config = test_config(&temp);
        let instances_dir = config.storage.instances_dir.clone();
        let initial_storage: Arc<dyn StorageProvider> = Arc::new(FileStorageProvider::with_images(
            config.storage.images_dir.clone(),
            instances_dir.clone(),
        ));
        let initial_state = build_test_state(
            config.clone(),
            test_policy(BackendKind::Mock, false),
            spawners(BackendKind::Mock, Arc::new(MockSpawner)),
            BackendKind::Mock,
            initial_storage,
        );
        let pause_hook = crate::failpoint::TestFailpoint::new(&["create-after-storage-acquire"]);
        let create_state = initial_state.clone();
        let create_hook = pause_hook.clone();
        let create = tokio::spawn(async move {
            create_hook
                .run(create_instance(&create_state, &test_request()))
                .await
        });
        pause_hook.wait_until_paused().await;

        let instance = initial_state
            .instances
            .lock()
            .expect("instances")
            .values()
            .next()
            .cloned()
            .expect("write-ahead instance");
        let id = instance.id;
        assert_eq!(instance.state, SandboxState::Creating);
        assert_eq!(instance.backend_ownership, BackendOwnership::NotStarted);
        assert!(
            config
                .daemon
                .state_dir
                .join(id.to_string())
                .join("state.json")
                .is_file()
        );
        assert!(instances_dir.join(id.to_string()).is_dir());

        create.abort();
        assert!(
            create
                .await
                .expect_err("create task aborted")
                .is_cancelled()
        );
        drop(initial_state);

        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let restarted_storage: Arc<dyn StorageProvider> =
            Arc::new(FileStorageProvider::with_images(
                config.storage.images_dir.clone(),
                instances_dir.clone(),
            ));
        let restarted = build_test_state(
            config,
            test_policy(BackendKind::Mock, false),
            spawners(
                BackendKind::Mock,
                Arc::new(RecordingSpawner {
                    cleanup_count: cleanup_count.clone(),
                }),
            ),
            BackendKind::Mock,
            restarted_storage,
        );
        assert!(
            restarted
                .instances
                .lock()
                .expect("instances")
                .contains_key(&id)
        );

        destroy_instance(&restarted, &id.to_string())
            .await
            .expect("destroy acquired slot after restart");
        assert_eq!(cleanup_count.load(Ordering::Acquire), 0);
        assert_eq!(
            restarted.instances.lock().expect("instances")[&id].state,
            SandboxState::Destroyed
        );
        assert!(!instances_dir.join(id.to_string()).exists());
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn warm_activation_and_destroy_are_serialized_per_instance() {
        let temp = tempfile::tempdir().expect("temp");
        let state = mock_state(&temp, true);
        let request = test_request();
        let cold = created_json(&state, &request).await;
        let id = cold["instance"]["id"].as_str().expect("id").to_string();
        reset_instance(&state, &id).await.expect("warm");

        let pause_hook = crate::failpoint::TestFailpoint::new(&["warm-before-state-commit"]);
        let create_state = state.clone();
        let create_request = request.clone();
        let activation_hook = pause_hook.clone();
        let activation = tokio::spawn(async move {
            activation_hook
                .run(create_instance(&create_state, &create_request))
                .await
        });
        pause_hook.wait_until_paused().await;

        let destroy_state = state.clone();
        let destroy_id = id.clone();
        let destroy =
            tokio::spawn(async move { destroy_instance(&destroy_state, &destroy_id).await });
        tokio::task::yield_now().await;
        assert!(!destroy.is_finished(), "destroy must wait for activation");

        pause_hook.release();
        activation
            .await
            .expect("activation task")
            .expect("activation");
        destroy.await.expect("destroy task").expect("destroy");
        let uuid = Uuid::parse_str(&id).expect("uuid");
        assert_eq!(
            state.instances.lock().expect("instances")[&uuid].state,
            SandboxState::Destroyed
        );
    }
}
