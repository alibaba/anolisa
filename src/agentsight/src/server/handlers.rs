//! API request handlers

use actix_web::http::StatusCode;
use actix_web::{HttpResponse, Responder, get, post, web};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::AppState;
use crate::grader::{
    EvaluationRequest, EvaluationResponse, GraderError, GraderType, RULE_GRADER_VERSION,
    RuleGrader, TargetType, load_conversation_input,
};
use crate::health::AgentHealthStatus;
use crate::security::{RiskCaseStatus, SecurityEventFilter, SecurityStoreError};
use crate::storage::sqlite::GenAISqliteStore;
use crate::storage::sqlite::genai::{ModelTimeseriesBucket, TimeseriesBucket};

// ─── Prometheus helpers ───────────────────────────────────────────────────────

/// Escape a Prometheus label value per the text format spec:
/// backslash → \\, double-quote → \", newline → \n
fn escape_label(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// GET /health — health check endpoint
#[get("/health")]
pub async fn health(data: web::Data<AppState>) -> impl Responder {
    HttpResponse::Ok().json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_seconds": data.start_time.elapsed().as_secs()
    }))
}

// ─── Authentication endpoints ────────────────────────────────────────────────

/// POST /api/auth/login
///
/// Accepts `{"token": "..."}` and sets a signed session cookie on success.
pub async fn auth_login(
    data: web::Data<AppState>,
    body: web::Json<serde_json::Value>,
) -> impl Responder {
    let candidate = body.get("token").and_then(|v| v.as_str()).unwrap_or("");

    if !data.auth.verify_token(candidate) {
        return HttpResponse::Unauthorized()
            .json(json!({"error": "invalid_token", "message": "The provided token is incorrect"}));
    }

    // Create a signed session cookie (24-hour TTL)
    let cookie_value = data.auth.create_session_cookie(24 * 3600);
    let cookie = actix_web::cookie::Cookie::build("agentsight_session", cookie_value)
        .path("/")
        .http_only(true)
        .same_site(actix_web::cookie::SameSite::Lax)
        .max_age(actix_web::cookie::time::Duration::hours(24))
        .finish();

    HttpResponse::Ok()
        .cookie(cookie)
        .json(json!({"status": "authenticated"}))
}

/// GET /api/auth/status
///
/// Returns whether authentication is enabled.  Exempt from auth middleware.
#[get("/status")]
pub async fn auth_status(data: web::Data<AppState>) -> impl Responder {
    HttpResponse::Ok().json(json!({
        "auth_enabled": data.auth.enabled,
        "mode": "linux",
        "capabilities": [
            "agent_observability",
            "sessions",
            "token_savings",
            "optimization",
            "skills",
            "security",
            "atif",
            "settings",
            "agent_health"
        ],
    }))
}

/// GET /api/auth/verify
///
/// Checks whether the current request carries a valid session.  Exempt from
/// auth middleware so the frontend can probe authentication state.
#[get("/verify")]
pub async fn auth_verify(data: web::Data<AppState>, req: actix_web::HttpRequest) -> impl Responder {
    if !data.auth.enabled {
        return HttpResponse::Ok().json(json!({"authenticated": true}));
    }

    // Loopback requests are trusted — consistent with AuthMiddleware bypass.
    let is_loopback = req
        .peer_addr()
        .map(|addr| addr.ip().is_loopback())
        .unwrap_or(false);
    if is_loopback {
        return HttpResponse::Ok().json(json!({"authenticated": true}));
    }

    // Check session cookie
    let authenticated = req
        .cookie("agentsight_session")
        .map(|c| data.auth.verify_session_cookie(c.value()))
        .unwrap_or(false);

    HttpResponse::Ok().json(json!({"authenticated": authenticated}))
}

// ─── Session / Trace query endpoints ───────────────────────────────────────

/// Query parameters for /api/sessions
#[derive(Debug, Deserialize)]
pub struct SessionQuery {
    /// Start of time range in nanoseconds (default: 24 h ago)
    pub start_ns: Option<i64>,
    /// End of time range in nanoseconds (default: now)
    pub end_ns: Option<i64>,
    /// Include auxiliary calls (recap, web_search) in results (default: false)
    pub include_auxiliary: Option<bool>,
}

/// GET /api/sessions?start_ns=<i64>&end_ns=<i64>
///
/// Returns a list of gen_ai.session_id values with aggregated stats.
#[get("/sessions")]
pub async fn list_sessions(
    data: web::Data<AppState>,
    query: web::Query<SessionQuery>,
) -> impl Responder {
    let db_path = &data.storage_path;

    let end_ns = query.end_ns.unwrap_or_else(|| now_ns() as i64);
    let start_ns = query
        .start_ns
        .unwrap_or_else(|| end_ns - 86_400_000_000_000i64); // 24 h

    match GenAISqliteStore::new_with_path(db_path) {
        Ok(store) => {
            match store.list_sessions(start_ns, end_ns, query.include_auxiliary.unwrap_or(false)) {
                Ok(sessions) => HttpResponse::Ok().json(sessions),
                Err(e) => HttpResponse::InternalServerError()
                    .json(serde_json::json!({"error": e.to_string()})),
            }
        }
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// GET /api/sessions/{session_id}/traces?start_ns=<i64>&end_ns=<i64>
///
/// Returns conversations belonging to a session with token stats.
/// Optional `start_ns`/`end_ns` query parameters filter conversations by time.
#[get("/sessions/{session_id}/traces")]
pub async fn list_traces_by_session(
    data: web::Data<AppState>,
    path: web::Path<String>,
    query: web::Query<TimeRangeQuery>,
) -> impl Responder {
    let db_path = &data.storage_path;
    let session_id = path.into_inner();

    let start_ns = query.start_ns;
    let end_ns = query.end_ns;

    match GenAISqliteStore::new_with_path(db_path) {
        Ok(store) => match store.list_traces_by_session(
            &session_id,
            start_ns,
            end_ns,
            query.include_auxiliary.unwrap_or(false),
        ) {
            Ok(traces) => HttpResponse::Ok().json(traces),
            Err(e) => HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()})),
        },
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// GET /api/traces/{trace_id}
///
/// Returns detailed LLM call events for a trace.
#[get("/traces/{trace_id}")]
pub async fn get_trace_detail(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    let db_path = &data.storage_path;
    let trace_id = path.into_inner();

    match GenAISqliteStore::new_with_path(db_path) {
        Ok(store) => match store.get_trace_events(&trace_id) {
            Ok(events) => HttpResponse::Ok().json(events),
            Err(e) => HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()})),
        },
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// GET /api/conversations/{conversation_id}
///
/// Returns detailed LLM call events for a conversation (user query).
#[get("/conversations/{conversation_id}")]
pub async fn get_conversation_events(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    let db_path = &data.storage_path;
    let conversation_id = path.into_inner();

    match GenAISqliteStore::new_with_path(db_path) {
        Ok(store) => match store.get_events_by_conversation(&conversation_id) {
            Ok(events) => HttpResponse::Ok().json(events),
            Err(e) => HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()})),
        },
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

// ─── Grader endpoints ────────────────────────────────────────────────────────

/// Query parameters for GET /api/grader/latest.
#[derive(Debug, Deserialize)]
pub struct GraderLatestQuery {
    pub target_type: String,
    pub target_id: String,
}

/// POST /api/grader/evaluate
///
/// Manually evaluate a conversation snapshot with the rule-based grader.
#[post("/grader/evaluate")]
pub async fn evaluate_grader(
    data: web::Data<AppState>,
    body: web::Json<EvaluationRequest>,
) -> impl Responder {
    let target_type = match parse_grader_target_type(&body.target_type) {
        Ok(target_type) => target_type,
        Err(error) => return grader_error_response(error),
    };

    if body.target_id.trim().is_empty() {
        return HttpResponse::BadRequest()
            .json(json!({"error": "bad_request", "message": "target_id is required"}));
    }

    let input = match load_conversation_input(
        &data.storage_path,
        data.interruption_store.as_deref(),
        &body.target_id,
        body.force,
    ) {
        Ok(input) => input,
        Err(error) => return grader_error_response(error),
    };
    let store = &data.evaluation_store;

    match store.find_completed(
        target_type,
        &body.target_id,
        &input.input_hash,
        GraderType::Rule,
        RULE_GRADER_VERSION,
    ) {
        Ok(Some(record)) => {
            if let Some(result) = record.result {
                return HttpResponse::Ok().json(EvaluationResponse {
                    result,
                    reused_existing_run: true,
                });
            }
        }
        Ok(None) => {}
        Err(error) => return grader_error_response(error),
    }

    let result = RuleGrader::evaluate(&input);
    match store.insert_completed(&result) {
        Ok(true) => {}
        Ok(false) => {
            return match store.find_completed(
                target_type,
                &body.target_id,
                &input.input_hash,
                GraderType::Rule,
                RULE_GRADER_VERSION,
            ) {
                Ok(Some(record)) => {
                    if let Some(result) = record.result {
                        HttpResponse::Ok().json(EvaluationResponse {
                            result,
                            reused_existing_run: true,
                        })
                    } else {
                        grader_error_response(GraderError::Storage(
                            "existing evaluation run is missing result_json".to_string(),
                        ))
                    }
                }
                Ok(None) => grader_error_response(GraderError::Storage(
                    "evaluation insert was ignored but no completed run was found".to_string(),
                )),
                Err(error) => grader_error_response(error),
            };
        }
        Err(error) => return grader_error_response(error),
    }

    HttpResponse::Ok().json(EvaluationResponse {
        result,
        reused_existing_run: false,
    })
}

/// GET /api/grader/latest?target_type=conversation&target_id=<id>
///
/// Return the latest completed evaluation result for a conversation.
#[get("/grader/latest")]
pub async fn latest_grader(
    data: web::Data<AppState>,
    query: web::Query<GraderLatestQuery>,
) -> impl Responder {
    let target_type = match parse_grader_target_type(&query.target_type) {
        Ok(target_type) => target_type,
        Err(error) => return grader_error_response(error),
    };

    if query.target_id.trim().is_empty() {
        return HttpResponse::BadRequest()
            .json(json!({"error": "bad_request", "message": "target_id is required"}));
    }

    match data
        .evaluation_store
        .latest_completed(target_type, &query.target_id)
    {
        Ok(Some(record)) => HttpResponse::Ok().json(record.result),
        Ok(None) => HttpResponse::Ok().json(serde_json::Value::Null),
        Err(error) => grader_error_response(error),
    }
}

fn parse_grader_target_type(value: &str) -> Result<TargetType, GraderError> {
    match value {
        "conversation" => Ok(TargetType::Conversation),
        other => Err(GraderError::UnsupportedTarget(other.to_string())),
    }
}

fn grader_error_response(error: GraderError) -> HttpResponse {
    match error {
        GraderError::ConversationNotFound(id) => HttpResponse::NotFound().json(json!({
            "error": "conversation_not_found",
            "message": format!("Conversation not found: {id}"),
        })),
        GraderError::ConversationNotReady { pending_count } => HttpResponse::Conflict().json(json!({
            "error": "conversation_not_ready",
            "message": "Conversation still has pending LLM calls. Retry after completion or use force=true.",
            "pending_call_count": pending_count,
        })),
        GraderError::UnsupportedTarget(target) => HttpResponse::BadRequest().json(json!({
            "error": "unsupported_target",
            "message": format!("Unsupported target_type: {target}. MVP supports only conversation."),
        })),
        GraderError::Storage(message) => HttpResponse::InternalServerError().json(json!({
            "error": "storage_error",
            "message": message,
        })),
        GraderError::Json(error) => HttpResponse::InternalServerError().json(json!({
            "error": "json_error",
            "message": error.to_string(),
        })),
    }
}

// ─── Agent-name & time-series endpoints ────────────────────────────────────

/// Query parameters shared by agent-name and time-series endpoints
#[derive(Debug, Deserialize)]
pub struct TimeRangeQuery {
    pub start_ns: Option<i64>,
    pub end_ns: Option<i64>,
    /// Include auxiliary calls (recap, web_search) in results (default: false)
    pub include_auxiliary: Option<bool>,
}

/// Query parameters for time-series endpoints
#[derive(Debug, Deserialize)]
pub struct TimeseriesQuery {
    pub start_ns: Option<i64>,
    pub end_ns: Option<i64>,
    /// Filter by a specific agent name (optional)
    pub agent_name: Option<String>,
    /// Number of buckets (default 30)
    pub buckets: Option<u32>,
}

/// GET /api/agent-names?start_ns=<i64>&end_ns=<i64>
///
/// Returns a sorted list of distinct agent_name values.
#[get("/agent-names")]
pub async fn list_agent_names(
    data: web::Data<AppState>,
    query: web::Query<TimeRangeQuery>,
) -> impl Responder {
    let db_path = &data.storage_path;
    let end_ns = query.end_ns.unwrap_or_else(|| now_ns() as i64);
    let start_ns = query
        .start_ns
        .unwrap_or_else(|| end_ns - 86_400_000_000_000i64);

    match GenAISqliteStore::new_with_path(db_path) {
        Ok(store) => match store.list_agent_names(start_ns, end_ns) {
            Ok(names) => HttpResponse::Ok().json(names),
            Err(e) => HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()})),
        },
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// Response body for /api/timeseries
#[derive(Debug, serde::Serialize)]
pub struct TimeseriesResponse {
    pub token_series: Vec<TimeseriesBucket>,
    pub model_series: Vec<ModelTimeseriesBucket>,
}

/// GET /api/timeseries?start_ns=<i64>&end_ns=<i64>&agent_name=<str>&buckets=<u32>
///
/// Returns time-bucketed token stats (input/output/total) and per-model total-token
/// breakdowns, both within the requested time range.
#[get("/timeseries")]
pub async fn get_timeseries(
    data: web::Data<AppState>,
    query: web::Query<TimeseriesQuery>,
) -> impl Responder {
    let db_path = &data.storage_path;
    let end_ns = query.end_ns.unwrap_or_else(|| now_ns() as i64);
    let start_ns = query
        .start_ns
        .unwrap_or_else(|| end_ns - 86_400_000_000_000i64);
    let buckets = query.buckets.unwrap_or(30);
    let agent_name = query.agent_name.as_deref();

    match GenAISqliteStore::new_with_path(db_path) {
        Ok(store) => {
            let token_series =
                match store.get_token_timeseries(start_ns, end_ns, agent_name, buckets) {
                    Ok(v) => v,
                    Err(e) => {
                        return HttpResponse::InternalServerError()
                            .json(serde_json::json!({"error": e.to_string()}));
                    }
                };
            let model_series =
                match store.get_model_timeseries(start_ns, end_ns, agent_name, buckets) {
                    Ok(v) => v,
                    Err(e) => {
                        return HttpResponse::InternalServerError()
                            .json(serde_json::json!({"error": e.to_string()}));
                    }
                };
            HttpResponse::Ok().json(TimeseriesResponse {
                token_series,
                model_series,
            })
        }
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Current UNIX time in nanoseconds
fn now_ns() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

// ─── AgentSight local security and system-audit endpoints ───────────────────

#[derive(Debug, Default, Deserialize)]
pub struct SecurityListQuery {
    start_ns: Option<u64>,
    end_ns: Option<u64>,
    event_type: Option<String>,
    result: Option<String>,
    policy_id: Option<String>,
    agent_id: Option<String>,
    session_id: Option<String>,
    binding_id: Option<uuid::Uuid>,
    limit: Option<usize>,
    offset: Option<i64>,
    group_by: Option<String>,
}

/// GET /api/security/status
///
/// Reports whether the AgentSight-owned local security store is ready.
#[get("/security/status")]
pub async fn security_status(data: web::Data<AppState>) -> impl Responder {
    let summary = match data.security_store.summary() {
        Ok(summary) => summary,
        Err(error) => {
            return HttpResponse::InternalServerError().json(json!({
                "error": {
                    "code": "security_store_unavailable",
                    "message": error.to_string(),
                    "retryable": true,
                }
            }));
        }
    };

    local_security_response(
        StatusCode::OK,
        "local_ready",
        json!({
            "source": "agentsight",
            "total_events": summary.total_events,
            "blocked_events": summary.blocked_events,
        }),
    )
}

pub(super) fn local_security_response(
    status: StatusCode,
    state: &str,
    data: Value,
) -> HttpResponse {
    HttpResponse::build(status).json(json!({
        "state": state,
        "data": data,
        "meta": { "source": "agentsight" },
    }))
}

/// GET /api/security/summary
#[get("/security/summary")]
pub async fn security_summary(
    data: web::Data<AppState>,
    query: web::Query<SecurityListQuery>,
) -> impl Responder {
    let filter = security_filter(&query);
    let events = match data.security_store.list_events(&SecurityEventFilter {
        limit: query.limit.unwrap_or(5).clamp(1, 100),
        ..filter
    }) {
        Ok(page) => page.items,
        Err(error) => return security_store_error(error),
    };
    let summary = match data.security_store.summary() {
        Ok(summary) => summary,
        Err(error) => return security_store_error(error),
    };
    let by_event_type = count_map(&data, "event_type");
    let by_result = count_map(&data, "result");
    let affected_sessions = events
        .iter()
        .filter_map(|event| event.identity.session_id.as_deref())
        .collect::<std::collections::HashSet<_>>()
        .len();
    local_security_response(
        StatusCode::OK,
        if summary.total_events == 0 {
            "empty"
        } else {
            "ok"
        },
        json!({
            "total": summary.total_events,
            "blocked": summary.blocked_events,
            "evidence_loss": summary.evidence_loss_events,
            "by_category": by_event_type.clone(),
            "by_event_type": by_event_type,
            "by_result": by_result,
            "affected_sessions": affected_sessions,
            "affected_runs": affected_sessions,
            "latest_events": events.iter().map(security_event_view).collect::<Vec<_>>(),
        }),
    )
}

/// GET /api/security/events/count-by
#[get("/security/events/count-by")]
pub async fn security_events_count_by(
    data: web::Data<AppState>,
    query: web::Query<SecurityListQuery>,
) -> impl Responder {
    let requested = query.group_by.as_deref().unwrap_or("event_type");
    let field = match requested {
        "category" => "event_type",
        "verdict" => "result",
        value => value,
    };
    match data.security_store.count_by(field) {
        Ok(items) => local_security_response(
            StatusCode::OK,
            if items.is_empty() { "empty" } else { "ok" },
            json!({
                "group_by": requested,
                "items": items.into_iter().map(|item| json!({
                    "value": item.key,
                    "count": item.count,
                })).collect::<Vec<_>>(),
            }),
        ),
        Err(error) => security_store_error(error),
    }
}

/// GET /api/security/events
#[get("/security/events")]
pub async fn security_events_list(
    data: web::Data<AppState>,
    query: web::Query<SecurityListQuery>,
) -> impl Responder {
    match data.security_store.list_events(&security_filter(&query)) {
        Ok(page) => {
            let total = page.offset.max(0) as usize + page.items.len();
            let state = if page.items.is_empty() { "empty" } else { "ok" };
            local_security_response(
                StatusCode::OK,
                state,
                json!({
                    "items": page.items.iter().map(security_event_view).collect::<Vec<_>>(),
                    "total": total,
                    "limit": page.limit,
                    "offset": page.offset,
                    "next_offset": (page.items.len() == page.limit)
                        .then_some(page.offset + page.limit as i64),
                }),
            )
        }
        Err(error) => security_store_error(error),
    }
}

/// GET /api/security/events/{event_id}
#[get("/security/events/{event_id}")]
pub async fn security_event_detail(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    let event_id = match uuid::Uuid::parse_str(&path.into_inner()) {
        Ok(event_id) => event_id,
        Err(_) => return bad_request_response("event_id must be a UUID".into()),
    };
    match data.security_store.event(event_id) {
        Ok(Some(event)) => local_security_response(
            StatusCode::OK,
            "found",
            json!({ "found": true, "event": security_event_view(&event) }),
        ),
        Ok(None) => local_security_response(
            StatusCode::NOT_FOUND,
            "not_found",
            json!({ "found": false, "event": Value::Null }),
        ),
        Err(error) => security_store_error(error),
    }
}

/// GET /api/security/observability/sessions
#[get("/security/observability/sessions")]
pub async fn security_observability_sessions(
    data: web::Data<AppState>,
    query: web::Query<SecurityListQuery>,
) -> impl Responder {
    let page = match data.security_store.list_events(&SecurityEventFilter {
        limit: 1_000,
        ..security_filter(&query)
    }) {
        Ok(page) => page,
        Err(error) => return security_store_error(error),
    };
    let mut sessions = std::collections::BTreeMap::<String, (u64, u64, u64)>::new();
    for event in page.items {
        if let Some(session_id) = event.identity.session_id {
            let entry = sessions.entry(session_id).or_insert((
                event.occurred_at_ns,
                event.occurred_at_ns,
                0,
            ));
            entry.0 = entry.0.min(event.occurred_at_ns);
            entry.1 = entry.1.max(event.occurred_at_ns);
            entry.2 += 1;
        }
    }
    let items = sessions
        .into_iter()
        .map(|(session_id, (first, last, count))| {
            json!({
                "session_id": session_id,
                "first_seen_ns": first,
                "last_seen_ns": last,
                "security_event_count": count,
                "observability_event_count": 0,
            })
        })
        .collect::<Vec<_>>();
    paginated_local_response(items, query.limit, query.offset)
}

/// GET /api/security/observability/sessions/{session_id}/runs
#[get("/security/observability/sessions/{session_id}/runs")]
pub async fn security_observability_runs(
    data: web::Data<AppState>,
    path: web::Path<String>,
    query: web::Query<SecurityListQuery>,
) -> impl Responder {
    let session_id = path.into_inner();
    let count = match data.security_store.list_events(&SecurityEventFilter {
        session_id: Some(session_id.clone()),
        limit: 1_000,
        ..security_filter(&query)
    }) {
        Ok(page) => page.items.len(),
        Err(error) => return security_store_error(error),
    };
    let items = if count == 0 {
        Vec::new()
    } else {
        vec![json!({
            "run_id": session_id,
            "security_event_count": count,
            "observability_event_count": 0,
        })]
    };
    paginated_local_response(items, query.limit, query.offset)
}

/// GET /api/security/observability/timeline
#[get("/security/observability/timeline")]
pub async fn security_observability_timeline(
    data: web::Data<AppState>,
    query: web::Query<SecurityListQuery>,
) -> impl Responder {
    let session_id = match query.session_id.as_deref() {
        Some(value) if !value.is_empty() => value,
        _ => return bad_request_response("session_id is required".into()),
    };
    match data.security_store.list_events(&SecurityEventFilter {
        session_id: Some(session_id.to_string()),
        ..security_filter(&query)
    }) {
        Ok(page) => local_security_response(
            StatusCode::OK,
            if page.items.is_empty() { "empty" } else { "ok" },
            json!({
                "session_id": session_id,
                "run_id": session_id,
                "items": page.items.iter().rev().map(|event| json!({
                    "kind": "security_event",
                    "id": event.event_id,
                    "timestamp_ns": event.occurred_at_ns,
                    "session_id": event.identity.session_id,
                    "tool_call_id": event.identity.tool_call_id,
                    "event": security_event_view(event),
                })).collect::<Vec<_>>(),
            }),
        ),
        Err(error) => security_store_error(error),
    }
}

/// GET /api/audit/cases
#[get("/audit/cases")]
pub async fn audit_cases_list(
    data: web::Data<AppState>,
    query: web::Query<SecurityListQuery>,
) -> impl Responder {
    let limit = query.limit.unwrap_or(100).clamp(1, 1_000);
    let offset = query.offset.unwrap_or(0).max(0);
    match data.security_store.list_cases(limit, offset) {
        Ok(items) => local_security_response(
            StatusCode::OK,
            if items.is_empty() { "empty" } else { "ok" },
            json!({ "total": items.len(), "items": items, "limit": limit, "offset": offset }),
        ),
        Err(error) => security_store_error(error),
    }
}

/// GET /api/audit/cases/{case_id}
#[get("/audit/cases/{case_id}")]
pub async fn audit_case_detail(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    let case_id = match uuid::Uuid::parse_str(&path.into_inner()) {
        Ok(case_id) => case_id,
        Err(_) => return bad_request_response("case_id must be a UUID".into()),
    };
    match data.security_store.case_detail(case_id) {
        Ok(detail) => match data.security_store.latest_containment_action(case_id) {
            Ok(action) => local_security_response(
                StatusCode::OK,
                "found",
                super::containment::case_detail_view(json!(detail), action.as_ref()),
            ),
            Err(error) => security_store_error(error),
        },
        Err(SecurityStoreError::MissingCase(_)) => local_security_response(
            StatusCode::NOT_FOUND,
            "not_found",
            json!({ "case_id": case_id }),
        ),
        Err(error) => security_store_error(error),
    }
}

#[derive(Debug, Deserialize)]
pub struct ReviewCaseRequest {
    status: RiskCaseStatus,
}

/// POST /api/audit/cases/{case_id}/review
#[post("/audit/cases/{case_id}/review")]
pub async fn review_audit_case(
    data: web::Data<AppState>,
    path: web::Path<String>,
    body: web::Json<ReviewCaseRequest>,
) -> impl Responder {
    let case_id = match uuid::Uuid::parse_str(&path.into_inner()) {
        Ok(case_id) => case_id,
        Err(_) => return bad_request_response("case_id must be a UUID".into()),
    };
    if body.status == RiskCaseStatus::Open {
        return bad_request_response(
            "status must be confirmed, false_positive, accepted_risk, or resolved".into(),
        );
    }
    match data
        .security_store
        .review_case(case_id, body.status, now_ns())
    {
        Ok(case) => local_security_response(StatusCode::OK, "updated", json!(case)),
        Err(SecurityStoreError::MissingCase(_)) => local_security_response(
            StatusCode::NOT_FOUND,
            "not_found",
            json!({ "case_id": case_id }),
        ),
        Err(error) => security_store_error(error),
    }
}

fn security_filter(query: &SecurityListQuery) -> SecurityEventFilter {
    SecurityEventFilter {
        start_ns: query.start_ns,
        end_ns: query.end_ns,
        event_type: query.event_type.clone(),
        result: query.result.clone(),
        policy_id: query.policy_id.clone(),
        agent_id: query.agent_id.clone(),
        session_id: query.session_id.clone(),
        binding_id: query.binding_id,
        limit: query.limit.unwrap_or(100),
        offset: query.offset.unwrap_or(0),
    }
}

fn security_event_view(event: &agentsight_enforcement_protocol::SecurityEvent) -> Value {
    let mut value = serde_json::to_value(event).unwrap_or(Value::Null);
    if let Value::Object(fields) = &mut value {
        fields.insert("timestamp_ns".into(), json!(event.occurred_at_ns));
        fields.insert("session_id".into(), json!(event.identity.session_id));
        fields.insert("tool_call_id".into(), json!(event.identity.tool_call_id));
        fields.insert("pid".into(), json!(event.identity.pid));
        fields.insert("category".into(), json!("system"));
        fields.insert("result".into(), json!(security_event_result(event)));
    }
    value
}

fn security_event_result(event: &agentsight_enforcement_protocol::SecurityEvent) -> &'static str {
    use agentsight_enforcement_protocol::SecurityEventKind;
    match &event.kind {
        SecurityEventKind::FileAction(action) => {
            if action.succeeded {
                "allowed"
            } else {
                "failed"
            }
        }
        SecurityEventKind::TaintTransition(_) => "changed",
        SecurityEventKind::NetworkAction(action) => {
            if action.succeeded {
                "allowed"
            } else {
                "blocked"
            }
        }
        SecurityEventKind::PolicyDecision(decision) => {
            if decision.blocked {
                "blocked"
            } else {
                "allowed"
            }
        }
        SecurityEventKind::EnforcementState(state) => {
            if state.ready {
                "ready"
            } else {
                "degraded"
            }
        }
    }
}

fn count_map(data: &web::Data<AppState>, field: &str) -> serde_json::Map<String, Value> {
    data.security_store
        .count_by(field)
        .unwrap_or_default()
        .into_iter()
        .map(|item| (item.key, json!(item.count)))
        .collect()
}

fn paginated_local_response(
    items: Vec<Value>,
    requested_limit: Option<usize>,
    requested_offset: Option<i64>,
) -> HttpResponse {
    let limit = requested_limit.unwrap_or(100).clamp(1, 1_000);
    let offset = requested_offset.unwrap_or(0).max(0);
    local_security_response(
        StatusCode::OK,
        if items.is_empty() { "empty" } else { "ok" },
        json!({ "total": items.len(), "items": items, "limit": limit, "offset": offset }),
    )
}

pub(super) fn security_store_error(error: SecurityStoreError) -> HttpResponse {
    log::error!("local security store request failed: {error}");
    local_error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "security_store_unavailable",
        "local security persistence is unavailable",
        true,
    )
}

fn bad_request_response(message: String) -> HttpResponse {
    local_error_response(StatusCode::BAD_REQUEST, "bad_request", &message, false)
}

/// Builds the stable error envelope shared by local security endpoints.
pub(super) fn local_error_response(
    status: StatusCode,
    code: &str,
    message: &str,
    retryable: bool,
) -> HttpResponse {
    HttpResponse::build(status).json(json!({
        "error": {
            "code": code,
            "message": message,
            "retryable": retryable,
        }
    }))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, RwLock};
    use std::time::Instant;

    use actix_web::App;
    use actix_web::body::to_bytes;
    use actix_web::test as awtest;

    use crate::genai::GenAIExporter;
    use crate::genai::semantic::{
        GenAISemanticEvent, LLMCall, LLMRequest, LLMResponse, MessagePart, OutputMessage,
        TokenUsage,
    };
    use crate::grader::EvaluationStore;
    use crate::health::HealthStore;
    use crate::security::{RiskCase, RiskCaseStatus, RiskSeverity};
    use crate::storage::sqlite::genai::{PendingCallInfo, PendingOrigin};
    use agentsight_enforcement_protocol::{
        EventIdentity, FileAction, SecurityEvent, SecurityEventKind,
    };
    use agentsight_trajectory_collector::{TrajectoryRecord, TrajectoryStore};
    use uuid::Uuid;

    use super::*;

    #[actix_web::test]
    async fn security_status_is_local_ready_without_agent_sec_socket() {
        let app = awtest::init_service(
            App::new()
                .app_data(test_app_state(0))
                .service(security_status),
        )
        .await;

        let response = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/security/status")
                .to_request(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = service_response_json(response).await;
        assert_eq!(body["state"], "local_ready");
        assert_eq!(body["data"]["source"], "agentsight");
    }

    #[actix_web::test]
    async fn security_case_detail_returns_ordered_evidence() {
        let data = test_app_state(0);
        let case_id = Uuid::new_v4();
        let mut evidence_ids = Vec::new();
        for (position, event_type) in ["read", "taint", "connect", "decision"]
            .into_iter()
            .enumerate()
        {
            let event_id = Uuid::new_v4();
            let event = SecurityEvent {
                event_id,
                occurred_at_ns: position as u64 + 1,
                observed_at_ns: position as u64 + 1,
                identity: EventIdentity {
                    binding_id: Uuid::new_v4(),
                    agent_id: "hermes-test".into(),
                    agent_name: Some("Hermes".into()),
                    session_id: Some("session-1".into()),
                    conversation_id: None,
                    tool_call_id: Some("tool-1".into()),
                    pid: 4242,
                    process_start_time: 99,
                    ppid: None,
                    cgroup_id: None,
                    protocol_version: agentsight_enforcement_protocol::PROTOCOL_VERSION,
                    enforcer_version: "test".into(),
                    actplane_revision: "test".into(),
                },
                kind: SecurityEventKind::FileAction(FileAction {
                    policy_id: "credential-exfiltration".into(),
                    policy_revision: 1,
                    operation: event_type.into(),
                    path: "~/.ssh/id_rsa".into(),
                    resource_class: "credential".into(),
                    succeeded: true,
                    errno: None,
                    rule_id: None,
                }),
            };
            data.security_store.insert_event(&event).unwrap();
            evidence_ids.push(event_id);
        }
        data.security_store
            .upsert_case(
                &RiskCase {
                    case_id,
                    correlation_key: "fixture-case".into(),
                    policy_id: "credential-exfiltration".into(),
                    policy_revision: 1,
                    agent_id: "hermes-test".into(),
                    session_id: Some("session-1".into()),
                    severity: RiskSeverity::High,
                    risk_score: 85,
                    status: RiskCaseStatus::Open,
                    blocked: false,
                    opened_at_ns: 1,
                    updated_at_ns: 4,
                    summary: "Suspected credential exfiltration".into(),
                },
                &evidence_ids,
            )
            .unwrap();
        let app = awtest::init_service(
            App::new()
                .app_data(data)
                .configure(super::super::configure_routes),
        )
        .await;

        let response = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri(&format!("/api/audit/cases/{case_id}"))
                .to_request(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = service_response_json(response).await;
        assert_eq!(body["data"]["evidence"].as_array().map(Vec::len), Some(4));
        assert!(body["data"]["containment"].is_null());
    }

    #[actix_web::test]
    async fn security_case_review_updates_only_review_status() {
        let data = test_app_state(0);
        let case_id = Uuid::new_v4();
        data.security_store
            .upsert_case(
                &RiskCase {
                    case_id,
                    correlation_key: "review-fixture".into(),
                    policy_id: "credential-exfiltration".into(),
                    policy_revision: 7,
                    agent_id: "hermes-test".into(),
                    session_id: Some("session-1".into()),
                    severity: RiskSeverity::High,
                    risk_score: 85,
                    status: RiskCaseStatus::Open,
                    blocked: false,
                    opened_at_ns: 1,
                    updated_at_ns: 1,
                    summary: "Suspected credential exfiltration".into(),
                },
                &[],
            )
            .unwrap();
        let app = awtest::init_service(
            App::new()
                .app_data(data)
                .configure(super::super::configure_routes),
        )
        .await;

        let response = awtest::call_service(
            &app,
            awtest::TestRequest::post()
                .uri(&format!("/api/audit/cases/{case_id}/review"))
                .set_json(json!({ "status": "confirmed" }))
                .to_request(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = service_response_json(response).await;
        assert_eq!(body["data"]["status"], "confirmed");
        assert_eq!(body["data"]["policy_revision"], 7);
    }

    #[actix_web::test]
    async fn latest_grader_uses_shared_evaluation_store() {
        let root = temp_root("latest_grader_shared_store");
        let evaluation_path = root.join("evaluation.db");
        let evaluation_store = Arc::new(EvaluationStore::new_with_path(&evaluation_path).unwrap());
        let result = test_evaluation_result("conv-shared");
        evaluation_store.insert_completed(&result).unwrap();

        let blocked_parent = root.join("not-a-directory");
        std::fs::write(&blocked_parent, b"file").unwrap();
        let auth_config = crate::config::ServerAuthConfig { enabled: false };
        let auth = Arc::new(crate::server::auth::DashboardAuth::init(
            &auth_config,
            std::path::Path::new("/tmp"),
        ));
        let data = web::Data::new(AppState {
            storage_path: blocked_parent.join("genai.db"),
            start_time: Instant::now(),
            health_store: Arc::new(RwLock::new(HealthStore::new())),
            interruption_store: None,
            evaluation_store: Arc::clone(&evaluation_store),
            enforcement: None,
            containment: None,
            security_store: Arc::new(crate::security::SecurityStore::open_in_memory().unwrap()),
            security_observability: super::super::SecurityObservabilityConfig { timeout_ms: 0 },
            auth,
            optimize: None,
            trajectory_store: Arc::new(RwLock::new(None)),
        });
        let app = awtest::init_service(App::new().app_data(data).service(latest_grader)).await;

        let response = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/grader/latest?target_type=conversation&target_id=conv-shared")
                .to_request(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &actix_web::body::to_bytes(response.into_body())
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["run_id"], "run-shared");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[actix_web::test]
    async fn evaluate_grader_reuses_existing_run_for_same_snapshot() {
        let root = temp_root("evaluate_grader_reuses_run");
        let db_path = root.join("events.db");
        write_completed_conversation_event(&db_path, "conv-reuse");
        let data = grader_app_state(db_path.clone());
        let app = awtest::init_service(App::new().app_data(data).service(evaluate_grader)).await;
        let request = EvaluationRequest {
            target_type: "conversation".to_string(),
            target_id: "conv-reuse".to_string(),
            force: false,
        };

        let first = awtest::call_service(
            &app,
            awtest::TestRequest::post()
                .uri("/grader/evaluate")
                .set_json(&request)
                .to_request(),
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);
        let first_body = service_response_json(first).await;
        assert_eq!(first_body["reused_existing_run"], false);
        let run_id = first_body["result"]["run_id"]
            .as_str()
            .expect("run_id should be present")
            .to_string();

        let second = awtest::call_service(
            &app,
            awtest::TestRequest::post()
                .uri("/grader/evaluate")
                .set_json(&request)
                .to_request(),
        )
        .await;
        assert_eq!(second.status(), StatusCode::OK);
        let second_body = service_response_json(second).await;
        assert_eq!(second_body["reused_existing_run"], true);
        assert_eq!(second_body["result"]["run_id"], run_id);

        cleanup_db(&db_path);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[actix_web::test]
    async fn evaluate_grader_maps_bad_request_not_found_and_pending() {
        let root = temp_root("evaluate_grader_errors");
        let db_path = root.join("events.db");
        write_pending_conversation_event(&db_path, "conv-pending");
        let data = grader_app_state(db_path.clone());
        let app = awtest::init_service(App::new().app_data(data).service(evaluate_grader)).await;

        let bad_request = awtest::call_service(
            &app,
            awtest::TestRequest::post()
                .uri("/grader/evaluate")
                .set_json(&EvaluationRequest {
                    target_type: "trace".to_string(),
                    target_id: "conv-pending".to_string(),
                    force: false,
                })
                .to_request(),
        )
        .await;
        assert_eq!(bad_request.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            service_response_json(bad_request).await["error"],
            "unsupported_target"
        );

        let not_found = awtest::call_service(
            &app,
            awtest::TestRequest::post()
                .uri("/grader/evaluate")
                .set_json(&EvaluationRequest {
                    target_type: "conversation".to_string(),
                    target_id: "missing-conv".to_string(),
                    force: false,
                })
                .to_request(),
        )
        .await;
        assert_eq!(not_found.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            service_response_json(not_found).await["error"],
            "conversation_not_found"
        );

        let pending = awtest::call_service(
            &app,
            awtest::TestRequest::post()
                .uri("/grader/evaluate")
                .set_json(&EvaluationRequest {
                    target_type: "conversation".to_string(),
                    target_id: "conv-pending".to_string(),
                    force: false,
                })
                .to_request(),
        )
        .await;
        assert_eq!(pending.status(), StatusCode::CONFLICT);
        let pending_body = service_response_json(pending).await;
        assert_eq!(pending_body["error"], "conversation_not_ready");
        assert_eq!(pending_body["pending_call_count"], 1);

        cleanup_db(&db_path);
        let _ = std::fs::remove_dir_all(&root);
    }

    async fn service_response_json(response: actix_web::dev::ServiceResponse) -> Value {
        let body = to_bytes(response.into_body())
            .await
            .expect("response body should be readable");
        serde_json::from_slice(&body).expect("response body should be JSON")
    }

    fn test_evaluation_result(target_id: &str) -> crate::grader::EvaluationResult {
        crate::grader::EvaluationResult {
            target_type: TargetType::Conversation,
            target_id: target_id.to_string(),
            run_id: "run-shared".to_string(),
            input_hash: "input-hash-shared".to_string(),
            verdict: crate::grader::Verdict::Pass,
            score: 1.0,
            summary: "ok".to_string(),
            root_cause: crate::grader::RootCause::None,
            recommended_action: "none".to_string(),
            dimensions: Vec::new(),
            findings: Vec::new(),
            metadata: crate::grader::EvaluationMetadata {
                evaluated_with_pending: false,
                pending_call_count: 0,
                input_event_count: 1,
                grader_type: GraderType::Rule,
                grader_version: RULE_GRADER_VERSION.to_string(),
                rubric_version: None,
                judge_model: None,
                prompt_hash: None,
                confidence: Some(1.0),
            },
        }
    }

    fn grader_app_state(storage_path: PathBuf) -> web::Data<AppState> {
        let auth_config = crate::config::ServerAuthConfig { enabled: false };
        let auth = Arc::new(crate::server::auth::DashboardAuth::init(
            &auth_config,
            std::path::Path::new("/tmp"),
        ));
        web::Data::new(AppState {
            evaluation_store: Arc::new(EvaluationStore::new_with_path(&storage_path).unwrap()),
            storage_path,
            start_time: Instant::now(),
            health_store: Arc::new(RwLock::new(HealthStore::new())),
            interruption_store: None,
            enforcement: None,
            containment: None,
            security_store: Arc::new(crate::security::SecurityStore::open_in_memory().unwrap()),
            security_observability: super::super::SecurityObservabilityConfig { timeout_ms: 0 },
            auth,
            optimize: None,
            trajectory_store: Arc::new(RwLock::new(None)),
        })
    }

    fn write_completed_conversation_event(path: &std::path::Path, conversation_id: &str) {
        let store = GenAISqliteStore::new_with_path(path).unwrap();
        let mut call = LLMCall::new(
            format!("call-{conversation_id}"),
            1_700_000_000_000_000_000,
            "anthropic".to_string(),
            "claude".to_string(),
            LLMRequest {
                messages: Vec::new(),
                temperature: None,
                max_tokens: None,
                frequency_penalty: None,
                presence_penalty: None,
                top_p: None,
                top_k: None,
                seed: None,
                stop_sequences: None,
                stream: false,
                tools: None,
                raw_body: None,
            },
            1234,
            "claude".to_string(),
        );
        call.agent_name = Some("claude".to_string());
        call.set_response(
            LLMResponse {
                messages: vec![OutputMessage {
                    role: "assistant".to_string(),
                    parts: vec![MessagePart::Text {
                        content: "done".to_string(),
                    }],
                    name: None,
                    finish_reason: Some("stop".to_string()),
                }],
                streamed: false,
                raw_body: None,
            },
            1_700_000_000_000_000_500,
        );
        call.set_token_usage(TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 15,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        });
        call.metadata
            .insert("conversation_id".to_string(), conversation_id.to_string());
        call.metadata.insert(
            "response_id".to_string(),
            format!("trace-{conversation_id}"),
        );
        call.metadata.insert(
            "session_id".to_string(),
            format!("session-{conversation_id}"),
        );
        call.metadata
            .insert("user_query".to_string(), "hello".to_string());

        store.export(&[GenAISemanticEvent::LLMCall(call)]);
        store.flush();
    }

    fn write_pending_conversation_event(path: &std::path::Path, conversation_id: &str) {
        let store = GenAISqliteStore::new_with_path(path).unwrap();
        store
            .insert_pending(&PendingCallInfo {
                call_id: format!("pending-{conversation_id}"),
                trace_id: Some(format!("trace-{conversation_id}")),
                conversation_id: Some(conversation_id.to_string()),
                session_id: Some("session-1".to_string()),
                start_timestamp_ns: 1_700_000_000_000_000_000,
                pid: 1234,
                process_name: "claude".to_string(),
                agent_name: Some("claude".to_string()),
                http_method: Some("POST".to_string()),
                http_path: Some("/v1/messages".to_string()),
                input_messages: Some(r#"[{"role":"user","content":"hello"}]"#.to_string()),
                system_instructions: None,
                user_query: Some("hello".to_string()),
                is_sse: true,
                model: Some("claude".to_string()),
                provider: Some("anthropic".to_string()),
                call_kind: "main".to_string(),
                pending_origin: PendingOrigin::RequestCapture,
                pending_match_key: None,
            })
            .unwrap();
        store.flush();
    }

    fn cleanup_db(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentsight_{label}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn test_app_state(timeout_ms: u64) -> web::Data<AppState> {
        let auth_config = crate::config::ServerAuthConfig { enabled: false };
        let auth = Arc::new(crate::server::auth::DashboardAuth::init(
            &auth_config,
            std::path::Path::new("/tmp"),
        ));
        web::Data::new(AppState {
            storage_path: PathBuf::from(":memory:"),
            start_time: Instant::now(),
            health_store: Arc::new(RwLock::new(HealthStore::new())),
            interruption_store: None,
            evaluation_store: Arc::new(
                EvaluationStore::new_with_path(std::path::Path::new(":memory:")).unwrap(),
            ),
            enforcement: None,
            containment: None,
            security_store: Arc::new(crate::security::SecurityStore::open_in_memory().unwrap()),
            security_observability: super::super::SecurityObservabilityConfig { timeout_ms },
            auth,
            optimize: None,
            trajectory_store: Arc::new(RwLock::new(None)),
        })
    }

    #[actix_web::test]
    async fn api_unmatched_returns_404_json() {
        let app = awtest::init_service(
            App::new()
                .app_data(test_app_state(0))
                .configure(super::super::configure_routes),
        )
        .await;

        let req = awtest::TestRequest::get()
            .uri("/api/definitely-not-a-route")
            .to_request();
        let resp = awtest::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body: Value =
            serde_json::from_slice(&actix_web::body::to_bytes(resp.into_body()).await.unwrap())
                .unwrap();
        assert_eq!(body["error"], "not_found");
    }

    #[actix_web::test]
    async fn health_unmatched_returns_404_json() {
        let app = awtest::init_service(
            App::new()
                .app_data(test_app_state(0))
                .configure(super::super::configure_routes),
        )
        .await;

        let req = awtest::TestRequest::get().uri("/health/nope").to_request();
        let resp = awtest::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body: Value =
            serde_json::from_slice(&actix_web::body::to_bytes(resp.into_body()).await.unwrap())
                .unwrap();
        assert_eq!(body["error"], "not_found");
    }

    #[actix_web::test]
    async fn non_api_path_uses_spa_fallback_not_api_404() {
        let app = awtest::init_service(
            App::new()
                .app_data(test_app_state(0))
                .configure(super::super::configure_routes),
        )
        .await;

        let req = awtest::TestRequest::get().uri("/dashboard").to_request();
        let resp = awtest::call_service(&app, req).await;

        // Should NOT be 404 JSON — it should go to SPA fallback (200 or 404 plain)
        // Definitely should not be a JSON 404 with error.code = "not_found"
        let status = resp.status();
        let body_bytes = actix_web::body::to_bytes(resp.into_body()).await.unwrap();
        let is_api_404 = serde_json::from_slice::<Value>(&body_bytes)
            .ok()
            .and_then(|v| v.get("error")?.get("code")?.as_str().map(String::from))
            == Some("not_found".to_string());
        assert!(
            !is_api_404,
            "SPA path /dashboard must not get API 404, got status={status}"
        );
    }

    // ─── Auth endpoint tests ──────────────────────────────────────────────────

    /// A test token that is >= 32 chars (required by read_or_create_token).
    const TEST_TOKEN: &str = "correct-token-for-auth-testing-32chars!!";

    fn test_app_state_with_auth(enabled: bool) -> web::Data<AppState> {
        // Use a unique dir per call to avoid test-parallelism races
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("handler_auth_test_{id}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok();
        if enabled {
            std::fs::write(dir.join(".dashboard_token"), TEST_TOKEN).ok();
        }
        let auth_config = crate::config::ServerAuthConfig { enabled };
        let auth = Arc::new(crate::server::auth::DashboardAuth::init(&auth_config, &dir));
        web::Data::new(AppState {
            storage_path: PathBuf::from(":memory:"),
            start_time: Instant::now(),
            health_store: Arc::new(RwLock::new(HealthStore::new())),
            interruption_store: None,
            evaluation_store: Arc::new(
                EvaluationStore::new_with_path(std::path::Path::new(":memory:")).unwrap(),
            ),
            enforcement: None,
            containment: None,
            security_store: Arc::new(crate::security::SecurityStore::open_in_memory().unwrap()),
            security_observability: super::super::SecurityObservabilityConfig { timeout_ms: 0 },
            auth,
            optimize: None,
            trajectory_store: Arc::new(RwLock::new(None)),
        })
    }

    #[actix_web::test]
    async fn auth_login_success_with_correct_token() {
        let app = awtest::init_service(
            App::new()
                .app_data(test_app_state_with_auth(true))
                .route("/api/auth/login", web::post().to(auth_login)),
        )
        .await;
        let req = awtest::TestRequest::post()
            .uri("/api/auth/login")
            .set_json(json!({"token": TEST_TOKEN}))
            .to_request();
        let resp = awtest::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        // Should have a session cookie set
        let cookie = resp
            .response()
            .cookies()
            .find(|c| c.name() == "agentsight_session");
        assert!(cookie.is_some(), "response should contain session cookie");
    }

    #[actix_web::test]
    async fn auth_login_fails_with_wrong_token() {
        let app = awtest::init_service(
            App::new()
                .app_data(test_app_state_with_auth(true))
                .route("/api/auth/login", web::post().to(auth_login)),
        )
        .await;
        let req = awtest::TestRequest::post()
            .uri("/api/auth/login")
            .set_json(json!({"token": "wrong-token"}))
            .to_request();
        let resp = awtest::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body: Value =
            serde_json::from_slice(&actix_web::body::to_bytes(resp.into_body()).await.unwrap())
                .unwrap();
        assert_eq!(body["error"], "invalid_token");
    }

    #[actix_web::test]
    async fn auth_status_reports_enabled() {
        let app = awtest::init_service(
            App::new()
                .app_data(test_app_state_with_auth(true))
                .service(auth_status),
        )
        .await;
        let req = awtest::TestRequest::get().uri("/status").to_request();
        let resp = awtest::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&actix_web::body::to_bytes(resp.into_body()).await.unwrap())
                .unwrap();
        assert_eq!(body["auth_enabled"], true);
    }

    #[actix_web::test]
    async fn auth_status_reports_disabled() {
        let app =
            awtest::init_service(App::new().app_data(test_app_state(0)).service(auth_status)).await;
        let req = awtest::TestRequest::get().uri("/status").to_request();
        let resp = awtest::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&actix_web::body::to_bytes(resp.into_body()).await.unwrap())
                .unwrap();
        assert_eq!(body["auth_enabled"], false);
    }

    #[actix_web::test]
    async fn auth_verify_returns_true_when_disabled() {
        let app =
            awtest::init_service(App::new().app_data(test_app_state(0)).service(auth_verify)).await;
        let req = awtest::TestRequest::get().uri("/verify").to_request();
        let resp = awtest::call_service(&app, req).await;
        let body: Value =
            serde_json::from_slice(&actix_web::body::to_bytes(resp.into_body()).await.unwrap())
                .unwrap();
        assert_eq!(body["authenticated"], true);
    }

    #[actix_web::test]
    async fn auth_verify_returns_false_without_cookie() {
        let app = awtest::init_service(
            App::new()
                .app_data(test_app_state_with_auth(true))
                .service(auth_verify),
        )
        .await;
        let req = awtest::TestRequest::get().uri("/verify").to_request();
        let resp = awtest::call_service(&app, req).await;
        let body: Value =
            serde_json::from_slice(&actix_web::body::to_bytes(resp.into_body()).await.unwrap())
                .unwrap();
        assert_eq!(body["authenticated"], false);
    }

    #[actix_web::test]
    async fn auth_verify_returns_true_with_valid_cookie() {
        let state = test_app_state_with_auth(true);
        let cookie_value = state.auth.create_session_cookie(3600);
        let app = awtest::init_service(App::new().app_data(state).service(auth_verify)).await;
        let req = awtest::TestRequest::get()
            .uri("/verify")
            .cookie(actix_web::cookie::Cookie::new(
                "agentsight_session",
                cookie_value,
            ))
            .to_request();
        let resp = awtest::call_service(&app, req).await;
        let body: Value =
            serde_json::from_slice(&actix_web::body::to_bytes(resp.into_body()).await.unwrap())
                .unwrap();
        assert_eq!(body["authenticated"], true);
    }

    fn test_app_state_with_storage(storage_path: PathBuf) -> web::Data<AppState> {
        let auth_config = crate::config::ServerAuthConfig { enabled: false };
        let auth = Arc::new(crate::server::auth::DashboardAuth::init(
            &auth_config,
            std::path::Path::new("/tmp"),
        ));
        web::Data::new(AppState {
            storage_path: storage_path.clone(),
            start_time: Instant::now(),
            health_store: Arc::new(RwLock::new(HealthStore::new())),
            interruption_store: None,
            evaluation_store: Arc::new(EvaluationStore::new_with_path(&storage_path).unwrap()),
            security_observability: super::super::SecurityObservabilityConfig { timeout_ms: 0 },
            auth,
            optimize: None,
            trajectory_store: Arc::new(RwLock::new(None)),
        })
    }

    fn test_app_state_with_interruption_store(
        store: Arc<crate::storage::sqlite::InterruptionStore>,
    ) -> web::Data<AppState> {
        let auth_config = crate::config::ServerAuthConfig { enabled: false };
        let auth = Arc::new(crate::server::auth::DashboardAuth::init(
            &auth_config,
            std::path::Path::new("/tmp"),
        ));
        web::Data::new(AppState {
            storage_path: PathBuf::from(":memory:"),
            start_time: Instant::now(),
            health_store: Arc::new(RwLock::new(HealthStore::new())),
            interruption_store: Some(store),
            evaluation_store: Arc::new(
                EvaluationStore::new_with_path(std::path::Path::new(":memory:")).unwrap(),
            ),
            security_observability: super::super::SecurityObservabilityConfig { timeout_ms: 0 },
            auth,
            optimize: None,
            trajectory_store: Arc::new(RwLock::new(None)),
        })
    }

    fn unique_handler_db(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentsight_handler_{label}_{}_{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    // ─── Trajectory handler tests ────────────────────────────────────────

    fn trajectory_record(session_id: &str, project: &str, source: &str) -> TrajectoryRecord {
        TrajectoryRecord {
            session_id: session_id.to_string(),
            schema_version: "ATIF-v1.7".to_string(),
            agent_name: "qoder".to_string(),
            model_name: Some("qwen-max".to_string()),
            num_steps: 2,
            total_prompt_tokens: Some(100),
            total_completion_tokens: Some(20),
            start_time: Some("2026-07-25T10:00:00Z".to_string()),
            end_time: Some("2026-07-25T10:00:05Z".to_string()),
            first_user_message: Some("修个 bug".to_string()),
            last_user_message: Some("跑下测试".to_string()),
            atif_json: format!(
                "{{\"schema_version\":\"ATIF-v1.7\",\"session_id\":\"{session_id}\"}}"
            ),
            project: project.to_string(),
            source: source.to_string(),
            is_subagent: false,
            file_path: format!("/root/.qoder/projects/{project}/{session_id}.jsonl"),
            file_size: 512,
            file_mtime_ns: 7,
        }
    }

    fn test_app_state_with_trajectory_store(
        store: Option<Arc<TrajectoryStore>>,
    ) -> web::Data<AppState> {
        let auth_config = crate::config::ServerAuthConfig { enabled: false };
        let auth = Arc::new(crate::server::auth::DashboardAuth::init(
            &auth_config,
            std::path::Path::new("/tmp"),
        ));
        web::Data::new(AppState {
            storage_path: PathBuf::from(":memory:"),
            start_time: Instant::now(),
            health_store: Arc::new(RwLock::new(HealthStore::new())),
            interruption_store: None,
            evaluation_store: Arc::new(
                EvaluationStore::new_with_path(std::path::Path::new(":memory:")).unwrap(),
            ),
            security_observability: super::super::SecurityObservabilityConfig { timeout_ms: 0 },
            auth,
            optimize: None,
            trajectory_store: Arc::new(RwLock::new(store)),
        })
    }

    fn seeded_trajectory_store(tag: &str) -> Arc<TrajectoryStore> {
        let db = unique_handler_db(tag);
        let store = TrajectoryStore::new_with_path(&db).unwrap();
        store
            .upsert_trajectory(&trajectory_record("s-1", "proj-a", "qoder"))
            .unwrap();
        store
            .upsert_trajectory(&trajectory_record("s-2", "proj-b", "qoderwork"))
            .unwrap();
        Arc::new(store)
    }

    #[actix_web::test]
    async fn trajectory_list_returns_rows_and_filters() {
        let data = test_app_state_with_trajectory_store(Some(seeded_trajectory_store("list")));
        let app = awtest::init_service(
            App::new()
                .app_data(data)
                .configure(crate::server::configure_routes),
        )
        .await;

        let all_resp = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/api/trajectories")
                .to_request(),
        )
        .await;
        let all: serde_json::Value = awtest::read_body_json(all_resp).await;
        assert_eq!(all.as_array().unwrap().len(), 2);

        let filtered_resp = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/api/trajectories?project=proj-a")
                .to_request(),
        )
        .await;
        let filtered: serde_json::Value = awtest::read_body_json(filtered_resp).await;
        let arr = filtered.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["session_id"], "s-1");
    }

    #[actix_web::test]
    async fn trajectory_detail_returns_raw_atif_and_404() {
        let data = test_app_state_with_trajectory_store(Some(seeded_trajectory_store("detail")));
        let app = awtest::init_service(
            App::new()
                .app_data(data)
                .configure(crate::server::configure_routes),
        )
        .await;

        let resp = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/api/trajectories/s-1")
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body()).await.unwrap();
        let doc: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(doc["session_id"], "s-1");

        let miss = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/api/trajectories/nope")
                .to_request(),
        )
        .await;
        assert_eq!(miss.status(), StatusCode::NOT_FOUND);
    }

    #[actix_web::test]
    async fn trajectory_filters_route_not_captured_by_session_id() {
        let data = test_app_state_with_trajectory_store(Some(seeded_trajectory_store("filters")));
        let app = awtest::init_service(
            App::new()
                .app_data(data)
                .configure(crate::server::configure_routes),
        )
        .await;

        let resp = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/api/trajectories/filters")
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = awtest::read_body_json(resp).await;
        // `filters` must hit the filters handler (object), not the detail handler (404).
        assert_eq!(body["projects"], serde_json::json!(["proj-a", "proj-b"]));
        assert_eq!(body["sources"], serde_json::json!(["qoder", "qoderwork"]));
    }

    #[actix_web::test]
    async fn trajectory_endpoints_graceful_when_store_absent() {
        let data = test_app_state_with_trajectory_store(None);
        let app = awtest::init_service(
            App::new()
                .app_data(data)
                .configure(crate::server::configure_routes),
        )
        .await;

        let list = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/api/trajectories")
                .to_request(),
        )
        .await;
        assert_eq!(list.status(), StatusCode::OK);
        let rows: serde_json::Value = awtest::read_body_json(list).await;
        assert_eq!(rows, serde_json::json!([]));

        let filters = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/api/trajectories/filters")
                .to_request(),
        )
        .await;
        assert_eq!(filters.status(), StatusCode::OK);
    }

    fn make_interruption_event(
        id: &str,
        session_id: &str,
        conversation_id: &str,
        itype: crate::interruption::InterruptionType,
    ) -> crate::interruption::InterruptionEvent {
        crate::interruption::InterruptionEvent {
            interruption_id: id.to_string(),
            session_id: Some(session_id.to_string()),
            trace_id: Some(format!("trace-{conversation_id}")),
            conversation_id: Some(conversation_id.to_string()),
            call_id: Some(format!("call-{id}")),
            pid: Some(1234),
            agent_name: Some("Agent-A".to_string()),
            interruption_type: itype,
            severity: crate::interruption::types::Severity::High,
            occurred_at_ns: 1_700_000_000_000_000_000,
            detail: Some(r#"{"error":"rate limit"}"#.to_string()),
            resolved: false,
        }
    }

    #[actix_web::test]
    async fn health_reports_status_version_and_uptime() {
        let app =
            awtest::init_service(App::new().app_data(test_app_state(0)).service(health)).await;
        let response =
            awtest::call_service(&app, awtest::TestRequest::get().uri("/health").to_request())
                .await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = service_response_json(response).await;
        assert_eq!(body["status"], "ok");
        assert!(body["version"].as_str().is_some());
        assert!(body["uptime_seconds"].as_u64().is_some());
    }

    #[actix_web::test]
    async fn genai_query_handlers_return_persisted_data() {
        let db_path = unique_handler_db("genai_queries");
        write_completed_conversation_event(&db_path, "conv-handler");
        let app = awtest::init_service(
            App::new()
                .app_data(test_app_state_with_storage(db_path.clone()))
                .service(list_sessions)
                .service(list_traces_by_session)
                .service(get_trace_detail)
                .service(get_conversation_events)
                .service(list_agent_names)
                .service(get_timeseries),
        )
        .await;

        let sessions = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/sessions?start_ns=0&end_ns=9223372036854775807")
                .to_request(),
        )
        .await;
        assert_eq!(sessions.status(), StatusCode::OK);
        let sessions_body = service_response_json(sessions).await;
        assert_eq!(sessions_body.as_array().unwrap().len(), 1);
        let session_id = sessions_body[0]["session_id"].as_str().unwrap();

        let traces = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri(&format!(
                    "/sessions/{session_id}/traces?start_ns=0&end_ns=9223372036854775807"
                ))
                .to_request(),
        )
        .await;
        assert_eq!(traces.status(), StatusCode::OK);
        let traces_body = service_response_json(traces).await;
        assert_eq!(traces_body.as_array().unwrap().len(), 1);

        let detail = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/traces/trace-conv-handler")
                .to_request(),
        )
        .await;
        assert_eq!(detail.status(), StatusCode::OK);
        assert_eq!(
            service_response_json(detail)
                .await
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let conversation = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/conversations/conv-handler")
                .to_request(),
        )
        .await;
        assert_eq!(conversation.status(), StatusCode::OK);
        assert_eq!(
            service_response_json(conversation)
                .await
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let agent_names = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/agent-names?start_ns=0&end_ns=9223372036854775807")
                .to_request(),
        )
        .await;
        assert_eq!(agent_names.status(), StatusCode::OK);
        assert_eq!(service_response_json(agent_names).await[0], "claude");

        let timeseries = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/timeseries?start_ns=0&end_ns=9223372036854775807&buckets=2")
                .to_request(),
        )
        .await;
        assert_eq!(timeseries.status(), StatusCode::OK);
        let timeseries_body = service_response_json(timeseries).await;
        assert!(timeseries_body["token_series"].as_array().is_some());
        assert!(timeseries_body["model_series"].as_array().is_some());

        cleanup_db(&db_path);
    }

    #[actix_web::test]
    async fn metrics_returns_prometheus_text_for_agent_tokens_and_interruptions() {
        let db_path = unique_handler_db("metrics");
        write_completed_conversation_event(&db_path, "conv-metrics");
        let interruption_path = unique_handler_db("metrics_interruptions");
        let istore = Arc::new(
            crate::storage::sqlite::InterruptionStore::new_with_path(&interruption_path).unwrap(),
        );
        istore
            .insert(&make_interruption_event(
                "int-metrics",
                "sess-metrics",
                "conv-metrics",
                crate::interruption::InterruptionType::RateLimit,
            ))
            .unwrap();
        let auth_config = crate::config::ServerAuthConfig { enabled: false };
        let auth = Arc::new(crate::server::auth::DashboardAuth::init(
            &auth_config,
            std::path::Path::new("/tmp"),
        ));
        let app = awtest::init_service(
            App::new()
                .app_data(web::Data::new(AppState {
                    storage_path: db_path.clone(),
                    start_time: Instant::now(),
                    health_store: Arc::new(RwLock::new(HealthStore::new())),
                    interruption_store: Some(Arc::clone(&istore)),
                    evaluation_store: Arc::new(EvaluationStore::new_with_path(&db_path).unwrap()),
                    security_observability: super::super::SecurityObservabilityConfig {
                        timeout_ms: 0,
                    },
                    auth,
                    optimize: None,
                    trajectory_store: Arc::new(RwLock::new(None)),
                }))
                .service(metrics),
        )
        .await;

        let response = awtest::call_service(
            &app,
            awtest::TestRequest::get().uri("/metrics").to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("agentsight_token_input_total"));
        assert!(text.contains("agent=\"claude\""));
        assert!(text.contains("agentsight_interruptions_total"));
        assert!(text.contains("type=\"rate_limit\""));

        cleanup_db(&db_path);
        cleanup_db(&interruption_path);
    }

    #[actix_web::test]
    async fn agent_health_filters_clients_and_allows_deletion() {
        let state = test_app_state(0);
        {
            let mut store = state.health_store.write().unwrap();
            store.update(
                1001,
                crate::health::AgentHealthStatus {
                    pid: 1001,
                    agent_name: "Gateway".to_string(),
                    category: "agent".to_string(),
                    exe_path: "/bin/gateway".to_string(),
                    ports: vec![8080],
                    status: crate::health::store::AgentHealthState::Healthy,
                    last_check_time: 1,
                    latency_ms: Some(10),
                    error_message: None,
                    restart_cmd: None,
                    offline_since: None,
                    role: crate::health::store::AgentRole::Gateway,
                    parent_pid: None,
                    has_crash: false,
                },
            );
            store.update(
                1002,
                crate::health::AgentHealthStatus {
                    pid: 1002,
                    agent_name: "Client".to_string(),
                    category: "agent".to_string(),
                    exe_path: "/bin/client".to_string(),
                    ports: Vec::new(),
                    status: crate::health::store::AgentHealthState::Healthy,
                    last_check_time: 1,
                    latency_ms: None,
                    error_message: None,
                    restart_cmd: None,
                    offline_since: None,
                    role: crate::health::store::AgentRole::Client,
                    parent_pid: None,
                    has_crash: false,
                },
            );
            store.update(
                1003,
                crate::health::AgentHealthStatus {
                    pid: 1003,
                    agent_name: "Cosh".to_string(),
                    category: "agent".to_string(),
                    exe_path: "/bin/cosh".to_string(),
                    ports: Vec::new(),
                    status: crate::health::store::AgentHealthState::Offline,
                    last_check_time: 1,
                    latency_ms: None,
                    error_message: Some("offline".to_string()),
                    restart_cmd: None,
                    offline_since: Some(1),
                    role: crate::health::store::AgentRole::Client,
                    parent_pid: None,
                    has_crash: true,
                },
            );
        }
        let app = awtest::init_service(
            App::new()
                .app_data(state)
                .service(get_agent_health)
                .route("/agent-health/{pid}", web::delete().to(delete_agent_health)),
        )
        .await;

        let filtered = awtest::call_service(
            &app,
            awtest::TestRequest::get().uri("/agent-health").to_request(),
        )
        .await;
        let filtered_body = service_response_json(filtered).await;
        let filtered_agents = filtered_body["agents"].as_array().unwrap();
        assert_eq!(filtered_agents.len(), 1);
        assert_eq!(filtered_agents[0]["pid"], 1001);

        let include_clients = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/agent-health?include_clients=true")
                .to_request(),
        )
        .await;
        let include_body = service_response_json(include_clients).await;
        let agents = include_body["agents"].as_array().unwrap();
        assert_eq!(agents.len(), 2, "Cosh should still be excluded");

        let deleted = awtest::call_service(
            &app,
            awtest::TestRequest::delete()
                .uri("/agent-health/1001")
                .to_request(),
        )
        .await;
        assert_eq!(deleted.status(), StatusCode::OK);
        assert_eq!(service_response_json(deleted).await["ok"], true);

        let missing = awtest::call_service(
            &app,
            awtest::TestRequest::delete()
                .uri("/agent-health/9999")
                .to_request(),
        )
        .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[actix_web::test]
    async fn interruption_handlers_cover_list_stats_counts_detail_and_resolve() {
        let interruption_path = unique_handler_db("interruptions");
        let istore = Arc::new(
            crate::storage::sqlite::InterruptionStore::new_with_path(&interruption_path).unwrap(),
        );
        istore
            .insert(&make_interruption_event(
                "int-handler-1",
                "sess-handler",
                "conv-handler-i",
                crate::interruption::InterruptionType::RateLimit,
            ))
            .unwrap();
        istore
            .insert(&make_interruption_event(
                "int-handler-2",
                "sess-handler",
                "conv-handler-i",
                crate::interruption::InterruptionType::AuthError,
            ))
            .unwrap();
        let app = awtest::init_service(
            App::new()
                .app_data(test_app_state_with_interruption_store(Arc::clone(&istore)))
                .service(list_interruptions)
                .service(interruption_count)
                .service(interruption_stats)
                .service(interruption_session_counts)
                .service(interruption_conversation_counts)
                .service(list_session_interruptions)
                .service(list_conversation_interruptions)
                .service(get_interruption)
                .route(
                    "/interruptions/{interruption_id}/resolve",
                    web::post().to(resolve_interruption),
                ),
        )
        .await;

        let list = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/interruptions?start_ns=0&end_ns=9223372036854775807")
                .to_request(),
        )
        .await;
        assert_eq!(list.status(), StatusCode::OK);
        assert_eq!(
            service_response_json(list).await.as_array().unwrap().len(),
            2
        );

        let count = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/interruptions/count?start_ns=0&end_ns=9223372036854775807")
                .to_request(),
        )
        .await;
        let count_body = service_response_json(count).await;
        assert_eq!(count_body["total"], 2);
        assert_eq!(count_body["by_severity"]["high"], 2);

        let stats = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/interruptions/stats?start_ns=0&end_ns=9223372036854775807")
                .to_request(),
        )
        .await;
        assert_eq!(
            service_response_json(stats).await.as_array().unwrap().len(),
            2
        );

        let session_counts = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/interruptions/session-counts?start_ns=0&end_ns=9223372036854775807")
                .to_request(),
        )
        .await;
        assert_eq!(service_response_json(session_counts).await[0]["total"], 2);

        let conversation_counts = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/interruptions/conversation-counts?start_ns=0&end_ns=9223372036854775807")
                .to_request(),
        )
        .await;
        assert_eq!(
            service_response_json(conversation_counts).await[0]["total"],
            2
        );

        let by_session = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/sessions/sess-handler/interruptions")
                .to_request(),
        )
        .await;
        assert_eq!(
            service_response_json(by_session)
                .await
                .as_array()
                .unwrap()
                .len(),
            2
        );

        let by_conversation = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/conversations/conv-handler-i/interruptions")
                .to_request(),
        )
        .await;
        assert_eq!(
            service_response_json(by_conversation)
                .await
                .as_array()
                .unwrap()
                .len(),
            2
        );

        let detail = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/interruptions/int-handler-1")
                .to_request(),
        )
        .await;
        assert_eq!(
            service_response_json(detail).await["interruption_id"],
            "int-handler-1"
        );

        let resolved = awtest::call_service(
            &app,
            awtest::TestRequest::post()
                .uri("/interruptions/int-handler-1/resolve")
                .to_request(),
        )
        .await;
        assert_eq!(resolved.status(), StatusCode::OK);
        assert_eq!(service_response_json(resolved).await["status"], "resolved");

        let missing = awtest::call_service(
            &app,
            awtest::TestRequest::get()
                .uri("/interruptions/missing-id")
                .to_request(),
        )
        .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        cleanup_db(&interruption_path);
    }

    #[actix_web::test]
    async fn interruption_handlers_return_unavailable_without_store() {
        let app = awtest::init_service(
            App::new()
                .app_data(test_app_state(0))
                .service(list_interruptions)
                .service(interruption_count)
                .service(interruption_stats),
        )
        .await;

        for uri in [
            "/interruptions",
            "/interruptions/count",
            "/interruptions/stats",
        ] {
            let response =
                awtest::call_service(&app, awtest::TestRequest::get().uri(uri).to_request()).await;
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        }
    }

    #[actix_web::test]
    async fn atif_export_handlers_return_documents_and_not_found() {
        let db_path = unique_handler_db("atif_exports");
        write_completed_conversation_event(&db_path, "conv-atif");
        let app = awtest::init_service(
            App::new()
                .app_data(test_app_state_with_storage(db_path.clone()))
                .service(export_atif_trace)
                .service(export_atif_session)
                .service(export_atif_conversation),
        )
        .await;

        for uri in [
            "/export/atif/trace/trace-conv-atif",
            "/export/atif/session/session-conv-atif",
            "/export/atif/conversation/conv-atif",
        ] {
            let response =
                awtest::call_service(&app, awtest::TestRequest::get().uri(uri).to_request()).await;
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "{uri} should export ATIF"
            );
            let body = service_response_json(response).await;
            assert!(body.is_object(), "{uri} should return a JSON document");
            // The export must speak the shared ATIF schema, same as the
            // collector-ingested trajectories served by /api/trajectories.
            assert_eq!(
                body["schema_version"],
                agentsight_atif::ATIF_SCHEMA_VERSION,
                "{uri} should export the shared ATIF schema version"
            );
            assert!(
                body["session_id"].is_string(),
                "{uri} should carry the queried id as session_id"
            );
            assert!(
                body["steps"].as_array().is_some_and(|s| !s.is_empty()),
                "{uri} should export at least one step"
            );
            // Round-trip through the shared validator: catches field drift and
            // non-contiguous step_ids that a plain is_object() check would miss.
            agentsight_atif::validate_trajectory_str(&body.to_string())
                .unwrap_or_else(|e| panic!("{uri} export failed ATIF validation: {e}"));
        }

        for (uri, message) in [
            ("/export/atif/trace/missing-trace", "trace not found"),
            ("/export/atif/session/missing-session", "session not found"),
            (
                "/export/atif/conversation/missing-conversation",
                "conversation not found",
            ),
        ] {
            let response =
                awtest::call_service(&app, awtest::TestRequest::get().uri(uri).to_request()).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert_eq!(service_response_json(response).await["error"], message);
        }

        cleanup_db(&db_path);
    }

    #[actix_web::test]
    async fn skill_metrics_handlers_return_reports_for_all_metric_variants() {
        let db_path = unique_handler_db("skill_metrics");
        write_completed_conversation_event(&db_path, "conv-skill-metrics");
        let app = awtest::init_service(
            App::new()
                .app_data(test_app_state_with_storage(db_path.clone()))
                .service(skill_metrics_all)
                .service(skill_metrics_downloads)
                .service(skill_metrics_loads)
                .service(skill_metrics_usage_ratio)
                .service(skill_metrics_distribution)
                .service(skill_metrics_hotness),
        )
        .await;

        for uri in [
            "/skill-metrics?start_ns=0&end_ns=9223372036854775807&granularity=day",
            "/skill-metrics/downloads?start_ns=0&end_ns=9223372036854775807",
            "/skill-metrics/loads?start_ns=0&end_ns=9223372036854775807",
            "/skill-metrics/usage-ratio?start_ns=0&end_ns=9223372036854775807",
            "/skill-metrics/distribution?start_ns=0&end_ns=9223372036854775807",
            "/skill-metrics/hotness?start_ns=0&end_ns=9223372036854775807&granularity=day",
        ] {
            let response =
                awtest::call_service(&app, awtest::TestRequest::get().uri(uri).to_request()).await;
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "{uri} should return report"
            );
            let body = service_response_json(response).await;
            assert!(body.is_object(), "{uri} should return JSON object");
        }

        cleanup_db(&db_path);
    }

    #[actix_web::test]
    async fn storage_backed_handlers_report_database_open_errors() {
        let root = temp_root("handler_open_errors");
        std::fs::write(&root, b"not a directory").unwrap();
        let blocked_db = root.join("events.db");
        let auth_config = crate::config::ServerAuthConfig { enabled: false };
        let auth = Arc::new(crate::server::auth::DashboardAuth::init(
            &auth_config,
            std::path::Path::new("/tmp"),
        ));
        let app = awtest::init_service(
            App::new()
                .app_data(web::Data::new(AppState {
                    storage_path: blocked_db.clone(),
                    start_time: Instant::now(),
                    health_store: Arc::new(RwLock::new(HealthStore::new())),
                    interruption_store: None,
                    evaluation_store: Arc::new(
                        EvaluationStore::new_with_path(std::path::Path::new(":memory:")).unwrap(),
                    ),
                    security_observability: super::super::SecurityObservabilityConfig {
                        timeout_ms: 0,
                    },
                    auth,
                    optimize: None,
                    trajectory_store: Arc::new(RwLock::new(None)),
                }))
                .service(list_sessions)
                .service(list_agent_names)
                .service(get_timeseries)
                .service(metrics)
                .service(export_atif_trace)
                .service(skill_metrics_all),
        )
        .await;

        for (uri, status) in [
            ("/sessions", StatusCode::INTERNAL_SERVER_ERROR),
            ("/agent-names", StatusCode::INTERNAL_SERVER_ERROR),
            ("/timeseries", StatusCode::INTERNAL_SERVER_ERROR),
            (
                "/export/atif/trace/trace-1",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            ("/skill-metrics", StatusCode::INTERNAL_SERVER_ERROR),
        ] {
            let response =
                awtest::call_service(&app, awtest::TestRequest::get().uri(uri).to_request()).await;
            assert_eq!(response.status(), status, "{uri} should fail opening DB");
        }

        let metrics_response = awtest::call_service(
            &app,
            awtest::TestRequest::get().uri("/metrics").to_request(),
        )
        .await;
        assert_eq!(metrics_response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = actix_web::body::to_bytes(metrics_response.into_body())
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("# ERROR opening database"));

        let _ = std::fs::remove_file(&root);
    }
}

// ─── Prometheus metrics endpoint ─────────────────────────────────────────────

/// GET /metrics — Prometheus text format token usage metrics
///
/// Exposes per-agent counters for input tokens, output tokens, total tokens,
/// and LLM request count, aggregated over all recorded history.
/// The response Content-Type is `text/plain; version=0.0.4` as required by
/// the Prometheus exposition format.
#[get("/metrics")]
pub async fn metrics(data: web::Data<AppState>) -> impl Responder {
    let db_path = &data.storage_path;

    let summaries = match GenAISqliteStore::new_with_path(db_path) {
        Ok(store) => match store.get_agent_token_summary() {
            Ok(v) => v,
            Err(e) => {
                return HttpResponse::InternalServerError()
                    .content_type("text/plain; version=0.0.4")
                    .body(format!("# ERROR querying metrics: {e}\n"));
            }
        },
        Err(e) => {
            return HttpResponse::InternalServerError()
                .content_type("text/plain; version=0.0.4")
                .body(format!("# ERROR opening database: {e}\n"));
        }
    };

    let mut out = String::with_capacity(512 + summaries.len() * 128);

    // agentsight_token_input_total
    out.push_str(
        "# HELP agentsight_token_input_total Total input tokens consumed by agent (all-time)\n",
    );
    out.push_str("# TYPE agentsight_token_input_total counter\n");
    for s in &summaries {
        out.push_str(&format!(
            "agentsight_token_input_total{{agent=\"{}\"}} {}\n",
            escape_label(&s.agent_name),
            s.input_tokens
        ));
    }
    out.push('\n');

    // agentsight_token_output_total
    out.push_str(
        "# HELP agentsight_token_output_total Total output tokens consumed by agent (all-time)\n",
    );
    out.push_str("# TYPE agentsight_token_output_total counter\n");
    for s in &summaries {
        out.push_str(&format!(
            "agentsight_token_output_total{{agent=\"{}\"}} {}\n",
            escape_label(&s.agent_name),
            s.output_tokens
        ));
    }
    out.push('\n');

    // agentsight_token_total_total
    out.push_str("# HELP agentsight_token_total_total Total tokens (input+output) consumed by agent (all-time)\n");
    out.push_str("# TYPE agentsight_token_total_total counter\n");
    for s in &summaries {
        out.push_str(&format!(
            "agentsight_token_total_total{{agent=\"{}\"}} {}\n",
            escape_label(&s.agent_name),
            s.total_tokens
        ));
    }
    out.push('\n');

    // agentsight_llm_requests_total
    out.push_str(
        "# HELP agentsight_llm_requests_total Total LLM requests made by agent (all-time)\n",
    );
    out.push_str("# TYPE agentsight_llm_requests_total counter\n");
    for s in &summaries {
        out.push_str(&format!(
            "agentsight_llm_requests_total{{agent=\"{}\"}} {}\n",
            escape_label(&s.agent_name),
            s.request_count
        ));
    }
    out.push('\n');

    // agentsight_interruptions_total (per type, all-time)
    if let Some(ref istore) = data.interruption_store {
        if let Ok(stats) = istore.stats(0, i64::MAX) {
            out.push_str(
                "# HELP agentsight_interruptions_total Total interruption events by type\n",
            );
            out.push_str("# TYPE agentsight_interruptions_total counter\n");
            for s in &stats {
                out.push_str(&format!(
                    "agentsight_interruptions_total{{type=\"{}\"}} {}\n",
                    escape_label(&s.interruption_type),
                    s.count
                ));
            }
            out.push('\n');
        }
    }

    HttpResponse::Ok()
        .content_type("text/plain; version=0.0.4")
        .body(out)
}

// ─── Agent health endpoint ──────────────────────────────────────────────────

/// Response body for /api/agent-health
#[derive(Debug, Serialize)]
pub struct AgentHealthResponse {
    pub agents: Vec<AgentHealthStatus>,
    pub last_scan_time: u64,
}

/// GET /api/agent-health
///
/// Returns the latest health check results for all discovered agent processes.
/// Cosh is excluded from the response: it has no HTTP port and no daemon process,
/// so there is nothing meaningful to display in the UI. Agent-crash interruption
/// detection for Cosh still works via the health checker background scan.
#[get("/agent-health")]
pub async fn get_agent_health(
    data: web::Data<AppState>,
    req: actix_web::HttpRequest,
) -> impl Responder {
    let include_clients = req.query_string().contains("include_clients=true");
    let store = data.health_store.read().unwrap();
    let agents = store
        .all_agents()
        .into_iter()
        .filter(|a| a.agent_name != "Cosh")
        .filter(|a| {
            include_clients
                || a.role == crate::health::store::AgentRole::Gateway
                || a.status == crate::health::store::AgentHealthState::Offline
        })
        .collect();
    HttpResponse::Ok().json(AgentHealthResponse {
        agents,
        last_scan_time: store.last_scan_time,
    })
}

/// DELETE /api/agent-health/{pid}
///
/// User-acknowledges an offline agent and removes it from the store.
pub async fn delete_agent_health(
    data: web::Data<AppState>,
    path: web::Path<u32>,
) -> impl Responder {
    let pid = path.into_inner();
    let removed = data.health_store.write().unwrap().remove_by_pid(pid);
    if removed {
        HttpResponse::Ok().json(serde_json::json!({"ok": true}))
    } else {
        HttpResponse::NotFound().json(serde_json::json!({"error": "pid not found"}))
    }
}

/// POST /api/agent-health/{pid}/restart
///
/// Kill the hung process and re-launch it with its original command line.
#[post("/agent-health/{pid}/restart")]
pub async fn restart_agent_health(
    data: web::Data<AppState>,
    path: web::Path<u32>,
) -> impl Responder {
    let pid = path.into_inner();

    let restart_cmd = {
        let store = data.health_store.read().unwrap();
        store
            .all_agents()
            .into_iter()
            .find(|a| a.pid == pid)
            .and_then(|a| a.restart_cmd)
    };

    let cmd = match restart_cmd {
        Some(c) if !c.is_empty() => c,
        _ => {
            return HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "no restart command available for this pid"}));
        }
    };

    // Step 1: kill -9
    use std::process::Command;
    let kill_result = Command::new("kill").args(["-9", &pid.to_string()]).output();

    if let Err(e) = kill_result {
        return HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": format!("kill failed: {}", e)}));
    }

    // Step 2: short wait for process to exit
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Step 3: re-exec (background, don't wait)
    let exe = &cmd[0];
    let args = &cmd[1..];
    match Command::new(exe).args(args).spawn() {
        Ok(child) => {
            let new_pid = child.id();
            log::info!("Restarted agent pid={pid} -> new pid={new_pid}, cmd={cmd:?}");
            data.health_store.write().unwrap().remove_by_pid(pid);
            HttpResponse::Ok().json(serde_json::json!({
                "ok": true,
                "new_pid": new_pid,
                "cmd": cmd,
            }))
        }
        Err(e) => HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": format!("re-exec failed: {}", e)})),
    }
}

// ─── ATIF export endpoints ──────────────────────────────────────────────────

/// GET /api/export/atif/trace/{trace_id}
///
/// Exports a single trace as an ATIF trajectory document (shared v1.7 schema).
#[get("/export/atif/trace/{trace_id}")]
pub async fn export_atif_trace(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    let db_path = &data.storage_path;
    let trace_id = path.into_inner();

    let store = match GenAISqliteStore::new_with_path(db_path) {
        Ok(s) => s,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()}));
        }
    };

    let events = match store.get_trace_events(&trace_id) {
        Ok(e) => e,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()}));
        }
    };

    if events.is_empty() {
        return HttpResponse::NotFound().json(serde_json::json!({"error": "trace not found"}));
    }

    match crate::atif::convert_trace_to_atif(&trace_id, events) {
        Ok(doc) => HttpResponse::Ok().json(doc),
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// GET /api/export/atif/session/{session_id}
///
/// Exports a full session (all traces) as an ATIF trajectory document (v1.7).
#[get("/export/atif/session/{session_id}")]
pub async fn export_atif_session(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    let db_path = &data.storage_path;
    let session_id = path.into_inner();

    let store = match GenAISqliteStore::new_with_path(db_path) {
        Ok(s) => s,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()}));
        }
    };

    let events = match store.get_events_by_session(&session_id) {
        Ok(e) => e,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()}));
        }
    };

    if events.is_empty() {
        return HttpResponse::NotFound().json(serde_json::json!({"error": "session not found"}));
    }

    match crate::atif::convert_session_to_atif(&session_id, events) {
        Ok(doc) => HttpResponse::Ok().json(doc),
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// GET /api/export/atif/conversation/{conversation_id}
///
/// Exports all LLM calls for a conversation as an ATIF trajectory document (v1.7).
#[get("/export/atif/conversation/{conversation_id}")]
pub async fn export_atif_conversation(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    let db_path = &data.storage_path;
    let conversation_id = path.into_inner();

    let store = match GenAISqliteStore::new_with_path(db_path) {
        Ok(s) => s,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()}));
        }
    };

    let events = match store.get_events_by_conversation(&conversation_id) {
        Ok(e) => e,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()}));
        }
    };

    if events.is_empty() {
        return HttpResponse::NotFound()
            .json(serde_json::json!({"error": "conversation not found"}));
    }

    match crate::atif::convert_trace_to_atif(&conversation_id, events) {
        Ok(doc) => HttpResponse::Ok().json(doc),
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

// ─── Interruption endpoints ────────────────────────────────────────────────────

/// Query parameters for /api/interruptions
#[derive(Debug, Deserialize)]
pub struct InterruptionQuery {
    pub start_ns: Option<i64>,
    pub end_ns: Option<i64>,
    pub agent_name: Option<String>,
    /// Filter by type: llm_error | sse_truncated | agent_crash | token_limit | context_overflow
    pub interruption_type: Option<String>,
    pub severity: Option<String>,
    pub resolved: Option<bool>,
    pub limit: Option<i64>,
}

/// GET /api/interruptions
///
/// Returns a list of interruption events matching the query.
#[get("/interruptions")]
pub async fn list_interruptions(
    data: web::Data<AppState>,
    query: web::Query<InterruptionQuery>,
) -> impl Responder {
    let Some(ref istore) = data.interruption_store else {
        return HttpResponse::ServiceUnavailable()
            .json(serde_json::json!({"error": "Interruption store not initialized"}));
    };

    let end_ns = query.end_ns.unwrap_or_else(|| now_ns() as i64);
    let start_ns = query
        .start_ns
        .unwrap_or_else(|| end_ns - 86_400_000_000_000i64); // 24 h
    let limit = query.limit.unwrap_or(200);

    match istore.list(
        start_ns,
        end_ns,
        query.agent_name.as_deref(),
        query.interruption_type.as_deref(),
        query.severity.as_deref(),
        query.resolved,
        limit,
    ) {
        Ok(rows) => HttpResponse::Ok().json(rows),
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// GET /api/interruptions/count?start_ns=<i64>&end_ns=<i64>&agent_name=<str>
///
/// Returns total interruption count + breakdown by severity within a time range.
/// Response: { total, by_severity: { critical, high, medium, low } }
#[get("/interruptions/count")]
pub async fn interruption_count(
    data: web::Data<AppState>,
    query: web::Query<InterruptionQuery>,
) -> impl Responder {
    let Some(ref istore) = data.interruption_store else {
        return HttpResponse::ServiceUnavailable()
            .json(serde_json::json!({"error": "Interruption store not initialized"}));
    };

    let end_ns = query.end_ns.unwrap_or_else(|| now_ns() as i64);
    let start_ns = query
        .start_ns
        .unwrap_or_else(|| end_ns - 86_400_000_000_000i64);

    match istore.stats(start_ns, end_ns) {
        Ok(stats) => {
            let mut total = 0u64;
            let mut critical = 0u64;
            let mut high = 0u64;
            let mut medium = 0u64;
            let mut low = 0u64;
            for s in &stats {
                total += s.count as u64;
                match s.severity.as_str() {
                    "critical" => critical += s.count as u64,
                    "high" => high += s.count as u64,
                    "medium" => medium += s.count as u64,
                    _ => low += s.count as u64,
                }
            }
            HttpResponse::Ok().json(serde_json::json!({
                "total": total,
                "by_severity": {
                    "critical": critical,
                    "high":     high,
                    "medium":   medium,
                    "low":      low
                }
            }))
        }
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// GET /api/interruptions/stats
///
/// Returns per-type count statistics within a time range.
#[get("/interruptions/stats")]
pub async fn interruption_stats(
    data: web::Data<AppState>,
    query: web::Query<InterruptionQuery>,
) -> impl Responder {
    let Some(ref istore) = data.interruption_store else {
        return HttpResponse::ServiceUnavailable()
            .json(serde_json::json!({"error": "Interruption store not initialized"}));
    };

    let end_ns = query.end_ns.unwrap_or_else(|| now_ns() as i64);
    let start_ns = query
        .start_ns
        .unwrap_or_else(|| end_ns - 86_400_000_000_000i64);

    match istore.stats(start_ns, end_ns) {
        Ok(stats) => HttpResponse::Ok().json(stats),
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// GET /api/interruptions/session-counts?start_ns=<i64>&end_ns=<i64>
///
/// Returns unresolved interruption breakdown per session_id, grouped by severity and type.
#[get("/interruptions/session-counts")]
pub async fn interruption_session_counts(
    data: web::Data<AppState>,
    query: web::Query<InterruptionQuery>,
) -> impl Responder {
    let Some(ref istore) = data.interruption_store else {
        return HttpResponse::ServiceUnavailable()
            .json(serde_json::json!({"error": "Interruption store not initialized"}));
    };

    let end_ns = query.end_ns.unwrap_or_else(|| now_ns() as i64);
    let start_ns = query
        .start_ns
        .unwrap_or_else(|| end_ns - 86_400_000_000_000i64);

    match istore.count_unresolved_by_session_detailed(start_ns, end_ns) {
        Ok(rows) => {
            let mut map: std::collections::HashMap<
                String,
                (
                    i64,
                    std::collections::HashMap<String, i64>,
                    Vec<serde_json::Value>,
                ),
            > = std::collections::HashMap::new();
            for (sid, severity, itype, cnt) in rows {
                let entry = map
                    .entry(sid)
                    .or_insert_with(|| (0, std::collections::HashMap::new(), Vec::new()));
                entry.0 += cnt;
                *entry.1.entry(severity.clone()).or_insert(0) += cnt;
                entry.2.push(serde_json::json!({
                    "interruption_type": itype,
                    "severity": severity,
                    "count": cnt,
                }));
            }
            let json: Vec<_> = map
                .into_iter()
                .map(|(sid, (total, by_sev, types))| {
                    serde_json::json!({
                        "session_id": sid,
                        "total": total,
                        "by_severity": {
                            "critical": by_sev.get("critical").copied().unwrap_or(0),
                            "high": by_sev.get("high").copied().unwrap_or(0),
                            "medium": by_sev.get("medium").copied().unwrap_or(0),
                            "low": by_sev.get("low").copied().unwrap_or(0),
                        },
                        "types": types,
                    })
                })
                .collect();
            HttpResponse::Ok().json(json)
        }
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// GET /api/interruptions/conversation-counts?start_ns=<i64>&end_ns=<i64>
///
/// Returns unresolved interruption breakdown per conversation_id, grouped by severity and type.
#[get("/interruptions/conversation-counts")]
pub async fn interruption_conversation_counts(
    data: web::Data<AppState>,
    query: web::Query<InterruptionQuery>,
) -> impl Responder {
    let Some(ref istore) = data.interruption_store else {
        return HttpResponse::ServiceUnavailable()
            .json(serde_json::json!({"error": "Interruption store not initialized"}));
    };

    let end_ns = query.end_ns.unwrap_or_else(|| now_ns() as i64);
    let start_ns = query
        .start_ns
        .unwrap_or_else(|| end_ns - 86_400_000_000_000i64);

    match istore.count_unresolved_by_conversation_detailed(start_ns, end_ns) {
        Ok(rows) => {
            let mut map: std::collections::HashMap<
                String,
                (
                    i64,
                    std::collections::HashMap<String, i64>,
                    Vec<serde_json::Value>,
                ),
            > = std::collections::HashMap::new();
            for (cid, severity, itype, cnt) in rows {
                let entry = map
                    .entry(cid)
                    .or_insert_with(|| (0, std::collections::HashMap::new(), Vec::new()));
                entry.0 += cnt;
                *entry.1.entry(severity.clone()).or_insert(0) += cnt;
                entry.2.push(serde_json::json!({
                    "interruption_type": itype,
                    "severity": severity,
                    "count": cnt,
                }));
            }
            let json: Vec<_> = map
                .into_iter()
                .map(|(cid, (total, by_sev, types))| {
                    serde_json::json!({
                        "conversation_id": cid,
                        "total": total,
                        "by_severity": {
                            "critical": by_sev.get("critical").copied().unwrap_or(0),
                            "high": by_sev.get("high").copied().unwrap_or(0),
                            "medium": by_sev.get("medium").copied().unwrap_or(0),
                            "low": by_sev.get("low").copied().unwrap_or(0),
                        },
                        "types": types,
                    })
                })
                .collect();
            HttpResponse::Ok().json(json)
        }
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// GET /api/sessions/{session_id}/interruptions
///
/// Returns all interruption events for a specific session.
#[get("/sessions/{session_id}/interruptions")]
pub async fn list_session_interruptions(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    let Some(ref istore) = data.interruption_store else {
        return HttpResponse::ServiceUnavailable()
            .json(serde_json::json!({"error": "Interruption store not initialized"}));
    };

    let session_id = path.into_inner();
    match istore.list_by_session(&session_id) {
        Ok(rows) => HttpResponse::Ok().json(rows),
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// GET /api/conversations/{conversation_id}/interruptions
///
/// Returns all interruption events for a specific conversation.
#[get("/conversations/{conversation_id}/interruptions")]
pub async fn list_conversation_interruptions(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    let Some(ref istore) = data.interruption_store else {
        return HttpResponse::ServiceUnavailable()
            .json(serde_json::json!({"error": "Interruption store not initialized"}));
    };

    let conversation_id = path.into_inner();
    match istore.list_by_conversation(&conversation_id) {
        Ok(rows) => HttpResponse::Ok().json(rows),
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// POST /api/interruptions/{interruption_id}/resolve
///
/// Mark a specific interruption event as resolved.
pub async fn resolve_interruption(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    let Some(ref istore) = data.interruption_store else {
        return HttpResponse::ServiceUnavailable()
            .json(serde_json::json!({"error": "Interruption store not initialized"}));
    };

    let interruption_id = path.into_inner();
    match istore.resolve(&interruption_id) {
        Ok(true) => HttpResponse::Ok().json(serde_json::json!({"status": "resolved"})),
        Ok(false) => {
            HttpResponse::NotFound().json(serde_json::json!({"error": "Interruption not found"}))
        }
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// GET /api/interruptions/{interruption_id}
///
/// Get a single interruption event by ID.
#[get("/interruptions/{interruption_id}")]
pub async fn get_interruption(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    let Some(ref istore) = data.interruption_store else {
        return HttpResponse::ServiceUnavailable()
            .json(serde_json::json!({"error": "Interruption store not initialized"}));
    };

    let interruption_id = path.into_inner();
    match istore.get_by_id(&interruption_id) {
        Ok(Some(row)) => HttpResponse::Ok().json(row),
        Ok(None) => {
            HttpResponse::NotFound().json(serde_json::json!({"error": "Interruption not found"}))
        }
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

// ─── Trajectory collection endpoints ────────────────────────────────────

/// Query parameters for `/api/trajectories`.
#[derive(Debug, Deserialize)]
pub struct TrajectoryQuery {
    pub project: Option<String>,
    pub source: Option<String>,
    pub agent_name: Option<String>,
    /// Max rows returned (default 200).
    pub limit: Option<i64>,
}

/// Default and hard-cap for the trajectory list `limit` parameter.
const TRAJECTORY_DEFAULT_LIMIT: i64 = 200;
const TRAJECTORY_MAX_LIMIT: i64 = 1000;

/// GET /api/trajectories?project=&source=&agent_name=&limit=
///
/// Lists collected trajectories (newest first) without the bulky `atif_json`
/// column. When the trajectory store is unavailable (collection never ran),
/// returns an empty list with 200 for graceful UI degradation.
#[get("/trajectories")]
pub async fn list_trajectories(
    data: web::Data<AppState>,
    query: web::Query<TrajectoryQuery>,
) -> impl Responder {
    let Some(tstore) = data.trajectory_store() else {
        return HttpResponse::Ok().json(Vec::<serde_json::Value>::new());
    };
    // Normalize: <= 0 falls back to default; cap at hard upper bound to
    // prevent unbounded full-table scans (SQLite treats negative LIMIT as
    // "no limit").
    let limit = match query.limit {
        Some(v) if v > 0 => v.min(TRAJECTORY_MAX_LIMIT),
        _ => TRAJECTORY_DEFAULT_LIMIT,
    };
    match tstore.list_summaries(
        query.project.as_deref(),
        query.source.as_deref(),
        query.agent_name.as_deref(),
        limit,
    ) {
        Ok(rows) => HttpResponse::Ok().json(rows),
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// GET /api/trajectories/filters
///
/// Returns distinct project / source / agent_name values for the list UI
/// dropdowns. Must be registered before `/trajectories/{session_id}`.
#[get("/trajectories/filters")]
pub async fn trajectory_filters(data: web::Data<AppState>) -> impl Responder {
    let Some(tstore) = data.trajectory_store() else {
        return HttpResponse::Ok().json(serde_json::json!({
            "projects": [], "sources": [], "agent_names": []
        }));
    };
    match tstore.list_filters() {
        Ok(filters) => HttpResponse::Ok().json(filters),
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// GET /api/trajectories/{session_id}
///
/// Returns the stored ATIF v1.7 JSON document for one trajectory. When the
/// session has subagent rows (`<session_id>:subagent:%`), they are embedded
/// into the document's `subagent_trajectories` array so the frontend can
/// navigate them via breadcrumb.
#[get("/trajectories/{session_id}")]
pub async fn get_trajectory_detail(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    let Some(tstore) = data.trajectory_store() else {
        return HttpResponse::NotFound().json(
            serde_json::json!({"error": "not_found", "message": "Trajectory store not available"}),
        );
    };
    let session_id = path.into_inner();
    match tstore.get_atif_json(&session_id) {
        Ok(Some(atif_json)) => {
            // Inject subagent_trajectories if any exist.
            let enriched = match tstore.get_subagent_atif_jsons(&session_id) {
                Ok(subs) if !subs.is_empty() => inject_subagents(&atif_json, &subs),
                _ => atif_json,
            };
            HttpResponse::Ok()
                .content_type("application/json")
                .body(enriched)
        }
        Ok(None) => HttpResponse::NotFound()
            .json(serde_json::json!({"error": "not_found", "message": "Trajectory not found"})),
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// Parse the parent ATIF JSON, embed subagent documents into
/// `subagent_trajectories`, and re-serialize.
fn inject_subagents(parent_json: &str, sub_jsons: &[String]) -> String {
    let mut doc: serde_json::Value = match serde_json::from_str(parent_json) {
        Ok(v) => v,
        Err(_) => return parent_json.to_string(),
    };
    let subs: Vec<serde_json::Value> = sub_jsons
        .iter()
        .filter_map(|s| serde_json::from_str(s).ok())
        .collect();
    if subs.is_empty() {
        return parent_json.to_string();
    }
    doc["subagent_trajectories"] = serde_json::Value::Array(subs);
    serde_json::to_string(&doc).unwrap_or_else(|_| parent_json.to_string())
}

// ─── Skill Metrics endpoints ─────────────────────────────────────────────────

/// Query parameters for skill metrics endpoints.
#[derive(Debug, Deserialize)]
pub struct SkillMetricsQuery {
    pub start_ns: Option<i64>,
    pub end_ns: Option<i64>,
    pub agent_name: Option<String>,
    /// Granularity for hotness trend: "day" or "week" (default: "week")
    pub granularity: Option<String>,
}

/// GET /api/skill-metrics — full skill metrics report
#[get("/skill-metrics")]
pub async fn skill_metrics_all(
    data: web::Data<AppState>,
    query: web::Query<SkillMetricsQuery>,
) -> impl Responder {
    compute_skill_metrics_response(
        &data.storage_path,
        &query,
        crate::skill_metrics::MetricOptions::all(),
    )
}

/// GET /api/skill-metrics/downloads
#[get("/skill-metrics/downloads")]
pub async fn skill_metrics_downloads(
    data: web::Data<AppState>,
    query: web::Query<SkillMetricsQuery>,
) -> impl Responder {
    compute_skill_metrics_response(
        &data.storage_path,
        &query,
        crate::skill_metrics::MetricOptions {
            downloads: true,
            ..Default::default()
        },
    )
}

/// GET /api/skill-metrics/loads
#[get("/skill-metrics/loads")]
pub async fn skill_metrics_loads(
    data: web::Data<AppState>,
    query: web::Query<SkillMetricsQuery>,
) -> impl Responder {
    compute_skill_metrics_response(
        &data.storage_path,
        &query,
        crate::skill_metrics::MetricOptions {
            loads: true,
            ..Default::default()
        },
    )
}

/// GET /api/skill-metrics/usage-ratio
#[get("/skill-metrics/usage-ratio")]
pub async fn skill_metrics_usage_ratio(
    data: web::Data<AppState>,
    query: web::Query<SkillMetricsQuery>,
) -> impl Responder {
    compute_skill_metrics_response(
        &data.storage_path,
        &query,
        crate::skill_metrics::MetricOptions {
            usage_ratio: true,
            ..Default::default()
        },
    )
}

/// GET /api/skill-metrics/distribution
#[get("/skill-metrics/distribution")]
pub async fn skill_metrics_distribution(
    data: web::Data<AppState>,
    query: web::Query<SkillMetricsQuery>,
) -> impl Responder {
    compute_skill_metrics_response(
        &data.storage_path,
        &query,
        crate::skill_metrics::MetricOptions {
            distribution: true,
            ..Default::default()
        },
    )
}

/// GET /api/skill-metrics/hotness
#[get("/skill-metrics/hotness")]
pub async fn skill_metrics_hotness(
    data: web::Data<AppState>,
    query: web::Query<SkillMetricsQuery>,
) -> impl Responder {
    compute_skill_metrics_response(
        &data.storage_path,
        &query,
        crate::skill_metrics::MetricOptions {
            hotness: true,
            ..Default::default()
        },
    )
}

/// Shared implementation for all skill metrics endpoints.
fn compute_skill_metrics_response(
    storage_path: &std::path::Path,
    query: &SkillMetricsQuery,
    mut options: crate::skill_metrics::MetricOptions,
) -> HttpResponse {
    // Apply granularity from query params
    if let Some(ref g) = query.granularity {
        if g == "day" {
            options.hotness_granularity = crate::skill_metrics::HotnessGranularity::Day;
        }
    }

    let end_ns = query.end_ns.unwrap_or_else(|| now_ns() as i64);
    // Default: 7 days
    let start_ns = query
        .start_ns
        .unwrap_or_else(|| end_ns - 7 * 86_400_000_000_000i64);

    let store = match GenAISqliteStore::new_with_path(storage_path) {
        Ok(s) => s,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()}));
        }
    };

    let events = match store.get_events_in_time_range(start_ns, end_ns, query.agent_name.as_deref())
    {
        Ok(e) => e,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()}));
        }
    };

    let report = crate::skill_metrics::compute_skill_metrics(&events, &options);
    HttpResponse::Ok().json(report)
}
