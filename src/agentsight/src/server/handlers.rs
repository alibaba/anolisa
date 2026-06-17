//! API request handlers

use std::collections::HashMap;

use actix_web::http::StatusCode;
use actix_web::{HttpResponse, Responder, get, post, web};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::AppState;
use crate::agent_sec::{AgentSecClient, AgentSecClientError, DaemonResponse};
use crate::health::AgentHealthStatus;
use crate::storage::sqlite::GenAISqliteStore;
use crate::storage::sqlite::genai::{ModelTimeseriesBucket, TimeseriesBucket};
use crate::storage::sqlite::tokenless::{self, TokenlessStatsStore};

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

// ─── Session / Trace query endpoints ───────────────────────────────────────

/// Query parameters for /api/sessions
#[derive(Debug, Deserialize)]
pub struct SessionQuery {
    /// Start of time range in nanoseconds (default: 24 h ago)
    pub start_ns: Option<i64>,
    /// End of time range in nanoseconds (default: now)
    pub end_ns: Option<i64>,
}

/// GET /api/sessions?start_ns=<i64>&end_ns=<i64>
///
/// Returns a list of gen_ai.session_id values with aggregated stats.
#[get("/api/sessions")]
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
        Ok(store) => match store.list_sessions(start_ns, end_ns) {
            Ok(sessions) => HttpResponse::Ok().json(sessions),
            Err(e) => HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()})),
        },
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// GET /api/sessions/{session_id}/traces?start_ns=<i64>&end_ns=<i64>
///
/// Returns conversations belonging to a session with token stats.
/// Optional `start_ns`/`end_ns` query parameters filter conversations by time.
#[get("/api/sessions/{session_id}/traces")]
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
        Ok(store) => match store.list_traces_by_session(&session_id, start_ns, end_ns) {
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
#[get("/api/traces/{trace_id}")]
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
#[get("/api/conversations/{conversation_id}")]
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

// ─── Agent-name & time-series endpoints ────────────────────────────────────

/// Query parameters shared by agent-name and time-series endpoints
#[derive(Debug, Deserialize)]
pub struct TimeRangeQuery {
    pub start_ns: Option<i64>,
    pub end_ns: Option<i64>,
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
#[get("/api/agent-names")]
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
#[get("/api/timeseries")]
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

// ─── agent-sec Security Observability endpoints ─────────────────────────────

/// GET /api/security/status
///
/// Reports only whether the agent-sec daemon is reachable. Data-plane failures
/// are surfaced by the individual security query endpoints.
#[get("/api/security/status")]
pub async fn security_status(data: web::Data<AppState>) -> impl Responder {
    let client = match agent_sec_client(&data) {
        Ok(client) => client,
        Err(err) => {
            return security_state_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "daemon_unreachable",
                json!({ "error": err.to_string() }),
                Some("agent-sec daemon is unavailable"),
            );
        }
    };

    let daemon_health = match call_daemon(client, "daemon.health", json!({})).await {
        Ok(response) if response.ok => response,
        Ok(response) => return daemon_error_response(response),
        Err(err) => {
            return security_state_response(
                client_error_status(&err),
                "daemon_unreachable",
                json!({ "error": err.to_string() }),
                Some("agent-sec daemon is unavailable"),
            );
        }
    };

    security_state_response(
        StatusCode::OK,
        "daemon_reachable",
        json!({
            "daemon": daemon_health.data,
            "socket_path": client_socket_path(&data),
        }),
        None,
    )
}

/// GET /api/security/summary
#[get("/api/security/summary")]
pub async fn security_summary(
    data: web::Data<AppState>,
    query: web::Query<HashMap<String, String>>,
) -> impl Responder {
    proxy_security_query(data, "sec.summary", query_to_params(&query)).await
}

/// GET /api/security/events/count-by
#[get("/api/security/events/count-by")]
pub async fn security_events_count_by(
    data: web::Data<AppState>,
    query: web::Query<HashMap<String, String>>,
) -> impl Responder {
    proxy_security_query(data, "sec.events.count_by", query_to_params(&query)).await
}

/// GET /api/security/events
#[get("/api/security/events")]
pub async fn security_events_list(
    data: web::Data<AppState>,
    query: web::Query<HashMap<String, String>>,
) -> impl Responder {
    proxy_security_query(data, "sec.events.list", query_to_params(&query)).await
}

/// GET /api/security/events/{event_id}
#[get("/api/security/events/{event_id}")]
pub async fn security_event_detail(
    data: web::Data<AppState>,
    path: web::Path<String>,
    query: web::Query<HashMap<String, String>>,
) -> impl Responder {
    let params = query_to_params(&query).map(|mut params| {
        params["event_id"] = Value::String(path.into_inner());
        params
    });
    proxy_security_query(data, "sec.events.get", params).await
}

/// GET /api/security/observability/sessions
#[get("/api/security/observability/sessions")]
pub async fn security_observability_sessions(
    data: web::Data<AppState>,
    query: web::Query<HashMap<String, String>>,
) -> impl Responder {
    proxy_security_query(data, "obs.sessions.list", query_to_params(&query)).await
}

/// GET /api/security/observability/sessions/{session_id}/runs
#[get("/api/security/observability/sessions/{session_id}/runs")]
pub async fn security_observability_runs(
    data: web::Data<AppState>,
    path: web::Path<String>,
    query: web::Query<HashMap<String, String>>,
) -> impl Responder {
    let params = query_to_params(&query).map(|mut params| {
        params["session_id"] = Value::String(path.into_inner());
        params
    });
    proxy_security_query(data, "obs.runs.list", params).await
}

/// GET /api/security/observability/timeline
#[get("/api/security/observability/timeline")]
pub async fn security_observability_timeline(
    data: web::Data<AppState>,
    query: web::Query<HashMap<String, String>>,
) -> impl Responder {
    proxy_security_query(data, "obs.timeline.get", query_to_params(&query)).await
}

async fn proxy_security_query(
    data: web::Data<AppState>,
    method: &'static str,
    params: Result<Value, HttpResponse>,
) -> HttpResponse {
    let params = match params {
        Ok(params) => params,
        Err(response) => return response,
    };

    let client = match agent_sec_client(&data) {
        Ok(client) => client,
        Err(err) => return client_error_response(err),
    };

    match call_daemon(client, method, params).await {
        Ok(response) if response.ok => {
            let state = derive_security_query_state(method, &response.data);
            security_state_response(StatusCode::OK, state, response.data, None)
        }
        Ok(response) => daemon_error_response(response),
        Err(err) => client_error_response(err),
    }
}

async fn call_daemon(
    client: AgentSecClient,
    method: &'static str,
    params: Value,
) -> Result<DaemonResponse, AgentSecClientError> {
    let method = method.to_string();
    match web::block(move || client.call(&method, params)).await {
        Ok(result) => result,
        Err(err) => Err(AgentSecClientError::Transport(format!(
            "daemon client task failed: {err}"
        ))),
    }
}

fn agent_sec_client(data: &web::Data<AppState>) -> Result<AgentSecClient, AgentSecClientError> {
    AgentSecClient::with_timeout(None, data.security_observability.timeout_ms)
}

fn client_socket_path(data: &web::Data<AppState>) -> Option<String> {
    agent_sec_client(data)
        .ok()
        .map(|client| client.socket_path().display().to_string())
}

fn query_to_params(query: &web::Query<HashMap<String, String>>) -> Result<Value, HttpResponse> {
    let mut params = serde_json::Map::new();
    for (key, raw_value) in query.iter() {
        let value = parse_security_query_value(key, raw_value)?;
        params.insert(key.clone(), value);
    }
    Ok(Value::Object(params))
}

fn parse_security_query_value(key: &str, raw_value: &str) -> Result<Value, HttpResponse> {
    match key {
        "start_ns" | "end_ns" | "limit" | "offset" | "latest_limit" => {
            let value = raw_value
                .parse::<i64>()
                .map_err(|_| bad_request_response(format!("{key} must be an integer")))?;
            Ok(Value::Number(value.into()))
        }
        "include_details" | "include_security" => parse_bool(raw_value)
            .map(Value::Bool)
            .ok_or_else(|| bad_request_response(format!("{key} must be a boolean"))),
        _ => Ok(Value::String(raw_value.to_string())),
    }
}

fn parse_bool(raw_value: &str) -> Option<bool> {
    match raw_value {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

fn derive_security_query_state(method: &str, data: &Value) -> &'static str {
    match method {
        "sec.summary" if data.get("total").and_then(Value::as_i64).unwrap_or(0) == 0 => "empty",
        "sec.events.list" | "obs.sessions.list" | "obs.runs.list"
            if data.get("total").and_then(Value::as_i64).unwrap_or(0) == 0 =>
        {
            "empty"
        }
        "sec.events.count_by"
            if data
                .get("items")
                .and_then(Value::as_array)
                .map(|items| items.is_empty())
                .unwrap_or(true) =>
        {
            "empty"
        }
        "sec.events.get" if !data.get("found").and_then(Value::as_bool).unwrap_or(false) => {
            "not_found"
        }
        "sec.events.get" => "found",
        "obs.timeline.get"
            if data
                .get("items")
                .and_then(Value::as_array)
                .map(|items| items.is_empty())
                .unwrap_or(true) =>
        {
            "empty"
        }
        _ => "ok",
    }
}

fn security_state_response(
    status: StatusCode,
    state: &str,
    data: Value,
    message: Option<&str>,
) -> HttpResponse {
    let mut body = json!({
        "state": state,
        "data": data,
        "meta": {
            "source": "agent-sec-daemon",
        },
    });
    if let Some(message) = message {
        body["message"] = Value::String(message.to_string());
    }
    HttpResponse::build(status).json(body)
}

fn bad_request_response(message: String) -> HttpResponse {
    HttpResponse::BadRequest().json(json!({
        "error": {
            "code": "bad_request",
            "message": message,
            "retryable": false,
        }
    }))
}

fn client_error_response(err: AgentSecClientError) -> HttpResponse {
    let status = client_error_status(&err);
    let (code, retryable) = match &err {
        AgentSecClientError::SocketPath(_) | AgentSecClientError::Transport(_) => {
            ("daemon_unavailable", true)
        }
        AgentSecClientError::Timeout(_) => ("daemon_timeout", true),
        AgentSecClientError::ResponseTooLarge(_) => ("payload_too_large", false),
        AgentSecClientError::Protocol(_) => ("daemon_protocol_mismatch", false),
    };

    HttpResponse::build(status).json(json!({
        "error": {
            "code": code,
            "message": err.to_string(),
            "retryable": retryable,
        }
    }))
}

fn client_error_status(err: &AgentSecClientError) -> StatusCode {
    match err {
        AgentSecClientError::SocketPath(_) | AgentSecClientError::Transport(_) => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        AgentSecClientError::Timeout(_) => StatusCode::GATEWAY_TIMEOUT,
        AgentSecClientError::ResponseTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
        AgentSecClientError::Protocol(_) => StatusCode::BAD_GATEWAY,
    }
}

fn daemon_error_response(response: DaemonResponse) -> HttpResponse {
    let daemon_error = response.error.clone();
    let daemon_code = daemon_error
        .as_ref()
        .map(|error| error.code.as_str())
        .unwrap_or("internal_error");
    let message = daemon_error
        .as_ref()
        .map(|error| error.message.clone())
        .unwrap_or_else(|| response.stderr.clone());

    let (status, code, retryable) = match daemon_code {
        "bad_request" => (StatusCode::BAD_REQUEST, "bad_request", false),
        "unknown_method" => (StatusCode::BAD_GATEWAY, "daemon_protocol_mismatch", false),
        "payload_too_large" => (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", false),
        "timeout" => (StatusCode::GATEWAY_TIMEOUT, "daemon_timeout", true),
        "busy" => (StatusCode::SERVICE_UNAVAILABLE, "daemon_busy", true),
        "unavailable" => (
            StatusCode::SERVICE_UNAVAILABLE,
            "daemon_capability_unavailable",
            true,
        ),
        "shutdown" => (StatusCode::SERVICE_UNAVAILABLE, "daemon_shutdown", true),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "daemon_internal_error",
            false,
        ),
    };

    HttpResponse::build(status).json(json!({
        "error": {
            "code": code,
            "message": message,
            "retryable": retryable,
            "daemon_code": daemon_code,
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

    use crate::agent_sec::DaemonErrorPayload;
    use crate::health::HealthStore;

    use super::*;

    #[test]
    fn query_to_params_parses_security_query_types() {
        let query = web::Query(HashMap::from([
            ("start_ns".to_string(), "100".to_string()),
            ("limit".to_string(), "25".to_string()),
            ("include_details".to_string(), "true".to_string()),
            ("agent_name".to_string(), "codex".to_string()),
        ]));

        let params = query_to_params(&query).expect("valid query should parse");

        assert_eq!(
            params,
            json!({
                "start_ns": 100,
                "limit": 25,
                "include_details": true,
                "agent_name": "codex",
            })
        );
    }

    #[actix_web::test]
    async fn query_to_params_rejects_invalid_security_query_types() {
        let query = web::Query(HashMap::from([(
            "include_security".to_string(),
            "sometimes".to_string(),
        )]));

        let response = query_to_params(&query).expect_err("invalid boolean should fail");
        let body = response_json(response).await;

        assert_eq!(body["error"]["code"], "bad_request");
        assert_eq!(body["error"]["retryable"], false);
        assert!(
            body["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("include_security"))
        );
    }

    #[test]
    fn derive_security_query_state_maps_empty_and_found_states() {
        assert_eq!(
            derive_security_query_state("sec.summary", &json!({})),
            "empty"
        );
        assert_eq!(
            derive_security_query_state("sec.events.list", &json!({ "total": 0 })),
            "empty"
        );
        assert_eq!(
            derive_security_query_state("sec.events.get", &json!({ "found": false })),
            "not_found"
        );
        assert_eq!(
            derive_security_query_state("sec.events.get", &json!({ "found": true })),
            "found"
        );
        assert_eq!(
            derive_security_query_state("obs.timeline.get", &json!({ "items": [] })),
            "empty"
        );
        assert_eq!(
            derive_security_query_state("obs.timeline.get", &json!({ "items": [{}] })),
            "ok"
        );
    }

    #[actix_web::test]
    async fn daemon_error_response_maps_daemon_codes_to_http_errors() {
        for (daemon_code, status, code, retryable) in [
            ("bad_request", StatusCode::BAD_REQUEST, "bad_request", false),
            (
                "unknown_method",
                StatusCode::BAD_GATEWAY,
                "daemon_protocol_mismatch",
                false,
            ),
            (
                "payload_too_large",
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                false,
            ),
            (
                "timeout",
                StatusCode::GATEWAY_TIMEOUT,
                "daemon_timeout",
                true,
            ),
            ("busy", StatusCode::SERVICE_UNAVAILABLE, "daemon_busy", true),
            (
                "unavailable",
                StatusCode::SERVICE_UNAVAILABLE,
                "daemon_capability_unavailable",
                true,
            ),
            (
                "shutdown",
                StatusCode::SERVICE_UNAVAILABLE,
                "daemon_shutdown",
                true,
            ),
            (
                "internal_error",
                StatusCode::INTERNAL_SERVER_ERROR,
                "daemon_internal_error",
                false,
            ),
        ] {
            let response = daemon_error_response(daemon_response_with_error(daemon_code));
            assert_eq!(response.status(), status);

            let body = response_json(response).await;
            assert_eq!(body["error"]["code"], code);
            assert_eq!(body["error"]["daemon_code"], daemon_code);
            assert_eq!(body["error"]["retryable"], retryable);
        }
    }

    #[actix_web::test]
    async fn client_error_response_maps_protocol_errors_to_bad_gateway() {
        for (err, status, code, retryable) in [
            (
                AgentSecClientError::SocketPath("missing runtime dir".to_string()),
                StatusCode::SERVICE_UNAVAILABLE,
                "daemon_unavailable",
                true,
            ),
            (
                AgentSecClientError::Transport("connect refused".to_string()),
                StatusCode::SERVICE_UNAVAILABLE,
                "daemon_unavailable",
                true,
            ),
            (
                AgentSecClientError::Timeout("read response".to_string()),
                StatusCode::GATEWAY_TIMEOUT,
                "daemon_timeout",
                true,
            ),
            (
                AgentSecClientError::ResponseTooLarge(128),
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                false,
            ),
            (
                AgentSecClientError::Protocol("unexpected response".to_string()),
                StatusCode::BAD_GATEWAY,
                "daemon_protocol_mismatch",
                false,
            ),
        ] {
            let response = client_error_response(err);
            assert_eq!(response.status(), status);

            let body = response_json(response).await;
            assert_eq!(body["error"]["code"], code);
            assert_eq!(body["error"]["retryable"], retryable);
        }
    }

    #[actix_web::test]
    async fn security_endpoints_report_client_errors_when_daemon_config_is_invalid() {
        let app = awtest::init_service(
            App::new()
                .app_data(test_app_state(0))
                .service(security_status)
                .service(security_summary)
                .service(security_events_count_by)
                .service(security_events_list)
                .service(security_event_detail)
                .service(security_observability_sessions)
                .service(security_observability_runs)
                .service(security_observability_timeline),
        )
        .await;

        for (uri, status) in [
            ("/api/security/status", StatusCode::SERVICE_UNAVAILABLE),
            ("/api/security/summary?limit=1", StatusCode::BAD_GATEWAY),
            (
                "/api/security/events/count-by?include_security=true",
                StatusCode::BAD_GATEWAY,
            ),
            ("/api/security/events?offset=1", StatusCode::BAD_GATEWAY),
            ("/api/security/events/event-1", StatusCode::BAD_GATEWAY),
            (
                "/api/security/observability/sessions?latest_limit=1",
                StatusCode::BAD_GATEWAY,
            ),
            (
                "/api/security/observability/sessions/session-1/runs",
                StatusCode::BAD_GATEWAY,
            ),
            (
                "/api/security/observability/timeline?end_ns=2",
                StatusCode::BAD_GATEWAY,
            ),
        ] {
            let response =
                awtest::call_service(&app, awtest::TestRequest::get().uri(uri).to_request()).await;

            assert_eq!(response.status(), status);
        }
    }

    async fn response_json(response: HttpResponse) -> Value {
        let body = to_bytes(response.into_body())
            .await
            .expect("response body should be readable");
        serde_json::from_slice(&body).expect("response body should be JSON")
    }

    fn daemon_response_with_error(code: &str) -> DaemonResponse {
        DaemonResponse {
            request_id: "req-1".to_string(),
            ok: false,
            data: Value::Null,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 1,
            error: Some(DaemonErrorPayload {
                code: code.to_string(),
                message: format!("{code} message"),
            }),
        }
    }

    fn test_app_state(timeout_ms: u64) -> web::Data<AppState> {
        web::Data::new(AppState {
            storage_path: PathBuf::from(":memory:"),
            start_time: Instant::now(),
            health_store: Arc::new(RwLock::new(HealthStore::new())),
            interruption_store: None,
            security_observability: super::super::SecurityObservabilityConfig { timeout_ms },
        })
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
#[get("/api/agent-health")]
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
#[actix_web::delete("/api/agent-health/{pid}")]
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
#[actix_web::post("/api/agent-health/{pid}/restart")]
pub async fn restart_agent_health(
    data: web::Data<AppState>,
    path: web::Path<u32>,
) -> impl Responder {
    let pid = path.into_inner();

    // 从 store 中取出 restart_cmd
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

    // Step 2: 短暂等待进程退出
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Step 3: re-exec（后台启动，不等待）
    let exe = &cmd[0];
    let args = &cmd[1..];
    match Command::new(exe).args(args).spawn() {
        Ok(child) => {
            let new_pid = child.id();
            log::info!("Restarted agent pid={pid} -> new pid={new_pid}, cmd={cmd:?}");
            // 从 store 中删除旧 PID 条目，下次扫描时新 PID 会自动加入
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
/// Exports a single trace as an ATIF v1.6 trajectory document.
#[get("/api/export/atif/trace/{trace_id}")]
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
/// Exports a full session (all traces) as an ATIF v1.6 trajectory document.
#[get("/api/export/atif/session/{session_id}")]
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
/// Exports all LLM calls for a conversation as an ATIF v1.6 trajectory document.
#[get("/api/export/atif/conversation/{conversation_id}")]
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
#[get("/api/interruptions")]
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
#[get("/api/interruptions/count")]
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
#[get("/api/interruptions/stats")]
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
/// Response: [ { session_id, total, by_severity: { critical, high, medium, low },
///              types: [ { interruption_type, severity, count }, ... ] }, ... ]
#[get("/api/interruptions/session-counts")]
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
            // Group by session_id
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
/// Response: [ { conversation_id, total, by_severity: { critical, high, medium, low },
///              types: [ { interruption_type, severity, count }, ... ] }, ... ]
#[get("/api/interruptions/conversation-counts")]
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
#[get("/api/sessions/{session_id}/interruptions")]
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
#[get("/api/conversations/{conversation_id}/interruptions")]
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
#[post("/api/interruptions/{interruption_id}/resolve")]
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
#[get("/api/interruptions/{interruption_id}")]
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

// ─── Token Savings endpoint ─────────────────────────────────────────────────

/// Query parameters for /api/token-savings
#[derive(Debug, Deserialize)]
pub struct TokenSavingsQuery {
    pub start_ns: Option<i64>,
    pub end_ns: Option<i64>,
    pub agent_name: Option<String>,
}

/// Per-strategy saved amounts
#[derive(Debug, Serialize)]
pub struct StrategyBreakdown {
    pub strategy: String,
    pub label: String,
    pub saved: i64,
    pub compounded_saved: i64,
}

/// Overall savings summary
#[derive(Debug, Serialize)]
pub struct SavingsSummary {
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_tokens: i64,
    pub total_saved_tokens: i64,
    pub total_compounded_saved: i64,
    pub savings_rate: f64,
    pub compounded_savings_rate: f64,
    pub total_tool_saved: i64,
    pub total_mcp_saved: i64,
    pub total_compounded_tool_saved: i64,
    pub total_compounded_mcp_saved: i64,
    pub strategy_breakdown: Vec<StrategyBreakdown>,
}

/// A single optimization item within a session
#[derive(Debug, Serialize, Clone)]
pub struct OptimizationItemDto {
    pub id: String,
    pub category: String,
    pub title: String,
    pub strategy: String,
    pub strategy_label: String,
    pub before_tokens: i64,
    pub after_tokens: i64,
    pub saved_tokens: i64,
    pub compounded_saved: i64,
    pub compounding_turns: i64,
    pub before_summary: String,
    pub after_summary: String,
    pub before_text: Option<String>,
    pub after_text: Option<String>,
    pub diff_lines: Vec<DiffLineDto>,
}

/// A single diff line
#[derive(Debug, Serialize, Clone)]
pub struct DiffLineDto {
    #[serde(rename = "type")]
    pub line_type: String,
    pub content: String,
}

/// Per-session savings data
#[derive(Debug, Serialize)]
pub struct SessionSavingsDto {
    pub session_id: String,
    pub agent_name: String,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_tokens: i64,
    pub saved_tokens: i64,
    pub compounded_saved: i64,
    pub savings_rate: f64,
    pub compounded_savings_rate: f64,
    pub request_count: i64,
    pub tool_saved: i64,
    pub mcp_saved: i64,
    pub optimization_items: Vec<OptimizationItemDto>,
}

/// Full response for /api/token-savings
#[derive(Debug, Serialize)]
pub struct TokenSavingsResponse {
    pub stats_available: bool,
    pub summary: SavingsSummary,
    pub sessions: Vec<SessionSavingsDto>,
}

/// Map stats.db operation field to frontend category.
fn map_operation_to_category(operation: &str) -> &str {
    match operation {
        "compress-response" | "compress-toon" => "mcp_response",
        "rewrite-command" | "compress-schema" => "tool_output",
        _ => "tool_output",
    }
}

/// Map operation to a human-readable title.
fn map_operation_to_title(operation: &str) -> &str {
    match operation {
        "compress-response" => "MCP\u{54cd}\u{5e94}\u{538b}\u{7f29}",
        "rewrite-command" => "\u{5de5}\u{5177}\u{8f93}\u{51fa}\u{4f18}\u{5316}",
        "compress-schema" => "Schema \u{538b}\u{7f29}",
        "compress-toon" => "TOON \u{7f16}\u{7801}",
        _ => "\u{5176}\u{4ed6}\u{4f18}\u{5316}",
    }
}

/// Map operation to a human-readable strategy label.
fn map_operation_to_strategy_label(operation: &str) -> &str {
    match operation {
        "compress-schema" => "Schema \u{538b}\u{7f29}",
        "compress-response" => "\u{54cd}\u{5e94}\u{538b}\u{7f29}",
        "rewrite-command" => "\u{547d}\u{4ee4}\u{91cd}\u{5199}",
        "compress-toon" => "TOON \u{7f16}\u{7801}",
        _ => "\u{5176}\u{4ed6}\u{4f18}\u{5316}",
    }
}

/// GET /api/token-savings?start_ns=<i64>&end_ns=<i64>&agent_name=<str>
///
/// Returns token savings data by cross-referencing genai_events.db
/// with the external ~/.tokenless/stats.db.
#[get("/api/token-savings")]
pub async fn get_token_savings(
    data: web::Data<AppState>,
    query: web::Query<TokenSavingsQuery>,
) -> impl Responder {
    let db_path = &data.storage_path;
    let end_ns = query.end_ns.unwrap_or_else(|| now_ns() as i64);
    let start_ns = query
        .start_ns
        .unwrap_or_else(|| end_ns - 86_400_000_000_000i64);
    let agent_name = query.agent_name.as_deref();

    // Step 1: Query sessions from genai_events.db
    let sessions = match GenAISqliteStore::new_with_path(db_path) {
        Ok(store) => match store.list_sessions_for_savings(start_ns, end_ns, agent_name) {
            Ok(s) => s,
            Err(e) => {
                return HttpResponse::InternalServerError()
                    .json(serde_json::json!({"error": e.to_string()}));
            }
        },
        Err(e) => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()}));
        }
    };

    // Step 2: Open stats.db (read-only, graceful if absent)
    let stats_path = tokenless::default_stats_path();
    let stats_store = TokenlessStatsStore::open_if_exists(&stats_path);
    let stats_available = stats_store.is_some();

    // Step 3: Build tool_call_id → (turn_index, session_id) map from genai_events.
    // This gives us all known tool_use_ids and their session membership.
    let session_ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
    let turn_indices = match GenAISqliteStore::new_with_path(db_path) {
        Ok(store) => store
            .get_tool_call_turn_indices(&session_ids)
            .unwrap_or_default(),
        Err(_) => std::collections::HashMap::new(),
    };

    // Step 4: Query stats.db by tool_use_ids (instead of session_ids)
    let stats_by_session = if let Some(ref store) = stats_store {
        let tool_use_ids: Vec<&str> = turn_indices.keys().map(|s| s.as_str()).collect();
        let rows = store.get_stats_by_tool_use_ids(&tool_use_ids);
        // Group by session: use turn_indices to determine session, fallback to row.session_id
        let mut map: std::collections::HashMap<String, Vec<_>> = std::collections::HashMap::new();
        for row in rows {
            let sid = turn_indices
                .get(&row.tool_use_id)
                .map(|info| info.session_id.clone())
                .unwrap_or_else(|| row.session_id.clone());
            map.entry(sid).or_default().push(row);
        }
        map
    } else {
        std::collections::HashMap::new()
    };

    // Step 5: Build response
    let mut resp_sessions = Vec::with_capacity(sessions.len());
    let mut grand_input: i64 = 0;
    let mut grand_output: i64 = 0;
    let mut grand_saved: i64 = 0;
    let mut grand_compounded_saved: i64 = 0;
    let mut grand_tool_saved: i64 = 0;
    let mut grand_mcp_saved: i64 = 0;
    let mut grand_compounded_tool_saved: i64 = 0;
    let mut grand_compounded_mcp_saved: i64 = 0;
    let mut grand_strategy_map: std::collections::HashMap<String, (i64, i64)> =
        std::collections::HashMap::new();

    for session in &sessions {
        let total_tokens = session.total_input_tokens + session.total_output_tokens;
        let request_count = session.request_count;
        let mut session_saved: i64 = 0;
        let mut session_compounded_saved: i64 = 0;
        let mut session_tool_saved: i64 = 0;
        let mut session_mcp_saved: i64 = 0;
        let mut session_compounded_tool_saved: i64 = 0;
        let mut session_compounded_mcp_saved: i64 = 0;
        let mut items = Vec::new();

        if let Some(stat_rows) = stats_by_session.get(&session.session_id) {
            for row in stat_rows {
                let saved = row.before_tokens - row.after_tokens;
                let category = map_operation_to_category(&row.operation);
                let title = map_operation_to_title(&row.operation);

                // Compounding: the shortened tool output appears in the
                // context of all LLM calls AFTER the one that triggered the
                // tool use. If the tool was invoked at turn N (1-based) out
                // of M total turns, the savings persist for (M - N) turns.
                let turn_index = turn_indices
                    .get(&row.tool_use_id)
                    .map(|info| info.turn_index)
                    .unwrap_or(1) as i64;
                let compounding_turns = (request_count - turn_index).max(1);
                let compounded = saved * compounding_turns;

                if category == "mcp_response" {
                    session_mcp_saved += saved;
                    session_compounded_mcp_saved += compounded;
                } else {
                    session_tool_saved += saved;
                    session_compounded_tool_saved += compounded;
                }
                session_saved += saved;
                session_compounded_saved += compounded;

                let diff_lines: Vec<DiffLineDto> = Vec::new();

                let strategy = row.operation.clone();
                let strategy_label = map_operation_to_strategy_label(&row.operation).to_string();

                // Accumulate per-strategy totals for the grand summary
                let entry = grand_strategy_map
                    .entry(row.operation.clone())
                    .or_insert((0, 0));
                entry.0 += saved;
                entry.1 += compounded;

                items.push(OptimizationItemDto {
                    id: row.tool_use_id.clone(),
                    category: category.to_string(),
                    title: title.to_string(),
                    strategy,
                    strategy_label,
                    before_tokens: row.before_tokens,
                    after_tokens: row.after_tokens,
                    saved_tokens: saved,
                    compounded_saved: compounded,
                    compounding_turns,
                    before_summary: format!(
                        "\u{539f}\u{59cb}\u{5185}\u{5bb9} {} tokens",
                        row.before_tokens
                    ),
                    after_summary: format!("\u{4f18}\u{5316}\u{540e} {} tokens", row.after_tokens),
                    before_text: row.before_text.clone(),
                    after_text: row.after_text.clone(),
                    diff_lines,
                });
            }
        }

        let savings_rate = if total_tokens > 0 {
            session_saved as f64 / total_tokens as f64 * 100.0
        } else {
            0.0
        };
        let compounded_savings_rate = if total_tokens > 0 {
            session_compounded_saved as f64 / total_tokens as f64 * 100.0
        } else {
            0.0
        };

        grand_input += session.total_input_tokens;
        grand_output += session.total_output_tokens;
        grand_saved += session_saved;
        grand_compounded_saved += session_compounded_saved;
        grand_tool_saved += session_tool_saved;
        grand_mcp_saved += session_mcp_saved;
        grand_compounded_tool_saved += session_compounded_tool_saved;
        grand_compounded_mcp_saved += session_compounded_mcp_saved;

        resp_sessions.push(SessionSavingsDto {
            session_id: session.session_id.clone(),
            agent_name: session.agent_name.clone().unwrap_or_default(),
            total_input_tokens: session.total_input_tokens,
            total_output_tokens: session.total_output_tokens,
            total_tokens,
            saved_tokens: session_saved,
            compounded_saved: session_compounded_saved,
            savings_rate,
            compounded_savings_rate,
            request_count,
            tool_saved: session_tool_saved,
            mcp_saved: session_mcp_saved,
            optimization_items: items,
        });
    }

    let grand_total = grand_input + grand_output;
    let grand_rate = if grand_total > 0 {
        grand_saved as f64 / grand_total as f64 * 100.0
    } else {
        0.0
    };
    let grand_compounded_rate = if grand_total > 0 {
        grand_compounded_saved as f64 / grand_total as f64 * 100.0
    } else {
        0.0
    };

    let strategy_breakdown: Vec<StrategyBreakdown> = grand_strategy_map
        .into_iter()
        .map(|(strategy, (saved, compounded_saved))| StrategyBreakdown {
            label: map_operation_to_strategy_label(&strategy).to_string(),
            strategy,
            saved,
            compounded_saved,
        })
        .collect();

    HttpResponse::Ok().json(TokenSavingsResponse {
        stats_available,
        summary: SavingsSummary {
            total_input_tokens: grand_input,
            total_output_tokens: grand_output,
            total_tokens: grand_total,
            total_saved_tokens: grand_saved,
            total_compounded_saved: grand_compounded_saved,
            savings_rate: grand_rate,
            compounded_savings_rate: grand_compounded_rate,
            total_tool_saved: grand_tool_saved,
            total_mcp_saved: grand_mcp_saved,
            total_compounded_tool_saved: grand_compounded_tool_saved,
            total_compounded_mcp_saved: grand_compounded_mcp_saved,
            strategy_breakdown,
        },
        sessions: resp_sessions,
    })
}

// ─── Session-scoped Token Savings endpoint ──────────────────────────────────

/// Response for /api/token-savings/session/{session_id}
#[derive(Debug, Serialize)]
pub struct SessionSavingsDetail {
    pub session_id: String,
    pub stats_available: bool,
    pub total_actual_tokens: i64,
    pub total_compounded_saved: i64,
    pub total_original_tokens: i64,
    pub savings_rate: f64,
    pub items: Vec<OptimizationItemDto>,
}

/// GET /api/token-savings/session/{session_id}
///
/// Returns token savings detail for a single session.
#[get("/api/token-savings/session/{session_id}")]
pub async fn get_session_savings(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    let session_id = path.into_inner();
    let db_path = &data.storage_path;

    // Step 1: Query session from genai_events.db
    let store = match GenAISqliteStore::new_with_path(db_path) {
        Ok(s) => s,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()}));
        }
    };

    let sessions = match store.list_sessions_for_savings(0, i64::MAX, None) {
        Ok(s) => s,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()}));
        }
    };

    let session = match sessions.iter().find(|s| s.session_id == session_id) {
        Some(s) => s,
        None => {
            return HttpResponse::Ok().json(SessionSavingsDetail {
                session_id,
                stats_available: false,
                total_actual_tokens: 0,
                total_compounded_saved: 0,
                total_original_tokens: 0,
                savings_rate: 0.0,
                items: Vec::new(),
            });
        }
    };

    let total_tokens = session.total_input_tokens + session.total_output_tokens;
    let request_count = session.request_count;

    // Step 2: Get turn indices for tool_call_ids
    let session_ids = vec![session_id.as_str()];
    let turn_indices = match GenAISqliteStore::new_with_path(db_path) {
        Ok(st) => st
            .get_tool_call_turn_indices(&session_ids)
            .unwrap_or_default(),
        Err(_) => std::collections::HashMap::new(),
    };

    // Step 3: Open stats.db
    let stats_path = tokenless::default_stats_path();
    let stats_store = TokenlessStatsStore::open_if_exists(&stats_path);
    let stats_available = stats_store.is_some();

    let mut items = Vec::new();
    let mut total_compounded_saved: i64 = 0;

    if let Some(ref store) = stats_store {
        let tool_use_ids: Vec<&str> = turn_indices.keys().map(|s| s.as_str()).collect();
        let rows = store.get_stats_by_tool_use_ids(&tool_use_ids);

        for row in &rows {
            // Only include rows belonging to this session
            let sid = turn_indices
                .get(&row.tool_use_id)
                .map(|info| info.session_id.as_str())
                .unwrap_or(&row.session_id);
            if sid != session_id {
                continue;
            }

            let saved = row.before_tokens - row.after_tokens;
            let category = map_operation_to_category(&row.operation);
            let title = map_operation_to_title(&row.operation);
            let strategy = row.operation.clone();
            let strategy_label = map_operation_to_strategy_label(&row.operation).to_string();

            let turn_index = turn_indices
                .get(&row.tool_use_id)
                .map(|info| info.turn_index)
                .unwrap_or(1) as i64;
            let compounding_turns = (request_count - turn_index).max(1);
            let compounded = saved * compounding_turns;
            total_compounded_saved += compounded;

            items.push(OptimizationItemDto {
                id: row.tool_use_id.clone(),
                category: category.to_string(),
                title: title.to_string(),
                strategy,
                strategy_label,
                before_tokens: row.before_tokens,
                after_tokens: row.after_tokens,
                saved_tokens: saved,
                compounded_saved: compounded,
                compounding_turns,
                before_summary: format!(
                    "\u{539f}\u{59cb}\u{5185}\u{5bb9} {} tokens",
                    row.before_tokens
                ),
                after_summary: format!("\u{4f18}\u{5316}\u{540e} {} tokens", row.after_tokens),
                before_text: row.before_text.clone(),
                after_text: row.after_text.clone(),
                diff_lines: Vec::new(),
            });
        }
    }

    let total_original = total_tokens + total_compounded_saved;
    let savings_rate = if total_original > 0 {
        total_compounded_saved as f64 / total_original as f64 * 100.0
    } else {
        0.0
    };

    HttpResponse::Ok().json(SessionSavingsDetail {
        session_id,
        stats_available,
        total_actual_tokens: total_tokens,
        total_compounded_saved,
        total_original_tokens: total_original,
        savings_rate,
        items,
    })
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

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod token_savings_tests {
    use super::*;
    use actix_web::test as actix_test;
    use actix_web::{App, web};
    use std::sync::{Arc, Mutex, RwLock};
    use std::time::Instant;

    // Tests manipulate the HOME env var which is process-global.
    // Use a mutex to serialize tests that depend on it.
    // allow(clippy::await_holding_lock): intentional — we need the lock held
    // for the entire test to prevent parallel env var races.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// Create a temp genai_events.db with test data and return its path.
    fn setup_genai_db(dir: &std::path::Path) -> std::path::PathBuf {
        let db_path = dir.join("genai_events.db");
        // Use GenAISqliteStore to create proper schema
        let store = crate::storage::sqlite::GenAISqliteStore::new_with_path(&db_path).unwrap();
        // Insert test data directly via raw connection
        drop(store);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO genai_events (event_type, session_id, call_id, agent_name, model, input_tokens, output_tokens, start_timestamp_ns, event_json, tool_call_ids)
             VALUES ('llm_call', 'sess-1', 'call-1', 'test-agent', 'gpt-4', 1000, 500, 100000000, '{}', '[\"tc-1\"]')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO genai_events (event_type, session_id, call_id, agent_name, model, input_tokens, output_tokens, start_timestamp_ns, event_json, tool_call_ids)
             VALUES ('llm_call', 'sess-1', 'call-2', 'test-agent', 'gpt-4', 800, 400, 200000000, '{}', '[\"tc-2\"]')",
            [],
        ).unwrap();
        db_path
    }

    /// Create a temp stats.db with test data and return its path.
    fn setup_stats_db(dir: &std::path::Path) -> std::path::PathBuf {
        let stats_dir = dir.join(".tokenless");
        std::fs::create_dir_all(&stats_dir).unwrap();
        let stats_path = stats_dir.join("stats.db");
        let conn = rusqlite::Connection::open(&stats_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE stats (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT,
                tool_use_id TEXT,
                before_tokens INTEGER,
                after_tokens INTEGER,
                before_text TEXT,
                after_text TEXT,
                operation TEXT
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO stats (session_id, tool_use_id, before_tokens, after_tokens, before_text, after_text, operation)
             VALUES ('sess-1', 'tc-1', 2000, 500, 'long text', 'short', 'compress-response')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO stats (session_id, tool_use_id, before_tokens, after_tokens, before_text, after_text, operation)
             VALUES ('sess-1', 'tc-2', 1000, 300, 'schema text', 'mini', 'compress-schema')",
            [],
        ).unwrap();
        stats_path
    }

    fn make_app_state(db_path: std::path::PathBuf) -> AppState {
        AppState {
            storage_path: db_path,
            start_time: Instant::now(),
            health_store: Arc::new(RwLock::new(crate::health::HealthStore::default())),
            interruption_store: None,
        }
    }

    // ─── Unit tests for mapping functions ─────────────────────────────────

    #[test]
    fn test_map_operation_to_category() {
        assert_eq!(
            map_operation_to_category("compress-response"),
            "mcp_response"
        );
        assert_eq!(map_operation_to_category("compress-toon"), "mcp_response");
        assert_eq!(map_operation_to_category("rewrite-command"), "tool_output");
        assert_eq!(map_operation_to_category("compress-schema"), "tool_output");
        assert_eq!(map_operation_to_category("unknown-op"), "tool_output");
    }

    #[test]
    fn test_map_operation_to_title() {
        assert_eq!(
            map_operation_to_title("compress-response"),
            "MCP\u{54cd}\u{5e94}\u{538b}\u{7f29}"
        );
        assert_eq!(
            map_operation_to_title("rewrite-command"),
            "\u{5de5}\u{5177}\u{8f93}\u{51fa}\u{4f18}\u{5316}"
        );
        assert_eq!(
            map_operation_to_title("compress-schema"),
            "Schema \u{538b}\u{7f29}"
        );
        assert_eq!(
            map_operation_to_title("compress-toon"),
            "TOON \u{7f16}\u{7801}"
        );
        assert_eq!(
            map_operation_to_title("other"),
            "\u{5176}\u{4ed6}\u{4f18}\u{5316}"
        );
    }

    #[test]
    fn test_map_operation_to_strategy_label() {
        assert_eq!(
            map_operation_to_strategy_label("compress-schema"),
            "Schema \u{538b}\u{7f29}"
        );
        assert_eq!(
            map_operation_to_strategy_label("compress-response"),
            "\u{54cd}\u{5e94}\u{538b}\u{7f29}"
        );
        assert_eq!(
            map_operation_to_strategy_label("rewrite-command"),
            "\u{547d}\u{4ee4}\u{91cd}\u{5199}"
        );
        assert_eq!(
            map_operation_to_strategy_label("compress-toon"),
            "TOON \u{7f16}\u{7801}"
        );
        assert_eq!(
            map_operation_to_strategy_label("unknown"),
            "\u{5176}\u{4ed6}\u{4f18}\u{5316}"
        );
    }

    // ─── Integration tests for handlers ───────────────────────────────────

    #[allow(clippy::await_holding_lock)]
    #[actix_web::test]
    async fn test_get_token_savings_no_stats_db() {
        let _lock = ENV_MUTEX.lock().unwrap();
        // When stats.db doesn't exist, handler should return stats_available=false
        let tmp = std::env::temp_dir().join(format!("agentsight_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let db_path = setup_genai_db(&tmp);

        // Point HOME to a dir without .tokenless/stats.db
        let fake_home = tmp.join("fakehome");
        std::fs::create_dir_all(&fake_home).unwrap();
        unsafe { std::env::set_var("HOME", &fake_home) };

        let state = make_app_state(db_path);
        let app = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .service(get_token_savings),
        )
        .await;

        let req = actix_test::TestRequest::get()
            .uri("/api/token-savings?start_ns=0&end_ns=9999999999999999")
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);

        let body: serde_json::Value = actix_test::read_body_json(resp).await;
        assert_eq!(body["stats_available"], false);
        assert!(!body["sessions"].as_array().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[allow(clippy::await_holding_lock)]
    #[actix_web::test]
    async fn test_get_token_savings_with_stats() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let tmp =
            std::env::temp_dir().join(format!("agentsight_test_stats_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let db_path = setup_genai_db(&tmp);
        let _stats_path = setup_stats_db(&tmp);

        // Point HOME to tmp so default_stats_path() finds .tokenless/stats.db
        unsafe { std::env::set_var("HOME", &tmp) };

        let state = make_app_state(db_path);
        let app = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .service(get_token_savings),
        )
        .await;

        let req = actix_test::TestRequest::get()
            .uri("/api/token-savings?start_ns=0&end_ns=9999999999999999")
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);

        let body: serde_json::Value = actix_test::read_body_json(resp).await;
        assert_eq!(body["stats_available"], true);
        // Should have strategy_breakdown
        let breakdown = body["summary"]["strategy_breakdown"].as_array().unwrap();
        assert!(!breakdown.is_empty());
        // Check savings were computed
        let total_saved = body["summary"]["total_saved_tokens"].as_i64().unwrap();
        assert!(total_saved > 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[allow(clippy::await_holding_lock)]
    #[actix_web::test]
    async fn test_get_session_savings_not_found() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("agentsight_test_sess_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let db_path = setup_genai_db(&tmp);

        let fake_home = tmp.join("fakehome2");
        std::fs::create_dir_all(&fake_home).unwrap();
        unsafe { std::env::set_var("HOME", &fake_home) };

        let state = make_app_state(db_path);
        let app = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .service(get_session_savings),
        )
        .await;

        let req = actix_test::TestRequest::get()
            .uri("/api/token-savings/session/nonexistent")
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);

        let body: serde_json::Value = actix_test::read_body_json(resp).await;
        assert_eq!(body["stats_available"], false);
        assert_eq!(body["session_id"], "nonexistent");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[allow(clippy::await_holding_lock)]
    #[actix_web::test]
    async fn test_get_session_savings_with_data() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let tmp =
            std::env::temp_dir().join(format!("agentsight_test_sess_data_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let db_path = setup_genai_db(&tmp);
        let _stats_path = setup_stats_db(&tmp);

        unsafe { std::env::set_var("HOME", &tmp) };

        let state = make_app_state(db_path);
        let app = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .service(get_session_savings),
        )
        .await;

        let req = actix_test::TestRequest::get()
            .uri("/api/token-savings/session/sess-1")
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);

        let body: serde_json::Value = actix_test::read_body_json(resp).await;
        assert_eq!(body["stats_available"], true);
        assert_eq!(body["session_id"], "sess-1");
        let items = body["items"].as_array().unwrap();
        assert!(!items.is_empty());
        // Verify strategy fields are present
        assert!(items[0]["strategy"].as_str().is_some());
        assert!(items[0]["strategy_label"].as_str().is_some());
        let compounded = body["total_compounded_saved"].as_i64().unwrap();
        assert!(compounded > 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

/// GET /api/skill-metrics — full skill metrics report
#[get("/api/skill-metrics")]
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
#[get("/api/skill-metrics/downloads")]
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
#[get("/api/skill-metrics/loads")]
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
#[get("/api/skill-metrics/usage-ratio")]
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
#[get("/api/skill-metrics/distribution")]
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
#[get("/api/skill-metrics/hotness")]
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
