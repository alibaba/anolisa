//! HTTP boundary for case-level containment planning and activation.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use actix_web::http::StatusCode;
use actix_web::{HttpResponse, get, post, web};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{AppState, handlers};
use crate::enforcement::read_process_start_time;
use crate::health::{AgentHealthState, AgentHealthStatus, HealthStore};
use crate::security::{
    ContainmentAction, ContainmentCandidate, ContainmentCoordinator, ContainmentError,
    ContainmentPlan, ContainmentRequest,
};

const RECONCILE_INTERVAL: Duration = Duration::from_secs(5);
const REQUESTED_BY: &str = "dashboard";

#[derive(Debug, Deserialize)]
pub(super) struct ContainCaseRequest {
    root_pid: i32,
    duration_secs: Option<u64>,
}

#[derive(Debug)]
enum OperationError {
    Containment(ContainmentError),
    HealthStoreUnavailable,
}

/// Builds a confirmation plan from fresh trusted process health.
#[get("/audit/cases/{case_id}/containment-plan")]
pub(super) async fn containment_plan(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> HttpResponse {
    let case_id = match parse_case_id(path.into_inner()) {
        Ok(case_id) => case_id,
        Err(response) => return response,
    };
    let Some(coordinator) = data.containment.clone() else {
        return unavailable();
    };
    let health_store = Arc::clone(&data.health_store);
    match web::block(move || {
        let candidates = candidate_snapshot(&health_store)?;
        coordinator
            .plan(case_id, candidates)
            .map_err(OperationError::Containment)
    })
    .await
    {
        Ok(Ok(plan)) => {
            handlers::local_security_response(StatusCode::OK, "found", containment_plan_view(&plan))
        }
        Ok(Err(error)) => operation_error(error),
        Err(error) => blocking_error(error),
    }
}

/// Revalidates a selected process and activates case-derived enforcement.
#[post("/audit/cases/{case_id}/contain")]
pub(super) async fn contain_case(
    data: web::Data<AppState>,
    path: web::Path<String>,
    body: Result<web::Json<ContainCaseRequest>, actix_web::Error>,
) -> HttpResponse {
    let case_id = match parse_case_id(path.into_inner()) {
        Ok(case_id) => case_id,
        Err(response) => return response,
    };
    let request = match body {
        Ok(body) => body.into_inner(),
        Err(_) => return request_error("request body must be valid JSON"),
    };
    let Some(coordinator) = data.containment.clone() else {
        return unavailable();
    };
    let health_store = Arc::clone(&data.health_store);
    match web::block(move || {
        let candidates = candidate_snapshot(&health_store)?;
        coordinator
            .contain(
                case_id,
                ContainmentRequest {
                    root_pid: request.root_pid,
                    duration_secs: request.duration_secs,
                },
                &candidates,
                REQUESTED_BY,
            )
            .map_err(OperationError::Containment)
    })
    .await
    {
        Ok(Ok(action)) => handlers::local_security_response(
            StatusCode::OK,
            "contained",
            containment_action_view(&action),
        ),
        Ok(Err(error)) => operation_error(error),
        Err(error) => blocking_error(error),
    }
}

pub(super) fn start_reconciler(
    coordinator: &ContainmentCoordinator,
) -> Result<std::thread::JoinHandle<()>, ContainmentError> {
    coordinator.start_reconciler(RECONCILE_INTERVAL)
}

pub(super) fn stop_reconciler(
    coordinator: &ContainmentCoordinator,
    worker: std::thread::JoinHandle<()>,
) {
    coordinator.stop();
    if worker.join().is_err() {
        log::error!("containment reconciler panicked during shutdown");
    }
}

fn candidate_snapshot(
    health_store: &Arc<RwLock<HealthStore>>,
) -> Result<Vec<ContainmentCandidate>, OperationError> {
    let statuses = health_store
        .read()
        .map_err(|_| OperationError::HealthStoreUnavailable)?
        .all_agents();
    Ok(trusted_candidates(statuses))
}

fn trusted_candidates(statuses: Vec<AgentHealthStatus>) -> Vec<ContainmentCandidate> {
    let mut candidates = statuses
        .into_iter()
        .filter(|status| status.status != AgentHealthState::Offline)
        .filter_map(|status| {
            let agent_id = status.agent_name.trim();
            let root_pid = i32::try_from(status.pid).ok()?;
            if agent_id.is_empty() {
                return None;
            }
            let process_start_time = read_process_start_time(root_pid).ok()?;
            Some(ContainmentCandidate {
                agent_id: agent_id.to_string(),
                root_pid,
                process_start_time,
                display_name: status.agent_name,
            })
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        (
            &left.agent_id,
            left.root_pid,
            left.process_start_time,
            &left.display_name,
        )
            .cmp(&(
                &right.agent_id,
                right.root_pid,
                right.process_start_time,
                &right.display_name,
            ))
    });
    candidates.dedup();

    let mut agent_pids = BTreeMap::<String, BTreeSet<i32>>::new();
    for candidate in &candidates {
        agent_pids
            .entry(candidate.agent_id.clone())
            .or_default()
            .insert(candidate.root_pid);
    }
    candidates.retain(|candidate| {
        agent_pids
            .get(&candidate.agent_id)
            .is_some_and(|pids| pids.len() == 1)
    });
    candidates
}

fn containment_plan_view(plan: &ContainmentPlan) -> Value {
    json!({
        "case_id": plan.case_id,
        "original_target": plan.original_target.as_ref().map(containment_candidate_view),
        "original_target_valid": plan.original_target_valid,
        "candidates": plan.candidates.iter().map(containment_candidate_view).collect::<Vec<_>>(),
        "default_duration_secs": plan.default_duration_secs,
        "min_duration_secs": plan.min_duration_secs,
        "max_duration_secs": plan.max_duration_secs,
        "existing_action": plan.existing_action.as_ref().map(containment_action_view),
    })
}

fn containment_candidate_view(candidate: &ContainmentCandidate) -> Value {
    json!({
        "agent_id": candidate.agent_id,
        "root_pid": candidate.root_pid,
        "process_start_time": candidate.process_start_time,
        "display_name": candidate.display_name,
    })
}

fn containment_action_view(action: &ContainmentAction) -> Value {
    json!({
        "action_id": action.action_id,
        "case_id": action.case_id,
        "binding_id": action.binding_id,
        "agent_id": action.agent_id,
        "root_pid": action.root_pid,
        "process_start_time": action.process_start_time,
        "duration_secs": action.duration_secs,
        "expires_at_ns": action.expires_at_ns,
        "lifecycle_state": action.lifecycle_state,
        "blocked_at_ns": action.blocked_at_ns,
        "requested_by": action.requested_by,
        "failure_stage": action.failure_stage,
        "attempt_count": action.attempt_count,
        "next_retry_at_ns": action.next_retry_at_ns,
        "created_at_ns": action.created_at_ns,
        "updated_at_ns": action.updated_at_ns,
    })
}

fn parse_case_id(value: String) -> Result<Uuid, HttpResponse> {
    Uuid::parse_str(&value).map_err(|_| request_error("case_id must be a UUID"))
}

fn operation_error(error: OperationError) -> HttpResponse {
    match error {
        OperationError::Containment(error) => containment_error(error),
        OperationError::HealthStoreUnavailable => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "health_store_unavailable",
            "trusted Agent health is unavailable",
            true,
        ),
    }
}

fn containment_error(error: ContainmentError) -> HttpResponse {
    use ContainmentError::*;
    let (status, code, message, retryable) = match error {
        MissingCase(_) => (
            StatusCode::NOT_FOUND,
            "case_not_found",
            "audit case was not found",
            false,
        ),
        SourcePolicyUnavailable(_) => (
            StatusCode::CONFLICT,
            "source_policy_unavailable",
            "source policy is unavailable",
            false,
        ),
        RootProcessStale(_) => (
            StatusCode::CONFLICT,
            "root_process_stale",
            "selected process identity is stale",
            true,
        ),
        AmbiguousCandidate(_) => (
            StatusCode::CONFLICT,
            "ambiguous_candidate",
            "selected process identity is ambiguous",
            false,
        ),
        IneligibleCase { .. } => (
            StatusCode::CONFLICT,
            "case_not_eligible",
            "audit case is not eligible for containment",
            false,
        ),
        InvalidDuration => (
            StatusCode::BAD_REQUEST,
            "invalid_duration",
            "duration must be null or between 60 and 86400 seconds",
            false,
        ),
        InvalidRequestedBy => (
            StatusCode::BAD_REQUEST,
            "invalid_requester",
            "requester identity is invalid",
            false,
        ),
        IncompatibleAction(_) => (
            StatusCode::CONFLICT,
            "incompatible_action",
            "an incompatible containment action is active",
            false,
        ),
        ContainmentInProgress(_) => (
            StatusCode::CONFLICT,
            "action_in_progress",
            "containment action is still in progress",
            true,
        ),
        ContainmentExpiring(_) => (
            StatusCode::CONFLICT,
            "action_expiring",
            "containment action is expiring",
            true,
        ),
        CaseEligibilityChanged { .. } => (
            StatusCode::CONFLICT,
            "case_eligibility_changed",
            "audit case eligibility changed",
            false,
        ),
        CleanupRequired { .. } => (
            StatusCode::SERVICE_UNAVAILABLE,
            "cleanup_required",
            "containment cleanup is required",
            true,
        ),
        Enforcer(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "enforcer_unavailable",
            "enforcement service is unavailable",
            true,
        ),
        Store(error) => return handlers::security_store_error(error),
        AlreadyRunning => (
            StatusCode::CONFLICT,
            "reconciler_already_running",
            "containment reconciler is already running",
            true,
        ),
        ReconcilerThread(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "reconciler_unavailable",
            "containment reconciler is unavailable",
            true,
        ),
        RecoveryFailed { .. } => (
            StatusCode::SERVICE_UNAVAILABLE,
            "recovery_failed",
            "containment recovery failed",
            true,
        ),
        ClaimLost(_) => (
            StatusCode::CONFLICT,
            "claim_lost",
            "containment lifecycle claim was lost",
            true,
        ),
        CorruptActions { .. } => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "corrupt_actions",
            "stored containment actions are invalid",
            true,
        ),
    };
    error_response(status, code, message, retryable)
}

fn unavailable() -> HttpResponse {
    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "containment_disabled",
        "containment coordinator is not configured",
        true,
    )
}

fn request_error(message: &str) -> HttpResponse {
    error_response(StatusCode::BAD_REQUEST, "bad_request", message, false)
}

fn blocking_error(_: actix_web::error::BlockingError) -> HttpResponse {
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "blocking_worker_failed",
        "containment worker failed",
        true,
    )
}

fn error_response(status: StatusCode, code: &str, message: &str, retryable: bool) -> HttpResponse {
    HttpResponse::build(status).json(json!({
        "error": { "code": code, "message": message, "retryable": retryable }
    }))
}
