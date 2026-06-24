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
