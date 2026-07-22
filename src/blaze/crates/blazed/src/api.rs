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

use blaze_core::backend::{BackendKind, BackendStatus, select_backend};
use blaze_core::kernel::HookKind;
use blaze_core::lifecycle::{SandboxInstance, SandboxState, StartPath};
use blaze_core::policy::{ImageMetadata, RuntimeDecision, WorkloadClass, parse_duration};
use blaze_core::pool::{PoolConfig, PoolKey};
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
        ("GET", ["v1", "health"]) => health(),
        ("GET", ["v1", "instances"]) => list_instances(state),
        ("POST", ["v1", "instances"]) => create_instance(state, &body).await,
        ("GET", ["v1", "instances", id]) => get_instance(state, id),
        ("POST", ["v1", "instances", id, "checkpoint"]) => checkpoint(state, id),
        ("POST", ["v1", "instances", id, "reset"]) => reset_instance(state, id),
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

fn health() -> Result<Response<Full<Bytes>>> {
    json_ok(&json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
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
    let backend = match select_backend(&decision.backend_priority, &availability) {
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

    // 3. Pool lookup.
    let pool_key = PoolKey::new(backend, decision.workload_class, req.image_digest.clone());
    let mut start_path = StartPath::Cold;
    let mut reused: Option<Uuid> = None;
    if decision.pool_eligible {
        let mut pool = state
            .pool
            .lock()
            .map_err(|_| BlazeDaemonError::Internal("pool lock poisoned".into()))?;
        if let Some(id) = pool.lookup(&pool_key) {
            reused = Some(id);
            start_path = StartPath::Warm;
            state.metrics.inc(&state.metrics.pool_hits);
        } else {
            state.metrics.inc(&state.metrics.pool_misses);
        }
    }

    // 4. Build (or revive) instance.
    let mut instance = if let Some(id) = reused {
        let mut map = state
            .instances
            .lock()
            .map_err(|_| BlazeDaemonError::Internal("instances lock poisoned".into()))?;
        match map.remove(&id) {
            Some(mut inst) => {
                // warm → creating
                inst.transition(SandboxState::Creating)?;
                inst
            }
            None => {
                // pool stat tracked the id but instance state was reaped;
                // fall back to a fresh boot so the request still succeeds.
                tracing::warn!(instance = %id, "warm id missing from instances map, cold-booting");
                start_path = StartPath::Cold;
                let mut inst = SandboxInstance::new(
                    backend,
                    decision.workload_class,
                    req.image_digest.clone(),
                    StartPath::Cold,
                    decision.policy_name.clone(),
                );
                inst.transition(SandboxState::Creating)?;
                inst
            }
        }
    } else {
        let mut inst = SandboxInstance::new(
            backend,
            decision.workload_class,
            req.image_digest.clone(),
            StartPath::Cold,
            decision.policy_name.clone(),
        );
        inst.transition(SandboxState::Creating)?;
        inst
    };

    // 5. Spawn the data-plane process via the BackendSpawner trait.
    //    The daemon picks LinuxSandboxSpawner when the configured
    //    backend binary exists; otherwise MockSpawner keeps the daemon
    //    usable on macOS dev hosts and in CI.
    let binary_path = {
        let cfg = state
            .config
            .lock()
            .map_err(|_| BlazeDaemonError::Internal("config lock poisoned".into()))?;
        cfg.backends
            .get(state.active_backend.as_str())
            .cloned()
            .unwrap_or_else(std::path::PathBuf::new)
    };
    let work_dir = state.state_dir.join(instance.id.to_string());
    let spawner = state.spawner.clone();
    let actual_backend = match spawner
        .spawn(
            &instance,
            &binary_path,
            &work_dir,
            &decision.backend,
            decision.vm.as_ref(),
        )
        .await
    {
        Ok(handle) => {
            let real_backend = handle.backend;
            let mut handles = state
                .spawn_handles
                .lock()
                .map_err(|_| BlazeDaemonError::Internal("spawn_handles lock poisoned".into()))?;
            handles.insert(instance.id, handle);
            real_backend
        }
        Err(err) => {
            // Fail forward into Destroyed so the lifecycle stays
            // consistent and surface the error to the caller.
            tracing::error!(instance = %instance.id, ?err, "spawn failed, marking destroyed");
            let _ = instance.transition(SandboxState::Destroyed);
            instance.persist(&state.state_dir)?;
            state.metrics.inc(&state.metrics.instances_destroyed);
            return Err(err.into());
        }
    };
    instance.transition(SandboxState::Running)?;
    instance.persist(&state.state_dir)?;

    // 6. Done.
    state.metrics.inc(&state.metrics.instances_created);
    {
        let mut map = state
            .instances
            .lock()
            .map_err(|_| BlazeDaemonError::Internal("instances lock poisoned".into()))?;
        map.insert(instance.id, instance.clone());
    }

    json_created(&CreateInstanceResp {
        instance,
        decision,
        start_path,
        selected_backend: actual_backend,
    })
}

fn checkpoint(state: &Arc<ServerState>, id: &str) -> Result<Response<Full<Bytes>>> {
    let uuid = parse_uuid(id)?;
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

fn reset_instance(state: &Arc<ServerState>, id: &str) -> Result<Response<Full<Bytes>>> {
    let uuid = parse_uuid(id)?;
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

fn destroy_instance_state(
    state: &Arc<ServerState>,
    uuid: Uuid,
) -> Result<(SandboxInstance, Option<crate::spawner::SpawnHandle>)> {
    let mut map = state
        .instances
        .lock()
        .map_err(|_| BlazeDaemonError::Internal("instances lock poisoned".into()))?;
    let inst = map
        .get_mut(&uuid)
        .ok_or_else(|| BlazeDaemonError::NotFound(format!("instance {uuid}")))?;
    inst.transition(SandboxState::Destroyed)?;
    inst.persist(&state.state_dir)?;
    let snapshot = inst.clone();
    drop(map);
    let handle = {
        let mut handles = state
            .spawn_handles
            .lock()
            .map_err(|_| BlazeDaemonError::Internal("spawn_handles lock poisoned".into()))?;
        handles.remove(&uuid)
    };
    Ok((snapshot, handle))
}

async fn destroy_instance(state: &Arc<ServerState>, id: &str) -> Result<Response<Full<Bytes>>> {
    let uuid = parse_uuid(id)?;
    let (inst, handle) = destroy_instance_state(state, uuid)?;
    if let Some(handle) = handle
        && let Err(err) = state.spawner.kill(&handle).await
    {
        tracing::warn!(instance = %uuid, ?err, "spawner.kill failed (non-fatal)");
    }
    state.metrics.inc(&state.metrics.instances_destroyed);
    json_ok(&json!({
        "destroyed": true,
        "instance_id": inst.id,
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
    use std::sync::Arc;

    use blaze_core::backend::BackendKind;
    use blaze_core::config::DaemonConfig;
    use blaze_core::kernel::HookRegistry;
    use blaze_core::policy::{
        BackendConfigs, FallbackOnMissingHook, PolicyEngine, PolicyFile, PolicyHooks, PolicyMatch,
        PolicySelect, WorkloadClass,
    };
    use blaze_core::pool::PoolManager;
    use blaze_core::template::TemplateRegistry;

    use crate::spawner::MockSpawner;
    use crate::state::ServerState;

    use super::*;

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
        let spawner: crate::spawner::DynSpawner = Arc::new(MockSpawner);
        let state = Arc::new(ServerState::build(
            config,
            engine,
            PoolManager::new(),
            TemplateRegistry::new(),
            HookRegistry::new(),
            spawner,
            BackendKind::Firecracker,
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
}
