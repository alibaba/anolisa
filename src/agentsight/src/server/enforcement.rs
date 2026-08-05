//! HTTP boundary for local enforcement control and evidence queries.

#[cfg(test)]
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use actix_web::{HttpResponse, delete, get, post, web};
use agentsight_enforcement_protocol::{
    ApplyCredentialPolicy, ApplyPolicy, Binding, BindingState, CredentialExfiltrationPolicy,
    DestinationScope, HealthStatus, PolicyMode,
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use super::AppState;
use crate::enforcement::{
    EnforcementCoordinatorError, canonical_policy_file, read_process_start_time,
};

/// Bounded evidence list query.
#[derive(Debug, Deserialize)]
pub(super) struct ViolationQuery {
    /// Maximum returned events, clamped to `1..=1000`.
    limit: Option<usize>,
}

const FILE_POLICY_REVISION: &str = "agentsight-file-open-v1";

/// Product-level fields for binding a sensitive file to an agent process.
#[derive(Debug, Deserialize)]
pub(super) struct FileBindingRequest {
    agent_id: String,
    session_id: Option<String>,
    root_pid: i32,
    path: PathBuf,
}

/// Product fields for a taint-aware credential exfiltration binding.
#[derive(Debug, Deserialize)]
pub(super) struct CredentialBindingRequest {
    agent_id: String,
    session_id: Option<String>,
    root_pid: i32,
    source_path: PathBuf,
    trusted_endpoint: Option<String>,
    revision: u64,
    mode: PolicyMode,
    taint_ttl_secs: Option<u64>,
    destination_scope: DestinationScope,
}

/// Returns privileged backend readiness.
#[get("/enforcement/health")]
pub(super) async fn health(data: web::Data<AppState>) -> HttpResponse {
    let Some(coordinator) = data.enforcement.clone() else {
        return unavailable();
    };
    match web::block(move || coordinator.health()).await {
        Ok(Ok(health)) => HttpResponse::Ok().json(public_health(health)),
        Ok(Err(error)) => coordinator_error(error),
        Err(error) => blocking_error(error),
    }
}

/// Validates, persists, and applies one desired policy binding.
#[post("/enforcement/bindings")]
pub(super) async fn apply_binding(
    data: web::Data<AppState>,
    body: web::Json<ApplyPolicy>,
) -> HttpResponse {
    let Some(coordinator) = data.enforcement.clone() else {
        return unavailable();
    };
    if let Err(message) = validate_target_identity(body.root_pid, body.process_start_time) {
        log::warn!("rejecting enforcement target identity: {message}");
        return error_response(
            actix_web::http::StatusCode::BAD_REQUEST,
            "invalid_target",
            "target process identity is invalid",
            false,
        );
    }
    let request = body.into_inner();
    run_binding(move || coordinator.apply(request)).await
}

/// Builds and applies an AgentSight-owned file-open policy.
#[post("/enforcement/file-bindings")]
pub(super) async fn apply_file_binding(
    data: web::Data<AppState>,
    body: web::Json<FileBindingRequest>,
) -> HttpResponse {
    let Some(coordinator) = data.enforcement.clone() else {
        return unavailable();
    };
    let request = match build_file_binding(body.into_inner()) {
        Ok(request) => request,
        Err(message) => {
            log::warn!("rejecting file enforcement binding: {message}");
            return error_response(
                actix_web::http::StatusCode::BAD_REQUEST,
                "invalid_file_binding",
                "file enforcement binding is invalid",
                false,
            );
        }
    };
    run_binding(move || coordinator.apply(request)).await
}

/// Builds a product-level taint policy and delegates DSL compilation to the enforcer.
#[post("/enforcement/credential-bindings")]
pub(super) async fn apply_credential_binding(
    data: web::Data<AppState>,
    body: web::Json<CredentialBindingRequest>,
) -> HttpResponse {
    let Some(coordinator) = data.enforcement.clone() else {
        return unavailable();
    };
    let request = match build_credential_binding(body.into_inner()) {
        Ok(request) => request,
        Err(message) => {
            log::warn!("rejecting credential enforcement binding: {message}");
            return error_response(
                actix_web::http::StatusCode::BAD_REQUEST,
                "invalid_credential_binding",
                "credential enforcement binding is invalid",
                false,
            );
        }
    };
    let audit_service = Arc::clone(&data.audit_service);
    let policy = request.policy.clone();
    let registered =
        web::block(move || audit_service.register_policy_revision(&policy, unix_epoch_ns())).await;
    match registered {
        Ok(Ok(())) => {}
        Ok(Err(agentsight_audit::AuditError::PolicyRevisionConflict { .. })) => {
            return error_response(
                actix_web::http::StatusCode::CONFLICT,
                "policy_revision_conflict",
                "policy revision already exists with different contents",
                false,
            );
        }
        Ok(Err(error)) => {
            log::error!("credential policy revision persistence failed: {error}");
            return error_response(
                actix_web::http::StatusCode::SERVICE_UNAVAILABLE,
                "policy_revision_store_unavailable",
                "policy revision could not be persisted",
                true,
            );
        }
        Err(error) => return blocking_error(error),
    }
    run_binding(move || coordinator.apply_credential_policy(request)).await
}

fn unix_epoch_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(u128::from(u64::MAX)) as u64
}

/// Lists AgentSight's persisted desired binding states.
#[get("/enforcement/bindings")]
pub(super) async fn list_bindings(data: web::Data<AppState>) -> HttpResponse {
    let Some(coordinator) = data.enforcement.clone() else {
        return unavailable();
    };
    match web::block(move || coordinator.bindings()).await {
        Ok(Ok(bindings)) => HttpResponse::Ok().json(json!({
            "bindings": bindings.into_iter().map(public_binding).collect::<Vec<_>>()
        })),
        Ok(Err(error)) => coordinator_error(error),
        Err(error) => blocking_error(error),
    }
}

/// Detaches one binding after a privileged-service acknowledgement.
#[delete("/enforcement/bindings/{binding_id}")]
pub(super) async fn detach_binding(
    data: web::Data<AppState>,
    binding_id: web::Path<Uuid>,
) -> HttpResponse {
    let binding_id = binding_id.into_inner();
    if let Some(containment) = data.containment.clone() {
        match web::block(move || containment.remove_binding(binding_id)).await {
            Ok(Ok(true)) => return HttpResponse::NoContent().finish(),
            Ok(Ok(false)) => {}
            Ok(Err(error)) => {
                log::error!("containment binding restoration failed: {error}");
                return error_response(
                    actix_web::http::StatusCode::SERVICE_UNAVAILABLE,
                    "containment_restore_failed",
                    "containment policy restoration failed",
                    true,
                );
            }
            Err(error) => return blocking_error(error),
        }
    }
    let Some(coordinator) = data.enforcement.clone() else {
        return unavailable();
    };
    match web::block(move || coordinator.detach(binding_id)).await {
        Ok(Ok(())) => HttpResponse::NoContent().finish(),
        Ok(Err(error)) => coordinator_error(error),
        Err(error) => blocking_error(error),
    }
}

/// Lists newest normalized violation facts.
#[get("/enforcement/violations")]
pub(super) async fn list_violations(
    data: web::Data<AppState>,
    query: web::Query<ViolationQuery>,
) -> HttpResponse {
    let Some(coordinator) = data.enforcement.clone() else {
        return unavailable();
    };
    let limit = query.limit.unwrap_or(100).clamp(1, 1000);
    match web::block(move || coordinator.violations(limit)).await {
        Ok(Ok(violations)) => HttpResponse::Ok().json(json!({ "violations": violations })),
        Ok(Err(error)) => coordinator_error(error),
        Err(error) => blocking_error(error),
    }
}

async fn run_binding<F>(operation: F) -> HttpResponse
where
    F: FnOnce() -> Result<Binding, EnforcementCoordinatorError> + Send + 'static,
{
    match web::block(operation).await {
        Ok(Ok(binding)) => HttpResponse::Ok().json(public_binding(binding)),
        Ok(Err(error)) => coordinator_error(error),
        Err(error) => blocking_error(error),
    }
}

fn public_binding(mut binding: Binding) -> Binding {
    if let Some(message) = binding.message.take() {
        log::warn!(
            "redacting enforcement binding {} status detail: {message}",
            binding.request.binding_id
        );
        binding.message = Some(
            match binding.state {
                BindingState::Pending => "enforcement binding is pending",
                BindingState::Enforced => "enforcement binding is active",
                BindingState::Failed => "enforcement binding failed",
                BindingState::Degraded => "enforcement binding is degraded",
                BindingState::Detaching => "enforcement binding is detaching",
                BindingState::Detached => "enforcement binding is detached",
            }
            .into(),
        );
    }
    binding
}

fn public_health(mut status: HealthStatus) -> HealthStatus {
    if let Some(message) = status.message.take() {
        log::warn!(
            "redacting {} enforcement health detail: {message}",
            status.backend
        );
        if !status.ready {
            status.message = Some("enforcement backend is not ready".into());
        }
    }
    status
}

fn validate_target_identity(root_pid: i32, expected_start_time: u64) -> Result<(), String> {
    let actual_start_time = read_process_start_time(root_pid).map_err(|error| error.to_string())?;
    if actual_start_time != expected_start_time {
        return Err(format!(
            "PID {root_pid} start time changed: expected {expected_start_time}, found {actual_start_time}"
        ));
    }
    Ok(())
}

fn build_file_binding(request: FileBindingRequest) -> Result<ApplyPolicy, String> {
    let agent_id = request.agent_id.trim();
    if agent_id.is_empty() || agent_id.len() > 128 {
        return Err("agent_id must contain 1 to 128 characters".into());
    }
    let path = canonical_policy_file(&request.path).map_err(|error| error.to_string())?;
    let process_start_time =
        read_process_start_time(request.root_pid).map_err(|error| error.to_string())?;
    let binding_id = Uuid::new_v4();
    let path = path
        .to_str()
        .ok_or_else(|| "path must be valid UTF-8".to_string())?;
    let policy_dsl = format!(
        "source AGENT = exec \"**\"\n\
         rule agentsight-file-open:\n\
           block open file \"{path}\" if AGENT\n\
           because \"AgentSight sensitive file policy\"\n"
    );
    Ok(ApplyPolicy {
        binding_id,
        agent_id: agent_id.into(),
        session_id: request
            .session_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        root_pid: request.root_pid,
        process_start_time,
        policy_id: format!("agentsight-file-open:{binding_id}"),
        policy_revision: FILE_POLICY_REVISION.into(),
        policy_dsl,
        policy_mode: Some(PolicyMode::Enforce),
    })
}

fn build_credential_binding(
    request: CredentialBindingRequest,
) -> Result<ApplyCredentialPolicy, String> {
    let agent_id = request.agent_id.trim();
    if agent_id.is_empty() || agent_id.len() > 128 {
        return Err("agent_id must contain 1 to 128 characters".into());
    }
    let source_path =
        canonical_policy_file(&request.source_path).map_err(|error| error.to_string())?;
    let source_path = source_path
        .to_str()
        .ok_or_else(|| "source_path must be valid UTF-8".to_string())?
        .to_string();
    let process_start_time =
        read_process_start_time(request.root_pid).map_err(|error| error.to_string())?;
    let trusted_endpoints = request
        .trusted_endpoint
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .into_iter()
        .collect();
    let policy = CredentialExfiltrationPolicy {
        policy_id: "agentsight-credential-exfiltration".into(),
        revision: request.revision,
        source_patterns: vec![source_path],
        trusted_endpoints,
        taint_label: "CREDENTIAL".into(),
        taint_ttl_secs: request.taint_ttl_secs.unwrap_or(900),
        destination_scope: request.destination_scope,
        mode: request.mode,
    };
    policy.validate().map_err(|error| error.to_string())?;
    Ok(ApplyCredentialPolicy {
        binding_id: Uuid::new_v4(),
        agent_id: agent_id.into(),
        session_id: request
            .session_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        root_pid: request.root_pid,
        process_start_time,
        policy,
    })
}

fn coordinator_error(error: EnforcementCoordinatorError) -> HttpResponse {
    let (status, code, message, retryable) = match &error {
        EnforcementCoordinatorError::IngestionUnavailable => (
            actix_web::http::StatusCode::SERVICE_UNAVAILABLE,
            "enforcement_ingestion_unavailable",
            "enforcement evidence ingestion is unavailable",
            true,
        ),
        EnforcementCoordinatorError::EnforcementUnavailable(_) => (
            actix_web::http::StatusCode::SERVICE_UNAVAILABLE,
            "enforcement_unavailable",
            "enforcement service is unavailable",
            true,
        ),
        EnforcementCoordinatorError::Store(
            crate::enforcement::EnforcementStoreError::MissingBinding(_),
        ) => (
            actix_web::http::StatusCode::NOT_FOUND,
            "binding_not_found",
            "enforcement binding was not found",
            false,
        ),
        EnforcementCoordinatorError::Client(crate::enforcement::EnforcementError::Remote {
            code,
            ..
        }) if matches!(code.as_str(), "compile_failure" | "stale_process") => (
            actix_web::http::StatusCode::UNPROCESSABLE_ENTITY,
            code.as_str(),
            "enforcement policy was rejected",
            false,
        ),
        EnforcementCoordinatorError::Client(crate::enforcement::EnforcementError::Remote {
            code,
            ..
        }) if code == "binding_conflict" => (
            actix_web::http::StatusCode::CONFLICT,
            "binding_conflict",
            "enforcement binding conflicts with the current state",
            false,
        ),
        EnforcementCoordinatorError::Client(crate::enforcement::EnforcementError::Remote {
            code,
            ..
        }) if code == "missing_binding" => (
            actix_web::http::StatusCode::NOT_FOUND,
            "binding_not_found",
            "enforcement binding was not found",
            false,
        ),
        EnforcementCoordinatorError::Client(_) => (
            actix_web::http::StatusCode::SERVICE_UNAVAILABLE,
            "enforcer_unavailable",
            "enforcement service is unavailable",
            true,
        ),
        _ => (
            actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
            "enforcement_error",
            "enforcement operation failed",
            false,
        ),
    };
    log::error!("enforcement coordinator request failed: {error}");
    error_response(status, code, message, retryable)
}

fn unavailable() -> HttpResponse {
    error_response(
        actix_web::http::StatusCode::SERVICE_UNAVAILABLE,
        "enforcement_disabled",
        "enforcement coordinator is not configured",
        true,
    )
}

fn blocking_error(error: actix_web::error::BlockingError) -> HttpResponse {
    log::error!("enforcement blocking worker failed: {error}");
    error_response(
        actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
        "blocking_worker_failed",
        "enforcement worker failed",
        true,
    )
}

fn error_response(
    status: actix_web::http::StatusCode,
    code: &str,
    message: &str,
    retryable: bool,
) -> HttpResponse {
    HttpResponse::build(status).json(json!({
        "error": { "code": code, "message": message, "retryable": retryable }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_file_binding_from_product_fields() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("fixture process should start");
        let path = std::env::temp_dir().join(format!("agentsight-secret-{}", Uuid::new_v4()));
        fs::write(&path, b"fixture").expect("fixture file should exist");

        let binding = build_file_binding(FileBindingRequest {
            agent_id: " qoder ".into(),
            session_id: Some(" session-1 ".into()),
            root_pid: child.id() as i32,
            path: path.clone(),
        })
        .expect("valid request should build");

        assert_eq!(binding.agent_id, "qoder");
        assert_eq!(binding.session_id.as_deref(), Some("session-1"));
        assert_eq!(binding.root_pid, child.id() as i32);
        assert!(binding.process_start_time > 0);
        assert_eq!(binding.policy_revision, "agentsight-file-open-v1");
        assert!(binding.policy_id.starts_with("agentsight-file-open:"));
        assert!(binding.policy_dsl.contains("source AGENT = exec \"**\""));
        assert!(binding.policy_dsl.contains("block open file"));
        assert!(
            binding
                .policy_dsl
                .contains(path.canonicalize().unwrap().to_str().unwrap())
        );

        child.kill().expect("fixture process should stop");
        child.wait().expect("fixture process should exit");
        fs::remove_file(path).expect("fixture file should be removed");
    }

    #[test]
    fn builds_taint_aware_credential_binding() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("fixture process should start");
        let path = std::env::temp_dir().join(format!("agentsight-credential-{}", Uuid::new_v4()));
        fs::write(&path, b"fixture").expect("fixture file should exist");

        let binding = build_credential_binding(CredentialBindingRequest {
            agent_id: " qoder ".into(),
            session_id: Some(" session-1 ".into()),
            root_pid: child.id() as i32,
            source_path: path.clone(),
            trusted_endpoint: Some(" 10.0.0.8 ".into()),
            revision: 3,
            mode: PolicyMode::Audit,
            taint_ttl_secs: None,
            destination_scope: DestinationScope::PublicIpv4,
        })
        .expect("valid credential policy should build");

        assert_eq!(binding.agent_id, "qoder");
        assert_eq!(binding.session_id.as_deref(), Some("session-1"));
        assert_eq!(binding.policy.mode, PolicyMode::Audit);
        assert_eq!(binding.policy.revision, 3);
        assert_eq!(binding.policy.taint_label, "CREDENTIAL");
        assert_eq!(binding.policy.taint_ttl_secs, 900);
        assert_eq!(
            binding.policy.destination_scope,
            DestinationScope::PublicIpv4
        );
        assert_eq!(binding.policy.trusted_endpoints, ["10.0.0.8"]);
        assert_eq!(
            binding.policy.source_patterns,
            [path.canonicalize().unwrap().to_str().unwrap()]
        );

        child.kill().expect("fixture process should stop");
        child.wait().expect("fixture process should exit");
        fs::remove_file(path).expect("fixture file should be removed");
    }

    #[test]
    fn rejects_unsafe_file_binding_inputs() {
        let directory = std::env::temp_dir();
        assert!(
            build_file_binding(FileBindingRequest {
                agent_id: "".into(),
                session_id: None,
                root_pid: 1,
                path: directory,
            })
            .is_err()
        );

        assert!(canonical_policy_file(std::path::Path::new("relative/secret")).is_err());
        assert!(canonical_policy_file(std::path::Path::new("/tmp/quote\"secret")).is_err());
    }

    #[test]
    fn rejects_self_file_binding_target() {
        let path = std::env::temp_dir().join(format!("agentsight-secret-{}", Uuid::new_v4()));
        fs::write(&path, b"fixture").expect("fixture file should exist");

        assert!(
            build_file_binding(FileBindingRequest {
                agent_id: "qoder".into(),
                session_id: None,
                root_pid: std::process::id() as i32,
                path: path.clone(),
            })
            .is_err()
        );

        fs::remove_file(path).expect("fixture file should be removed");
    }

    #[test]
    fn rejects_protected_service_file_binding_targets() {
        let directory =
            std::env::temp_dir().join(format!("agentsight-protected-{}", Uuid::new_v4()));
        fs::create_dir(&directory).expect("fixture directory should exist");
        let path = directory.join("secret");
        fs::write(&path, b"fixture").expect("fixture file should exist");

        for process_name in ["agentsight", "agentsight-enforcer"] {
            let executable = directory.join(process_name);
            std::os::unix::fs::symlink("/bin/sleep", &executable)
                .expect("protected-service fixture should exist");
            let mut child = std::process::Command::new(&executable)
                .arg("30")
                .spawn()
                .expect("fixture process should start");
            let expected_process_name: String = process_name.chars().take(15).collect();
            let mut actual_process_name = String::new();
            for _ in 0..10 {
                let stat = fs::read_to_string(format!("/proc/{}/stat", child.id()))
                    .expect("fixture proc stat should exist");
                let open = stat
                    .find('(')
                    .expect("fixture proc stat should contain open");
                let close = stat
                    .rfind(')')
                    .expect("fixture proc stat should contain close");
                actual_process_name = stat[open + 1..close].into();
                if actual_process_name == expected_process_name {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert_eq!(actual_process_name, expected_process_name);

            assert!(
                build_file_binding(FileBindingRequest {
                    agent_id: "qoder".into(),
                    session_id: None,
                    root_pid: child.id() as i32,
                    path: path.clone(),
                })
                .is_err()
            );

            child.kill().expect("fixture process should stop");
            child.wait().expect("fixture process should exit");
            fs::remove_file(executable).expect("protected-service fixture should be removed");
        }

        fs::remove_file(path).expect("fixture file should be removed");
        fs::remove_dir(directory).expect("fixture directory should be removed");
    }

    #[test]
    fn rejects_init_and_self_targets_before_uds_calls() {
        assert!(validate_target_identity(1, 0).is_err());
        assert!(validate_target_identity(std::process::id() as i32, 0).is_err());
    }

    #[test]
    fn accepts_live_target_and_rejects_pid_reuse_marker() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let close = stat.rfind(')').unwrap();
        let start_time = stat[close + 1..]
            .split_whitespace()
            .nth(19)
            .unwrap()
            .parse::<u64>()
            .unwrap();

        assert!(validate_target_identity(pid, start_time).is_ok());
        assert!(validate_target_identity(pid, start_time + 1).is_err());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn maps_policy_rejections_to_actionable_http_statuses() {
        for (code, expected) in [
            (
                "compile_failure",
                actix_web::http::StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                "stale_process",
                actix_web::http::StatusCode::UNPROCESSABLE_ENTITY,
            ),
            ("binding_conflict", actix_web::http::StatusCode::CONFLICT),
            ("missing_binding", actix_web::http::StatusCode::NOT_FOUND),
        ] {
            let response = coordinator_error(EnforcementCoordinatorError::Client(
                crate::enforcement::EnforcementError::Remote {
                    code: code.into(),
                    message: "fixture rejection".into(),
                },
            ));
            assert_eq!(response.status(), expected);
        }
    }

    #[actix_web::test]
    async fn maps_ingestion_unavailable_to_retryable_service_unavailable() {
        let response = coordinator_error(EnforcementCoordinatorError::IngestionUnavailable);
        assert_eq!(
            response.status(),
            actix_web::http::StatusCode::SERVICE_UNAVAILABLE
        );
        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .expect("error response body should load");
        let value: serde_json::Value =
            serde_json::from_slice(&body).expect("error response should be JSON");
        assert_eq!(
            value["error"]["code"],
            serde_json::Value::String("enforcement_ingestion_unavailable".into())
        );
        assert_eq!(value["error"]["retryable"], serde_json::Value::Bool(true));
    }

    #[actix_web::test]
    async fn maps_combined_health_failure_to_retryable_service_unavailable() {
        let response = coordinator_error(EnforcementCoordinatorError::EnforcementUnavailable(
            "violation event delivery loss".into(),
        ));
        assert_eq!(
            response.status(),
            actix_web::http::StatusCode::SERVICE_UNAVAILABLE
        );
        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .expect("error response body should load");
        let value: serde_json::Value =
            serde_json::from_slice(&body).expect("error response should be JSON");
        assert_eq!(
            value["error"]["code"],
            serde_json::Value::String("enforcement_unavailable".into())
        );
        assert_eq!(value["error"]["retryable"], serde_json::Value::Bool(true));
    }

    #[actix_web::test]
    async fn coordinator_errors_do_not_expose_backend_details() {
        let response = coordinator_error(EnforcementCoordinatorError::Client(
            crate::enforcement::EnforcementError::Remote {
                code: "compile_failure".into(),
                message: "socket /run/agentsight/private.sock rejected /root/credential".into(),
            },
        ));

        assert_eq!(
            response.status(),
            actix_web::http::StatusCode::UNPROCESSABLE_ENTITY
        );
        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .expect("error response body should load");
        let text = String::from_utf8_lossy(&body);
        assert!(!text.contains("/run/agentsight/private.sock"));
        assert!(!text.contains("/root/credential"));
        let value: serde_json::Value =
            serde_json::from_slice(&body).expect("error response should be JSON");
        assert_eq!(value["error"]["code"], "compile_failure");
        assert_eq!(value["error"]["retryable"], false);
    }

    #[test]
    fn successful_binding_and_health_payloads_redact_internal_details() {
        let request = agentsight_enforcement_protocol::ApplyPolicy {
            binding_id: Uuid::new_v4(),
            agent_id: "agent-1".into(),
            session_id: None,
            root_pid: 42,
            process_start_time: 7,
            policy_id: "policy-1".into(),
            policy_revision: "revision-1".into(),
            policy_dsl: "label AGENT".into(),
            policy_mode: None,
        };
        let binding = public_binding(Binding {
            request,
            state: BindingState::Degraded,
            message: Some("socket /run/agentsight/private.sock failed".into()),
            domain_id: None,
        });
        let status = public_health(HealthStatus {
            ready: false,
            backend: "actplane".into(),
            capabilities: agentsight_enforcement_protocol::EnforcementCapabilities::actplane(),
            message: Some("database /root/private/enforcement.db failed".into()),
        });

        assert_eq!(
            binding.message.as_deref(),
            Some("enforcement binding is degraded")
        );
        assert_eq!(
            status.message.as_deref(),
            Some("enforcement backend is not ready")
        );
    }
}
