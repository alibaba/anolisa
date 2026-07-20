//! HTTP boundary for local enforcement control and evidence queries.

use std::fs;
use std::path::{Path, PathBuf};

use actix_web::{HttpResponse, delete, get, post, web};
use agentsight_enforcement_protocol::ApplyPolicy;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use super::AppState;
use crate::enforcement::EnforcementCoordinatorError;

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

/// Returns privileged backend readiness.
#[get("/enforcement/health")]
pub(super) async fn health(data: web::Data<AppState>) -> HttpResponse {
    let Some(coordinator) = data.enforcement.clone() else {
        return unavailable();
    };
    run_blocking(move || coordinator.health()).await
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
        return error_response(
            actix_web::http::StatusCode::BAD_REQUEST,
            "invalid_target",
            &message,
            false,
        );
    }
    let request = body.into_inner();
    run_blocking(move || coordinator.apply(request)).await
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
            return error_response(
                actix_web::http::StatusCode::BAD_REQUEST,
                "invalid_file_binding",
                &message,
                false,
            );
        }
    };
    run_blocking(move || coordinator.apply(request)).await
}

/// Lists AgentSight's persisted desired binding states.
#[get("/enforcement/bindings")]
pub(super) async fn list_bindings(data: web::Data<AppState>) -> HttpResponse {
    let Some(coordinator) = data.enforcement.clone() else {
        return unavailable();
    };
    match web::block(move || coordinator.bindings()).await {
        Ok(Ok(bindings)) => HttpResponse::Ok().json(json!({ "bindings": bindings })),
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
    let Some(coordinator) = data.enforcement.clone() else {
        return unavailable();
    };
    match web::block(move || coordinator.detach(binding_id.into_inner())).await {
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

async fn run_blocking<T, F>(operation: F) -> HttpResponse
where
    T: serde::Serialize + Send + 'static,
    F: FnOnce() -> Result<T, EnforcementCoordinatorError> + Send + 'static,
{
    match web::block(operation).await {
        Ok(Ok(value)) => HttpResponse::Ok().json(value),
        Ok(Err(error)) => coordinator_error(error),
        Err(error) => blocking_error(error),
    }
}

fn validate_target_identity(root_pid: i32, expected_start_time: u64) -> Result<(), String> {
    let actual_start_time = read_target_start_time(root_pid)?;
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
    let path = validate_policy_path(&request.path)?;
    let process_start_time = read_target_start_time(request.root_pid)?;
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
    })
}

fn validate_policy_path(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("path must be absolute".into());
    }
    validate_policy_path_text(path)?;
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize path {}: {error}", path.display()))?;
    let metadata = fs::metadata(&canonical)
        .map_err(|error| format!("cannot inspect path {}: {error}", canonical.display()))?;
    if !metadata.is_file() {
        return Err("path must identify an existing regular file".into());
    }
    validate_policy_path_text(&canonical)?;
    Ok(canonical)
}

fn validate_policy_path_text(path: &Path) -> Result<(), String> {
    let value = path
        .to_str()
        .ok_or_else(|| "path must be valid UTF-8".to_string())?;
    if value.contains(['\0', '"', '\r', '\n']) {
        return Err("path contains characters unsupported by the policy lexer".into());
    }
    Ok(())
}

fn read_target_start_time(root_pid: i32) -> Result<u64, String> {
    if root_pid <= 1 {
        return Err("root_pid must identify a non-init process".into());
    }
    if root_pid == std::process::id() as i32 {
        return Err("AgentSight cannot enforce itself".into());
    }
    let stat_path = format!("/proc/{root_pid}/stat");
    let stat = fs::read_to_string(&stat_path)
        .map_err(|error| format!("cannot read {stat_path}: {error}"))?;
    let open = stat
        .find('(')
        .ok_or_else(|| "invalid proc stat".to_string())?;
    let close = stat
        .rfind(')')
        .filter(|close| *close > open)
        .ok_or_else(|| "invalid proc stat".to_string())?;
    let process_name = &stat[open + 1..close];
    if matches!(
        process_name,
        "agentsight" | "agentsight-enfo" | "agentsight-enforcer"
    ) {
        return Err(format!("cannot target protected service {process_name}"));
    }
    stat[close + 1..]
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| "proc stat is missing start time".to_string())?
        .parse::<u64>()
        .map_err(|error| format!("invalid proc start time: {error}"))
}

fn coordinator_error(error: EnforcementCoordinatorError) -> HttpResponse {
    let (status, code, retryable) = match &error {
        EnforcementCoordinatorError::Store(
            crate::enforcement::EnforcementStoreError::MissingBinding(_),
        ) => (
            actix_web::http::StatusCode::NOT_FOUND,
            "binding_not_found",
            false,
        ),
        EnforcementCoordinatorError::Client(crate::enforcement::EnforcementError::Remote {
            code,
            ..
        }) if matches!(code.as_str(), "compile_failure" | "stale_process") => (
            actix_web::http::StatusCode::UNPROCESSABLE_ENTITY,
            code.as_str(),
            false,
        ),
        EnforcementCoordinatorError::Client(crate::enforcement::EnforcementError::Remote {
            code,
            ..
        }) if code == "binding_conflict" => (
            actix_web::http::StatusCode::CONFLICT,
            "binding_conflict",
            false,
        ),
        EnforcementCoordinatorError::Client(crate::enforcement::EnforcementError::Remote {
            code,
            ..
        }) if code == "missing_binding" => (
            actix_web::http::StatusCode::NOT_FOUND,
            "binding_not_found",
            false,
        ),
        EnforcementCoordinatorError::Client(_) => (
            actix_web::http::StatusCode::SERVICE_UNAVAILABLE,
            "enforcer_unavailable",
            true,
        ),
        _ => (
            actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
            "enforcement_error",
            false,
        ),
    };
    error_response(status, code, &error.to_string(), retryable)
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
    error_response(
        actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
        "blocking_worker_failed",
        &error.to_string(),
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

        assert!(validate_policy_path(std::path::Path::new("relative/secret")).is_err());
        assert!(validate_policy_path(std::path::Path::new("/tmp/quote\"secret")).is_err());
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
            let stat = fs::read_to_string(format!("/proc/{}/stat", child.id()))
                .expect("fixture proc stat should exist");
            let open = stat
                .find('(')
                .expect("fixture proc stat should contain open");
            let close = stat
                .rfind(')')
                .expect("fixture proc stat should contain close");
            let expected_process_name: String = process_name.chars().take(15).collect();
            assert_eq!(&stat[open + 1..close], expected_process_name);

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
}
