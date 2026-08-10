// SPDX-License-Identifier: Apache-2.0
//! UDS HTTP API server.
//!
//! Routing is a hand-rolled `match` on `(method, path-segments)` rather
//! than a router framework — the surface is small (~17 endpoints) and
//! the cost of a fresh dependency outweighs the readability win.

use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use blaze_core::backend::{BackendKind, BackendStatus, select_backend};
use blaze_core::kernel::HookKind;
use blaze_core::lifecycle::{SandboxInstance, SandboxState, StartPath};
use blaze_core::policy::{ImageMetadata, RuntimeDecision, WorkloadClass};
use blaze_core::pool::{PoolConfig, PoolKey};
use http_body_util::{BodyExt, Full};
use hyper::body::{Body, Bytes, Incoming};
use hyper::header::CONTENT_TYPE;
use hyper::{Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::error::{BlazeDaemonError, Result};
use crate::guest::MAX_GUEST_FILE_BYTES;
use crate::sandbox::CreateSandbox;
use crate::state::ServerState;

const MAX_EXEC_TIMEOUT_SECS: u32 = 20;
const MAX_GUEST_HTTP_BODY_BYTES: usize = 22 * 1024 * 1024;

/// Top-level request handler. Always returns `Ok(Response)`; internal
/// errors are turned into JSON error bodies so hyper never sees a panic.
pub async fn handle(
    req: Request<Incoming>,
    state: Arc<ServerState>,
) -> std::result::Result<Response<Full<Bytes>>, Infallible> {
    handle_request(req, state).await
}

async fn handle_request<B>(
    req: Request<B>,
    state: Arc<ServerState>,
) -> std::result::Result<Response<Full<Bytes>>, Infallible>
where
    B: Body<Data = Bytes> + Unpin,
    B::Error: std::fmt::Display,
{
    state.metrics.inc(&state.metrics.requests_total);

    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let limit = guest_body_route(&method, &path).then_some(MAX_GUEST_HTTP_BODY_BYTES);

    let response = match collect_body(req, limit).await {
        Ok(body) => dispatch(&method, &path, &query, body, &state).await,
        Err(e) => Err(e),
    };

    let resp = match response {
        Ok(r) => r,
        Err(e) => error_response(&e),
    };
    Ok(resp)
}

fn guest_body_route(method: &Method, path: &str) -> bool {
    if method != Method::POST {
        return false;
    }
    let parts = path
        .trim_start_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    matches!(
        parts.as_slice(),
        [
            "v1",
            "instances" | "sandboxes",
            _,
            "exec" | "read" | "write"
        ]
    )
}

async fn collect_body<B>(req: Request<B>, limit: Option<usize>) -> Result<Vec<u8>>
where
    B: Body<Data = Bytes> + Unpin,
    B::Error: std::fmt::Display,
{
    let mut body = req.into_body();
    let mut collected = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame
            .map_err(|error| BlazeDaemonError::BadRequest(format!("request body: {error}")))?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        if let Some(limit) = limit
            && collected.len().saturating_add(data.len()) > limit
        {
            return Err(crate::guest::GuestError::PayloadTooLarge {
                actual: collected.len().saturating_add(data.len()),
                limit,
            }
            .into());
        }
        collected.extend_from_slice(&data);
    }
    Ok(collected)
}

const fn max_base64_len(decoded_bytes: usize) -> usize {
    decoded_bytes
        .saturating_add(2)
        .saturating_div(3)
        .saturating_mul(4)
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
        ("GET", ["v1", "instances"]) | ("GET", ["v1", "sandboxes"]) => list_instances(state),
        ("POST", ["v1", "instances"]) | ("POST", ["v1", "sandboxes"]) => {
            create_instance(state, &body).await
        }
        ("GET", ["v1", "instances", id]) | ("GET", ["v1", "sandboxes", id]) => {
            get_instance(state, id)
        }
        ("POST", ["v1", "sandboxes", id, "exec"]) | ("POST", ["v1", "instances", id, "exec"]) => {
            exec_instance(state, id, &body).await
        }
        ("POST", ["v1", "sandboxes", id, "read"]) | ("POST", ["v1", "instances", id, "read"]) => {
            read_instance_file(state, id, &body).await
        }
        ("POST", ["v1", "sandboxes", id, "write"]) | ("POST", ["v1", "instances", id, "write"]) => {
            write_instance_file(state, id, &body).await
        }
        ("POST", ["v1", "instances", id, "checkpoint"]) => checkpoint(state, id).await,
        ("POST", ["v1", "instances", id, "reset"]) => reset_instance(state, id).await,
        ("DELETE", ["v1", "instances", id])
        | ("DELETE", ["v1", "sandboxes", id])
        | ("POST", ["v1", "instances", id, "destroy"]) => destroy_instance(state, id).await,
        ("GET", ["v1", "pools"]) => list_pools(state),
        ("GET", ["v1", "pools", backend, class]) => pool_status(state, backend, class),
        ("POST", ["v1", "pools", backend, class, "drain"]) => drain_pool(state, backend, class),
        ("PUT", ["v1", "pools", backend, class, "sizing"]) => {
            resize_pool(state, backend, class, &body)
        }
        ("GET", ["v1", "templates"]) => list_templates(state).await,
        ("GET", ["v1", "templates", name]) => get_template(state, name).await,
        ("POST", ["v1", "templates", "import"]) => import_template(state, &body).await,
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
    json_ok(&state.manager.list()?)
}

fn get_instance(state: &Arc<ServerState>, id: &str) -> Result<Response<Full<Bytes>>> {
    json_ok(&state.manager.get(parse_uuid(id)?)?)
}

async fn create_instance(state: &Arc<ServerState>, body: &[u8]) -> Result<Response<Full<Bytes>>> {
    let req: CreateInstanceReq = serde_json::from_slice(body)
        .map_err(|e| BlazeDaemonError::BadRequest(format!("invalid create body: {e}")))?;

    let image = ImageMetadata {
        digest: req.image_digest.clone(),
        workload_class: Some(req.workload_class),
        kernel_version: req.kernel_version.clone(),
    };
    let decision = {
        let engine = state
            .policy
            .lock()
            .map_err(|_| BlazeDaemonError::Internal("policy lock poisoned".into()))?;
        match engine.evaluate(&req.labels, &image) {
            Ok(decision) => decision,
            Err(error) => {
                state.metrics.inc(&state.metrics.policy_eval_failures);
                return Err(error.into());
            }
        }
    };

    // Constrain availability to the implementation selected at daemon boot.
    let availability: Vec<BackendStatus> = {
        let config = state
            .config
            .lock()
            .map_err(|_| BlazeDaemonError::Internal("config lock poisoned".into()))?;
        decision
            .backend_priority
            .iter()
            .map(|kind| {
                let available = *kind == state.active_backend
                    && (state.active_backend == BackendKind::Mock
                        || config
                            .backends
                            .get(kind.as_str())
                            .map(|path| path.exists())
                            .unwrap_or(false));
                BackendStatus {
                    kind: *kind,
                    available,
                    version: None,
                }
            })
            .collect()
    };
    let policy_backend = match select_backend(&decision.backend_priority, &availability) {
        Ok(backend) => backend,
        Err(_) if state.active_backend == BackendKind::Mock => {
            *decision.backend_priority.first().ok_or_else(|| {
                BlazeDaemonError::Internal("policy has empty backend_priority".into())
            })?
        }
        Err(error) => return Err(error.into()),
    };
    let runtime_backend = if state.active_backend == BackendKind::Mock {
        BackendKind::Mock
    } else {
        policy_backend
    };
    let binary_path = state
        .config
        .lock()
        .map_err(|_| BlazeDaemonError::Internal("config lock poisoned".into()))?
        .backends
        .get(state.active_backend.as_str())
        .cloned()
        .unwrap_or_default();

    let created = state
        .manager
        .create(CreateSandbox {
            decision: decision.clone(),
            image_digest: req.image_digest,
            runtime_backend,
            binary_path,
        })
        .await?;
    json_created(&CreateInstanceResp {
        start_path: created.instance.start_path,
        instance: created.instance,
        decision,
        selected_backend: created.selected_backend,
    })
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
    state.manager.destroy(uuid).await?;
    json_ok(&json!({
        "destroyed": true,
        "instance_id": uuid,
    }))
}

#[derive(Debug, Deserialize)]
struct ExecRequest {
    cmd: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    env: Option<HashMap<String, String>>,
    #[serde(default)]
    timeout: Option<u32>,
}

async fn exec_instance(
    state: &Arc<ServerState>,
    id: &str,
    body: &[u8],
) -> Result<Response<Full<Bytes>>> {
    let request: ExecRequest = serde_json::from_slice(body)
        .map_err(|error| BlazeDaemonError::BadRequest(format!("invalid exec body: {error}")))?;
    if request.cmd.is_empty() {
        return Err(BlazeDaemonError::BadRequest(
            "exec command is required".to_string(),
        ));
    }
    let timeout = request.timeout.unwrap_or(MAX_EXEC_TIMEOUT_SECS);
    if timeout == 0 || timeout > MAX_EXEC_TIMEOUT_SECS {
        return Err(BlazeDaemonError::BadRequest(format!(
            "exec timeout must be between 1 and {MAX_EXEC_TIMEOUT_SECS} seconds"
        )));
    }
    let result = state
        .manager
        .exec(
            parse_uuid(id)?,
            request.cmd,
            request.cwd,
            request.env,
            timeout,
        )
        .await?;
    json_ok(&json!({
        "exit_code": result.exit_code,
        "stdout_b64": BASE64.encode(result.stdout),
        "stderr_b64": BASE64.encode(result.stderr),
    }))
}

#[derive(Debug, Deserialize)]
struct FileRequest {
    path: String,
    #[serde(default)]
    data_b64: Option<String>,
}

async fn read_instance_file(
    state: &Arc<ServerState>,
    id: &str,
    body: &[u8],
) -> Result<Response<Full<Bytes>>> {
    let request: FileRequest = serde_json::from_slice(body)
        .map_err(|error| BlazeDaemonError::BadRequest(format!("invalid read body: {error}")))?;
    let data = state
        .manager
        .read_file(parse_uuid(id)?, request.path)
        .await?;
    json_ok(&json!({"data_b64": BASE64.encode(data)}))
}

async fn write_instance_file(
    state: &Arc<ServerState>,
    id: &str,
    body: &[u8],
) -> Result<Response<Full<Bytes>>> {
    let request: FileRequest = serde_json::from_slice(body)
        .map_err(|error| BlazeDaemonError::BadRequest(format!("invalid write body: {error}")))?;
    let encoded = request
        .data_b64
        .ok_or_else(|| BlazeDaemonError::BadRequest("data_b64 is required".to_string()))?;
    let data = decode_guest_file(&encoded, MAX_GUEST_FILE_BYTES)?;
    state
        .manager
        .write_file(parse_uuid(id)?, request.path, &data)
        .await?;
    json_ok(&json!({"written": true, "bytes": data.len()}))
}

fn decode_guest_file(encoded: &str, limit: usize) -> Result<Vec<u8>> {
    let encoded_limit = max_base64_len(limit);
    if encoded.len() > encoded_limit {
        return Err(crate::guest::GuestError::PayloadTooLarge {
            actual: encoded.len(),
            limit: encoded_limit,
        }
        .into());
    }
    let data = BASE64
        .decode(encoded)
        .map_err(|error| BlazeDaemonError::BadRequest(format!("invalid base64: {error}")))?;
    if data.len() > limit {
        return Err(crate::guest::GuestError::PayloadTooLarge {
            actual: data.len(),
            limit,
        }
        .into());
    }
    Ok(data)
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

async fn list_templates(state: &Arc<ServerState>) -> Result<Response<Full<Bytes>>> {
    json_bytes_ok(state.manager.list_templates().await?)
}

async fn get_template(state: &Arc<ServerState>, name: &str) -> Result<Response<Full<Bytes>>> {
    json_bytes_ok(state.manager.get_template(name.to_string()).await?)
}

#[derive(Debug, Deserialize)]
struct ImportTemplateRequest {
    name: String,
    source: PathBuf,
    #[serde(default)]
    description: String,
}

async fn import_template(state: &Arc<ServerState>, body: &[u8]) -> Result<Response<Full<Bytes>>> {
    let request: ImportTemplateRequest = serde_json::from_slice(body).map_err(|error| {
        BlazeDaemonError::BadRequest(format!("invalid runtime template import body: {error}"))
    })?;
    let imported = state
        .manager
        .import_template(request.name, request.source, request.description)
        .await?;
    json_response(StatusCode::CREATED, &imported)
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

fn json_bytes_ok(body: Bytes) -> Result<Response<Full<Bytes>>> {
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(body))?)
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
    let mut body = json!({
        "error": err.to_string(),
        "status": status.as_u16(),
    });
    if let Some(code) = err.api_code() {
        body["code"] = json!(code);
    }
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
    use blaze_core::BlazeError;
    use blaze_core::backend::{BackendKind, SpawnRequest};
    use blaze_core::config::DaemonConfig;
    use blaze_core::kernel::HookRegistry;
    use blaze_core::lifecycle::{BackendOwnership, OperationKind};
    use blaze_core::policy::{
        BackendConfigs, FallbackOnMissingHook, PolicyEngine, PolicyFile, PolicyHooks, PolicyMatch,
        PolicyPool, PolicySelect, ResetMode, WorkloadClass,
    };
    use blaze_core::pool::PoolManager;
    use blaze_core::storage::{
        AcquireOpts, PoolStatus, StorageAcquireError, StorageProvider, StorageSlot,
    };

    use crate::file_provider::FileStorageProvider;
    #[cfg(target_os = "linux")]
    use crate::spawner::BubblewrapSpawner;
    use crate::spawner::{
        BackendInstance, BackendSpawner, DynBackendInstance, DynSpawner, GuestMockSpawner,
        MockSpawner, SpawnFailure, SpawnResult, SpawnerRegistry,
    };
    use crate::state::ServerState;
    #[cfg(target_os = "linux")]
    use tokio::sync::Notify;

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
        config.template.dir = temp.path().join("templates");
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
        Arc::new(
            ServerState::build(
                config,
                PolicyEngine::with_policies(vec![policy]),
                PoolManager::new(),
                HookRegistry::new(),
                registry,
                active_backend,
                storage,
            )
            .expect("state"),
        )
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

    #[cfg(feature = "test-failpoints")]
    fn guest_mock_state(temp: &tempfile::TempDir, pooled: bool) -> Arc<ServerState> {
        let config = test_config(temp);
        let storage: Arc<dyn StorageProvider> = Arc::new(FileStorageProvider::with_images(
            config.storage.images_dir.clone(),
            config.storage.instances_dir.clone(),
        ));
        build_test_state(
            config,
            test_policy(BackendKind::Mock, pooled),
            spawners(BackendKind::Mock, Arc::new(GuestMockSpawner)),
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

    async fn dispatched_json(
        state: &Arc<ServerState>,
        method: Method,
        path: &str,
        body: Vec<u8>,
    ) -> (StatusCode, serde_json::Value) {
        let response = dispatch(&method, path, "", body, state)
            .await
            .expect("dispatch");
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("response body")
            .to_bytes();
        let value = serde_json::from_slice(&body).expect("response json");
        (status, value)
    }

    async fn handled_json(
        state: &Arc<ServerState>,
        method: Method,
        path: &str,
        body: Vec<u8>,
    ) -> (StatusCode, serde_json::Value) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(hyper::header::CONTENT_LENGTH, body.len())
            .body(Full::new(Bytes::from(body)))
            .expect("request");
        let response = handle_request(request, state.clone())
            .await
            .expect("infallible response");
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("response body")
            .to_bytes();
        let value = serde_json::from_slice(&body).expect("response json");
        (status, value)
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

        async fn sync_artifacts(&self, slot: &StorageSlot) -> blaze_core::Result<()> {
            self.inner.sync_artifacts(slot).await
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
            assert_eq!(
                instance.operation.as_ref().map(|operation| operation.kind),
                Some(OperationKind::Create)
            );
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

        async fn sync_artifacts(&self, slot: &StorageSlot) -> blaze_core::Result<()> {
            self.inner.sync_artifacts(slot).await
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

    #[cfg(target_os = "linux")]
    struct PreSpawnBoundarySpawner {
        reached: Arc<Notify>,
    }

    #[cfg(target_os = "linux")]
    #[async_trait]
    impl BackendSpawner for PreSpawnBoundarySpawner {
        async fn prepare_spawn(&self, run_dir: &Path) -> blaze_core::Result<()> {
            BubblewrapSpawner.prepare_spawn(run_dir).await
        }

        async fn spawn(
            &self,
            _request: SpawnRequest,
        ) -> std::result::Result<DynBackendInstance, SpawnFailure> {
            self.reached.notify_one();
            std::future::pending().await
        }

        async fn probe(&self, _binary_path: &Path) -> blaze_core::Result<bool> {
            Ok(true)
        }

        async fn cleanup_orphan(
            &self,
            instance_id: Uuid,
            run_dir: &Path,
        ) -> blaze_core::Result<()> {
            BubblewrapSpawner.cleanup_orphan(instance_id, run_dir).await
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

    struct SelectiveCleanupSpawner {
        failed_id: Uuid,
        cleanup_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl BackendSpawner for SelectiveCleanupSpawner {
        async fn spawn(
            &self,
            request: SpawnRequest,
        ) -> std::result::Result<DynBackendInstance, SpawnFailure> {
            MockSpawner.spawn(request).await
        }

        async fn probe(&self, _binary_path: &Path) -> blaze_core::Result<bool> {
            Ok(true)
        }

        async fn cleanup_orphan(
            &self,
            instance_id: Uuid,
            _run_dir: &Path,
        ) -> blaze_core::Result<()> {
            self.cleanup_count.fetch_add(1, Ordering::AcqRel);
            if instance_id == self.failed_id {
                return Err(BlazeError::BackendError {
                    msg: "cleanup deferred".into(),
                });
            }
            Ok(())
        }
    }

    struct CountingOwner {
        instance_id: Uuid,
        kill_count: Arc<AtomicUsize>,
        killed: AtomicBool,
    }

    #[async_trait]
    impl BackendInstance for CountingOwner {
        fn backend(&self) -> BackendKind {
            BackendKind::Mock
        }

        async fn try_wait(&self) -> blaze_core::Result<Option<SpawnResult>> {
            Ok(self.killed.load(Ordering::Acquire).then_some(SpawnResult {
                instance_id: self.instance_id,
                exit_code: Some(0),
                signal: None,
            }))
        }

        async fn kill(&self) -> blaze_core::Result<()> {
            if !self.killed.swap(true, Ordering::AcqRel) {
                self.kill_count.fetch_add(1, Ordering::AcqRel);
            }
            Ok(())
        }
    }

    struct CountingSpawner {
        kill_count: Arc<AtomicUsize>,
        orphan_cleanup_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl BackendSpawner for CountingSpawner {
        async fn spawn(
            &self,
            request: SpawnRequest,
        ) -> std::result::Result<DynBackendInstance, SpawnFailure> {
            Ok(Arc::new(CountingOwner {
                instance_id: request.instance_id,
                kill_count: self.kill_count.clone(),
                killed: AtomicBool::new(false),
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
            self.orphan_cleanup_count.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    struct StalledGuestOwner {
        instance_id: Uuid,
        socket: PathBuf,
        kill_count: Arc<AtomicUsize>,
        killed: AtomicBool,
    }

    #[async_trait]
    impl BackendInstance for StalledGuestOwner {
        fn backend(&self) -> BackendKind {
            BackendKind::Mock
        }

        fn guest_socket_path(&self) -> &Path {
            &self.socket
        }

        async fn try_wait(&self) -> blaze_core::Result<Option<SpawnResult>> {
            Ok(self.killed.load(Ordering::Acquire).then_some(SpawnResult {
                instance_id: self.instance_id,
                exit_code: Some(0),
                signal: None,
            }))
        }

        async fn kill(&self) -> blaze_core::Result<()> {
            if !self.killed.swap(true, Ordering::AcqRel) {
                self.kill_count.fetch_add(1, Ordering::AcqRel);
            }
            Ok(())
        }
    }

    struct CountingStorage {
        inner: FileStorageProvider,
        release_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl StorageProvider for CountingStorage {
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
            self.release_count.fetch_add(1, Ordering::AcqRel);
            self.inner.release(slot).await
        }

        async fn release_by_id(&self, instance_id: &str) -> blaze_core::Result<()> {
            self.release_count.fetch_add(1, Ordering::AcqRel);
            self.inner.release_by_id(instance_id).await
        }

        async fn reconstruct(&self, instance_id: &str) -> blaze_core::Result<StorageSlot> {
            self.inner.reconstruct(instance_id).await
        }

        async fn sync_artifacts(&self, slot: &StorageSlot) -> blaze_core::Result<()> {
            self.inner.sync_artifacts(slot).await
        }

        fn pool_status(&self) -> PoolStatus {
            self.inner.pool_status()
        }

        async fn drain_pool(&self) -> blaze_core::Result<usize> {
            self.inner.drain_pool().await
        }
    }

    #[cfg(feature = "test-failpoints")]
    fn counting_state(
        temp: &tempfile::TempDir,
    ) -> (
        Arc<ServerState>,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
    ) {
        let config = test_config(temp);
        let kill_count = Arc::new(AtomicUsize::new(0));
        let orphan_cleanup_count = Arc::new(AtomicUsize::new(0));
        let release_count = Arc::new(AtomicUsize::new(0));
        let storage: Arc<dyn StorageProvider> = Arc::new(CountingStorage {
            inner: FileStorageProvider::with_images(
                config.storage.images_dir.clone(),
                config.storage.instances_dir.clone(),
            ),
            release_count: release_count.clone(),
        });
        let state = build_test_state(
            config,
            test_policy(BackendKind::Mock, false),
            spawners(
                BackendKind::Mock,
                Arc::new(CountingSpawner {
                    kill_count: kill_count.clone(),
                    orphan_cleanup_count: orphan_cleanup_count.clone(),
                }),
            ),
            BackendKind::Mock,
            storage,
        );
        (state, kill_count, orphan_cleanup_count, release_count)
    }

    #[tokio::test]
    async fn sandbox_collection_and_item_routes_match_instance_routes() {
        let temp = tempfile::tempdir().expect("temp");
        let config = test_config(&temp);
        let storage: Arc<dyn StorageProvider> = Arc::new(FileStorageProvider::with_images(
            config.storage.images_dir.clone(),
            config.storage.instances_dir.clone(),
        ));
        let state = build_test_state(
            config,
            test_policy(BackendKind::Mock, false),
            spawners(BackendKind::Mock, Arc::new(MockSpawner)),
            BackendKind::Mock,
            storage,
        );

        let (status, created) =
            dispatched_json(&state, Method::POST, "/v1/sandboxes", test_request()).await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(created["instance"]["state"], "running");
        let id = created["instance"]["id"].as_str().expect("instance id");

        let (_, sandboxes) =
            dispatched_json(&state, Method::GET, "/v1/sandboxes", Vec::new()).await;
        let (_, instances) =
            dispatched_json(&state, Method::GET, "/v1/instances", Vec::new()).await;
        assert_eq!(sandboxes, instances);

        let (_, sandbox) = dispatched_json(
            &state,
            Method::GET,
            &format!("/v1/sandboxes/{id}"),
            Vec::new(),
        )
        .await;
        let (_, instance) = dispatched_json(
            &state,
            Method::GET,
            &format!("/v1/instances/{id}"),
            Vec::new(),
        )
        .await;
        assert_eq!(sandbox, instance);
    }

    #[tokio::test]
    async fn destroy_route_forms_share_managed_cleanup() {
        let temp = tempfile::tempdir().expect("temp");
        let config = test_config(&temp);
        let storage: Arc<dyn StorageProvider> = Arc::new(FileStorageProvider::with_images(
            config.storage.images_dir.clone(),
            config.storage.instances_dir.clone(),
        ));
        let state = build_test_state(
            config,
            test_policy(BackendKind::Mock, false),
            spawners(BackendKind::Mock, Arc::new(MockSpawner)),
            BackendKind::Mock,
            storage,
        );

        let mut ids = Vec::new();
        for _ in 0..3 {
            let created = created_json(&state, &test_request()).await;
            ids.push(
                Uuid::parse_str(created["instance"]["id"].as_str().expect("instance id"))
                    .expect("uuid"),
            );
        }
        let routes = [
            (Method::DELETE, format!("/v1/sandboxes/{}", ids[0]), ids[0]),
            (Method::DELETE, format!("/v1/instances/{}", ids[1]), ids[1]),
            (
                Method::POST,
                format!("/v1/instances/{}/destroy", ids[2]),
                ids[2],
            ),
        ];

        for (method, path, id) in routes {
            let (status, response) = dispatched_json(&state, method, &path, Vec::new()).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(response["destroyed"], true);
            assert_eq!(response["instance_id"], id.to_string());
            assert_eq!(
                state.manager.get(id).expect("destroyed state").state,
                SandboxState::Destroyed
            );
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
        config.template.dir = tmp.join("templates");
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
        let state = Arc::new(
            ServerState::build(
                config,
                engine,
                PoolManager::new(),
                HookRegistry::new(),
                spawners(BackendKind::Firecracker, spawner),
                BackendKind::Firecracker,
                storage,
            )
            .expect("state"),
        );

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
        config.template.dir = temp.path().join("templates");
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
        let state = Arc::new(
            ServerState::build(
                config,
                PolicyEngine::with_policies(vec![policy]),
                PoolManager::new(),
                HookRegistry::new(),
                spawners(BackendKind::Mock, Arc::new(MockSpawner)),
                BackendKind::Mock,
                storage,
            )
            .expect("state"),
        );
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
            .manager
            .backend_owner(Uuid::parse_str(&id).expect("uuid"))
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
    async fn sandbox_guest_routes_use_owned_runtime() {
        let temp = tempfile::tempdir().expect("temp");
        let config = test_config(&temp);
        let storage: Arc<dyn StorageProvider> = Arc::new(FileStorageProvider::with_images(
            config.storage.images_dir.clone(),
            config.storage.instances_dir.clone(),
        ));
        let state = build_test_state(
            config,
            test_policy(BackendKind::Mock, false),
            spawners(BackendKind::Mock, Arc::new(GuestMockSpawner)),
            BackendKind::Mock,
            storage,
        );
        let created = created_json(&state, &test_request()).await;
        let id = created["instance"]["id"].as_str().expect("instance id");

        let (status, exec) = dispatched_json(
            &state,
            Method::POST,
            &format!("/v1/sandboxes/{id}/exec"),
            serde_json::to_vec(&json!({
                "cmd": "printf routed",
                "timeout": 5,
            }))
            .expect("exec request"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(exec["exit_code"], 0);
        assert_eq!(exec["stdout_b64"], BASE64.encode(b"printf routed"));

        let encoded = "AAEC/2d1ZXN0";
        let (status, written) = dispatched_json(
            &state,
            Method::POST,
            &format!("/v1/instances/{id}/write"),
            serde_json::to_vec(&json!({
                "path": "/tmp/value",
                "data_b64": encoded,
            }))
            .expect("write request"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(written["bytes"], 9);

        let (status, read) = dispatched_json(
            &state,
            Method::POST,
            &format!("/v1/sandboxes/{id}/read"),
            serde_json::to_vec(&json!({"path": "/tmp/value"})).expect("read request"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(read["data_b64"], encoded);

        let invalid_timeout = dispatch(
            &Method::POST,
            &format!("/v1/sandboxes/{id}/exec"),
            "",
            serde_json::to_vec(&json!({
                "cmd": "true",
                "timeout": MAX_EXEC_TIMEOUT_SECS + 1,
            }))
            .expect("invalid request"),
            &state,
        )
        .await
        .expect_err("timeout above the API limit must fail");
        assert!(matches!(invalid_timeout, BlazeDaemonError::BadRequest(_)));

        assert_eq!(
            decode_guest_file(&BASE64.encode(b"1234"), 4).expect("boundary"),
            b"1234"
        );
        assert!(matches!(
            decode_guest_file(&BASE64.encode(b"12345"), 4),
            Err(BlazeDaemonError::Guest(
                crate::guest::GuestError::PayloadTooLarge { .. }
            ))
        ));
        assert!(matches!(
            decode_guest_file("not/base64!", 16),
            Err(BlazeDaemonError::BadRequest(_))
        ));

        let (status, destroyed) = dispatched_json(
            &state,
            Method::DELETE,
            &format!("/v1/sandboxes/{id}"),
            Vec::new(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(destroyed["destroyed"], true);
    }

    #[tokio::test]
    async fn production_mock_rejects_guest_operations() {
        let temp = tempfile::tempdir().expect("temp");
        let config = test_config(&temp);
        let storage: Arc<dyn StorageProvider> = Arc::new(FileStorageProvider::with_images(
            config.storage.images_dir.clone(),
            config.storage.instances_dir.clone(),
        ));
        let state = build_test_state(
            config,
            test_policy(BackendKind::Mock, false),
            spawners(BackendKind::Mock, Arc::new(MockSpawner)),
            BackendKind::Mock,
            storage,
        );
        let created = created_json(&state, &test_request()).await;
        let id = created["instance"]["id"].as_str().expect("instance id");

        let (status, error) = handled_json(
            &state,
            Method::POST,
            &format!("/v1/sandboxes/{id}/exec"),
            serde_json::to_vec(&json!({"cmd": "true"})).expect("exec request"),
        )
        .await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert!(
            error["error"]
                .as_str()
                .expect("error message")
                .contains("no guest transport")
        );
    }

    #[tokio::test]
    async fn guest_write_respects_http_and_decoded_limits() {
        let temp = tempfile::tempdir().expect("temp");
        let config = test_config(&temp);
        let storage: Arc<dyn StorageProvider> = Arc::new(FileStorageProvider::with_images(
            config.storage.images_dir.clone(),
            config.storage.instances_dir.clone(),
        ));
        let state = build_test_state(
            config,
            test_policy(BackendKind::Mock, false),
            spawners(BackendKind::Mock, Arc::new(GuestMockSpawner)),
            BackendKind::Mock,
            storage,
        );
        let created = created_json(&state, &test_request()).await;
        let id = created["instance"]["id"].as_str().expect("instance id");
        let uuid = Uuid::parse_str(id).expect("uuid");
        let path = format!("/v1/sandboxes/{id}/write");

        let envelope_payload = vec![b'y'; 17 * 1024 * 1024];
        let envelope_body = serde_json::to_vec(&json!({
            "path": "/tmp/http-envelope",
            "data_b64": BASE64.encode(&envelope_payload),
        }))
        .expect("write request above the guest HTTP limit");
        assert!(envelope_body.len() > MAX_GUEST_HTTP_BODY_BYTES);
        let (status, error) = handled_json(&state, Method::POST, &path, envelope_body).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(error["status"], 413);

        let mut payload = vec![b'z'; MAX_GUEST_FILE_BYTES];
        let body = serde_json::to_vec(&json!({
            "path": "/tmp/max-size",
            "data_b64": BASE64.encode(&payload),
        }))
        .expect("write request");
        assert!(body.len() <= MAX_GUEST_HTTP_BODY_BYTES);

        let (status, written) = handled_json(&state, Method::POST, &path, body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(written["bytes"], MAX_GUEST_FILE_BYTES);
        let readback = state
            .manager
            .read_file(uuid, "/tmp/max-size".into())
            .await
            .expect("read maximum file");
        assert_eq!(readback, payload);
        drop(readback);

        payload.push(b'z');
        let oversized = serde_json::to_vec(&json!({
            "path": "/tmp/too-large",
            "data_b64": BASE64.encode(&payload),
        }))
        .expect("oversized write request");
        assert!(oversized.len() <= MAX_GUEST_HTTP_BODY_BYTES);
        let (status, error) = handled_json(&state, Method::POST, &path, oversized).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(error["status"], 413);
    }

    #[tokio::test]
    async fn write_route_reports_unknown_after_delivery_failure() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

        let temp = tempfile::tempdir().expect("temp");
        let config = test_config(&temp);
        let storage: Arc<dyn StorageProvider> = Arc::new(FileStorageProvider::with_images(
            config.storage.images_dir.clone(),
            config.storage.instances_dir.clone(),
        ));
        let state = build_test_state(
            config,
            test_policy(BackendKind::Mock, false),
            spawners(BackendKind::Mock, Arc::new(MockSpawner)),
            BackendKind::Mock,
            storage,
        );
        let created = created_json(&state, &test_request()).await;
        let id = created["instance"]["id"].as_str().expect("instance id");
        let uuid = Uuid::parse_str(id).expect("uuid");
        state
            .manager
            .backend_owner(uuid)
            .expect("mock owner")
            .kill()
            .await
            .expect("stop mock guest");

        let socket = temp.path().join("uncertain.uds");
        let listener = tokio::net::UnixListener::bind(&socket).expect("bind guest endpoint");
        state
            .manager
            .insert_backend_owner(
                uuid,
                Arc::new(StalledGuestOwner {
                    instance_id: uuid,
                    socket,
                    kill_count: Arc::new(AtomicUsize::new(0)),
                    killed: AtomicBool::new(false),
                }),
            )
            .expect("replace backend owner");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept guest request");
            let mut reader = tokio::io::BufReader::new(stream);
            let mut connect = String::new();
            reader.read_line(&mut connect).await.expect("read connect");
            assert_eq!(connect, "CONNECT 5000\n");
            reader
                .get_mut()
                .write_all(b"OK 5000\n")
                .await
                .expect("write handshake");
            let mut request = String::new();
            reader
                .read_line(&mut request)
                .await
                .expect("read guest request");
            let request: serde_json::Value =
                serde_json::from_str(&request).expect("parse guest request");
            assert_eq!(request["op"], "write");
        });

        let body = serde_json::to_vec(&json!({
            "path": "/tmp/value",
            "data_b64": BASE64.encode(b"value"),
        }))
        .expect("write request");
        let (status, error) = handled_json(
            &state,
            Method::POST,
            &format!("/v1/sandboxes/{id}/write"),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(error["code"], "guest_outcome_unknown");
        server.await.expect("guest server");
    }

    #[tokio::test]
    async fn unknown_guest_outcome_has_stable_api_code() {
        let response = error_response(&BlazeDaemonError::Guest(
            crate::guest::GuestError::OutcomeUnknown("response lost".into()),
        ));
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("response body")
            .to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body).expect("error json");
        assert_eq!(value["code"], "guest_outcome_unknown");
        assert_eq!(value["status"], 504);

        let response = error_response(&BlazeDaemonError::Guest(
            crate::guest::GuestError::ResponseTooLarge {
                actual: 5,
                limit: 4,
            },
        ));
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("response body")
            .to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body).expect("error json");
        assert_eq!(value["code"], "guest_response_too_large");

        let response = error_response(&BlazeDaemonError::Guest(crate::guest::GuestError::Timeout(
            "connect stalled".into(),
        )));
        let body = response
            .into_body()
            .collect()
            .await
            .expect("response body")
            .to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body).expect("error json");
        assert_eq!(value["code"], "guest_timeout");
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

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn restart_reconciles_durable_starting_before_spawn() {
        let temp = tempfile::tempdir().expect("temp");
        let mut config = test_config(&temp);
        config.storage.rootfs_size = 64;
        config.storage.mem_size = 32;
        config
            .backends
            .insert(BackendKind::Bubblewrap.as_str().into(), "/bin/true".into());
        let storage: Arc<dyn StorageProvider> = Arc::new(FileStorageProvider::with_images(
            config.storage.images_dir.clone(),
            config.storage.instances_dir.clone(),
        ));
        let reached = Arc::new(Notify::new());
        let state = build_test_state(
            config.clone(),
            test_policy(BackendKind::Bubblewrap, false),
            spawners(
                BackendKind::Bubblewrap,
                Arc::new(PreSpawnBoundarySpawner {
                    reached: reached.clone(),
                }),
            ),
            BackendKind::Bubblewrap,
            storage,
        );
        let create_state = state.clone();
        let create =
            tokio::spawn(async move { create_instance(&create_state, &test_request()).await });
        tokio::time::timeout(std::time::Duration::from_secs(2), reached.notified())
            .await
            .expect("create reached the pre-spawn boundary");

        let instance = state
            .manager
            .list()
            .expect("instances")
            .into_iter()
            .next()
            .expect("durable create state");
        let persisted = SandboxInstance::load(&config.daemon.state_dir, instance.id)
            .expect("load durable Starting state");
        assert_eq!(persisted.state, SandboxState::Creating);
        assert_eq!(persisted.backend_ownership, BackendOwnership::Starting);
        let pid_file = config
            .daemon
            .state_dir
            .join(instance.id.to_string())
            .join("backend.pid");
        assert_eq!(std::fs::read(&pid_file).expect("prepared PID handoff"), b"");
        assert!(
            config
                .storage
                .instances_dir
                .join(instance.id.to_string())
                .is_dir()
        );

        create.abort();
        assert!(
            create
                .await
                .expect_err("simulated daemon exit cancels create")
                .is_cancelled()
        );
        drop(state);

        let recovered_storage: Arc<dyn StorageProvider> =
            Arc::new(FileStorageProvider::with_images(
                config.storage.images_dir.clone(),
                config.storage.instances_dir.clone(),
            ));
        let recovered = build_test_state(
            config.clone(),
            test_policy(BackendKind::Bubblewrap, false),
            spawners(BackendKind::Bubblewrap, Arc::new(BubblewrapSpawner)),
            BackendKind::Bubblewrap,
            recovered_storage,
        );

        let report = recovered.manager.reconcile_startup().await;

        assert_eq!(report.attempted, 1);
        assert_eq!(report.completed, 1);
        assert!(report.failures.is_empty());
        assert_eq!(
            recovered
                .manager
                .get(instance.id)
                .expect("reconciled state")
                .state,
            SandboxState::Destroyed
        );
        assert!(
            !config
                .storage
                .instances_dir
                .join(instance.id.to_string())
                .exists()
        );
        assert!(
            config
                .daemon
                .state_dir
                .join(instance.id.to_string())
                .join("backend.stopped")
                .is_file()
        );
        assert!(!pid_file.exists());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn restart_retains_locked_handoff_until_retry() {
        use std::os::fd::AsRawFd;

        let temp = tempfile::tempdir().expect("temp");
        let mut config = test_config(&temp);
        config.storage.rootfs_size = 64;
        config.storage.mem_size = 32;
        let storage = Arc::new(FileStorageProvider::with_images(
            config.storage.images_dir.clone(),
            config.storage.instances_dir.clone(),
        ));
        let mut instance = SandboxInstance::new(
            BackendKind::Bubblewrap,
            WorkloadClass::AgentTool,
            "sha256:locked-handoff".into(),
            StartPath::Cold,
            "pid-handoff-test".into(),
        );
        instance
            .transition(SandboxState::Creating)
            .expect("creating");
        instance.begin_operation(OperationKind::Create);
        let run_dir = config.daemon.state_dir.join(instance.id.to_string());
        BubblewrapSpawner
            .prepare_spawn(&run_dir)
            .await
            .expect("prepare PID handoff");
        instance.backend_ownership = BackendOwnership::Starting;
        instance
            .persist(&config.daemon.state_dir)
            .expect("persist Starting state");
        storage
            .acquire(&AcquireOpts {
                instance_id: instance.id.to_string(),
                rootfs_size: config.storage.rootfs_size,
                mem_size: config.storage.mem_size,
            })
            .await
            .expect("storage");
        let pid_file = run_dir.join("backend.pid");
        let handoff = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&pid_file)
            .expect("open PID handoff");
        assert_eq!(
            unsafe { libc::flock(handoff.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0,
            "lock PID handoff"
        );
        let state = build_test_state(
            config.clone(),
            test_policy(BackendKind::Bubblewrap, false),
            spawners(BackendKind::Bubblewrap, Arc::new(BubblewrapSpawner)),
            BackendKind::Bubblewrap,
            storage,
        );

        let first = state.manager.reconcile_startup().await;

        assert_eq!(first.attempted, 1);
        assert_eq!(first.completed, 0);
        assert_eq!(first.failures.len(), 1);
        assert!(first.failures[0].error.contains("still in progress"));
        assert_eq!(
            state
                .manager
                .get(instance.id)
                .expect("retained state")
                .state,
            SandboxState::RecoveryRequired
        );
        assert!(
            config
                .storage
                .instances_dir
                .join(instance.id.to_string())
                .is_dir()
        );
        assert!(!run_dir.join("backend.stopped").exists());

        drop(handoff);
        let retry = state.manager.reconcile_startup().await;

        assert_eq!(retry.attempted, 1);
        assert_eq!(retry.completed, 1);
        assert!(retry.failures.is_empty());
        assert_eq!(
            state
                .manager
                .get(instance.id)
                .expect("destroyed state")
                .state,
            SandboxState::Destroyed
        );
        assert!(
            !config
                .storage
                .instances_dir
                .join(instance.id.to_string())
                .exists()
        );
        assert!(run_dir.join("backend.stopped").is_file());
        assert!(!pid_file.exists());
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
        assert!(cold["instance"]["operation"].is_null());

        reset_instance(&state, &id).await.expect("return to pool");
        let warm = created_json(&state, &request).await;
        assert_eq!(warm["instance"]["id"], id);
        assert_eq!(warm["instance"]["backend"], "mock");
        assert_eq!(warm["selected_backend"], "mock");
        assert_eq!(warm["start_path"], "warm");
        assert!(warm["instance"]["operation"].is_null());
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
        assert_eq!(instance.state, SandboxState::RecoveryRequired);
        assert_eq!(instance.backend_ownership, BackendOwnership::Running);
        assert_eq!(
            instance.operation.as_ref().map(|operation| operation.kind),
            Some(OperationKind::Create)
        );
        assert!(instances_dir.join(instance.id.to_string()).is_dir());
        assert!(state.manager.backend_owner(instance.id).is_some());

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
    async fn assert_warm_state_commit_failure_restores_claim(failpoint: &'static str) {
        let temp = tempfile::tempdir().expect("temp");
        let state = mock_state(&temp, true);
        let request = test_request();
        let cold = created_json(&state, &request).await;
        let id = cold["instance"]["id"].as_str().expect("id").to_string();
        let uuid = Uuid::parse_str(&id).expect("uuid");
        reset_instance(&state, &id).await.expect("warm");
        let owner = state.manager.backend_owner(uuid).expect("backend owner");
        let key = PoolKey::new(
            BackendKind::Mock,
            WorkloadClass::AgentTool,
            "sha256:ownership-test".into(),
        );

        let hook = crate::failpoint::TestFailpoint::new(&[failpoint]);
        let error = hook
            .run(create_instance(&state, &request))
            .await
            .expect_err("state commit failure");

        assert!(matches!(error, BlazeDaemonError::RecoveryRequired(_)));
        let restored = state.instances.lock().expect("instances")[&uuid].clone();
        assert_eq!(restored.state, SandboxState::Warm);
        assert!(restored.operation.is_none());
        let persisted =
            SandboxInstance::load(&state.state_dir, uuid).expect("persisted warm state");
        assert_eq!(persisted.state, SandboxState::Warm);
        assert_eq!(persisted.backend_ownership, BackendOwnership::Running);
        assert!(persisted.operation.is_none());
        let retained_owner = state.manager.backend_owner(uuid).expect("retained owner");
        assert!(Arc::ptr_eq(&owner, &retained_owner));
        assert_eq!(state.pool.lock().expect("pool").stats(&key).warm_count, 1);
        assert!(retained_owner.try_wait().await.expect("liveness").is_none());

        let retried = created_json(&state, &request).await;
        assert_eq!(retried["instance"]["id"], id);
        assert_eq!(retried["start_path"], "warm");
        let persisted =
            SandboxInstance::load(&state.state_dir, uuid).expect("persisted running state");
        assert_eq!(persisted.state, SandboxState::Running);
        assert_eq!(persisted.backend_ownership, BackendOwnership::Running);
        assert!(persisted.operation.is_none());
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn warm_intent_commit_failure_restores_the_claim() {
        assert_warm_state_commit_failure_restores_claim("warm-intent-state-commit").await;
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn warm_final_commit_failure_restores_the_claim() {
        assert_warm_state_commit_failure_restores_claim("warm-final-state-commit").await;
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn guest_readiness_failure_compensates_owned_resources() {
        let request = test_request();
        let temp = tempfile::tempdir().expect("temp");
        let state = guest_mock_state(&temp, false);
        let hook = crate::failpoint::TestFailpoint::new(&["create-guest-ready"]);

        hook.run(create_instance(&state, &request))
            .await
            .expect_err("guest readiness failure");

        let instance = state
            .instances
            .lock()
            .expect("instances")
            .values()
            .next()
            .cloned()
            .expect("destroyed create");
        assert_eq!(instance.state, SandboxState::Destroyed);
        assert!(state.manager.backend_owner(instance.id).is_none());
        assert!(
            !temp
                .path()
                .join("instances")
                .join(instance.id.to_string())
                .exists()
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
                .manager
                .backend_owner(commit_instance.id)
                .is_none()
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
        let failed_destroy = destroy_state.instances.lock().expect("instances")[&uuid].clone();
        assert_eq!(failed_destroy.state, SandboxState::RecoveryRequired);
        assert_eq!(
            failed_destroy
                .operation
                .as_ref()
                .map(|operation| operation.kind),
            Some(OperationKind::Destroy)
        );
        assert!(destroy_state.manager.backend_owner(uuid).is_some());
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
        assert_eq!(
            release_state.instances.lock().expect("instances")[&uuid]
                .operation
                .as_ref()
                .map(|operation| operation.kind),
            Some(OperationKind::Destroy)
        );
        destroy_instance(&release_state, &id)
            .await
            .expect("release retry");
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn destroy_intent_failure_does_not_touch_owned_resources() {
        let temp = tempfile::tempdir().expect("temp");
        let (state, kill_count, orphan_cleanup_count, release_count) = counting_state(&temp);
        let created = created_json(&state, &test_request()).await;
        let id = created["instance"]["id"].as_str().expect("id").to_string();
        let uuid = Uuid::parse_str(&id).expect("uuid");
        let hook = crate::failpoint::TestFailpoint::new(&["destroy-intent-state-commit"]);

        let error = hook
            .run(destroy_instance(&state, &id))
            .await
            .expect_err("intent failure");

        assert!(matches!(error, BlazeDaemonError::RecoveryRequired(_)));
        assert_eq!(kill_count.load(Ordering::Acquire), 0);
        assert_eq!(orphan_cleanup_count.load(Ordering::Acquire), 0);
        assert_eq!(release_count.load(Ordering::Acquire), 0);
        let retained = state.instances.lock().expect("instances")[&uuid].clone();
        assert_eq!(retained.state, SandboxState::RecoveryRequired);
        assert!(retained.operation.is_none());
        let persisted =
            SandboxInstance::load(&state.state_dir, uuid).expect("persisted recovery state");
        assert_eq!(persisted.state, SandboxState::RecoveryRequired);
        assert_eq!(persisted.backend_ownership, BackendOwnership::Running);
        assert!(persisted.operation.is_none());
        assert!(temp.path().join("instances").join(&id).is_dir());

        destroy_instance(&state, &id).await.expect("destroy retry");
        assert_eq!(kill_count.load(Ordering::Acquire), 1);
        assert_eq!(release_count.load(Ordering::Acquire), 1);
        assert_eq!(
            state.instances.lock().expect("instances")[&uuid].state,
            SandboxState::Destroyed
        );
        let persisted =
            SandboxInstance::load(&state.state_dir, uuid).expect("persisted destroyed state");
        assert_eq!(persisted.state, SandboxState::Destroyed);
        assert!(persisted.operation.is_none());
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn destroy_stop_commit_failure_retains_storage_for_retry() {
        let temp = tempfile::tempdir().expect("temp");
        let (state, kill_count, orphan_cleanup_count, release_count) = counting_state(&temp);
        let created = created_json(&state, &test_request()).await;
        let id = created["instance"]["id"].as_str().expect("id").to_string();
        let uuid = Uuid::parse_str(&id).expect("uuid");
        let hook = crate::failpoint::TestFailpoint::new(&["destroy-stop-state-commit"]);

        let error = hook
            .run(destroy_instance(&state, &id))
            .await
            .expect_err("stop commit failure");

        assert!(matches!(error, BlazeDaemonError::RecoveryRequired(_)));
        assert_eq!(kill_count.load(Ordering::Acquire), 1);
        assert_eq!(orphan_cleanup_count.load(Ordering::Acquire), 0);
        assert_eq!(release_count.load(Ordering::Acquire), 0);
        let retained = state.instances.lock().expect("instances")[&uuid].clone();
        assert_eq!(retained.state, SandboxState::RecoveryRequired);
        assert_eq!(retained.backend_ownership, BackendOwnership::Stopped);
        assert_eq!(
            retained.operation.as_ref().map(|operation| operation.kind),
            Some(OperationKind::Destroy)
        );
        let persisted =
            SandboxInstance::load(&state.state_dir, uuid).expect("persisted recovery state");
        assert_eq!(persisted.state, SandboxState::RecoveryRequired);
        assert_eq!(persisted.backend_ownership, BackendOwnership::Stopped);
        assert_eq!(
            persisted.operation.as_ref().map(|operation| operation.kind),
            Some(OperationKind::Destroy)
        );
        assert!(temp.path().join("instances").join(&id).is_dir());

        destroy_instance(&state, &id).await.expect("destroy retry");
        assert_eq!(kill_count.load(Ordering::Acquire), 1);
        assert_eq!(release_count.load(Ordering::Acquire), 1);
        let persisted =
            SandboxInstance::load(&state.state_dir, uuid).expect("persisted destroyed state");
        assert_eq!(persisted.state, SandboxState::Destroyed);
        assert!(persisted.operation.is_none());
    }

    #[cfg(feature = "test-failpoints")]
    #[tokio::test]
    async fn destroy_final_commit_failure_retains_retryable_metadata() {
        let temp = tempfile::tempdir().expect("temp");
        let (state, kill_count, orphan_cleanup_count, release_count) = counting_state(&temp);
        let created = created_json(&state, &test_request()).await;
        let id = created["instance"]["id"].as_str().expect("id").to_string();
        let uuid = Uuid::parse_str(&id).expect("uuid");
        let hook = crate::failpoint::TestFailpoint::new(&["destroy-final-state-commit"]);

        let error = hook
            .run(destroy_instance(&state, &id))
            .await
            .expect_err("final commit failure");

        assert!(matches!(error, BlazeDaemonError::RecoveryRequired(_)));
        assert_eq!(kill_count.load(Ordering::Acquire), 1);
        assert_eq!(orphan_cleanup_count.load(Ordering::Acquire), 0);
        assert_eq!(release_count.load(Ordering::Acquire), 1);
        assert!(!temp.path().join("instances").join(&id).exists());
        let retained = state.instances.lock().expect("instances")[&uuid].clone();
        assert_eq!(retained.state, SandboxState::RecoveryRequired);
        assert_eq!(retained.backend_ownership, BackendOwnership::Stopped);
        assert_eq!(
            retained.operation.as_ref().map(|operation| operation.kind),
            Some(OperationKind::Destroy)
        );
        let persisted =
            SandboxInstance::load(&state.state_dir, uuid).expect("persisted recovery state");
        assert_eq!(persisted.state, SandboxState::RecoveryRequired);
        assert_eq!(persisted.backend_ownership, BackendOwnership::Stopped);
        assert_eq!(
            persisted.operation.as_ref().map(|operation| operation.kind),
            Some(OperationKind::Destroy)
        );

        destroy_instance(&state, &id).await.expect("destroy retry");
        assert_eq!(kill_count.load(Ordering::Acquire), 1);
        assert_eq!(release_count.load(Ordering::Acquire), 2);
        let destroyed = state.instances.lock().expect("instances")[&uuid].clone();
        assert_eq!(destroyed.state, SandboxState::Destroyed);
        assert!(destroyed.operation.is_none());
        let persisted =
            SandboxInstance::load(&state.state_dir, uuid).expect("persisted destroyed state");
        assert_eq!(persisted.state, SandboxState::Destroyed);
        assert!(persisted.operation.is_none());
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
        assert_eq!(instance.state, SandboxState::RecoveryRequired);
        assert_eq!(instance.backend_ownership, BackendOwnership::NotStarted);
        assert_eq!(
            instance.operation.as_ref().map(|operation| operation.kind),
            Some(OperationKind::Create)
        );
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
        assert_eq!(
            instance.operation.as_ref().map(|operation| operation.kind),
            Some(OperationKind::Create)
        );
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
        let uuid = Uuid::parse_str(&id).expect("uuid");
        assert_eq!(
            state.instances.lock().expect("instances")[&uuid]
                .operation
                .as_ref()
                .map(|operation| operation.kind),
            Some(OperationKind::Create)
        );

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
        assert_eq!(
            state.instances.lock().expect("instances")[&uuid].state,
            SandboxState::Destroyed
        );
    }

    #[tokio::test]
    async fn startup_reconciliation_continues_after_one_cleanup_failure() {
        let temp = tempfile::tempdir().expect("temp");
        let config = test_config(&temp);
        let storage = Arc::new(FileStorageProvider::with_images(
            config.storage.images_dir.clone(),
            config.storage.instances_dir.clone(),
        ));
        let failed_id = Uuid::new_v4();
        let completed_id = Uuid::new_v4();
        for id in [failed_id, completed_id] {
            let mut instance = SandboxInstance::new(
                BackendKind::Mock,
                WorkloadClass::AgentTool,
                "sha256:reconcile".into(),
                StartPath::Cold,
                "reconcile-test".into(),
            );
            instance.id = id;
            instance
                .transition(SandboxState::Creating)
                .expect("creating");
            instance.transition(SandboxState::Running).expect("running");
            instance.backend_ownership = BackendOwnership::Running;
            instance.persist(&config.daemon.state_dir).expect("persist");
            storage
                .acquire(&AcquireOpts {
                    instance_id: id.to_string(),
                    rootfs_size: 64,
                    mem_size: 32,
                })
                .await
                .expect("storage");
        }
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let state = build_test_state(
            config.clone(),
            test_policy(BackendKind::Mock, false),
            spawners(
                BackendKind::Mock,
                Arc::new(SelectiveCleanupSpawner {
                    failed_id,
                    cleanup_count: cleanup_count.clone(),
                }),
            ),
            BackendKind::Mock,
            storage,
        );

        let report = state.manager.reconcile_startup().await;

        assert_eq!(report.attempted, 2);
        assert_eq!(report.completed, 1);
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].instance_id, failed_id);
        assert_eq!(cleanup_count.load(Ordering::Acquire), 2);
        assert_eq!(
            state.instances.lock().expect("instances")[&failed_id].state,
            SandboxState::RecoveryRequired
        );
        assert_eq!(
            state.instances.lock().expect("instances")[&completed_id].state,
            SandboxState::Destroyed
        );
        assert!(
            config
                .storage
                .instances_dir
                .join(failed_id.to_string())
                .is_dir()
        );
        assert!(
            !config
                .storage
                .instances_dir
                .join(completed_id.to_string())
                .exists()
        );
        let created = created_json(&state, &test_request()).await;
        assert_eq!(created["instance"]["state"], "running");
    }

    #[tokio::test]
    async fn startup_reconciliation_skips_cleanup_for_known_stopped_states() {
        let temp = tempfile::tempdir().expect("temp");
        let config = test_config(&temp);
        let release_count = Arc::new(AtomicUsize::new(0));
        let storage: Arc<dyn StorageProvider> = Arc::new(CountingStorage {
            inner: FileStorageProvider::with_images(
                config.storage.images_dir.clone(),
                config.storage.instances_dir.clone(),
            ),
            release_count: release_count.clone(),
        });
        let not_started_id = Uuid::new_v4();
        let stopped_id = Uuid::new_v4();

        let mut not_started = SandboxInstance::new(
            BackendKind::Mock,
            WorkloadClass::AgentTool,
            "sha256:not-started".into(),
            StartPath::Cold,
            "reconcile-test".into(),
        );
        not_started.id = not_started_id;
        not_started
            .transition(SandboxState::Creating)
            .expect("creating");
        not_started
            .persist(&config.daemon.state_dir)
            .expect("persist");

        let mut stopped = SandboxInstance::new(
            BackendKind::Mock,
            WorkloadClass::AgentTool,
            "sha256:stopped".into(),
            StartPath::Cold,
            "reconcile-test".into(),
        );
        stopped.id = stopped_id;
        stopped
            .transition(SandboxState::Creating)
            .expect("creating");
        stopped.transition(SandboxState::Running).expect("running");
        stopped.backend_ownership = BackendOwnership::Stopped;
        stopped.persist(&config.daemon.state_dir).expect("persist");

        for id in [not_started_id, stopped_id] {
            storage
                .acquire(&AcquireOpts {
                    instance_id: id.to_string(),
                    rootfs_size: 64,
                    mem_size: 32,
                })
                .await
                .expect("storage");
        }
        let kill_count = Arc::new(AtomicUsize::new(0));
        let orphan_cleanup_count = Arc::new(AtomicUsize::new(0));
        let state = build_test_state(
            config,
            test_policy(BackendKind::Mock, false),
            spawners(
                BackendKind::Mock,
                Arc::new(CountingSpawner {
                    kill_count: kill_count.clone(),
                    orphan_cleanup_count: orphan_cleanup_count.clone(),
                }),
            ),
            BackendKind::Mock,
            storage,
        );

        let report = state.manager.reconcile_startup().await;

        assert_eq!(report.attempted, 2);
        assert_eq!(report.completed, 2);
        assert!(report.failures.is_empty());
        assert_eq!(kill_count.load(Ordering::Acquire), 0);
        assert_eq!(orphan_cleanup_count.load(Ordering::Acquire), 0);
        assert_eq!(release_count.load(Ordering::Acquire), 2);
        assert_eq!(
            state.instances.lock().expect("instances")[&not_started_id].state,
            SandboxState::Destroyed
        );
        assert_eq!(
            state.instances.lock().expect("instances")[&stopped_id].state,
            SandboxState::Destroyed
        );
    }

    #[tokio::test]
    async fn template_routes_import_list_and_get_published_artifacts() {
        let temp = tempfile::tempdir().expect("temp");
        let import_root = temp.path().join("imports");
        let source = import_root.join("source");
        std::fs::create_dir(&import_root).expect("import root");
        std::fs::create_dir(&source).expect("source");
        std::fs::write(source.join("vmstate.snap"), b"snapshot").expect("snapshot");
        std::fs::write(source.join("mem.bin"), b"memory").expect("memory");
        std::fs::write(source.join("rootfs.ext4"), b"rootfs").expect("rootfs");

        let mut config = DaemonConfig::default();
        config.daemon.state_dir = temp.path().join("state");
        config.storage.images_dir = temp.path().join("images");
        config.storage.instances_dir = temp.path().join("instances");
        config.template.dir = temp.path().join("templates");
        config.template.import_root = Some(import_root);
        for directory in [
            &config.daemon.state_dir,
            &config.storage.images_dir,
            &config.storage.instances_dir,
            &config.template.dir,
        ] {
            std::fs::create_dir_all(directory).expect("directory");
        }
        let storage: Arc<dyn blaze_core::storage::StorageProvider> =
            Arc::new(FileStorageProvider::with_images(
                config.storage.images_dir.clone(),
                config.storage.instances_dir.clone(),
            ));
        let state = Arc::new(
            ServerState::build(
                config,
                PolicyEngine::with_policies(Vec::new()),
                PoolManager::new(),
                HookRegistry::new(),
                spawners(BackendKind::Mock, Arc::new(MockSpawner)),
                BackendKind::Mock,
                storage,
            )
            .expect("state"),
        );

        for (method, path) in [
            (Method::GET, "/v1/runtime-templates"),
            (Method::POST, "/v1/templates/gc"),
        ] {
            let error = dispatch(&method, path, "", Vec::new(), &state)
                .await
                .expect_err("retired template route");
            assert!(matches!(error, BlazeDaemonError::NotFound(_)));
        }

        let request = serde_json::to_vec(&json!({
            "name": "runtime-base",
            "source": "source",
            "description": "reusable runtime",
        }))
        .expect("request");
        let imported = dispatch(
            &Method::POST,
            "/v1/templates/import",
            "",
            request.clone(),
            &state,
        )
        .await
        .expect("import");
        assert_eq!(imported.status(), StatusCode::CREATED);
        let imported = serde_json::from_slice::<serde_json::Value>(
            &imported
                .into_body()
                .collect()
                .await
                .expect("body")
                .to_bytes(),
        )
        .expect("json");
        assert_eq!(imported["name"], "runtime-base");
        assert_eq!(imported["description"], "reusable runtime");

        let listed = dispatch(&Method::GET, "/v1/templates", "", Vec::new(), &state)
            .await
            .expect("list");
        let concurrent_list = dispatch(&Method::GET, "/v1/templates", "", Vec::new(), &state)
            .await
            .expect_err("list response must retain the single-flight permit");
        assert!(matches!(
            concurrent_list,
            BlazeDaemonError::ServiceUnavailable(_)
        ));
        let listed = serde_json::from_slice::<serde_json::Value>(
            &listed.into_body().collect().await.expect("body").to_bytes(),
        )
        .expect("json");
        assert_eq!(listed, json!([{ "name": "runtime-base" }]));

        dispatch(&Method::GET, "/v1/templates", "", Vec::new(), &state)
            .await
            .expect("list after response body release");

        let fetched = dispatch(
            &Method::GET,
            "/v1/templates/runtime-base",
            "",
            Vec::new(),
            &state,
        )
        .await
        .expect("get");
        assert_eq!(
            fetched.headers().get(CONTENT_TYPE).expect("content type"),
            "application/json"
        );
        let concurrent_get = dispatch(
            &Method::GET,
            "/v1/templates/runtime-base",
            "",
            Vec::new(),
            &state,
        )
        .await
        .expect_err("item response must retain the single-flight permit");
        assert!(matches!(
            concurrent_get,
            BlazeDaemonError::ServiceUnavailable(_)
        ));
        let fetched = serde_json::from_slice::<serde_json::Value>(
            &fetched
                .into_body()
                .collect()
                .await
                .expect("body")
                .to_bytes(),
        )
        .expect("json");
        assert_eq!(fetched, imported);

        dispatch(
            &Method::GET,
            "/v1/templates/runtime-base",
            "",
            Vec::new(),
            &state,
        )
        .await
        .expect("get after response body release");

        let duplicate = dispatch(&Method::POST, "/v1/templates/import", "", request, &state)
            .await
            .expect_err("duplicate");
        assert!(matches!(duplicate, BlazeDaemonError::Conflict(_)));
    }
}
