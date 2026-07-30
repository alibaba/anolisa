//! HTTP boundary for local enforcement control and evidence queries.

use std::fs;

use actix_web::{HttpResponse, delete, get, post, web};
use agentsight_enforcement_protocol::{ApplyPolicy, Binding, BindingState, HealthStatus};
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
    if matches!(process_name, "agentsight" | "agentsight-enforcer") {
        return Err(format!("cannot target protected service {process_name}"));
    }
    let actual_start_time = stat[close + 1..]
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| "proc stat is missing start time".to_string())?
        .parse::<u64>()
        .map_err(|error| format!("invalid proc start time: {error}"))?;
    if actual_start_time != expected_start_time {
        return Err(format!(
            "PID {root_pid} start time changed: expected {expected_start_time}, found {actual_start_time}"
        ));
    }
    Ok(())
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
}
